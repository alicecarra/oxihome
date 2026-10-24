use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use jiff::civil::{DateTime, Time, Weekday};
use jiff::{SignedDuration, ToSpan, Zoned};
use oxihome_protocol::{EventKind, Value};
use serde::Deserialize;

use super::{Expected, ValueTest};
use crate::node::events::EventRecord;
use crate::traffic::{Alert, AlertKind};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "TriggerFields")]
pub enum Trigger {
    Event(EventTrigger),
    Schedule(ScheduleTrigger),
    Cron(CronTrigger),
    State(StateTrigger),
    Offline(OfflineTrigger),
    Traffic(TrafficTrigger),
    RemoteKey(RemoteKeyTrigger),
    Button(ButtonTrigger),
    Notice(NoticeTrigger),
}

impl Trigger {
    /// The first run strictly after `now`; `None` for event rules.
    pub fn next_run(&self, now: &Zoned) -> Option<Zoned> {
        match self {
            Trigger::Event(_)
            | Trigger::State(_)
            | Trigger::Offline(_)
            | Trigger::Traffic(_)
            | Trigger::RemoteKey(_)
            | Trigger::Button(_)
            | Trigger::Notice(_) => None,
            Trigger::Schedule(trigger) => trigger.next_run(now),
            Trigger::Cron(trigger) => trigger.cron.find_next_occurrence(now, false).ok(),
        }
    }

    /// The placeholders this trigger fills in the rule's texts.
    pub fn placeholders(&self) -> &'static [&'static str] {
        match self {
            Trigger::Event(trigger) if trigger.event == EventKind::Received => {
                &["node", "channel", "value"]
            }
            Trigger::Event(_) => &["node", "channel"],
            Trigger::State(_) => &["node", "channel", "value"],
            Trigger::Offline(_) => &["node"],
            Trigger::Traffic(TrafficTrigger {
                kind: AlertKind::Stopped | AlertKind::Noisy,
                ..
            }) => &["node", "channel", "detail"],
            Trigger::Traffic(_) => &["node", "detail"],
            Trigger::Notice(_) => &["node", "title", "message"],
            Trigger::Schedule(_)
            | Trigger::Cron(_)
            | Trigger::RemoteKey(_)
            | Trigger::Button(_) => &[],
        }
    }

    pub(super) fn is_due(&self, minute: DateTime) -> bool {
        match self {
            Trigger::Event(_)
            | Trigger::State(_)
            | Trigger::Offline(_)
            | Trigger::Traffic(_)
            | Trigger::RemoteKey(_)
            | Trigger::Button(_)
            | Trigger::Notice(_) => false,
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
    data: Option<String>,
    equals: Option<Expected>,
    above: Option<f32>,
    below: Option<f32>,
    #[serde(rename = "for")]
    lasting: Option<String>,
    at: Option<Time>,
    #[serde(default)]
    days: Vec<Day>,
    cron: Option<String>,
    offline: Option<String>,
    stopped: Option<String>,
    noisy: Option<String>,
    unanswered: Option<String>,
    reported: Option<String>,
    remote: Option<String>,
    key: Option<String>,
    button: Option<String>,
    notice: Option<String>,
}

impl TriggerFields {
    /// The keys this `when` has.
    fn keys(&self) -> Vec<&'static str> {
        [
            ("device", self.device.is_some()),
            ("entity", self.entity.is_some()),
            ("channel", self.channel.is_some()),
            ("event", self.event.is_some()),
            ("data", self.data.is_some()),
            ("equals", self.equals.is_some()),
            ("above", self.above.is_some()),
            ("below", self.below.is_some()),
            ("for", self.lasting.is_some()),
            ("at", self.at.is_some()),
            ("days", !self.days.is_empty()),
            ("cron", self.cron.is_some()),
            ("offline", self.offline.is_some()),
            ("stopped", self.stopped.is_some()),
            ("noisy", self.noisy.is_some()),
            ("unanswered", self.unanswered.is_some()),
            ("reported", self.reported.is_some()),
            ("remote", self.remote.is_some()),
            ("key", self.key.is_some()),
            ("button", self.button.is_some()),
            ("notice", self.notice.is_some()),
        ]
        .into_iter()
        .filter_map(|(key, present)| present.then_some(key))
        .collect()
    }

    fn has_only(&self, allowed: &[&str]) -> bool {
        self.keys().iter().all(|key| allowed.contains(key))
    }
}

