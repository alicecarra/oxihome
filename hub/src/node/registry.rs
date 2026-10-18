use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use oxihome_protocol::management::{self, HEARTBEAT_INTERVAL};
use oxihome_protocol::{
    Access, Build, Channel, EntityDescriptor, Message, NAME_MAX_LENGTH, Payload, Value, ValueType,
};

use crate::config::nodes::NameTarget;

pub const DEFAULT_MISSED_HEARTBEATS: u32 = 3;

pub struct Node {
    pub build: Build,
    /// Of its last `Hello`; older than `PROTOCOL_VERSION` means it needs an update.
    pub protocol_version: u8,
    pub entities: Vec<EntityDescriptor>,
    pub online: bool,
    /// For presence.
    pub last_seen: Instant,
    /// For showing and storing; survives a restart.
    pub last_seen_at: Timestamp,
    pub uptime_seconds: Option<u32>,
    pub rssi: Option<i8>,
    /// From the node's `HeartbeatInterval`; the default until it reports one.
    heartbeat_interval: Duration,
    /// From before the hub restarted and silent since: its `Hello` is not a return.
    restored: bool,
}

impl Node {
    pub fn declares(&self, entity: u8, channel: u8) -> bool {
        self.entities.iter().any(|descriptor| {
            descriptor.id == entity && descriptor.channels.iter().any(|known| known.id == channel)
        })
    }

    pub fn channel(&self, entity: u8, channel: u8) -> Option<&Channel> {
        self.entities
            .iter()
            .find(|descriptor| descriptor.id == entity)?
            .channels
            .iter()
            .find(|known| known.id == channel)
    }

    /// The value to send to a `Trigger` channel the node declares: an option of its `Enum`, a
    /// `Text` up to its maximum length, or nothing for a channel without a value.
    pub fn invoke_value(
        &self,
        entity: u8,
        channel: u8,
        value: Option<&str>,
    ) -> Result<Option<Value>, String> {
        let Some(declared) = self.channel(entity, channel) else {
            return Err(format!("the node has no channel {entity}/{channel}"));
        };
        if declared.access != Access::Trigger {
            return Err(format!("channel {entity}/{channel} is not a Trigger"));
        }
        match (&declared.value, value) {
            (None, None) => Ok(None),
            (None, Some(value)) => Err(format!(
                "channel {entity}/{channel} takes no value, got \"{value}\""
            )),
            (Some(ValueType::Enum(options)), Some(value))
                if options.iter().any(|option| option == value) =>
            {
                Ok(Some(Value::Enum(value.to_string())))
            }
            (Some(ValueType::Enum(options)), _) => Err(format!(
                "{} is not one of {}",
                value.map_or("no value".to_string(), |value| format!("\"{value}\"")),
                options.join(", ")
            )),
            (Some(ValueType::Text { max_length }), Some(value))
                if value.len() <= usize::from(*max_length) =>
            {
                Ok(Some(Value::Text(value.to_string())))
            }
            (Some(ValueType::Text { max_length }), Some(value)) => Err(format!(
                "text of {} bytes is longer than the {max_length} channel {entity}/{channel} takes",
                value.len()
            )),
            (Some(ValueType::Text { .. }), None) => {
                Err(format!("channel {entity}/{channel} needs a text"))
            }
            (Some(other), _) => Err(format!(
                "channel {entity}/{channel} takes {other:?}, not a name or text"
            )),
        }
    }

    fn offline_after(&self, missed_heartbeats: u32) -> Duration {
        self.heartbeat_interval * missed_heartbeats
    }
}

/// What the hub keeps of a node across restarts.
#[derive(Debug, Clone)]
pub struct StoredNode {
    pub device_id: String,
    pub build: Build,
    pub protocol_version: u8,
    pub entities: Vec<EntityDescriptor>,
    pub last_seen: Timestamp,
    pub uptime_seconds: Option<u32>,
    pub rssi: Option<i8>,
    pub heartbeat_interval: Duration,
}

