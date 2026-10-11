use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use anyhow::bail;
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use jiff::civil::{DateTime, Time, Weekday};
use jiff::{ToSpan, Zoned};
use oxihome_protocol::{Confidence, EventKind, Value};
use serde::Deserialize;

use crate::hub::Hub;
use crate::node::events::EventRecord;
use crate::node::state::ChannelUpdate;

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub when: Trigger,
    pub then: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "TriggerFields")]
pub enum Trigger {
    Event(EventTrigger),
    Schedule(ScheduleTrigger),
    Cron(CronTrigger),
    Threshold(ThresholdTrigger),
    Offline(OfflineTrigger),
}

impl Trigger {
    /// The first run strictly after `now`; `None` for event rules.
    pub fn next_run(&self, now: &Zoned) -> Option<Zoned> {
        match self {
            Trigger::Event(_) | Trigger::Threshold(_) | Trigger::Offline(_) => None,
            Trigger::Schedule(trigger) => trigger.next_run(now),
            Trigger::Cron(trigger) => trigger.cron.find_next_occurrence(now, false).ok(),
        }
    }

    fn is_due(&self, minute: DateTime) -> bool {
        match self {
            Trigger::Event(_) | Trigger::Threshold(_) | Trigger::Offline(_) => false,
            Trigger::Schedule(trigger) => trigger.is_due(minute),
            Trigger::Cron(trigger) => trigger.cron.is_time_matching(&minute).unwrap_or(false),
        }
    }
}

/// Every key a `when` may have; which ones are present picks the trigger.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerFields {
    device: Option<String>,
    entity: Option<u8>,
    channel: Option<u8>,
    event: Option<EventKind>,
    above: Option<f32>,
    below: Option<f32>,
    at: Option<Time>,
    #[serde(default)]
    days: Vec<Day>,
    cron: Option<String>,
    offline: Option<String>,
}

impl TryFrom<TriggerFields> for Trigger {
    type Error = String;

    fn try_from(fields: TriggerFields) -> Result<Self, String> {
        if let Some(device) = fields.offline {
            let alone = fields.device.is_none()
                && fields.entity.is_none()
                && fields.channel.is_none()
                && fields.event.is_none()
                && fields.above.is_none()
                && fields.below.is_none()
                && fields.at.is_none()
                && fields.days.is_empty()
                && fields.cron.is_none();
            if !alone {
                return Err("`offline` goes alone in `when`".into());
            }
            let device = (device != "*").then_some(device);
            return Ok(Trigger::Offline(OfflineTrigger { device }));
        }
        let channel_kinds = [
            fields.event.is_some(),
            fields.above.is_some(),
            fields.below.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        let is_channel_trigger = channel_kinds > 0
            || fields.device.is_some()
            || fields.entity.is_some()
            || fields.channel.is_some();
        if is_channel_trigger {
            if fields.at.is_some() || fields.cron.is_some() || !fields.days.is_empty() {
                return Err("a channel trigger cannot have `at`, `days` or `cron`".into());
            }
            if channel_kinds > 1 {
                return Err("use only one of `event`, `above` and `below`".into());
            }
            let (Some(device), Some(entity), Some(channel)) =
                (fields.device, fields.entity, fields.channel)
            else {
                return Err("a channel trigger needs `device`, `entity` and `channel`".into());
            };
            let limit = match (fields.event, fields.above, fields.below) {
                (Some(event), _, _) => {
                    return Ok(Trigger::Event(EventTrigger {
                        device,
                        entity,
                        channel,
                        event,
                    }));
                }
                (_, Some(above), _) => Limit::Above(above),
                (_, _, Some(below)) => Limit::Below(below),
                _ => return Err("a channel trigger needs `event`, `above` or `below`".into()),
            };
            return Ok(Trigger::Threshold(ThresholdTrigger {
                device,
                entity,
                channel,
                limit,
            }));
        }
        if fields.at.is_some() && fields.cron.is_some() {
            return Err("use either `at` or `cron`, not both".into());
        }
        if let Some(expression) = fields.cron {
            if !fields.days.is_empty() {
                return Err("`days` goes with `at`; put the days in the cron expression".into());
            }
            return Ok(Trigger::Cron(CronTrigger::parse(&expression)?));
        }
        if let Some(at) = fields.at {
            let days = fields.days;
            return Ok(Trigger::Schedule(ScheduleTrigger { at, days }));
        }
        if !fields.days.is_empty() {
            return Err("`days` needs `at`".into());
        }
        Err("`when` needs an event or a limit on a channel (`device`, `entity`, `channel`), `at`, `cron` or `offline`".into())
    }
}

/// Runs when a confirmed numeric value crosses the limit, not again while it stays past it.
/// The first known value of the channel counts as a crossing.
#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub limit: Limit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Limit {
    Above(f32),
    Below(f32),
}

