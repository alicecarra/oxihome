//! A software node whose entities control the simulated nodes, so the hub's pages, dashboard and
//! rules can do what the console does. The hub only switches `Bool` channels and fires triggers,
//! so every control is one of those.

use std::collections::HashMap;
use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    Message, NAME_MAX_LENGTH, Value, ValueType,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::action::Action;
use crate::console;

const ONLINE_CHANNEL: u8 = 0;
const REBOOT_CHANNEL: u8 = 1;
const AUTOMATIC_CHANNEL: u8 = 2;
/// Text typed on the hub's page for a reading that is not text itself.
const TYPED_VALUE_LENGTH: u16 = 32;

pub struct ControlNode {
    core: NodeCore,
    device: ControlDevice,
    started_at: Instant,
}

struct ControlDevice {
    entities: Vec<EntityDescriptor>,
    /// By control entity: the simulated node it stands for.
    nodes: HashMap<u8, String>,
    targets: HashMap<(u8, u8), Target>,
    online: HashMap<String, bool>,
    actions: UnboundedSender<(String, Action)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Target {
    Online,
    Reboot,
    Automatic,
    Fix {
        entity: u8,
        channel: u8,
        value_type: ValueType,
    },
    Event {
        entity: u8,
        channel: u8,
        value_type: Option<ValueType>,
    },
}

impl ControlNode {
    /// One entity per simulated node, numbered from 1 in the order given.
    pub fn new(
        device_id: String,
        nodes: &[(String, Vec<EntityDescriptor>)],
        actions: UnboundedSender<(String, Action)>,
    ) -> Self {
        let mut device = ControlDevice {
            entities: Vec::new(),
            nodes: HashMap::new(),
            targets: HashMap::new(),
            online: HashMap::new(),
            actions,
        };
        for ((node, entities), id) in nodes.iter().zip(1..=u8::MAX) {
            let mut channels = vec![
                channel(ONLINE_CHANNEL, Access::ReadWrite, Some(ValueType::Bool)).named("Online"),
                channel(REBOOT_CHANNEL, Access::Trigger, None).named("Reboot"),
                channel(AUTOMATIC_CHANNEL, Access::Trigger, None).named("Automatic readings"),
            ];
            device.targets.insert((id, ONLINE_CHANNEL), Target::Online);
            device.targets.insert((id, REBOOT_CHANNEL), Target::Reboot);
            device
                .targets
                .insert((id, AUTOMATIC_CHANNEL), Target::Automatic);
            let controls: Vec<_> = entities
                .iter()
                .flat_map(|entity| {
                    entity
                        .channels
                        .iter()
                        .filter_map(move |declared| control(entity, declared))
                })
                .collect();
            let first_free = AUTOMATIC_CHANNEL + 1;
            if controls.len() > usize::from(u8::MAX - first_free) + 1 {
                println!("{node}: only the first controls fit in one entity");
            }
            for ((name, value_type, target), channel_id) in
                controls.into_iter().zip(first_free..=u8::MAX)
            {
                channels.push(channel(channel_id, Access::Trigger, value_type).named(name));
                device.targets.insert((id, channel_id), target);
            }
            device.entities.push(
                EntityDescriptor {
                    id,
                    name: None,
                    channels,
                }
                .named(node.clone()),
            );
            device.nodes.insert(id, node.clone());
            device.online.insert(node.clone(), true);
        }
        if nodes.len() > usize::from(u8::MAX) {
            println!("only the first {} nodes get controls", u8::MAX);
        }
        Self {
            core: NodeCore::new(
                device_id,
                Build::native("simulator", env!("CARGO_PKG_VERSION")),
            ),
            device,
            started_at: Instant::now(),
        }
    }

    pub fn entities(&self) -> &[EntityDescriptor] {
        &self.device.entities
    }

    /// Follows a node switched on or off from the console; the new state when it changed.
    pub fn reflect(&mut self, node: &str, online: bool) -> Vec<Message> {
        if self.device.online.insert(node.to_string(), online) == Some(online) {
            return Vec::new();
        }
        let entity = self
            .device
            .nodes
            .iter()
            .find(|(_, candidate)| *candidate == node)
            .map(|(entity, _)| *entity);
        entity
            .and_then(|entity| self.core.state(&self.device, entity))
            .into_iter()
            .collect()
    }
}

impl SoftwareNode for ControlNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.core.handle(&mut self.device, message)
    }

    fn invalid_message(&mut self, detail: String) -> Vec<Message> {
        vec![self.core.invalid_message(detail)]
    }

    fn announcement(&mut self) -> Vec<Message> {
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        vec![self.core.heartbeat(uptime_s, None, None)]
    }
}

