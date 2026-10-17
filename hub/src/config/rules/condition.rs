use std::fmt;

use jiff::civil::{DateTime, Time, Weekday};
use oxihome_protocol::Value;
use serde::Deserialize;

use super::{Day, number};
use crate::hub::Hub;

/// Something about the home right now: a channel's value, the local time, the day or a node's
/// presence.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "ConditionFields")]
pub enum Condition {
    Channel(ChannelCondition),
    /// From `from` (included) to `to` (excluded); crosses midnight when `from` is later than `to`.
    Time {
        from: Time,
        to: Time,
    },
    Days(Vec<Day>),
    Online(String),
    Offline(String),
}

/// Holds only while the node is online and its value is known.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelCondition {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub test: ValueTest,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValueTest {
    Equals(Expected),
    Above(f32),
    Below(f32),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Expected {
    Bool(bool),
    Number(f32),
    Text(String),
}

impl Expected {
    fn matches(&self, value: &Value) -> bool {
        match (self, value) {
            (Expected::Bool(expected), Value::Bool(actual)) => expected == actual,
            (Expected::Number(expected), _) => number(value) == Some(*expected),
            (Expected::Text(expected), Value::Text(actual) | Value::Enum(actual)) => {
                expected == actual
            }
            _ => false,
        }
    }
}

impl fmt::Display for Expected {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Expected::Bool(expected) => write!(formatter, "{expected}"),
            Expected::Number(expected) => write!(formatter, "{expected}"),
            Expected::Text(expected) => write!(formatter, "\"{expected}\""),
        }
    }
}

impl ValueTest {
    fn passes(&self, value: &Value) -> bool {
        match self {
            ValueTest::Equals(expected) => expected.matches(value),
            ValueTest::Above(limit) => number(value).is_some_and(|number| number > *limit),
            ValueTest::Below(limit) => number(value).is_some_and(|number| number < *limit),
        }
    }
}

/// What conditions look at; the hub in practice.
pub trait HomeState {
    fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value>;
    fn is_online(&self, device_id: &str) -> bool;
}

impl Condition {
    /// `now` is the local time in the hub's zone.
    pub fn holds(&self, now: DateTime, home: &impl HomeState) -> bool {
        match self {
            Condition::Channel(condition) => {
                home.is_online(&condition.device)
                    && home
                        .value(&condition.device, condition.entity, condition.channel)
                        .is_some_and(|value| condition.test.passes(&value))
            }
            Condition::Time { from, to } => {
                let time = now.time();
                if from < to {
                    *from <= time && time < *to
                } else {
                    *from <= time || time < *to
                }
            }
            Condition::Days(days) => days.iter().any(|day| Weekday::from(*day) == now.weekday()),
            Condition::Online(device) => home.is_online(device),
            Condition::Offline(device) => !home.is_online(device),
        }
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Condition::Channel(condition) => {
                let ChannelCondition {
                    device,
                    entity,
                    channel,
                    test,
                } = condition;
                match test {
                    ValueTest::Equals(expected) => {
                        write!(formatter, "{device} {entity}/{channel} equals {expected}")
                    }
                    ValueTest::Above(limit) => {
                        write!(formatter, "{device} {entity}/{channel} above {limit}")
                    }
                    ValueTest::Below(limit) => {
                        write!(formatter, "{device} {entity}/{channel} below {limit}")
                    }
                }
            }
            Condition::Time { from, to } => write!(
                formatter,
                "from {} to {}",
                from.strftime("%H:%M"),
                to.strftime("%H:%M")
            ),
            Condition::Days(days) => write!(formatter, "days {days:?}"),
            Condition::Online(device) => write!(formatter, "{device} online"),
            Condition::Offline(device) => write!(formatter, "{device} offline"),
        }
    }
}

