//! What the pages show, built from a `Hub::snapshot`.

use std::time::{Duration, Instant};

use oxihome_protocol::management::{self, PARAMETERS, Parameter};
use oxihome_protocol::{Access, EventKind, Quantity, Unit, Value, ValueType};

use super::format::{channel_label, entity_label};
use crate::config::names::{NameTarget, Names};
use crate::config::rules::{Action, EventTrigger};
use crate::hub::{Hub, Snapshot};

pub struct NodeOverview {
    pub device_id: String,
    pub name: Option<String>,
    pub firmware_version: String,
    pub online: bool,
    pub uptime_seconds: Option<u32>,
    pub rssi: Option<i8>,
    pub channels: Vec<ChannelOverview>,
}

pub struct NodeParametersOverview {
    pub device_id: String,
    pub name: Option<String>,
    pub parameters: Vec<ParameterOverview>,
}

pub struct ParameterOverview {
    pub parameter: Parameter,
    /// As reported by the node.
    pub current: Option<i32>,
    /// From `nodes.toml`; `None` leaves the node's value alone.
    pub desired: Option<i32>,
}

pub struct ChannelOverview {
    pub entity: u8,
    pub entity_name: Option<String>,
    pub channel: u8,
    pub channel_name: Option<String>,
    pub has_value: bool,
    pub value: Option<Value>,
    pub last_event: Option<(EventKind, Duration)>,
    pub switchable: bool,
    pub quantity: Option<Quantity>,
    pub unit: Option<Unit>,
}

pub struct RuleOverview {
    pub name: String,
    pub trigger: String,
    pub actions: Vec<String>,
    pub enabled: bool,
    /// How long ago it ran and the errors of the actions that failed.
    pub last_run: Option<(Duration, Vec<String>)>,
}

/// One entry per registered node, for the nodes page.
pub fn nodes(hub: &Hub) -> Vec<NodeOverview> {
    let Snapshot {
        state,
        registry,
        events,
        names,
        ..
    } = &hub.snapshot();
    let now = Instant::now();
    let name = |target: NameTarget| names.get(&target).map(String::from);
    registry
        .nodes()
        .into_iter()
        .map(|(device_id, node)| NodeOverview {
            device_id: device_id.clone(),
            name: name(NameTarget::Node(device_id.clone())),
            firmware_version: node.firmware_version.clone(),
            online: node.online,
            uptime_seconds: node.uptime_seconds,
            rssi: node.rssi,
            channels: node
                .entities
                .iter()
                .flat_map(|entity| {
                    entity.channels.iter().map(|channel| ChannelOverview {
                        entity: entity.id,
                        entity_name: name(NameTarget::Entity(device_id.clone(), entity.id)),
                        channel: channel.id,
                        channel_name: name(NameTarget::Channel(
                            device_id.clone(),
                            entity.id,
                            channel.id,
                        )),
                        has_value: channel.value.is_some(),
                        value: state.value(device_id, entity.id, channel.id).cloned(),
                        last_event: events
                            .last(device_id, entity.id, channel.id)
                            .map(|(kind, at)| (kind, now - at)),
                        switchable: channel.value == Some(ValueType::Bool)
                            && matches!(channel.access, Access::Write | Access::ReadWrite),
                        quantity: channel.quantity,
                        unit: channel.unit,
                    })
                })
                .collect(),
        })
        .collect()
}