impl Device for ControlDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        self.entities.clone()
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        let online = self.online.get(self.nodes.get(&entity)?)?;
        Some(vec![ChannelValue {
            channel: ONLINE_CHANNEL,
            value: Value::Bool(*online),
            confidence: Confidence::Confirmed,
        }])
    }

    /// Commands arrive checked against `entities`: right channel, operation and value.
    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp) -> Result<(), Rejection> {
        let (Some(node), Some(target)) = (
            self.nodes.get(&entity).cloned(),
            self.targets.get(&(entity, channel)),
        ) else {
            return Err(Rejection::new(
                ErrorCode::UnknownChannel,
                format!("no control {entity}.{channel}"),
            ));
        };
        let typed = |value_type: &ValueType, value: &Option<Value>| {
            let text = match value {
                Some(Value::Text(text) | Value::Enum(text)) => text.as_str(),
                _ => "",
            };
            console::parse_value(value_type, text)
                .map_err(|error| Rejection::new(ErrorCode::InvalidValue, error))
        };
        let action = match (target, op) {
            (Target::Online, CommandOp::Set(Value::Bool(online))) => {
                self.online.insert(node.clone(), *online);
                if *online {
                    Action::Online
                } else {
                    Action::Offline
                }
            }
            (Target::Reboot, CommandOp::Invoke(_)) => Action::Reboot,
            (Target::Automatic, CommandOp::Invoke(_)) => Action::Auto { channel: None },
            (
                Target::Fix {
                    entity,
                    channel,
                    value_type,
                },
                CommandOp::Invoke(value),
            ) => Action::Set {
                entity: *entity,
                channel: *channel,
                value: typed(value_type, value)?,
            },
            (
                Target::Event {
                    entity,
                    channel,
                    value_type,
                },
                CommandOp::Invoke(value),
            ) => Action::Event {
                entity: *entity,
                channel: Some(*channel),
                value: value_type
                    .as_ref()
                    .map(|value_type| typed(value_type, value))
                    .transpose()?,
            },
            _ => {
                return Err(Rejection::new(
                    ErrorCode::InvalidOperation,
                    format!("{op:?} does nothing on control {entity}.{channel}"),
                ));
            }
        };
        let _ = self.actions.send((node, action));
        Ok(())
    }
}