impl TryFrom<TriggerFields> for Trigger {
    type Error = String;

    fn try_from(fields: TriggerFields) -> Result<Self, String> {
        let traffic_alone = fields.has_only(&["stopped", "noisy", "unanswered", "reported"]);
        let button_alone = fields.has_only(&["button"]);
        let remote_key_alone = fields.has_only(&["remote", "key"]);
        let offline_alone = fields.has_only(&["offline", "for"]);
        let notice_alone = fields.has_only(&["notice"]);
        let traffic: Vec<(AlertKind, String)> = [
            (AlertKind::Stopped, fields.stopped),
            (AlertKind::Noisy, fields.noisy),
            (AlertKind::Unanswered, fields.unanswered),
            (AlertKind::Reported, fields.reported),
        ]
        .into_iter()
        .filter_map(|(kind, device)| device.map(|device| (kind, device)))
        .collect();
        if !traffic.is_empty() {
            let [(kind, device)] = <[_; 1]>::try_from(traffic).map_err(|_| {
                "use only one of `stopped`, `noisy`, `unanswered` and `reported`".to_string()
            })?;
            if !traffic_alone {
                return Err(
                    "`stopped`, `noisy`, `unanswered` and `reported` go alone in `when`".into(),
                );
            }
            let device = (device != "*").then_some(device);
            return Ok(Trigger::Traffic(TrafficTrigger { kind, device }));
        }
        if let Some(button) = fields.button {
            if !button_alone {
                return Err("a button trigger is `button` alone".into());
            }
            return Ok(Trigger::Button(ButtonTrigger { button }));
        }
        if fields.remote.is_some() || fields.key.is_some() {
            let (Some(remote), Some(key), true) = (fields.remote, fields.key, remote_key_alone)
            else {
                return Err("a remote key trigger is `remote` and `key` alone".into());
            };
            return Ok(Trigger::RemoteKey(RemoteKeyTrigger { remote, key }));
        }
        if let Some(device) = fields.notice {
            if !notice_alone {
                return Err("`notice` goes alone in `when`".into());
            }
            let device = (device != "*").then_some(device);
            return Ok(Trigger::Notice(NoticeTrigger { device }));
        }
        if let Some(device) = fields.offline {
            if !offline_alone {
                return Err("`offline` goes alone in `when`, or with `for`".into());
            }
            let device = (device != "*").then_some(device);
            let lasting = fields.lasting.as_deref().map(lasting).transpose()?;
            return Ok(Trigger::Offline(OfflineTrigger { device, lasting }));
        }
        let channel_kinds = [
            fields.event.is_some(),
            fields.equals.is_some(),
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
                return Err("use only one of `event`, `equals`, `above` and `below`".into());
            }
            if fields.data.is_some() && fields.event.is_none() {
                return Err("`data` goes with `event`".into());
            }
            if fields.lasting.is_some() && fields.event.is_some() {
                return Err(FOR_GOES_WITH.into());
            }
            let (Some(device), Some(entity), Some(channel)) =
                (fields.device, fields.entity, fields.channel)
            else {
                return Err("a channel trigger needs `device`, `entity` and `channel`".into());
            };
            let test = match (fields.event, fields.equals, fields.above, fields.below) {
                (Some(event), _, _, _) => {
                    return Ok(Trigger::Event(EventTrigger {
                        device,
                        entity,
                        channel,
                        event,
                        data: fields.data,
                    }));
                }
                (_, Some(expected), _, _) => ValueTest::Equals(expected),
                (_, _, Some(above), _) => ValueTest::Above(above),
                (_, _, _, Some(below)) => ValueTest::Below(below),
                _ => {
                    return Err(
                        "a channel trigger needs `event`, `equals`, `above` or `below`".into(),
                    );
                }
            };
            let lasting = fields.lasting.as_deref().map(lasting).transpose()?;
            return Ok(Trigger::State(StateTrigger {
                device,
                entity,
                channel,
                test,
                lasting,
            }));
        }
        if fields.lasting.is_some() {
            return Err(FOR_GOES_WITH.into());
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
        Err("`when` needs an event, a value or a limit on a channel (`device`, `entity`, `channel`), `at`, `cron`, `offline` or `notice`".into())
    }
}