/// Every key a condition may have; which ones are present picks the condition.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConditionFields {
    device: Option<String>,
    entity: Option<u8>,
    channel: Option<u8>,
    equals: Option<Expected>,
    above: Option<f32>,
    below: Option<f32>,
    from: Option<Time>,
    to: Option<Time>,
    days: Option<Vec<Day>>,
    online: Option<String>,
    offline: Option<String>,
}

impl TryFrom<ConditionFields> for Condition {
    type Error = String;

    fn try_from(fields: ConditionFields) -> Result<Self, String> {
        let is_channel = fields.device.is_some()
            || fields.entity.is_some()
            || fields.channel.is_some()
            || fields.equals.is_some()
            || fields.above.is_some()
            || fields.below.is_some();
        let is_time = fields.from.is_some() || fields.to.is_some();
        let kinds = [
            is_channel,
            is_time,
            fields.days.is_some(),
            fields.online.is_some(),
            fields.offline.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if kinds != 1 {
            return Err("a condition is one of: a channel value (`device`, `entity`, `channel` with `equals`, `above` or `below`), `from` and `to`, `days`, `online` or `offline`".into());
        }
        if is_channel {
            let (Some(device), Some(entity), Some(channel)) =
                (fields.device, fields.entity, fields.channel)
            else {
                return Err("a channel condition needs `device`, `entity` and `channel`".into());
            };
            let test = match (fields.equals, fields.above, fields.below) {
                (Some(expected), None, None) => ValueTest::Equals(expected),
                (None, Some(limit), None) => ValueTest::Above(limit),
                (None, None, Some(limit)) => ValueTest::Below(limit),
                _ => {
                    return Err(
                        "a channel condition needs one of `equals`, `above` and `below`".into(),
                    );
                }
            };
            return Ok(Condition::Channel(ChannelCondition {
                device,
                entity,
                channel,
                test,
            }));
        }
        if is_time {
            let (Some(from), Some(to)) = (fields.from, fields.to) else {
                return Err("a time condition needs both `from` and `to`".into());
            };
            if from == to {
                return Err("`from` and `to` must differ".into());
            }
            return Ok(Condition::Time { from, to });
        }
        if let Some(days) = fields.days {
            if days.is_empty() {
                return Err("`days` needs at least one day".into());
            }
            return Ok(Condition::Days(days));
        }
        if let Some(device) = fields.online {
            return Ok(Condition::Online(device));
        }
        let device = fields.offline.expect("one kind is present");
        Ok(Condition::Offline(device))
    }
}

impl HomeState for Hub {
    fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value> {
        Hub::value(self, device_id, entity, channel)
    }

