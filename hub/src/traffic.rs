//! The last MQTT messages the hub received and sent, in memory, for the live traffic page.

mod counters;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use oxihome_protocol::Payload;
use tokio::sync::broadcast;

use counters::Counters;
pub use counters::{Highlight, NodeSummary};

const KEPT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From a node to the hub.
    Up,
    /// From the hub to a node or to all of them.
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Hello,
    Heartbeat,
    State,
    Event,
    Command,
    Query,
    Ack,
    Error,
}

impl MessageKind {
    pub const ALL: [MessageKind; 8] = [
        MessageKind::Hello,
        MessageKind::Heartbeat,
        MessageKind::State,
        MessageKind::Event,
        MessageKind::Command,
        MessageKind::Query,
        MessageKind::Ack,
        MessageKind::Error,
    ];

    pub fn of(payload: &Payload) -> Self {
        match payload {
            Payload::Hello { .. } => MessageKind::Hello,
            Payload::Heartbeat { .. } => MessageKind::Heartbeat,
            Payload::State { .. } => MessageKind::State,
            Payload::Event { .. } => MessageKind::Event,
            Payload::Command { .. } => MessageKind::Command,
            Payload::Query { .. } => MessageKind::Query,
            Payload::Ack { .. } => MessageKind::Ack,
            Payload::Error { .. } => MessageKind::Error,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            MessageKind::Hello => "Hello",
            MessageKind::Heartbeat => "Heartbeat",
            MessageKind::State => "State",
            MessageKind::Event => "Event",
            MessageKind::Command => "Command",
            MessageKind::Query => "Query",
            MessageKind::Ack => "Ack",
            MessageKind::Error => "Error",
        }
    }
}

/// What the hub made of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Read {
        kind: MessageKind,
        entity: Option<u8>,
        /// Several for a `State` with more than one value.
        channels: Vec<u8>,
    },
    /// Thrown away, with the reason.
    Dropped(String),
}

impl Reading {
    pub fn of(payload: &Payload) -> Self {
        let (entity, channels) = match payload {
            Payload::State { entity, values } => (
                Some(*entity),
                values.iter().map(|value| value.channel).collect(),
            ),
            Payload::Event {
                entity, channel, ..
            }
            | Payload::Command {
                entity, channel, ..
            } => (Some(*entity), vec![*channel]),
            _ => (None, Vec::new()),
        };
        Reading::Read {
            kind: MessageKind::of(payload),
            entity,
            channels,
        }
    }
}

#[derive(Debug)]
pub struct TrafficEntry {
    pub at: Timestamp,
    pub direction: Direction,
    pub topic: String,
    /// From the topic; `None` for a broadcast or a topic outside the protocol.
    pub device_id: Option<String>,
    pub reading: Reading,
    pub text: String,
}

impl TrafficEntry {
    pub fn kind(&self) -> Option<MessageKind> {
        match self.reading {
            Reading::Read { kind, .. } => Some(kind),
            Reading::Dropped(_) => None,
        }
    }

    pub fn touches(&self, entity: u8, channel: u8) -> bool {
        match &self.reading {
            Reading::Read {
                entity: Some(own),
                channels,
                ..
            } => *own == entity && channels.contains(&channel),
            _ => false,
        }
    }
}

/// `oxihome/<device id>/up` and `oxihome/<device id>/down`.
fn device_of(topic: &str) -> Option<String> {
    let rest = topic.strip_prefix("oxihome/")?;
    let device_id = rest
        .strip_suffix("/up")
        .or_else(|| rest.strip_suffix("/down"))?;
    (device_id != "broadcast" && !device_id.is_empty()).then(|| device_id.to_string())
}

pub struct Traffic {
    recent: Mutex<VecDeque<Arc<TrafficEntry>>>,
    counters: Mutex<Counters>,
    sender: broadcast::Sender<Arc<TrafficEntry>>,
}

impl Default for Traffic {
    fn default() -> Self {
        Self {
            recent: Mutex::default(),
            counters: Mutex::new(Counters::new(Timestamp::now())),
            sender: broadcast::channel(256).0,
        }
    }
}

impl Traffic {
    pub fn record(&self, direction: Direction, topic: &str, text: &str, reading: Reading) {
        let entry = Arc::new(TrafficEntry {
            at: Timestamp::now(),
            direction,
            topic: topic.to_string(),
            device_id: device_of(topic),
            reading,
            text: text.to_string(),
        });
        {
            let mut recent = self.recent.lock().unwrap();
            if recent.len() == KEPT {
                recent.pop_front();
            }
            recent.push_back(entry.clone());
        }
        self.counters.lock().unwrap().record(&entry);
        let _ = self.sender.send(entry);
    }

    /// Since `counting_since`.
    pub fn summary(&self) -> Vec<NodeSummary> {
        self.counters.lock().unwrap().summary(Timestamp::now())
    }

    pub fn counting_since(&self) -> Timestamp {
        self.counters.lock().unwrap().started()
    }

    /// Oldest first.
    pub fn recent(&self) -> Vec<Arc<TrafficEntry>> {
        self.recent.lock().unwrap().iter().cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TrafficEntry>> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::{ChannelValue, Confidence, Value};

    use super::*;

    #[test]
    fn device_comes_from_the_topic() {
        assert_eq!(
            device_of("oxihome/esp32-test/up").as_deref(),
            Some("esp32-test")
        );
        assert_eq!(
            device_of("oxihome/esp32-test/down").as_deref(),
            Some("esp32-test")
        );
        assert_eq!(device_of("oxihome/broadcast/down"), None);
        assert_eq!(device_of("other/esp32-test/up"), None);
    }

    #[test]
    fn state_reads_every_channel() {
        let reading = Reading::of(&Payload::State {
            entity: 3,
            values: [0, 2]
                .map(|channel| ChannelValue {
                    channel,
                    value: Value::Bool(true),
                    confidence: Confidence::Confirmed,
                })
                .to_vec(),
        });
        assert_eq!(
            reading,
            Reading::Read {
                kind: MessageKind::State,
                entity: Some(3),
                channels: vec![0, 2],
            }
        );
    }

    #[test]
    fn keeps_the_last_entries() {
        let traffic = Traffic::default();
        for number in 0..KEPT + 3 {
            traffic.record(
                Direction::Up,
                "oxihome/a/up",
                &number.to_string(),
                Reading::Dropped("invalid".into()),
            );
        }
        let recent = traffic.recent();
        assert_eq!(recent.len(), KEPT);
        assert_eq!(recent.first().unwrap().text, "3");
        assert_eq!(recent.last().unwrap().text, (KEPT + 2).to_string());
    }

    #[test]
    fn touches_matches_entity_and_channel() {
        let traffic = Traffic::default();
        traffic.record(
            Direction::Up,
            "oxihome/a/up",
            "{}",
            Reading::Read {
                kind: MessageKind::Event,
                entity: Some(1),
                channels: vec![2],
            },
        );
        let entry = &traffic.recent()[0];
        assert!(entry.touches(1, 2));
        assert!(!entry.touches(1, 0));
        assert!(!entry.touches(2, 2));
    }
}