#[derive(Debug, PartialEq)]
pub enum NodeEvent {
    Registered {
        device_id: String,
        build: Build,
        entity_count: usize,
    },
    Online(String),
    Rebooted {
        device_id: String,
        previous_uptime: u32,
        uptime: u32,
    },
    Offline(String),
    Unknown(String),
}

impl NodeEvent {
    pub fn device_id(&self) -> &str {
        match self {
            NodeEvent::Registered { device_id, .. } | NodeEvent::Rebooted { device_id, .. } => {
                device_id
            }
            NodeEvent::Online(device_id)
            | NodeEvent::Offline(device_id)
            | NodeEvent::Unknown(device_id) => device_id,
        }
    }
}

impl fmt::Display for NodeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            NodeEvent::Registered {
                device_id,
                build,
                entity_count,
            } => write!(
                formatter,
                "{device_id} online ({build}, {entity_count} entities)"
            ),
            NodeEvent::Online(device_id) => write!(formatter, "{device_id} online"),
            NodeEvent::Rebooted {
                device_id,
                previous_uptime,
                uptime,
            } => write!(
                formatter,
                "{device_id} rebooted (uptime {previous_uptime} s -> {uptime} s)"
            ),
            NodeEvent::Offline(device_id) => write!(formatter, "{device_id} offline"),
            NodeEvent::Unknown(device_id) => {
                write!(formatter, "{device_id} unknown, asking for its Hello")
            }
        }
    }
}

pub struct Registry {
    nodes: HashMap<String, Node>,
    asked_for_hello: HashSet<String>,
    /// Heartbeats a node may miss before it counts as offline.
    missed_heartbeats: u32,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new(DEFAULT_MISSED_HEARTBEATS)
    }
}

impl Registry {
    pub fn new(missed_heartbeats: u32) -> Self {
        Self {
            nodes: HashMap::new(),
            asked_for_hello: HashSet::new(),
            missed_heartbeats,
        }
    }

    /// Sorted by device id.
    pub fn nodes(&self) -> Vec<(&String, &Node)> {
        let mut nodes: Vec<_> = self.nodes.iter().collect();
        nodes.sort_by_key(|(device_id, _)| *device_id);
        nodes
    }

    pub fn node(&self, device_id: &str) -> Option<&Node> {
        self.nodes.get(device_id)
    }

    /// What the node calls an entity or channel in its `Hello`; an empty or too long name counts as
    /// none.
    pub fn declared_name(&self, target: &NameTarget) -> Option<&str> {
        let name = match target {
            NameTarget::Node(_) => None,
            NameTarget::Entity(device_id, entity) => self
                .node(device_id)?
                .entities
                .iter()
                .find(|descriptor| descriptor.id == *entity)?
                .name
                .as_deref(),
            NameTarget::Channel(device_id, entity, channel) => self
                .node(device_id)?
                .channel(*entity, *channel)?
                .name
                .as_deref(),
        }?;
        (!name.trim().is_empty() && name.len() <= NAME_MAX_LENGTH).then_some(name)
    }

    pub fn stored(&self, device_id: &str) -> Option<StoredNode> {
        self.nodes.get(device_id).map(|node| StoredNode {
            device_id: device_id.to_string(),
            build: node.build.clone(),
            protocol_version: node.protocol_version,
            entities: node.entities.clone(),
            last_seen: node.last_seen_at,
            uptime_seconds: node.uptime_seconds,
            rssi: node.rssi,
            heartbeat_interval: node.heartbeat_interval,
        })
    }

    /// A node from before the restart, offline until it talks again.
    pub fn restore(&mut self, stored: StoredNode, now: Instant) {
        self.nodes.insert(
            stored.device_id,
            Node {
                build: stored.build,
                protocol_version: stored.protocol_version,
                entities: stored.entities,
                online: false,
                last_seen: now,
                last_seen_at: stored.last_seen,
                uptime_seconds: stored.uptime_seconds,
                rssi: stored.rssi,
                heartbeat_interval: stored.heartbeat_interval,
                restored: true,
            },
        );
    }

