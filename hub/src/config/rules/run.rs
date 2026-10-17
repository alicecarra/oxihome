use std::fmt;
use std::sync::Arc;

use anyhow::bail;
use jiff::Timestamp;
use oxihome_protocol::Value;
use serde::Deserialize;

use super::{
    Condition, EventTrigger, Limit, OfflineTrigger, Rule, RuleRun, ThresholdTrigger,
    TrafficTrigger, Trigger,
};
use crate::hub::{CommandOrigin, Hub};
use crate::node::events::{EventRecord, data_label};
use crate::node::state::ChannelUpdate;
use crate::traffic::AlertKind;

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

impl Action {
    /// The texts that may have placeholders: a notification's title and message, an invoke's
    /// value.
    pub fn texts(&self) -> Vec<&str> {
        match self {
            Action::Notify { title, message } => vec![title, message],
            Action::Invoke {
                value: Some(value), ..
            } => vec![value],
            _ => Vec::new(),
        }
    }
}

/// What a rule runs about, for `{node}`, `{channel}`, `{detail}` and `{value}` in the text of its
/// `notify` and `invoke` actions. Which ones a rule may use depends on its trigger.
#[derive(Debug, Clone, Default)]
pub struct Subject {
    pub node: Option<String>,
    pub channel: Option<String>,
    pub detail: Option<String>,
    pub value: Option<String>,
}

/// Friendly names and values for a subject; the hub in practice.
pub trait Names {
    fn node_name(&self, device_id: &str) -> String;
    fn channel_name(&self, device_id: &str, entity: u8, channel: u8) -> String;
    /// With the channel's unit, e.g. `22.3 °C`.
    fn value_label(&self, device_id: &str, entity: u8, channel: u8, value: &Value) -> String;
}

impl Names for Hub {
    fn node_name(&self, device_id: &str) -> String {
        Hub::node_name(self, device_id)
    }

    fn channel_name(&self, device_id: &str, entity: u8, channel: u8) -> String {
        Hub::channel_name(self, device_id, entity, channel)
    }

    fn value_label(&self, device_id: &str, entity: u8, channel: u8, value: &Value) -> String {
        Hub::value_label(self, device_id, entity, channel, value)
    }
}

impl Subject {
    /// `{value}` is the event's data, e.g. `offline` or an IR code.
    pub fn of_event(event: &EventRecord, names: &impl Names) -> Self {
        Self {
            node: Some(names.node_name(&event.device_id)),
            channel: Some(names.channel_name(&event.device_id, event.entity, event.channel)),
            detail: None,
            value: event.data.as_ref().map(data_label),
        }
    }

    /// `{value}` is the new value, with its unit.
    pub fn of_update(update: &ChannelUpdate, names: &impl Names) -> Self {
        let ChannelUpdate {
            device_id,
            entity,
            channel,
            value,
            ..
        } = update;
        Self {
            node: Some(names.node_name(device_id)),
            channel: Some(names.channel_name(device_id, *entity, *channel)),
            detail: None,
            value: Some(names.value_label(device_id, *entity, *channel, value)),
        }
    }

    /// For a rule run from `/rules`: stand-ins that read well in a message, named after what the
    /// trigger names.
    pub fn by_hand(trigger: &Trigger, names: &impl Names) -> Self {
        let node = |device: &Option<String>| {
            Some(
                device
                    .as_deref()
                    .map_or_else(|| "any node".to_string(), |device| names.node_name(device)),
            )
        };
        match trigger {
            Trigger::Offline(OfflineTrigger { device }) => Self {
                node: node(device),
                ..Self::default()
            },
            Trigger::Traffic(TrafficTrigger { kind, device }) => Self {
                node: node(device),
                channel: matches!(kind, AlertKind::Stopped | AlertKind::Noisy)
                    .then(|| "a channel".to_string()),
                detail: Some("run by hand".into()),
                value: None,
            },
            Trigger::Event(EventTrigger {
                device,
                entity,
                channel,
                data,
                ..
            }) => Self {
                node: Some(names.node_name(device)),
                channel: Some(names.channel_name(device, *entity, *channel)),
                detail: None,
                value: Some(data.clone().unwrap_or_else(|| "a value".into())),
            },
            Trigger::Threshold(ThresholdTrigger {
                device,
                entity,
                channel,
                limit: Limit::Above(limit) | Limit::Below(limit),
            }) => Self {
                node: Some(names.node_name(device)),
                channel: Some(names.channel_name(device, *entity, *channel)),
                detail: None,
                value: Some(names.value_label(device, *entity, *channel, &Value::Float(*limit))),
            },
            _ => Self::default(),
        }
    }