    fn is_online(&self, device_id: &str) -> bool {
        Hub::is_online(self, device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::rules::minute;

    #[derive(Default)]
    struct Home {
        values: Vec<(&'static str, u8, u8, Value)>,
        online: Vec<&'static str>,
    }

    impl HomeState for Home {
        fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value> {
            self.values
                .iter()
                .find(|(device, known_entity, known_channel, _)| {
                    *device == device_id && *known_entity == entity && *known_channel == channel
                })
                .map(|(_, _, _, value)| value.clone())
        }

        fn is_online(&self, device_id: &str) -> bool {
            self.online.contains(&device_id)
        }
    }

    fn condition(text: &str) -> Result<Condition, String> {
        #[derive(Deserialize)]
        struct Wrapper {
            condition: Condition,
        }
        toml::from_str::<Wrapper>(&format!("condition = {text}"))
            .map(|wrapper| wrapper.condition)
            .map_err(|error| error.message().to_string())
    }

    #[test]
    fn invalid_conditions_explain_what_is_wrong() {
        let error = |text: &str| condition(text).unwrap_err();
        assert!(error(r#"{ device = "a", entity = 1, channel = 0 }"#).contains("one of `equals`"));
        assert!(
            error(r#"{ device = "a", entity = 1, channel = 0, above = 1, below = 2 }"#)
                .contains("one of `equals`")
        );
        assert!(error(r#"{ device = "a", above = 1 }"#).contains("needs `device`"));
        assert!(error(r#"{ from = "22:00" }"#).contains("both `from` and `to`"));
        assert!(error(r#"{ from = "22:00", to = "22:00" }"#).contains("must differ"));
        assert!(error(r#"{ days = [] }"#).contains("at least one day"));
        assert!(error(r#"{ online = "a", days = ["monday"] }"#).contains("a condition is one of"));
        assert!(error("{}").contains("a condition is one of"));
        assert!(error(r#"{ at = "07:00" }"#).contains("unknown field"));
    }

    #[test]
    fn channel_conditions_compare_the_current_value() {
        let home = Home {
            values: vec![
                ("a", 1, 0, Value::Float(26.5)),
                ("a", 2, 0, Value::Bool(true)),
                ("a", 3, 0, Value::Enum("On".into())),
                ("a", 4, 0, Value::Int(3)),
            ],
            online: vec!["a"],
        };
        let holds = |text: &str| {
            condition(text)
                .unwrap()
                .holds(minute("2026-10-12T12:00"), &home)
        };
        assert!(holds(
            r#"{ device = "a", entity = 1, channel = 0, above = 25 }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 1, channel = 0, below = 25 }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 1, channel = 0, above = 26.5 }"#
        ));
        assert!(holds(
            r#"{ device = "a", entity = 2, channel = 0, equals = true }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 2, channel = 0, equals = false }"#
        ));
        assert!(holds(
            r#"{ device = "a", entity = 3, channel = 0, equals = "On" }"#
        ));
        assert!(holds(
            r#"{ device = "a", entity = 4, channel = 0, equals = 3 }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 4, channel = 0, equals = true }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 9, channel = 0, equals = true }"#
        ));
    }

    #[test]
    fn channel_conditions_do_not_hold_while_the_node_is_offline() {
        let home = Home {
            values: vec![("a", 2, 0, Value::Bool(true))],
            online: Vec::new(),
        };
        let condition =
            condition(r#"{ device = "a", entity = 2, channel = 0, equals = true }"#).unwrap();
        assert!(!condition.holds(minute("2026-10-12T12:00"), &home));
    }

    #[test]
    fn time_ranges_include_the_start_and_may_cross_midnight() {
        let home = Home::default();
        let night = condition(r#"{ from = "22:00", to = "06:00" }"#).unwrap();
        let day = condition(r#"{ from = "08:00", to = "18:00" }"#).unwrap();
        let at = |text: &str| minute(text);
        assert!(night.holds(at("2026-10-12T22:00"), &home));
        assert!(night.holds(at("2026-10-12T03:00"), &home));
        assert!(!night.holds(at("2026-10-12T06:00"), &home));
        assert!(!night.holds(at("2026-10-12T12:00"), &home));
        assert!(day.holds(at("2026-10-12T08:00"), &home));
        assert!(!day.holds(at("2026-10-12T18:00"), &home));
        assert!(!day.holds(at("2026-10-12T03:00"), &home));
    }

    #[test]
    fn day_and_presence_conditions() {
        let home = Home {
            values: Vec::new(),
            online: vec!["phone"],
        };
        let weekend = condition(r#"{ days = ["saturday", "sunday"] }"#).unwrap();
        // 2026-10-12 is a Monday, 2026-10-17 a Saturday.
        assert!(weekend.holds(minute("2026-10-17T12:00"), &home));
        assert!(!weekend.holds(minute("2026-10-12T12:00"), &home));
        let noon = minute("2026-10-12T12:00");
        assert!(
            condition(r#"{ online = "phone" }"#)
                .unwrap()
                .holds(noon, &home)
        );
        assert!(
            !condition(r#"{ online = "tv" }"#)
                .unwrap()
                .holds(noon, &home)
        );
        assert!(
            condition(r#"{ offline = "tv" }"#)
                .unwrap()
                .holds(noon, &home)
        );
        assert!(
            !condition(r#"{ offline = "phone" }"#)
                .unwrap()
                .holds(noon, &home)
        );
    }
}