/// Nodes that declare the management entity, for the settings page.
pub fn parameters(hub: &Hub) -> Vec<NodeParametersOverview> {
    let Snapshot {
        state,
        registry,
        names,
        parameters,
        ..
    } = &hub.snapshot();
    registry
        .nodes()
        .into_iter()
        .filter(|(_, node)| {
            node.entities
                .iter()
                .any(|entity| entity.id == management::ENTITY)
        })
        .map(|(device_id, node)| {
            let desired = parameters.desired(device_id);
            NodeParametersOverview {
                device_id: device_id.clone(),
                name: names
                    .get(&NameTarget::Node(device_id.clone()))
                    .map(String::from),
                parameters: PARAMETERS
                    .into_iter()
                    .filter(|parameter| node.declares(management::ENTITY, parameter.channel.into()))
                    .map(|parameter| ParameterOverview {
                        parameter,
                        current: match state.value(
                            device_id,
                            management::ENTITY,
                            parameter.channel.into(),
                        ) {
                            Some(Value::Int(value)) => Some(*value),
                            _ => None,
                        },
                        desired: desired
                            .iter()
                            .find(|(known, _)| *known == parameter)
                            .map(|(_, value)| *value),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Every rule in `rules.toml`, described with friendly names, for the rules page.
pub fn rules(hub: &Hub) -> Vec<RuleOverview> {
    let rules = hub.rules();
    let mut runs = hub.rule_runs();
    let snapshot = hub.snapshot();
    let now = Instant::now();
    rules
        .into_iter()
        .map(|rule| RuleOverview {
            trigger: trigger_description(&snapshot.names, &rule.when),
            actions: rule
                .then
                .iter()
                .map(|action| action_description(&snapshot.names, action))
                .collect(),
            last_run: runs
                .remove(&rule.name)
                .map(|run| (now - run.at, run.errors)),
            name: rule.name,
            enabled: rule.enabled,
        })
        .collect()
}

fn trigger_description(names: &Names, trigger: &EventTrigger) -> String {
    let target = channel_description(names, &trigger.device, trigger.entity, trigger.channel);
    format!("{target}: {:?}", trigger.event)
}

fn action_description(names: &Names, action: &Action) -> String {
    match action {
        Action::Set {
            device,
            entity,
            channel,
            value,
        } => format!(
            "Set {} to {}",
            channel_description(names, device, *entity, *channel),
            if *value { "on" } else { "off" }
        ),
        Action::Toggle {
            device,
            entity,
            channel,
        } => format!(
            "Toggle {}",
            channel_description(names, device, *entity, *channel)
        ),
        Action::Notify { title, message } => format!("Notify \"{title}\": {message}"),
    }
}

/// `Test board › LED`; the channel only shows when it is not the entity's first.
fn channel_description(names: &Names, device_id: &str, entity: u8, channel: u8) -> String {
    let name = |target: NameTarget| names.get(&target).map(String::from);
    let node =
        name(NameTarget::Node(device_id.to_string())).unwrap_or_else(|| device_id.to_string());
    let entity_name = entity_label(
        entity,
        &name(NameTarget::Entity(device_id.to_string(), entity)),
    );
    if channel == 0 {
        return format!("{node} › {entity_name}");
    }
    let channel_name = channel_label(
        channel,
        &name(NameTarget::Channel(device_id.to_string(), entity, channel)),
        None,
    );
    format!("{node} › {entity_name} · {channel_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxihome_protocol::EventKind;

    const NAMES: &str = r#"
        [nodes."esp32-test"]
        name = "Test board"
        entities.1 = { name = "LED", channels.1 = "Brightness" }
        entities.2 = { name = "BOOT button" }
    "#;

    fn names() -> Names {
        toml::from_str(NAMES).unwrap()
    }

    #[test]
    fn trigger_uses_friendly_names() {
        let trigger = EventTrigger {
            device: "esp32-test".into(),
            entity: 2,
            channel: 0,
            event: EventKind::LongPress,
        };
        assert_eq!(
            trigger_description(&names(), &trigger),
            "Test board › BOOT button: LongPress"
        );
    }

    #[test]
    fn unnamed_targets_fall_back_to_ids() {
        let trigger = EventTrigger {
            device: "sim-unknown".into(),
            entity: 3,
            channel: 0,
            event: EventKind::Press,
        };
        assert_eq!(
            trigger_description(&names(), &trigger),
            "sim-unknown › entity 3: Press"
        );
    }

    #[test]
    fn channels_after_the_first_are_shown() {
        let toggle = Action::Toggle {
            device: "esp32-test".into(),
            entity: 1,
            channel: 1,
        };
        assert_eq!(
            action_description(&names(), &toggle),
            "Toggle Test board › LED · Brightness"
        );
    }

    #[test]
    fn set_and_notify_descriptions() {
        let set = Action::Set {
            device: "esp32-test".into(),
            entity: 1,
            channel: 0,
            value: false,
        };
        let notify = Action::Notify {
            title: "Good night".into(),
            message: "All lamps are off".into(),
        };
        assert_eq!(
            action_description(&names(), &set),
            "Set Test board › LED to off"
        );
        assert_eq!(
            action_description(&names(), &notify),
            "Notify \"Good night\": All lamps are off"
        );
    }
}
