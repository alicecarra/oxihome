use std::fmt;
use std::sync::Arc;

use anyhow::bail;
use jiff::Timestamp;
use oxihome_protocol::Value;
use serde::Deserialize;

use super::{Condition, OfflineTrigger, Rule, RuleRun, TrafficTrigger, Trigger};
use crate::hub::{CommandOrigin, Hub};
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

/// What a rule runs about, for `{node}`, `{channel}` and `{detail}` in the text of its `notify`
/// and `invoke` actions.
#[derive(Debug, Clone, Default)]
pub struct Subject {
    pub node: Option<String>,
    pub channel: Option<String>,
    pub detail: Option<String>,
}

impl Subject {
    /// For a rule run from `/rules`: stand-ins that read well in a message, named after what the
    /// trigger names. `node_name` gives a node's name from its id.
    pub fn by_hand(trigger: &Trigger, node_name: impl Fn(&str) -> String) -> Self {
        let node = |device: &Option<String>| {
            Some(
                device
                    .as_deref()
                    .map_or_else(|| "any node".to_string(), &node_name),
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
            },
            _ => Self::default(),
        }
    }

    fn fill(&self, text: &str) -> String {
        let mut text = text.to_string();
        for (placeholder, value) in [
            ("{node}", &self.node),
            ("{channel}", &self.channel),
            ("{detail}", &self.detail),
        ] {
            if let Some(value) = value {
                text = text.replace(placeholder, value);
            }
        }
        text
    }
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
    use super::*;
    use crate::config::rules::when;

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
    }

    #[test]
    fn a_rule_run_by_hand_reads_well() {
        let name = |device: &str| format!("Board {device}");
        let filled = |trigger: &str| {
            Subject::by_hand(&when(trigger).unwrap(), name).fill("{node} › {channel}: {detail}")
        };
        assert_eq!(
            filled(r#"{ stopped = "*" }"#),
            "any node › a channel: run by hand"
        );
        assert_eq!(
            filled(r#"{ unanswered = "a" }"#),
            "Board a › {channel}: run by hand"
        );
        assert_eq!(
            filled(r#"{ offline = "a" }"#),
            "Board a › {channel}: {detail}"
        );
    }
}