    pub fn handle(&mut self, message: &Message, now: Instant) -> Vec<NodeEvent> {
        if let Payload::Hello {
            build, entities, ..
        } = &message.payload
        {
            // A rebooted node sends `Hello` before its first heartbeat: keep the last
            // uptime so that heartbeat can still reveal the reboot.
            let previous = self.nodes.get(&message.src);
            let came_back = previous.is_some_and(|node| !node.online && !node.restored);
            let (uptime_seconds, rssi, heartbeat_interval) = previous
                .map_or((None, None, default_heartbeat_interval()), |node| {
                    (node.uptime_seconds, node.rssi, node.heartbeat_interval)
                });
            self.nodes.insert(
                message.src.clone(),
                Node {
                    build: build.clone(),
                    protocol_version: message.version,
                    entities: entities.clone(),
                    online: true,
                    last_seen: now,
                    last_seen_at: Timestamp::now(),
                    uptime_seconds,
                    rssi,
                    heartbeat_interval,
                    restored: false,
                },
            );
            self.asked_for_hello.remove(&message.src);
            let mut events = Vec::new();
            if came_back {
                events.push(NodeEvent::Online(message.src.clone()));
            }
            events.push(NodeEvent::Registered {
                device_id: message.src.clone(),
                build: build.clone(),
                entity_count: entities.len(),
            });
            return events;
        }

        let Some(node) = self.nodes.get_mut(&message.src) else {
            return self
                .asked_for_hello
                .insert(message.src.clone())
                .then(|| NodeEvent::Unknown(message.src.clone()))
                .into_iter()
                .collect();
        };

        let mut events = Vec::new();
        node.last_seen = now;
        node.last_seen_at = Timestamp::now();
        node.restored = false;
        if !node.online {
            node.online = true;
            events.push(NodeEvent::Online(message.src.clone()));
        }

        if let Payload::State {
            entity: management::ENTITY,
            values,
        } = &message.payload
        {
            let reported = values.iter().find_map(|channel_value| {
                match (
                    management::parameter(channel_value.channel),
                    &channel_value.value,
                ) {
                    (Some(HEARTBEAT_INTERVAL), Value::Int(seconds)) => u64::try_from(*seconds).ok(),
                    _ => None,
                }
            });
            if let Some(seconds) = reported {
                node.heartbeat_interval = Duration::from_secs(seconds);
            }
        }

        if let Payload::Heartbeat { uptime_s, rssi, .. } = &message.payload {
            node.rssi = *rssi;
            let previous = node.uptime_seconds.replace(*uptime_s);
            if let Some(previous) = previous.filter(|previous| uptime_s < previous) {
                events.push(NodeEvent::Rebooted {
                    device_id: message.src.clone(),
                    previous_uptime: previous,
                    uptime: *uptime_s,
                });
            }
        }
        events
    }

    pub fn check_presence(&mut self, now: Instant) -> Vec<NodeEvent> {
        self.nodes
            .iter_mut()
            .filter(|(_, node)| {
                node.online && now - node.last_seen >= node.offline_after(self.missed_heartbeats)
            })
            .map(|(device_id, node)| {
                node.online = false;
                NodeEvent::Offline(device_id.clone())
            })
            .collect()
    }
}

