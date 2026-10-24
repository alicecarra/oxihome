//! The last MQTT messages the hub received and sent, in memory, for the live traffic page.

mod counters;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use oxihome_protocol::{Message, Payload};
use tokio::sync::broadcast;

use counters::Counters;
pub use counters::{Highlight, NodeSummary};

const KEPT: usize = 500;
/// The same alert is not told again sooner, so one at the edge of its limit does not flap.
const ALERT_COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// The MQTT buffer of the firmware, both ways.
pub const BOARD_LIMIT: usize = 4096;
/// The broker's `max_packet_size`.
pub const BROKER_LIMIT: usize = 64 * 1024;
/// Messages from this share of their limit up are large.
const LARGE_SHARE: f64 = 0.75;

/// `true` when `size` is close to `limit`.
pub fn is_large(size: usize, limit: usize) -> bool {
    size as f64 >= limit as f64 * LARGE_SHARE
}

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
    Notice,
}

impl MessageKind {
    pub const ALL: [MessageKind; 9] = [
        MessageKind::Hello,
        MessageKind::Heartbeat,
        MessageKind::State,
        MessageKind::Event,
        MessageKind::Command,
        MessageKind::Query,
        MessageKind::Ack,
        MessageKind::Error,
        MessageKind::Notice,
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
            Payload::Notice { .. } => MessageKind::Notice,
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
            MessageKind::Notice => "Notice",
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
        message_id: u32,
        reply_to: Option<u32>,
    },
    /// Thrown away, with the reason.
    Dropped(String),
}

