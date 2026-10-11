use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::{Duration, Instant};

use oxihome_protocol::management::{self, HEARTBEAT_INTERVAL};
use oxihome_protocol::{EntityDescriptor, Message, Payload, Value};

pub const DEFAULT_MISSED_HEARTBEATS: u32 = 3;

pub struct Node {
    pub firmware_version: String,
    pub entities: Vec<EntityDescriptor>,
    pub online: bool,
    last_seen: Instant,
    pub uptime_seconds: Option<u32>,
    pub rssi: Option<i8>,
    /// From the node's `HeartbeatInterval`; the default until it reports one.
    heartbeat_interval: Duration,
}

impl Node {
    pub fn declares(&self, entity: u8, channel: u8) -> bool {
        self.entities.iter().any(|descriptor| {
            descriptor.id == entity && descriptor.channels.iter().any(|known| known.id == channel)
        })
    }

    fn offline_after(&self, missed_heartbeats: u32) -> Duration {
        self.heartbeat_interval * missed_heartbeats
    }
}

#[derive(Debug, PartialEq)]
pub enum NodeEvent {
    Registered {
        device_id: String,
        firmware_version: String,
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
                firmware_version,
                entity_count,
            } => write!(
                formatter,
                "{device_id} online (firmware {firmware_version}, {entity_count} entities)"
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

    pub fn handle(&mut self, message: &Message, now: Instant) -> Vec<NodeEvent> {
        if let Payload::Hello {
            fw_version,
            entities,
        } = &message.payload
        {
            // A rebooted node sends `Hello` before its first heartbeat: keep the last
            // uptime so that heartbeat can still reveal the reboot.
            let (uptime_seconds, rssi, heartbeat_interval) = self
                .nodes
                .get(&message.src)
                .map_or((None, None, default_heartbeat_interval()), |node| {
                    (node.uptime_seconds, node.rssi, node.heartbeat_interval)
                });
            self.nodes.insert(
                message.src.clone(),
                Node {
                    firmware_version: fw_version.clone(),
                    entities: entities.clone(),
                    online: true,
                    last_seen: now,
                    uptime_seconds,
                    rssi,
                    heartbeat_interval,
                },
            );
            self.asked_for_hello.remove(&message.src);
            return vec![NodeEvent::Registered {
                device_id: message.src.clone(),
                firmware_version: fw_version.clone(),
                entity_count: entities.len(),
            }];
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
        Message {
            version: 1,
            msg_id: 0,
            src: DEVICE.into(),
            dst: None,
            ttl: 1,
            reply_to: None,
            timestamp: None,
            payload,
            auth: None,
        }
    }

    fn hello() -> Message {
        message(Payload::Hello {
            fw_version: "0.3.1".into(),
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
                firmware_version: "0.3.1".into(),
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
}
