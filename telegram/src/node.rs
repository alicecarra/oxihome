use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    EventKind, Message, Value, ValueType,
};
use tokio::sync::mpsc;

const MESSAGE_ENTITY: u8 = 1;
const COMMAND_ENTITY: u8 = 2;
const CONNECTION_ENTITY: u8 = 3;
const SILENT_CHANNEL: u8 = 1;
/// Telegram's own limit, in characters; bytes are never fewer.
const MESSAGE_MAX_LENGTH: u16 = 4096;
const ERROR_MAX_LENGTH: u16 = 256;

/// A message for the default chat, waiting to be sent.
pub struct Outgoing {
    pub text: String,
    pub silent: bool,
    pub queued_at: Instant,
}

pub struct TelegramNode {
    core: NodeCore,
    device: TelegramDevice,
    started_at: Instant,
}

struct TelegramDevice {
    commands: Vec<String>,
    outgoing: mpsc::Sender<Outgoing>,
    /// `None` until the first answer or failure of `getUpdates`.
    reachable: Option<bool>,
    last_error: String,
}

impl TelegramNode {
    pub fn new(device_id: String, commands: Vec<String>, outgoing: mpsc::Sender<Outgoing>) -> Self {
        Self {
            core: NodeCore::new(
                device_id,
                Build::native("telegram", env!("CARGO_PKG_VERSION")),
            ),
            device: TelegramDevice {
                commands,
                outgoing,
                reachable: None,
                last_error: String::new(),
            },
            started_at: Instant::now(),
        }
    }

    pub fn command_received(&mut self, command: String) -> Vec<Message> {
        vec![self.core.event(
            COMMAND_ENTITY,
            0,
            EventKind::Received,
            Some(Value::Enum(command)),
        )]
    }

    /// The connection's state, only when it changed.
    pub fn reachable(&mut self, reachable: bool) -> Vec<Message> {
        if self.device.reachable == Some(reachable) {
            return Vec::new();
        }
        self.device.reachable = Some(reachable);
        self.connection_state()
    }

    pub fn failed(&mut self, error: String) -> Vec<Message> {
        self.device.last_error = truncate(error, ERROR_MAX_LENGTH as usize);
        self.connection_state()
    }

    fn connection_state(&mut self) -> Vec<Message> {
        self.core
            .state(&self.device, CONNECTION_ENTITY)
            .into_iter()
            .collect()
    }
}

impl SoftwareNode for TelegramNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.core.handle(&mut self.device, message)
    }

    fn announcement(&mut self) -> Vec<Message> {
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        vec![self.core.heartbeat(uptime_s, None, None)]
    }
}

impl Device for TelegramDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let message = |id| Channel {
            id,
            access: Access::Trigger,
            value: Some(ValueType::Text {
                max_length: MESSAGE_MAX_LENGTH,
            }),
            quantity: None,
            unit: None,
        };
        vec![
            EntityDescriptor {
                id: MESSAGE_ENTITY,
                channels: vec![message(0), message(SILENT_CHANNEL)],
            },
            EntityDescriptor {
                id: COMMAND_ENTITY,
                channels: vec![Channel {
                    id: 0,
                    access: Access::Event,
                    value: Some(ValueType::Enum(self.commands.clone())),
                    quantity: None,
                    unit: None,
                }],
            },
            EntityDescriptor {
                id: CONNECTION_ENTITY,
                channels: vec![
                    Channel {
                        id: 0,
                        access: Access::Read,
                        value: Some(ValueType::Bool),
                        quantity: None,
                        unit: None,
                    },
                    Channel {
                        id: 1,
                        access: Access::Read,
                        value: Some(ValueType::Text {
                            max_length: ERROR_MAX_LENGTH,
                        }),
                        quantity: None,
                        unit: None,
                    },
                ],
            },
        ]
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        match entity {
            CONNECTION_ENTITY => Some(vec![
                confirmed(0, Value::Bool(self.reachable?)),
                confirmed(1, Value::Text(self.last_error.clone())),
            ]),
            _ => None,
        }
    }

    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp) -> Result<(), Rejection> {
        let CommandOp::Invoke(Some(Value::Text(text))) = op else {
            return Err(Rejection::new(
                ErrorCode::InvalidOperation,
                format!("entity {entity} channel {channel} takes no commands"),
            ));
        };
        if text.trim().is_empty() {
            return Err(Rejection::new(ErrorCode::InvalidValue, "empty message"));
        }
        self.outgoing
            .try_send(Outgoing {
                text: text.clone(),
                silent: channel == SILENT_CHANNEL,
                queued_at: Instant::now(),
            })
            .map_err(|error| {
                let detail = match error {
                    mpsc::error::TrySendError::Full(_) => "too many messages waiting",
                    mpsc::error::TrySendError::Closed(_) => "the sender stopped",
                };
                Rejection::new(ErrorCode::Internal, detail)
            })
    }
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}