fn default_heartbeat_interval() -> Duration {
    Duration::from_secs(HEARTBEAT_INTERVAL.default.unsigned_abs().into())
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::{ChannelValue, Confidence};

    use super::*;

    /// Three heartbeats at the default interval.
    const OFFLINE_AFTER: Duration = Duration::from_secs(90);

    const DEVICE: &str = "esp32-test";

    fn message(payload: Payload) -> Message {
        Message::new(DEVICE, 0, payload)
    }

    fn hello() -> Message {
        message(Payload::Hello {
            build: Build::new("firmware", "0.3.1", "esp32"),
            ip: None,
            entities: vec![],
        })
    }

    fn heartbeat(uptime_s: u32) -> Message {
        message(Payload::Heartbeat {
            uptime_s,
            rssi: Some(-50),
            battery: None,
        })
    }

    fn heartbeat_interval(seconds: i32) -> Message {
        message(Payload::State {
            entity: management::ENTITY,
            values: vec![ChannelValue {
                channel: HEARTBEAT_INTERVAL.channel.into(),
                value: Value::Int(seconds),
                confidence: Confidence::Confirmed,
            }],
        })
    }

    fn buzzer_node() -> Node {
        let mut registry = Registry::default();
        registry.handle(
            &message(Payload::Hello {
                build: Build::new("firmware", "0.7.0", "esp32"),
                ip: None,
                entities: vec![EntityDescriptor {
                    id: 5,
                    name: None,
                    channels: vec![
                        Channel {
                            id: 0,
                            name: None,
                            access: Access::Trigger,
                            value: Some(ValueType::Enum(vec!["Short".into(), "Alarm".into()])),
                            quantity: None,
                            unit: None,
                        },
                        Channel {
                            id: 1,
                            name: None,
                            access: Access::Trigger,
                            value: None,
                            quantity: None,
                            unit: None,
                        },
                        Channel {
                            id: 3,
                            name: None,
                            access: Access::Trigger,
                            value: Some(ValueType::Text { max_length: 5 }),
                            quantity: None,
                            unit: None,
                        },
                        Channel {
                            id: 2,
                            name: None,
                            access: Access::ReadWrite,
                            value: Some(ValueType::Bool),
                            quantity: None,
                            unit: None,
                        },
                    ],
                }],
            }),
            Instant::now(),
        );
        registry.nodes.remove(DEVICE).unwrap()
    }

    #[test]
    fn invoke_checks_the_declared_options() {
        let node = buzzer_node();
        assert_eq!(
            node.invoke_value(5, 0, Some("Alarm")),
            Ok(Some(Value::Enum("Alarm".into())))
        );
        assert_eq!(node.invoke_value(5, 1, None), Ok(None));
        assert_eq!(
            node.invoke_value(5, 0, Some("Siren")),
            Err("\"Siren\" is not one of Short, Alarm".into())
        );
        assert_eq!(
            node.invoke_value(5, 0, None),
            Err("no value is not one of Short, Alarm".into())
        );
        assert!(
            node.invoke_value(5, 1, Some("Alarm"))
                .unwrap_err()
                .contains("takes no value")
        );
        assert!(
            node.invoke_value(5, 2, None)
                .unwrap_err()
                .contains("not a Trigger")
        );
        assert!(
            node.invoke_value(6, 0, None)
                .unwrap_err()
                .contains("no channel 6/0")
        );
    }

    #[test]
    fn invoke_sends_text_up_to_the_declared_length() {
        let node = buzzer_node();
        assert_eq!(
            node.invoke_value(5, 3, Some("hello")),
            Ok(Some(Value::Text("hello".into())))
        );
        assert!(
            node.invoke_value(5, 3, Some("hello!"))
                .unwrap_err()
                .contains("6 bytes is longer than the 5")
        );
        assert!(
            node.invoke_value(5, 3, None)
                .unwrap_err()
                .contains("needs a text")
        );
    }

    #[test]
    fn reported_heartbeat_interval_sets_the_offline_timeout() {
        let mut registry = Registry::default();
        let start = Instant::now();
        registry.handle(&hello(), start);
        registry.handle(&heartbeat_interval(10), start);

        assert!(
            registry
                .check_presence(start + Duration::from_secs(29))
                .is_empty()
        );
        assert_eq!(
            registry.check_presence(start + Duration::from_secs(30)),
            vec![NodeEvent::Offline(DEVICE.into())]
        );
    }

    #[test]
    fn missed_heartbeats_are_configurable() {
        let mut registry = Registry::new(5);
        let start = Instant::now();
        registry.handle(&hello(), start);
        registry.handle(&heartbeat_interval(10), start);

        assert!(
            registry
                .check_presence(start + Duration::from_secs(49))
                .is_empty()
        );
        assert_eq!(
            registry.check_presence(start + Duration::from_secs(50)),
            vec![NodeEvent::Offline(DEVICE.into())]
        );
    }

    #[test]
    fn hello_keeps_the_heartbeat_interval() {
        let mut registry = Registry::default();
        let start = Instant::now();
        registry.handle(&hello(), start);
        registry.handle(&heartbeat_interval(100), start);
        registry.handle(&hello(), start);

        assert!(registry.check_presence(start + OFFLINE_AFTER).is_empty());
    }

    #[test]
    fn hello_registers_the_node() {
        let mut registry = Registry::default();
        let event = registry.handle(&hello(), Instant::now());
        assert_eq!(
            event,
            vec![NodeEvent::Registered {
                device_id: DEVICE.into(),
                build: Build::new("firmware", "0.3.1", "esp32"),
                entity_count: 0,
            }]
        );
    }

    #[test]
    fn silent_node_goes_offline() {
        let mut registry = Registry::default();
        let start = Instant::now();
        registry.handle(&hello(), start);

        assert!(
            registry
                .check_presence(start + OFFLINE_AFTER / 2)
                .is_empty()
        );
        assert_eq!(
            registry.check_presence(start + OFFLINE_AFTER),
            vec![NodeEvent::Offline(DEVICE.into())]
        );
        assert!(
            registry
                .check_presence(start + OFFLINE_AFTER * 2)
                .is_empty()
        );
    }

    #[test]
    fn offline_node_comes_back_online() {
        let mut registry = Registry::default();
        let start = Instant::now();
        registry.handle(&hello(), start);
        registry.check_presence(start + OFFLINE_AFTER);

        assert_eq!(
            registry.handle(&heartbeat(10), start + OFFLINE_AFTER * 2),
            vec![NodeEvent::Online(DEVICE.into())]
        );
        assert!(
            registry
                .handle(&heartbeat(40), start + OFFLINE_AFTER * 2)
                .is_empty()
        );
    }

    #[test]
    fn offline_node_comes_back_with_its_hello() {
        let mut registry = Registry::default();
        let start = Instant::now();
        registry.handle(&hello(), start);
        registry.check_presence(start + OFFLINE_AFTER);

        assert_eq!(
            registry.handle(&hello(), start + OFFLINE_AFTER * 2),
            vec![
                NodeEvent::Online(DEVICE.into()),
                NodeEvent::Registered {
                    device_id: DEVICE.into(),
                    build: Build::new("firmware", "0.3.1", "esp32"),
                    entity_count: 0,
                },
            ]
        );
        assert_eq!(
            registry.handle(&hello(), start + OFFLINE_AFTER * 2).len(),
            1
        );
    }

    #[test]
    fn hello_after_a_hub_restart_is_not_a_return() {
        let mut source = Registry::default();
        let start = Instant::now();
        source.handle(&hello(), start);
        let mut registry = Registry::default();
        registry.restore(source.stored(DEVICE).unwrap(), start);

        assert!(matches!(
            registry.handle(&hello(), start).as_slice(),
            [NodeEvent::Registered { .. }]
        ));
    }

    #[test]
    fn uptime_drop_is_a_reboot() {
        let mut registry = Registry::default();
        let now = Instant::now();
        registry.handle(&hello(), now);
        registry.handle(&heartbeat(100), now);

        assert_eq!(
            registry.handle(&heartbeat(5), now),
            vec![NodeEvent::Rebooted {
                device_id: DEVICE.into(),
                previous_uptime: 100,
                uptime: 5,
            }]
        );
    }

    #[test]
    fn reboot_is_detected_after_the_new_hello() {
        let mut registry = Registry::default();
        let now = Instant::now();
        registry.handle(&hello(), now);
        registry.handle(&heartbeat(100), now);
        registry.handle(&hello(), now);

        assert_eq!(
            registry.handle(&heartbeat(2), now),
            vec![NodeEvent::Rebooted {
                device_id: DEVICE.into(),
                previous_uptime: 100,
                uptime: 2,
            }]
        );
    }

    #[test]
    fn reconnecting_without_reboot_is_not_a_reboot() {
        let mut registry = Registry::default();
        let now = Instant::now();
        registry.handle(&hello(), now);
        registry.handle(&heartbeat(100), now);
        registry.handle(&hello(), now);

        assert!(registry.handle(&heartbeat(130), now).is_empty());
    }

    #[test]
    fn growing_uptime_is_not_a_reboot() {
        let mut registry = Registry::default();
        let now = Instant::now();
        registry.handle(&hello(), now);
        registry.handle(&heartbeat(100), now);

        assert!(registry.handle(&heartbeat(130), now).is_empty());
    }

    #[test]
    fn offline_node_that_rebooted_reports_both() {
        let mut registry = Registry::default();
        let start = Instant::now();
        registry.handle(&hello(), start);
        registry.handle(&heartbeat(100), start);
        registry.check_presence(start + OFFLINE_AFTER);

        assert_eq!(
            registry.handle(&heartbeat(3), start + OFFLINE_AFTER * 2),
            vec![
                NodeEvent::Online(DEVICE.into()),
                NodeEvent::Rebooted {
                    device_id: DEVICE.into(),
                    previous_uptime: 100,
                    uptime: 3,
                },
            ]
        );
    }

    #[test]
    fn unknown_node_is_asked_once() {
        let mut registry = Registry::default();
        let now = Instant::now();

        assert_eq!(
            registry.handle(&heartbeat(10), now),
            vec![NodeEvent::Unknown(DEVICE.into())]
        );
        assert!(registry.handle(&heartbeat(40), now).is_empty());
    }

    #[test]
    fn hello_allows_asking_again() {
        let mut registry = Registry::default();
        let now = Instant::now();
        registry.handle(&heartbeat(10), now);
        registry.handle(&hello(), now);

        assert!(!registry.asked_for_hello.contains(DEVICE));
    }

    #[test]
    fn declared_names() {
        let channel = |id: u8, name: &str| {
            Channel {
                id,
                name: None,
                access: Access::Read,
                value: Some(ValueType::Bool),
                quantity: None,
                unit: None,
            }
            .named(name)
        };
        let mut registry = Registry::default();
        registry.handle(
            &message(Payload::Hello {
                build: Build::new("rss", "0.2.0", "linux-x86_64"),
                ip: None,
                entities: vec![
                    EntityDescriptor {
                        id: 2,
                        name: None,
                        channels: vec![
                            channel(0, "Reachable"),
                            channel(1, ""),
                            channel(2, &"x".repeat(NAME_MAX_LENGTH + 1)),
                        ],
                    }
                    .named("Mosquitto releases"),
                ],
            }),
            Instant::now(),
        );
        let name = |target: NameTarget| registry.declared_name(&target);

        assert_eq!(
            name(NameTarget::Entity(DEVICE.into(), 2)),
            Some("Mosquitto releases")
        );
        assert_eq!(
            name(NameTarget::Channel(DEVICE.into(), 2, 0)),
            Some("Reachable")
        );
        assert_eq!(name(NameTarget::Channel(DEVICE.into(), 2, 1)), None);
        assert_eq!(name(NameTarget::Channel(DEVICE.into(), 2, 2)), None);
        assert_eq!(name(NameTarget::Channel(DEVICE.into(), 2, 3)), None);
        assert_eq!(name(NameTarget::Node(DEVICE.into())), None);
    }
}