    /// The text with each placeholder replaced; one without a value stays as written.
    pub fn fill(&self, text: &str) -> String {
        let Ok(pieces) = pieces(text) else {
            return text.to_string();
        };
        let mut filled = String::new();
        for piece in pieces {
            match piece {
                Piece::Literal(literal) => filled.push_str(literal),
                Piece::Placeholder(name) => match self.placeholder(name) {
                    Some(value) => filled.push_str(value),
                    None => filled.push_str(&format!("{{{name}}}")),
                },
            }
        }
        filled
    }

    fn placeholder(&self, name: &str) -> Option<&str> {
        match name {
            "node" => self.node.as_deref(),
            "channel" => self.channel.as_deref(),
            "detail" => self.detail.as_deref(),
            "value" => self.value.as_deref(),
            _ => None,
        }
    }
}

enum Piece<'a> {
    Literal(&'a str),
    Placeholder(&'a str),
}

/// Splits a text into literal parts and `{placeholder}` names; `{{` and `}}` are literal braces.
fn pieces(text: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['{', '}']) {
        pieces.push(Piece::Literal(&rest[..start]));
        let brace = &rest[start..];
        if brace.starts_with("{{") || brace.starts_with("}}") {
            pieces.push(Piece::Literal(&brace[..1]));
            rest = &brace[2..];
        } else if brace.starts_with('}') {
            return Err(format!("a lone `}}` in \"{text}\" (`}}}}` is a brace)"));
        } else {
            let Some(end) = brace.find('}') else {
                return Err(format!(
                    "a `{{` without `}}` in \"{text}\" (`{{{{` is a brace)"
                ));
            };
            pieces.push(Piece::Placeholder(&brace[1..end]));
            rest = &brace[end + 1..];
        }
    }
    pieces.push(Piece::Literal(rest));
    Ok(pieces)
}

pub fn has_placeholders(text: &str) -> bool {
    pieces(text).is_ok_and(|pieces| {
        pieces
            .iter()
            .any(|piece| matches!(piece, Piece::Placeholder(_)))
    })
}

/// Every placeholder in `text` is one of `available`, the ones the rule's trigger fills.
pub fn check_placeholders(text: &str, available: &[&str]) -> Result<(), String> {
    for piece in pieces(text)? {
        if let Piece::Placeholder(name) = piece
            && !available.contains(&name)
        {
            let offer = match available {
                [] => "this trigger has no placeholders".to_string(),
                _ => {
                    let names: Vec<String> =
                        available.iter().map(|name| format!("{{{name}}}")).collect();
                    format!("this trigger has {}", names.join(", "))
                }
            };
            return Err(format!("{{{name}}} in \"{text}\": {offer}"));
        }
    }
    Ok(())
}