fn truncate(mut text: String, max_bytes: usize) -> String {
    if text.len() > max_bytes {
        let mut end = max_bytes;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::Payload;

    use super::*;

    fn node(queue: usize) -> (TelegramNode, mpsc::Receiver<Outgoing>) {
        let (sender, receiver) = mpsc::channel(queue);
        let commands = vec!["receiver".into(), "achar".into()];
        (
            TelegramNode::new("telegram".into(), commands, sender),
            receiver,
        )
    }

    fn invoke(message_id: u32, channel: u8, value: Option<Value>) -> Message {
        Message::new(
            "oxihome-hub",
            message_id,
            Payload::Command {
                entity: MESSAGE_ENTITY,
                channel,
                op: CommandOp::Invoke(value),
            },
        )
        .to("telegram")
    }

    #[test]
    fn invoking_a_message_channel_queues_it_with_or_without_sound() {
        let (mut node, mut receiver) = node(4);
        let answers = node.handle(&invoke(1, 0, Some(Value::Text("um nó caiu".into()))));
        assert!(matches!(answers[0].payload, Payload::Ack { ok: true }));
        node.handle(&invoke(
            2,
            SILENT_CHANNEL,
            Some(Value::Text("28 °C".into())),
        ));
        let first = receiver.try_recv().unwrap();
        assert_eq!((first.text.as_str(), first.silent), ("um nó caiu", false));
        let second = receiver.try_recv().unwrap();
        assert_eq!((second.text.as_str(), second.silent), ("28 °C", true));
    }

    #[test]
    fn empty_messages_and_a_full_queue_are_refused() {
        let (mut node, _receiver) = node(1);
        let answers = node.handle(&invoke(1, 0, Some(Value::Text("  ".into()))));
        assert!(matches!(answers[0].payload, Payload::Error { code: 5, .. }));
        node.handle(&invoke(2, 0, Some(Value::Text("one".into()))));
        let answers = node.handle(&invoke(3, 0, Some(Value::Text("two".into()))));
        assert!(matches!(answers[0].payload, Payload::Error { code: 7, .. }));
    }

    #[test]
    fn commands_are_enum_events() {
        let (mut node, _receiver) = node(1);
        let messages = node.command_received("receiver".into());
        assert!(matches!(
            &messages[0].payload,
            Payload::Event { entity: COMMAND_ENTITY, event: EventKind::Received, data: Some(Value::Enum(command)), .. }
                if command == "receiver"
        ));
    }

    #[test]
    fn the_connection_is_reported_once_known_and_when_it_changes() {
        let (mut node, _receiver) = node(1);
        assert_eq!(node.announcement().len(), 1, "only the Hello");
        assert_eq!(node.reachable(true).len(), 1);
        assert!(node.reachable(true).is_empty());
        let messages = node.failed("é".repeat(200));
        assert!(matches!(
            &messages[0].payload,
            Payload::State { values, .. } if matches!(&values[1].value, Value::Text(error) if error.len() == 256)
        ));
    }
}