impl Reading {
    pub fn of(message: &Message) -> Self {
        let payload = &message.payload;
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
            message_id: message.msg_id,
            reply_to: message.reply_to,
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

/// What a rule can run on, from the highlights of the traffic map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    /// A channel that reported regularly went quiet.
    Stopped,
    /// A channel sends far more than usual.
    Noisy,
    /// Commands to the node went without an answer in the last hour.
    Unanswered,
    /// The node reported a problem by itself: an `Error` that answers nothing, such as a sensor
    /// that stopped responding. Not from the map: it comes with the message.
    Reported,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Alert {
    pub kind: AlertKind,
    pub device_id: String,
    /// Entity and channel; `None` for `Unanswered` and `Reported`.
    pub channel: Option<(u8, u8)>,
}

/// An alert that just went up, with what to tell about it.
#[derive(Debug, Clone, PartialEq)]
pub struct RaisedAlert {
    pub alert: Alert,
    pub detail: String,
}

fn seconds_label(duration: Duration) -> String {
    match duration.as_secs() {
        seconds @ 0..60 => format!("{seconds} s"),
        seconds => format!("{} min", seconds / 60),
    }
}

/// The alerts standing in `summary`.
fn alerts(summary: &[NodeSummary]) -> Vec<RaisedAlert> {
    let mut alerts = Vec::new();
    for node in summary {
        let Some(device_id) = &node.device_id else {
            continue;
        };
        for channel in &node.channels {
            let (kind, detail) = match channel.highlight {
                Some(Highlight::Stopped { typical }) => (
                    AlertKind::Stopped,
                    format!("Used to report every {}", seconds_label(typical)),
                ),
                Some(Highlight::Noisy) => (
                    AlertKind::Noisy,
                    format!("{:.0} messages a minute", channel.per_minute),
                ),
                None => continue,
            };
            alerts.push(RaisedAlert {
                alert: Alert {
                    kind,
                    device_id: device_id.clone(),
                    channel: Some((channel.entity, channel.channel)),
                },
                detail,
            });
        }
        if node.unanswered > 0 {
            alerts.push(RaisedAlert {
                alert: Alert {
                    kind: AlertKind::Unanswered,
                    device_id: device_id.clone(),
                    channel: None,
                },
                detail: match node.unanswered {
                    1 => "1 command without an answer in the last hour".into(),
                    count => format!("{count} commands without an answer in the last hour"),
                },
            });
        }
    }
    alerts
}

/// Which alerts are up and when each was last told.
#[derive(Default)]
struct AlertState {
    raised: HashSet<Alert>,
    told: HashMap<Alert, Timestamp>,
}

impl AlertState {
    /// The `standing` alerts to tell: not up at the last check, about a node `may_tell` accepts
    /// (online and not updating), and not told in the last `ALERT_COOLDOWN`. One that goes up
    /// while `may_tell` refuses counts as up, so the node coming back does not tell it.
    fn check(
        &mut self,
        standing: Vec<RaisedAlert>,
        now: Timestamp,
        may_tell: impl Fn(&str) -> bool,
    ) -> Vec<RaisedAlert> {
        let to_tell: Vec<RaisedAlert> = standing
            .iter()
            .filter(|standing| !self.raised.contains(&standing.alert))
            .filter(|standing| may_tell(&standing.alert.device_id))
            .filter(|standing| {
                self.told.get(&standing.alert).is_none_or(|told| {
                    Duration::try_from(now.duration_since(*told))
                        .is_ok_and(|ago| ago >= ALERT_COOLDOWN)
                })
            })
            .cloned()
            .collect();
        for told in &to_tell {
            self.told.insert(told.alert.clone(), now);
        }
        self.told.retain(|_, told| {
            Duration::try_from(now.duration_since(*told)).is_ok_and(|ago| ago < ALERT_COOLDOWN)
        });
        self.raised = standing
            .into_iter()
            .map(|standing| standing.alert)
            .collect();
        to_tell
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
    alerts: Mutex<AlertState>,
    sender: broadcast::Sender<Arc<TrafficEntry>>,
}

impl Default for Traffic {
    fn default() -> Self {
        Self {
            recent: Mutex::default(),
            counters: Mutex::new(Counters::new(Timestamp::now())),
            alerts: Mutex::default(),
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

    /// Alerts that went up since the last call, to tell; see `AlertState::check`.
    pub fn new_alerts(&self, may_tell: impl Fn(&str) -> bool) -> Vec<RaisedAlert> {
        let standing = alerts(&self.summary());
        self.alerts
            .lock()
            .unwrap()
            .check(standing, Timestamp::now(), may_tell)
    }

    pub fn counting_since(&self) -> Timestamp {
        self.counters.lock().unwrap().started()
    }

    /// The command an `Ack` or `Error` from `device_id` answers, while it is still kept.
    pub fn answered(&self, device_id: &str, reply_to: u32) -> Option<Arc<TrafficEntry>> {
        self.recent
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|entry| {
                entry.direction == Direction::Down
                    && entry.device_id.as_deref() == Some(device_id)
                    && matches!(
                        entry.reading,
                        Reading::Read {
                            kind: MessageKind::Command,
                            message_id,
                            ..
                        } if message_id == reply_to
                    )
            })
            .cloned()
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
        let reading = Reading::of(&Message::new(
            "a",
            7,
            Payload::State {
                entity: 3,
                values: [0, 2]
                    .map(|channel| ChannelValue {
                        channel,
                        value: Value::Bool(true),
                        confidence: Confidence::Confirmed,
                    })
                    .to_vec(),
            },
        ));
        assert_eq!(
            reading,
            Reading::Read {
                kind: MessageKind::State,
                entity: Some(3),
                channels: vec![0, 2],
                message_id: 7,
                reply_to: None,
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
                message_id: 0,
                reply_to: None,
            },
        );
        let entry = &traffic.recent()[0];
        assert!(entry.touches(1, 2));
        assert!(!entry.touches(1, 0));
        assert!(!entry.touches(2, 2));
    }

    #[test]
    fn finds_the_command_an_answer_is_for() {
        let traffic = Traffic::default();
        let command = |message_id| Reading::Read {
            kind: MessageKind::Command,
            entity: Some(1),
            channels: vec![0],
            message_id,
            reply_to: None,
        };
        traffic.record(Direction::Down, "oxihome/a/down", "a 4", command(4));
        traffic.record(Direction::Down, "oxihome/b/down", "b 5", command(5));
        assert_eq!(traffic.answered("a", 4).unwrap().text, "a 4");
        assert!(traffic.answered("a", 5).is_none());
    }

    fn raised(kind: AlertKind, device_id: &str) -> RaisedAlert {
        RaisedAlert {
            alert: Alert {
                kind,
                device_id: device_id.into(),
                channel: None,
            },
            detail: String::new(),
        }
    }

    fn minutes(minutes: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + minutes * 60).unwrap()
    }

    #[test]
    fn an_alert_is_told_once_while_it_stays_up() {
        let mut state = AlertState::default();
        let online = |_: &str| true;
        let stopped = raised(AlertKind::Stopped, "a");
        let noisy = raised(AlertKind::Noisy, "b");
        assert_eq!(
            state.check(vec![stopped.clone()], minutes(0), online),
            [stopped.clone()]
        );
        assert_eq!(
            state.check(vec![stopped.clone(), noisy.clone()], minutes(1), online),
            [noisy.clone()]
        );
        assert!(state.check(vec![noisy], minutes(2), online).is_empty());
        // Down and up again within the cooldown: not told.
        assert!(
            state
                .check(vec![stopped.clone()], minutes(3), online)
                .is_empty()
        );
        assert!(state.check(Vec::new(), minutes(40), online).is_empty());
        assert_eq!(
            state.check(vec![stopped.clone()], minutes(41), online),
            [stopped]
        );
    }

    #[test]
    fn an_offline_node_is_not_told_even_when_it_comes_back() {
        let mut state = AlertState::default();
        let unanswered = raised(AlertKind::Unanswered, "a");
        assert!(
            state
                .check(vec![unanswered.clone()], minutes(0), |_| false)
                .is_empty()
        );
        assert!(
            state
                .check(vec![unanswered.clone()], minutes(1), |_| true)
                .is_empty()
        );
        assert!(state.check(Vec::new(), minutes(70), |_| true).is_empty());
        assert_eq!(
            state.check(vec![unanswered.clone()], minutes(71), |_| true),
            [unanswered]
        );
    }

    #[test]
    fn large_is_three_quarters_of_the_limit() {
        assert!(!is_large(3071, BOARD_LIMIT));
        assert!(is_large(3072, BOARD_LIMIT));
    }
}