/// Runs the actions in order when every condition holds; a failed action does not stop the
/// next ones. Returns the condition that did not hold when it skips the rule.
pub async fn run(hub: Arc<Hub>, rule: Rule, subject: Subject) -> Option<Condition> {
    let at = Timestamp::now();
    let now = at.to_zoned(hub.time_zone().clone()).datetime();
    if let Some(condition) = rule
        .conditions
        .iter()
        .find(|condition| !condition.holds(now, hub.as_ref()))
    {
        println!("rule \"{}\": skipped, {condition} does not hold", rule.name);
        return Some(condition.clone());
    }
    let mut errors = Vec::new();
    for action in &rule.then {
        match perform(&hub, action, &rule.name, &subject).await {
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
    None
}

/// Returns what happened, for the log.
async fn perform(
    hub: &Hub,
    action: &Action,
    rule_name: &str,
    subject: &Subject,
) -> anyhow::Result<String> {
    let (device, entity, channel, value) = match action {
        Action::Notify { title, message } => {
            hub.notify(
                subject.fill(title),
                subject.fill(message),
                rule_name.to_string(),
            );
            return Ok("shown".into());
        }
        Action::Invoke {
            device,
            entity,
            channel,
            value,
        } => {
            hub.invoke(
                device,
                *entity,
                *channel,
                value.as_deref().map(|value| subject.fill(value)),
                CommandOrigin::Rule(rule_name.to_string()),
            )
            .await?;
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
    hub.set(
        device,
        entity,
        channel,
        Value::Bool(value),
        CommandOrigin::Rule(rule_name.to_string()),
    )
    .await?;
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::EventKind;

    use super::*;
    use crate::config::rules::when;

    struct Board;

    impl Names for Board {
        fn node_name(&self, device_id: &str) -> String {
            format!("Board {device_id}")
        }

        fn channel_name(&self, _: &str, entity: u8, channel: u8) -> String {
            format!("Sensor {entity}.{channel}")
        }

        fn value_label(&self, _: &str, _: u8, _: u8, value: &Value) -> String {
            match value {
                Value::Float(number) => format!("{number:.1} km/h"),
                other => format!("{other:?}"),
            }
        }
    }

    #[test]
    fn subject_fills_its_placeholders() {
        let subject = Subject {
            node: Some("Test board".into()),
            ..Subject::default()
        };
        assert_eq!(
            subject.fill("{node} › {channel} stopped"),
            "Test board › {channel} stopped"
        );
        assert_eq!(Subject::default().fill("{node}"), "{node}");
        assert_eq!(subject.fill("{{node}} is {node}"), "{node} is Test board");
        assert_eq!(subject.fill("a lone } stays"), "a lone } stays");
    }

    #[test]
    fn events_and_updates_fill_value() {
        let event = EventRecord {
            device_id: "a".into(),
            entity: 1,
            channel: 1,
            kind: EventKind::Received,
            data: Some(Value::Text("offline".into())),
        };
        assert_eq!(
            Subject::of_event(&event, &Board).fill("{node} › {channel}: {value}"),
            "Board a › Sensor 1.1: offline"
        );
        let update = ChannelUpdate {
            device_id: "weather".into(),
            entity: 1,
            channel: 3,
            value: Value::Float(52.25),
            confidence: oxihome_protocol::Confidence::Confirmed,
            previous: None,
        };
        assert_eq!(
            Subject::of_update(&update, &Board).fill("Wind at {value}"),
            "Wind at 52.2 km/h"
        );
    }

    #[test]
    fn a_rule_run_by_hand_reads_well() {
        let filled = |trigger: &str| {
            Subject::by_hand(&when(trigger).unwrap(), &Board)
                .fill("{node} › {channel}: {detail} {value}")
        };
        assert_eq!(
            filled(r#"{ stopped = "*" }"#),
            "any node › a channel: run by hand {value}"
        );
        assert_eq!(
            filled(r#"{ unanswered = "a" }"#),
            "Board a › {channel}: run by hand {value}"
        );
        assert_eq!(
            filled(r#"{ offline = "a" }"#),
            "Board a › {channel}: {detail} {value}"
        );
        assert_eq!(
            filled(r#"{ device = "w", entity = 1, channel = 3, above = 50 }"#),
            "Board w › Sensor 1.3: {detail} 50.0 km/h"
        );
        assert_eq!(
            filled(
                r#"{ device = "n", entity = 1, channel = 1, event = "Received", data = "online" }"#
            ),
            "Board n › Sensor 1.1: {detail} online"
        );
    }

    #[test]
    fn placeholders_are_checked_against_the_trigger() {
        let check = |text: &str| check_placeholders(text, &["node", "detail"]);
        assert!(check("{node}: {detail}").is_ok());
        assert!(check("{{value}} is literal").is_ok());
        assert_eq!(
            check("{value}").unwrap_err(),
            "{value} in \"{value}\": this trigger has {node}, {detail}"
        );
        assert_eq!(
            check_placeholders("at {node}", &[]).unwrap_err(),
            "{node} in \"at {node}\": this trigger has no placeholders"
        );
        assert!(check("{node").unwrap_err().contains("without `}`"));
        assert!(check("node}").unwrap_err().contains("a lone `}`"));
        assert!(has_placeholders("{node} went offline"));
        assert!(!has_placeholders("Alarm"));
        assert!(!has_placeholders("{{literal}}"));
    }
}
