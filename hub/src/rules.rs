use std::fmt;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, bail};
use oxihome_protocol::{EventKind, Value};
use serde::Deserialize;

use crate::events::EventRecord;
use crate::hub::Hub;

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub name: String,
    pub when: EventTrigger,
    pub then: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EventTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub event: EventKind,
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

#[derive(Default, Deserialize)]
pub struct Rules {
    #[serde(default)]
    rules: Vec<Rule>,
}

impl Rules {
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn matching(&self, event: &EventRecord) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.when.device == event.device_id
                    && rule.when.entity == event.entity
                    && rule.when.channel == event.channel
                    && rule.when.event == event.kind
            })
            .collect()
    }
}

/// Runs the actions in order; a failed action does not stop the next ones.
pub async fn run(hub: Arc<Hub>, rule: Rule) {
    for action in &rule.then {
        match perform(&hub, action, &rule.name).await {
            Ok(outcome) => println!("rule \"{}\": {action} -> {outcome}", rule.name),
            Err(error) => println!("rule \"{}\": {action} failed: {error}", rule.name),
        }
    }
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
        assert_eq!(rules.len(), 2);
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
    fn empty_file_has_no_rules() {
        let rules: Rules = toml::from_str("").unwrap();
        assert_eq!(rules.len(), 0);
    }
}