const FOR_GOES_WITH: &str = "`for` goes with `equals`, `above`, `below` or `offline`";

/// Runs when a confirmed value starts passing the test, not again while it keeps passing.
/// The first known value of the channel counts as a start.
#[derive(Debug, Clone, PartialEq)]
pub struct StateTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub test: ValueTest,
    /// `for = "5m"`: runs only if no confirmed value that fails the test comes in this long
    /// after the start.
    pub lasting: Option<SignedDuration>,
}

const LONGEST_LASTING: SignedDuration = SignedDuration::from_hours(24);

fn lasting(text: &str) -> Result<SignedDuration, String> {
    let duration: SignedDuration = text
        .parse()
        .map_err(|error| format!("for \"{text}\": {error}"))?;
    if duration <= SignedDuration::ZERO || duration > LONGEST_LASTING {
        return Err(format!(
            "for \"{text}\": must be more than zero and at most 24 hours"
        ));
    }
    Ok(duration)
}

/// A button on the dashboard or the `/rules` page; its name is unique among rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonTrigger {
    pub button: String,
}

/// Runs when any IR receiver gets the key's code (not its repeats), by the names in
/// `remotes.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteKeyTrigger {
    pub remote: String,
    pub key: String,
}

/// Runs when the hub marks a node offline; `None` for any node (`offline = "*"`).
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineTrigger {
    pub device: Option<String>,
    /// `for = "30m"`: runs only if the node does not come back this long after going offline.
    pub lasting: Option<SignedDuration>,
}

/// Runs when the node sends a `Notice`; `None` for any node (`notice = "*"`).
#[derive(Debug, Clone, PartialEq)]
pub struct NoticeTrigger {
    pub device: Option<String>,
}

impl NoticeTrigger {
    pub(super) fn matches(&self, device_id: &str) -> bool {
        self.device
            .as_deref()
            .is_none_or(|device| device == device_id)
    }
}

/// Runs when the traffic map raises an alert about the node; `None` for any node (`"*"`).
#[derive(Debug, Clone, PartialEq)]
pub struct TrafficTrigger {
    pub kind: AlertKind,
    pub device: Option<String>,
}

