use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use anyhow::bail;
use jiff::civil::{DateTime, Time, Weekday};
use jiff::{ToSpan, Zoned};
use oxihome_protocol::{EventKind, Value};
use serde::Deserialize;

use crate::hub::Hub;
use crate::node::events::EventRecord;

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub when: Trigger,
    pub then: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Trigger {
    Event(EventTrigger),
    Schedule(ScheduleTrigger),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub event: EventKind,
}

/// Local time; no `days` means every day.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ScheduleTrigger {
    pub at: Time,
    #[serde(default)]
    pub days: Vec<Day>,
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

    /// The first run strictly after `now`; `None` only if the time never exists in the zone.
    pub fn next_run(&self, now: &Zoned) -> Option<Zoned> {
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

    /// Scheduled rules for a local minute (seconds are ignored).
    pub fn due(&self, minute: DateTime) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Schedule(trigger) if trigger.is_due(minute))
            })
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
}