impl Limit {
    fn crossed(self, previous: Option<&Value>, value: &Value) -> bool {
        let Some(now) = number(value) else {
            return false;
        };
        let before = previous.and_then(number);
        match self {
            Limit::Above(limit) => now > limit && before.is_none_or(|before| before <= limit),
            Limit::Below(limit) => now < limit && before.is_none_or(|before| before >= limit),
        }
    }
}

fn number(value: &Value) -> Option<f32> {
    match value {
        Value::Int(number) => Some(*number as f32),
        Value::Float(number) => Some(*number),
        _ => None,
    }
}

/// Runs when the hub marks a node offline; `None` for any node (`offline = "*"`).
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineTrigger {
    pub device: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub event: EventKind,
}

/// Local time; no `days` means every day.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleTrigger {
    pub at: Time,
    pub days: Vec<Day>,
}

/// Standard five fields (minute, hour, day of month, month, day of week), in local time.
#[derive(Debug, Clone, PartialEq)]
pub struct CronTrigger {
    pub expression: String,
    pub cron: Box<Cron>,
}

impl CronTrigger {
    fn parse(expression: &str) -> Result<Self, String> {
        let parser = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .build();
        parser
            .parse(expression)
            .map(|cron| Self {
                expression: expression.to_string(),
                cron: Box::new(cron),
            })
            .map_err(|error| format!("cron \"{expression}\": {error}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl From<Day> for Weekday {
    fn from(day: Day) -> Self {
        match day {
            Day::Monday => Weekday::Monday,
            Day::Tuesday => Weekday::Tuesday,
            Day::Wednesday => Weekday::Wednesday,
            Day::Thursday => Weekday::Thursday,
            Day::Friday => Weekday::Friday,
            Day::Saturday => Weekday::Saturday,
            Day::Sunday => Weekday::Sunday,
        }
    }
}

impl ScheduleTrigger {
    fn runs_on(&self, weekday: Weekday) -> bool {
        self.days.is_empty() || self.days.iter().any(|day| Weekday::from(*day) == weekday)
    }

    fn is_due(&self, minute: DateTime) -> bool {
        self.at.hour() == minute.hour()
            && self.at.minute() == minute.minute()
            && self.runs_on(minute.weekday())
    }

    /// `None` only if the time never exists in the zone.
    fn next_run(&self, now: &Zoned) -> Option<Zoned> {
        (0..=7)
            .filter_map(|days| now.date().checked_add(days.days()).ok())
            .filter(|date| self.runs_on(date.weekday()))
            .filter_map(|date| {
                date.to_datetime(self.at)
                    .to_zoned(now.time_zone().clone())
                    .ok()
            })
            .find(|run| run > now)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Set {
        device: String,
        entity: u8,
        channel: u8,
        value: bool,
    },
    Toggle {
        device: String,
        entity: u8,
        channel: u8,
    },
    /// Fires a `Trigger` channel, e.g. a buzzer pattern; `value` is one of the channel's `Enum` options.
    Invoke {
        device: String,
        entity: u8,
        channel: u8,
        value: Option<String>,
    },
    Notify {
        title: String,
        message: String,
    },
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Action::Set {
                device,
                entity,
                channel,
                value,
            } => write!(formatter, "set {device} {entity}/{channel} to {value}"),
            Action::Toggle {
                device,
                entity,
                channel,
            } => write!(formatter, "toggle {device} {entity}/{channel}"),
            Action::Invoke {
                device,
                entity,
                channel,
                value: Some(value),
            } => write!(formatter, "invoke {device} {entity}/{channel} with {value}"),
            Action::Invoke {
                device,
                entity,
                channel,
                value: None,
            } => write!(formatter, "invoke {device} {entity}/{channel}"),
            Action::Notify { title, .. } => write!(formatter, "notify \"{title}\""),
        }
    }
}

/// How the last run of a rule went; `errors` is empty when every action worked.
#[derive(Debug, Clone)]
pub struct RuleRun {
    pub at: Instant,
    pub errors: Vec<String>,
}

#[derive(Default, Deserialize)]
pub struct Rules {
    #[serde(default)]
    rules: Vec<Rule>,
}

impl Rules {
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn all(&self) -> &[Rule] {
        &self.rules
    }

    pub fn matching(&self, event: &EventRecord) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Event(trigger)
                        if trigger.device == event.device_id
                            && trigger.entity == event.entity
                            && trigger.channel == event.channel
                            && trigger.event == event.kind)
            })
            .collect()
    }

    pub fn crossed(&self, update: &ChannelUpdate) -> Vec<&Rule> {
        if update.confidence != Confidence::Confirmed {
            return Vec::new();
        }
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Threshold(trigger)
                        if trigger.device == update.device_id
                            && trigger.entity == update.entity
                            && trigger.channel == update.channel
                            && trigger.limit.crossed(update.previous.as_ref(), &update.value))
            })
            .collect()
    }

    pub fn offline(&self, device_id: &str) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Offline(trigger)
                        if trigger.device.as_deref().is_none_or(|device| device == device_id))
            })
            .collect()
    }

    /// Scheduled rules for a local minute (seconds are ignored).
    pub fn due(&self, minute: DateTime) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| rule.enabled && rule.when.is_due(minute))
            .collect()
    }
}