/// The control of a simulated channel: a fixed reading for what is read, the event for events.
fn control(
    entity: &EntityDescriptor,
    declared: &Channel,
) -> Option<(String, Option<ValueType>, Target)> {
    let label: Vec<&str> = [entity.name.as_deref(), declared.name.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let name = |verb: &str| {
        let name = format!("{verb} {}.{} {}", entity.id, declared.id, label.join(" / "));
        let mut end = name.trim_end().len().min(NAME_MAX_LENGTH);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name[..end].to_string()
    };
    match declared.access {
        Access::Read => {
            let value_type = declared.value.clone()?;
            Some((
                name("Fix"),
                Some(typed_as(&value_type)?),
                Target::Fix {
                    entity: entity.id,
                    channel: declared.id,
                    value_type,
                },
            ))
        }
        Access::Event => {
            let control_type = match &declared.value {
                None => None,
                Some(value_type) => Some(typed_as(value_type)?),
            };
            Some((
                name("Event"),
                control_type,
                Target::Event {
                    entity: entity.id,
                    channel: declared.id,
                    value_type: declared.value.clone(),
                },
            ))
        }
        Access::Write | Access::ReadWrite | Access::Trigger => None,
    }
}

/// What the hub sends for a value of `value_type`: options as buttons, the rest as text.
fn typed_as(value_type: &ValueType) -> Option<ValueType> {
    match value_type {
        ValueType::Enum(options) => Some(ValueType::Enum(options.clone())),
        ValueType::Bool => Some(ValueType::Enum(vec!["false".into(), "true".into()])),
        ValueType::Text { max_length } => Some(ValueType::Text {
            max_length: *max_length,
        }),
        ValueType::Int { .. } | ValueType::Float { .. } => Some(ValueType::Text {
            max_length: TYPED_VALUE_LENGTH,
        }),
        ValueType::Color(_) | ValueType::Bytes => None,
    }
}

fn channel(id: u8, access: Access, value: Option<ValueType>) -> Channel {
    Channel {
        id,
        name: None,
        access,
        value,
        quantity: None,
        unit: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use oxihome_protocol::Payload;
    use tokio::sync::mpsc::{self, UnboundedReceiver};

    use super::*;
    use crate::node::Profile;

    fn control_node() -> (ControlNode, UnboundedReceiver<(String, Action)>) {
        let living_room = [Profile::Lamp, Profile::Button, Profile::ClimateSensor]
            .iter()
            .zip(1..)
            .map(|(profile, id)| profile.descriptor(id))
            .collect();
        let infrared = EntityDescriptor {
            id: 6,
            name: None,
            channels: vec![channel(
                0,
                Access::Event,
                Some(ValueType::Text { max_length: 1024 }),
            )],
        };
        let nodes = [
            ("sim-living-room".to_string(), living_room),
            ("sim-remote".to_string(), vec![infrared]),
        ];
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            ControlNode::new("simulator".into(), &nodes, sender),
            receiver,
        )
    }

    /// A new message id each time: a repeated one gets the first answer again.
    fn command(entity: u8, channel: u8, op: CommandOp) -> Message {
        static MESSAGE_ID: AtomicU32 = AtomicU32::new(0);
        Message::new(
            "oxihome-hub",
            MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
            Payload::Command {
                entity,
                channel,
                op,
            },
        )
        .to("simulator")
    }

    fn invoke(entity: u8, channel: u8, text: &str) -> Message {
        command(
            entity,
            channel,
            CommandOp::Invoke(Some(Value::Text(text.into()))),
        )
    }

    #[test]
    fn controls_come_from_the_simulated_channels() {
        let (node, _) = control_node();
        let entities = node.entities();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].name.as_deref(), Some("sim-living-room"));
        let names: Vec<&str> = entities[0]
            .channels
            .iter()
            .map(|channel| channel.name.as_deref().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "Online",
                "Reboot",
                "Automatic readings",
                "Event 2.0 Button / Press",
                "Fix 3.0 Climate sensor / Temperature",
                "Fix 3.1 Climate sensor / Humidity",
            ]
        );
        assert_eq!(entities[0].channels[3].value, None);
        assert_eq!(
            entities[0].channels[4].value,
            Some(ValueType::Text { max_length: 32 })
        );
        assert_eq!(
            entities[1].channels[3].value,
            Some(ValueType::Text { max_length: 1024 })
        );
    }

    #[test]
    fn online_switch_sends_the_action_and_answers_the_state() {
        let (mut node, mut actions) = control_node();
        let answers = node.handle(&command(1, 0, CommandOp::Set(Value::Bool(false))));
        assert_eq!(answers[0].payload, Payload::Ack { ok: true });
        assert_eq!(
            answers[1].payload,
            Payload::State {
                entity: 1,
                values: vec![ChannelValue {
                    channel: 0,
                    value: Value::Bool(false),
                    confidence: Confidence::Confirmed,
                }],
            }
        );
        assert_eq!(
            actions.try_recv(),
            Ok(("sim-living-room".into(), Action::Offline))
        );
    }

    #[test]
    fn fixed_readings_are_typed_and_checked() {
        let (mut node, mut actions) = control_node();
        node.handle(&invoke(1, 4, "31"));
        assert_eq!(
            actions.try_recv(),
            Ok((
                "sim-living-room".into(),
                Action::Set {
                    entity: 3,
                    channel: 0,
                    value: Value::Float(31.0),
                }
            ))
        );
        for text in ["abc", "99"] {
            let answers = node.handle(&invoke(1, 4, text));
            assert!(
                matches!(answers[0].payload, Payload::Error { code, .. } if code == u16::from(ErrorCode::InvalidValue)),
                "{text}: {answers:?}"
            );
            assert!(actions.try_recv().is_err());
        }
    }

    #[test]
    fn events_and_node_controls_become_actions() {
        let (mut node, mut actions) = control_node();
        node.handle(&command(1, 3, CommandOp::Invoke(None)));
        node.handle(&invoke(2, 3, "NEC 00 45"));
        node.handle(&command(2, 1, CommandOp::Invoke(None)));
        let received: Vec<(String, Action)> =
            std::iter::from_fn(|| actions.try_recv().ok()).collect();
        assert_eq!(
            received,
            [
                (
                    "sim-living-room".to_string(),
                    Action::Event {
                        entity: 2,
                        channel: Some(0),
                        value: None,
                    }
                ),
                (
                    "sim-remote".to_string(),
                    Action::Event {
                        entity: 6,
                        channel: Some(0),
                        value: Some(Value::Text("NEC 00 45".into())),
                    }
                ),
                ("sim-remote".to_string(), Action::Reboot),
            ]
        );
    }

    #[test]
    fn console_changes_show_on_the_switch() {
        let (mut node, _) = control_node();
        assert_eq!(node.reflect("sim-remote", false).len(), 1);
        assert!(node.reflect("sim-remote", false).is_empty());
        assert_eq!(node.device.state(2).unwrap()[0].value, Value::Bool(false));
    }
}
