use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use oxihome_protocol::{Message, Payload};

/// Three missed heartbeats (the firmware sends one every 30 s).
pub const OFFLINE_AFTER: Duration = Duration::from_secs(90);

struct Node {
    last_seen: Instant,
    online: bool,
}

#[derive(Debug, PartialEq)]
pub enum NodeEvent {
    Registered {
        device_id: String,
        firmware_version: String,
        entity_count: usize,
    },
    Online(String),
    Offline(String),
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
            NodeEvent::Offline(device_id) => write!(formatter, "{device_id} offline"),
        }
    }
}

#[derive(Default)]
pub struct Registry {
    nodes: HashMap<String, Node>,
}

impl Registry {
    pub fn handle(&mut self, message: &Message, now: Instant) -> Option<NodeEvent> {
        if let Payload::Hello {
            fw_version,
            entities,
        } = &message.payload
        {
            self.nodes.insert(
                message.src.clone(),
                Node {
                    last_seen: now,
                    online: true,
                },
            );
            return Some(NodeEvent::Registered {
                device_id: message.src.clone(),
                firmware_version: fw_version.clone(),
                entity_count: entities.len(),
            });
        }

        let node = self.nodes.get_mut(&message.src)?;
        node.last_seen = now;
        if node.online {
            return None;
        }
        node.online = true;
        Some(NodeEvent::Online(message.src.clone()))
    }

    pub fn check_presence(&mut self, now: Instant) -> Vec<NodeEvent> {
        self.nodes
            .iter_mut()
            .filter(|(_, node)| node.online && now - node.last_seen >= OFFLINE_AFTER)
            .map(|(device_id, node)| {
                node.online = false;
                NodeEvent::Offline(device_id.clone())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn heartbeat() -> Message {
        message(Payload::Heartbeat {
            uptime_s: 10,
            rssi: None,
            battery: None,
        })
    }

    #[test]
    fn hello_registers_the_node() {
        let mut registry = Registry::default();
        let event = registry.handle(&hello(), Instant::now());
        assert_eq!(
            event,
            Some(NodeEvent::Registered {
                device_id: DEVICE.into(),
                firmware_version: "0.3.1".into(),
                entity_count: 0,
            })
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
            registry.handle(&heartbeat(), start + OFFLINE_AFTER * 2),
            Some(NodeEvent::Online(DEVICE.into()))
        );
        assert_eq!(
            registry.handle(&heartbeat(), start + OFFLINE_AFTER * 2),
            None
        );
    }
}