/// Runs the actions in order; a failed action does not stop the next ones.
pub async fn run(hub: Arc<Hub>, rule: Rule) {
    let at = Instant::now();
    let mut errors = Vec::new();
    for action in &rule.then {
        match perform(&hub, action, &rule.name).await {
            Ok(outcome) => println!("rule \"{}\": {action} -> {outcome}", rule.name),
            Err(error) => {
                println!("rule \"{}\": {action} failed: {error}", rule.name);
                hub.notify(
                    "Rule failed".into(),
                    format!("{}: {action}: {error}", rule.name),
                    rule.name.clone(),
                );
                errors.push(format!("{action}: {error}"));
            }
        }
    }
    hub.record_rule_run(&rule.name, RuleRun { at, errors });
}

/// Returns what happened, for the log.
async fn perform(hub: &Hub, action: &Action, rule_name: &str) -> anyhow::Result<String> {
    let (device, entity, channel, value) = match action {
        Action::Notify { title, message } => {
            hub.notify(title.clone(), message.clone(), rule_name.to_string());
            return Ok("shown".into());
        }
        Action::Invoke {
            device,
            entity,
            channel,
            value,
        } => {
            hub.invoke(device, *entity, *channel, value.clone()).await?;
            return Ok("done".into());
        }
        Action::Set {
            device,
            entity,
            channel,
            value,
        } => (device, *entity, *channel, *value),
        Action::Toggle {
            device,
            entity,
            channel,
        } => match hub.value(device, *entity, *channel) {
            Some(Value::Bool(current)) => (device, *entity, *channel, !current),
            Some(other) => bail!("current value is {other:?}, not a Bool"),
            None => bail!("current value is unknown"),
        },
    };
    hub.set(device, entity, channel, Value::Bool(value)).await?;
    Ok(value.to_string())
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
        [[rules]]
        name = "Living room button toggles bedroom lamp"
        when = { device = "sim-living-room", entity = 2, channel = 0, event = "Press" }
        then = [{ toggle = { device = "sim-bedroom", entity = 1, channel = 0 } }]

        [[rules]]
        name = "Long press turns everything off"
        when = { device = "sim-living-room", entity = 2, channel = 0, event = "LongPress" }
        then = [
            { set = { device = "sim-bedroom", entity = 1, channel = 0, value = false } },
            { set = { device = "sim-living-room", entity = 1, channel = 0, value = false } },
            { notify = { title = "Good night", message = "All lamps are off" } },
        ]

        [[rules]]
        name = "Disabled"
        enabled = false
        when = { device = "sim-living-room", entity = 2, channel = 0, event = "LongPress" }
        then = [{ notify = { title = "Error", message = "if this is disabled, why are you seeing this?" } }]
    "#;

    fn event(kind: EventKind, entity: u8) -> EventRecord {
        EventRecord {
            device_id: "sim-living-room".into(),
            entity,
            channel: 0,
            kind,
            data: None,
        }
    }

    #[test]
    fn example_file_parses() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(
            rules.rules[1].then[0],
            Action::Set {
                device: "sim-bedroom".into(),
                entity: 1,
                channel: 0,
                value: false,
            }
        );
    }

    #[test]
    fn notify_action_parses() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(
            rules.rules[1].then[2],
            Action::Notify {
                title: "Good night".into(),
                message: "All lamps are off".into(),
            }
        );
    }

    #[test]
    fn matching_event_selects_its_rules() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        let matched = rules.matching(&event(EventKind::Press, 2));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "Living room button toggles bedroom lamp");
    }

    #[test]
    fn other_events_select_nothing() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert!(rules.matching(&event(EventKind::DoublePress, 2)).is_empty());
        assert!(rules.matching(&event(EventKind::Press, 3)).is_empty());
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        let matched = rules.matching(&event(EventKind::LongPress, 2));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "Long press turns everything off");
    }

    #[test]
    fn empty_file_has_no_rules() {
        let rules: Rules = toml::from_str("").unwrap();
        assert_eq!(rules.len(), 0);
    }

    const SCHEDULED: &str = r#"
        [[rules]]
        name = "Lights off at night"
        when = { at = "23:00" }
        then = [{ set = { device = "sim-bedroom", entity = 1, channel = 0, value = false } }]

        [[rules]]
        name = "Wake up"
        when = { at = "07:00", days = ["monday", "tuesday", "wednesday", "thursday", "friday"] }
        then = [{ set = { device = "sim-bedroom", entity = 1, channel = 0, value = true } }]
    "#;

    fn minute(text: &str) -> DateTime {
        text.parse().unwrap()
    }

    fn names(rules: Vec<&Rule>) -> Vec<&str> {
        rules.into_iter().map(|rule| rule.name.as_str()).collect()
    }

    #[test]
    fn scheduled_rules_parse() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        assert_eq!(
            rules.rules[1].when,
            Trigger::Schedule(ScheduleTrigger {
                at: Time::constant(7, 0, 0, 0),
                days: vec![
                    Day::Monday,
                    Day::Tuesday,
                    Day::Wednesday,
                    Day::Thursday,
                    Day::Friday
                ],
            })
        );
    }

    #[test]
    fn rules_are_due_at_their_minute_and_days() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        // 2026-10-12 is a Monday, 2026-10-17 a Saturday.
        assert_eq!(
            names(rules.due(minute("2026-10-12T23:00"))),
            ["Lights off at night"]
        );
        assert_eq!(names(rules.due(minute("2026-10-12T07:00"))), ["Wake up"]);
        assert!(rules.due(minute("2026-10-17T07:00")).is_empty());
        assert!(rules.due(minute("2026-10-12T07:01")).is_empty());
    }

    #[test]
    fn events_do_not_run_scheduled_rules() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        assert!(rules.matching(&event(EventKind::Press, 2)).is_empty());
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert!(rules.due(minute("2026-10-12T23:00")).is_empty());
    }

    #[test]
    fn next_run_skips_to_the_next_allowed_day() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        let Trigger::Schedule(wake_up) = &rules.rules[1].when else {
            panic!("scheduled rule");
        };
        let friday_night: Zoned = "2026-10-16T22:00:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        let run = wake_up.next_run(&friday_night).unwrap();
        assert_eq!(run.datetime(), minute("2026-10-19T07:00"));
        let monday_at_seven: Zoned = "2026-10-19T07:00:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        let run = wake_up.next_run(&monday_at_seven).unwrap();
        assert_eq!(run.datetime(), minute("2026-10-20T07:00"));
    }

    fn when(text: &str) -> Result<Trigger, String> {
        #[derive(Deserialize)]
        struct Wrapper {
            when: Trigger,
        }
        toml::from_str::<Wrapper>(&format!("when = {text}"))
            .map(|wrapper| wrapper.when)
            .map_err(|error| error.message().to_string())
    }

    #[test]
    fn cron_rules_are_due_when_the_expression_matches() {
        let trigger = when(r#"{ cron = "*/30 8-22 * * mon-fri" }"#).unwrap();
        assert!(trigger.is_due(minute("2026-10-12T08:30")));
        assert!(trigger.is_due(minute("2026-10-12T22:00")));
        assert!(!trigger.is_due(minute("2026-10-12T08:15")));
        assert!(!trigger.is_due(minute("2026-10-12T23:00")));
        assert!(!trigger.is_due(minute("2026-10-17T08:30")));
    }

    #[test]
    fn cron_next_run_skips_the_weekend() {
        let trigger = when(r#"{ cron = "0 7 * * 1-5" }"#).unwrap();
        let friday_night: Zoned = "2026-10-16T22:00:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        let run = trigger.next_run(&friday_night).unwrap();
        assert_eq!(run.datetime(), minute("2026-10-19T07:00"));
    }

    #[test]
    fn invalid_triggers_explain_what_is_wrong() {
        let error = |text: &str| when(text).unwrap_err();
        assert!(error(r#"{ cron = "61 * * * *" }"#).contains("cron \"61 * * * *\""));
        assert!(error(r#"{ cron = "0 0 7 * * *" }"#).contains("cron"));
        assert!(error(r#"{ at = "07:00", cron = "0 7 * * *" }"#).contains("not both"));
        assert!(error(r#"{ cron = "0 7 * * *", days = ["monday"] }"#).contains("cron expression"));
        assert!(error(r#"{ days = ["monday"] }"#).contains("needs `at`"));
        assert!(error(r#"{ device = "sim-bedroom", event = "Press" }"#).contains("needs `device`"));
        assert!(error(r#"{ device = "sim-bedroom", entity = 1, channel = 0, event = "Press", at = "07:00" }"#).contains("cannot have"));
        assert!(error(r#"{ at = "07:00", day = ["monday"] }"#).contains("unknown field"));
        assert!(error("{}").contains("needs an event"));
    }

    const OFFLINE: &str = r#"
        [[rules]]
        name = "Any node offline"
        when = { offline = "*" }
        then = [{ invoke = { device = "esp32-08d1f9ca41e4", entity = 5, channel = 0, value = "Alarm" } }]

        [[rules]]
        name = "Bedroom offline"
        when = { offline = "sim-bedroom" }
        then = []
    "#;

    #[test]
    fn offline_rules_match_their_node_or_any() {
        let rules: Rules = toml::from_str(OFFLINE).unwrap();
        assert_eq!(
            names(rules.offline("sim-bedroom")),
            ["Any node offline", "Bedroom offline"]
        );
        assert_eq!(
            names(rules.offline("sim-living-room")),
            ["Any node offline"]
        );
        assert!(rules.matching(&event(EventKind::Press, 2)).is_empty());
        assert!(rules.due(minute("2026-10-12T23:00")).is_empty());
    }

    #[test]
    fn invoke_action_parses() {
        let rules: Rules = toml::from_str(OFFLINE).unwrap();
        assert_eq!(
            rules.rules[0].then[0],
            Action::Invoke {
                device: "esp32-08d1f9ca41e4".into(),
                entity: 5,
                channel: 0,
                value: Some("Alarm".into()),
            }
        );
    }

    #[test]
    fn offline_trigger_goes_alone() {
        assert_eq!(
            when(r#"{ offline = "*" }"#),
            Ok(Trigger::Offline(OfflineTrigger { device: None }))
        );
        assert!(
            when(r#"{ offline = "sim-bedroom", at = "07:00" }"#)
                .unwrap_err()
                .contains("goes alone")
        );
    }

    const THRESHOLDS: &str = r#"
        [[rules]]
        name = "Hot"
        when = { device = "sim-living-room", entity = 3, channel = 0, above = 28 }
        then = []

        [[rules]]
        name = "Cooled down"
        when = { device = "sim-living-room", entity = 3, channel = 0, below = 27 }
        then = []

        [[rules]]
        name = "Disabled"
        enabled = false
        when = { device = "sim-living-room", entity = 3, channel = 0, above = 28 }
        then = []
    "#;

    fn reading(value: Value, previous: Option<Value>) -> ChannelUpdate {
        ChannelUpdate {
            device_id: "sim-living-room".into(),
            entity: 3,
            channel: 0,
            value,
            confidence: Confidence::Confirmed,
            previous,
        }
    }

    fn crossed(update: ChannelUpdate) -> Vec<String> {
        let rules: Rules = toml::from_str(THRESHOLDS).unwrap();
        rules
            .crossed(&update)
            .into_iter()
            .map(|rule| rule.name.clone())
            .collect()
    }

    #[test]
    fn limits_run_when_the_value_crosses_them() {
        let float = Value::Float;
        assert_eq!(crossed(reading(float(29.0), Some(float(27.5)))), ["Hot"]);
        assert_eq!(
            crossed(reading(float(26.0), Some(float(27.5)))),
            ["Cooled down"]
        );
        assert_eq!(
            crossed(reading(float(28.0), Some(float(27.0)))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn staying_past_the_limit_runs_nothing() {
        let float = Value::Float;
        assert!(crossed(reading(float(30.0), Some(float(29.0)))).is_empty());
        assert!(crossed(reading(float(25.0), Some(float(26.0)))).is_empty());
    }

    #[test]
    fn hysteresis_band_runs_nothing() {
        let float = Value::Float;
        assert!(crossed(reading(float(27.5), Some(float(28.5)))).is_empty());
        assert!(crossed(reading(float(27.5), Some(float(26.5)))).is_empty());
    }

    #[test]
    fn first_known_value_counts_as_crossing() {
        assert_eq!(crossed(reading(Value::Float(30.0), None)), ["Hot"]);
        assert_eq!(crossed(reading(Value::Int(20), None)), ["Cooled down"]);
    }

    #[test]
    fn assumed_and_non_numeric_values_run_nothing() {
        let mut assumed = reading(Value::Float(30.0), Some(Value::Float(20.0)));
        assumed.confidence = Confidence::Assumed;
        assert!(crossed(assumed).is_empty());
        assert!(crossed(reading(Value::Bool(true), None)).is_empty());
    }

    #[test]
    fn threshold_triggers_parse_and_reject_mixtures() {
        assert_eq!(
            when(r#"{ device = "sim-living-room", entity = 3, channel = 1, below = 40.5 }"#),
            Ok(Trigger::Threshold(ThresholdTrigger {
                device: "sim-living-room".into(),
                entity: 3,
                channel: 1,
                limit: Limit::Below(40.5),
            }))
        );
        let error = |text: &str| when(text).unwrap_err();
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, above = 28, below = 27 }"#)
                .contains("only one of")
        );
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, above = 28, at = "07:00" }"#)
                .contains("cannot have")
        );
        assert!(error(r#"{ above = 28 }"#).contains("needs `device`"));
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0 }"#)
                .contains("needs `event`, `above` or `below`")
        );
    }
}