impl TrafficTrigger {
    pub(super) fn matches(&self, alert: &Alert) -> bool {
        self.kind == alert.kind
            && self
                .device
                .as_deref()
                .is_none_or(|device| device == alert.device_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub event: EventKind,
    /// When set, the event's data must be exactly this text, e.g. an IR code `NEC 00 45`
    /// (`NEC 00 45 repeat`, sent while the key is held, does not match).
    pub data: Option<String>,
}

impl EventTrigger {
    pub(super) fn matches(&self, event: &EventRecord) -> bool {
        self.device == event.device_id
            && self.entity == event.entity
            && self.channel == event.channel
            && self.event == event.kind
            && self.data.as_ref().is_none_or(|expected| {
                matches!(&event.data, Some(Value::Text(text) | Value::Enum(text)) if text == expected)
            })
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::rules::{minute, when};

    #[test]
    fn button_triggers_are_button_alone() {
        assert!(matches!(
            when(r#"{ button = "A" }"#),
            Ok(Trigger::Button(_))
        ));
        assert!(
            when(r#"{ button = "A", at = "07:00" }"#)
                .unwrap_err()
                .contains("alone")
        );
        assert!(when(r#"{ panel = "P", button = "A" }"#).is_err());
    }

    #[test]
    fn remote_key_triggers_go_alone() {
        assert!(
            when(r#"{ remote = "A" }"#)
                .unwrap_err()
                .contains("`remote` and `key` alone")
        );
        assert!(
            when(r#"{ remote = "A", key = "0", device = "x" }"#)
                .unwrap_err()
                .contains("alone")
        );
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

    #[test]
    fn offline_trigger_goes_alone_or_with_for() {
        assert_eq!(
            when(r#"{ offline = "*" }"#),
            Ok(Trigger::Offline(OfflineTrigger {
                device: None,
                lasting: None,
            }))
        );
        assert_eq!(
            when(r#"{ offline = "tv", for = "30m" }"#),
            Ok(Trigger::Offline(OfflineTrigger {
                device: Some("tv".into()),
                lasting: Some(SignedDuration::from_mins(30)),
            }))
        );
        assert!(
            when(r#"{ offline = "tv", for = "25h" }"#)
                .unwrap_err()
                .contains("at most 24 hours")
        );
        assert!(
            when(r#"{ offline = "sim-bedroom", at = "07:00" }"#)
                .unwrap_err()
                .contains("goes alone")
        );
    }

    #[test]
    fn notice_trigger_goes_alone_and_fills_title_and_message() {
        let any = when(r#"{ notice = "*" }"#).unwrap();
        assert_eq!(any, Trigger::Notice(NoticeTrigger { device: None }));
        assert_eq!(any.placeholders(), ["node", "title", "message"]);
        let Trigger::Notice(reminders) = when(r#"{ notice = "reminders" }"#).unwrap() else {
            panic!("not a notice trigger");
        };
        assert!(reminders.matches("reminders"));
        assert!(!reminders.matches("weather"));
        assert!(
            when(r#"{ notice = "reminders", offline = "*" }"#)
                .unwrap_err()
                .contains("goes alone")
        );
    }

    #[test]
    fn data_goes_with_event() {
        let error = |text: &str| when(text).unwrap_err();
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, above = 28, data = "x" }"#)
                .contains("`data` goes with `event`")
        );
        assert!(error(r#"{ offline = "*", data = "x" }"#).contains("goes alone"));
    }

    #[test]
    fn state_triggers_parse_and_reject_mixtures() {
        assert_eq!(
            when(r#"{ device = "sim-living-room", entity = 3, channel = 1, below = 40.5 }"#),
            Ok(Trigger::State(StateTrigger {
                device: "sim-living-room".into(),
                entity: 3,
                channel: 1,
                test: ValueTest::Below(40.5),
                lasting: None,
            }))
        );
        assert_eq!(
            when(r#"{ device = "door", entity = 7, channel = 0, equals = true, for = "10m" }"#),
            Ok(Trigger::State(StateTrigger {
                device: "door".into(),
                entity: 7,
                channel: 0,
                test: ValueTest::Equals(Expected::Bool(true)),
                lasting: Some(SignedDuration::from_mins(10)),
            }))
        );
        assert!(matches!(
            when(r#"{ device = "tv", entity = 1, channel = 2, equals = "Standby" }"#),
            Ok(Trigger::State(StateTrigger {
                test: ValueTest::Equals(Expected::Text(_)),
                lasting: None,
                ..
            }))
        ));
        assert_eq!(
            when(r#"{ device = "a", entity = 3, channel = 0, above = 90, for = "5m" }"#).map(
                |trigger| match trigger {
                    Trigger::State(trigger) => trigger.lasting,
                    _ => None,
                }
            ),
            Ok(Some(SignedDuration::from_mins(5)))
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
            error(r#"{ device = "a", entity = 3, channel = 0, equals = true, above = 1 }"#)
                .contains("only one of")
        );
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, equals = true, event = "Press" }"#)
                .contains("only one of")
        );
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, equals = true, data = "x" }"#)
                .contains("`data` goes with `event`")
        );
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, event = "Press", for = "5m" }"#)
                .contains("`for` goes with")
        );
        assert!(error(r#"{ at = "07:00", for = "5m" }"#).contains("`for` goes with"));
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, above = 28, for = "0s" }"#)
                .contains("more than zero")
        );
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0 }"#)
                .contains("needs `event`, `equals`, `above` or `below`")
        );
    }

    #[test]
    fn traffic_triggers_parse() {
        assert_eq!(
            when(r#"{ stopped = "*" }"#),
            Ok(Trigger::Traffic(TrafficTrigger {
                kind: AlertKind::Stopped,
                device: None,
            }))
        );
        assert_eq!(
            when(r#"{ unanswered = "esp32-test" }"#),
            Ok(Trigger::Traffic(TrafficTrigger {
                kind: AlertKind::Unanswered,
                device: Some("esp32-test".into()),
            }))
        );
        assert!(
            when(r#"{ stopped = "*", noisy = "*" }"#)
                .unwrap_err()
                .contains("only one")
        );
        assert!(
            when(r#"{ noisy = "*", offline = "*" }"#)
                .unwrap_err()
                .contains("go alone")
        );
    }

    #[test]
    fn reported_trigger_parses() {
        assert_eq!(
            when(r#"{ reported = "*" }"#),
            Ok(Trigger::Traffic(TrafficTrigger {
                kind: AlertKind::Reported,
                device: None,
            }))
        );
    }
}
