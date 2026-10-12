//! What every Oxihome node does, whatever it runs on: answering the hub, numbering messages,
//! answering duplicate commands without running them again, and checking commands against the
//! entities the node declares. No network and no clock: messages in, messages out. The `mqtt`
//! feature adds a runtime for nodes that run on a computer.

#[cfg(feature = "mqtt")]
pub mod mqtt;
mod validation;

use std::collections::HashMap;

use oxihome_protocol::{
    ChannelValue, CommandOp, EntityDescriptor, ErrorCode, EventKind, Message, Payload, QueryKind,
    Value,
};

pub use validation::Rejection;

/// The part each node writes: what it has and what it does.
pub trait Device {
    /// Declared in the `Hello`; commands are checked against it before reaching [`Device::command`].
    fn entities(&self) -> Vec<EntityDescriptor>;

    /// Current values of an entity; `None` for entities without state, such as buttons.
    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>>;

    /// Runs a command already checked against the declaration.
    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp) -> Result<(), Rejection>;
}

/// The last command answered, per sender.
struct AnsweredCommand {
    message_id: u32,
    ack: Message,
}

pub struct NodeCore {
    device_id: String,
    firmware_version: String,
    next_message_id: u32,
    answered_commands: HashMap<String, AnsweredCommand>,
}

impl NodeCore {
    pub fn new(device_id: impl Into<String>, firmware_version: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            firmware_version: firmware_version.into(),
            next_message_id: 0,
            answered_commands: HashMap::new(),
        }
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Like a power cycle: message ids from zero and no memory of answered commands.
    pub fn reset(&mut self) {
        self.next_message_id = 0;
        self.answered_commands.clear();
    }

    /// The answers to a message from the hub; nothing for messages to other nodes.
    pub fn handle(&mut self, device: &mut impl Device, message: &Message) -> Vec<Message> {
        if message
            .dst
            .as_ref()
            .is_some_and(|dst| *dst != self.device_id)
        {
            return Vec::new();
        }
        let reply_to = Some(message.msg_id);
        match &message.payload {
            Payload::Command {
                entity,
                channel,
                op,
            } => {
                let duplicate = self
                    .answered_commands
                    .get(&message.src)
                    .filter(|answered| answered.message_id == message.msg_id);
                if let Some(answered) = duplicate {
                    return vec![answered.ack.clone()];
                }
                self.command(device, message, *entity, *channel, op)
            }
            Payload::Query {
                what: QueryKind::Hello,
            } => vec![self.hello_replying(device, reply_to)],
            Payload::Query {
                what: QueryKind::State { entity: None },
            } => self.states_replying(device, reply_to),
            Payload::Query {
                what: QueryKind::State {
                    entity: Some(entity),
                },
            } => {
                let payload = if !declares_entity(device, *entity) {
                    rejection(ErrorCode::UnknownEntity, format!("unknown entity {entity}"))
                } else {
                    match device.state(*entity) {
                        Some(values) => Payload::State {
                            entity: *entity,
                            values,
                        },
                        None => rejection(
                            ErrorCode::InvalidOperation,
                            format!("entity {entity} has no state"),
                        ),
                    }
                };
                vec![self.message(payload, reply_to)]
            }
            other => vec![self.message(
                rejection(
                    ErrorCode::Unsupported,
                    format!("unsupported message: {other:?}"),
                ),
                reply_to,
            )],
        }
    }

    /// What a node sends when it connects: its `Hello` and the state of every entity.
    pub fn announcement(&mut self, device: &impl Device) -> Vec<Message> {
        let mut messages = vec![self.hello_replying(device, None)];
        messages.extend(self.states_replying(device, None));
        messages
    }

    pub fn hello(&mut self, device: &impl Device) -> Message {
        self.hello_replying(device, None)
    }

    /// One `State` per entity that has values.
    pub fn states(&mut self, device: &impl Device) -> Vec<Message> {
        self.states_replying(device, None)
    }

    /// `None` when the entity has no state.
    pub fn state(&mut self, device: &impl Device, entity: u8) -> Option<Message> {
        let values = device.state(entity)?;
        Some(self.message(Payload::State { entity, values }, None))
    }

    pub fn heartbeat(&mut self, uptime_s: u32, rssi: Option<i8>, battery: Option<u8>) -> Message {
        self.message(
            Payload::Heartbeat {
                uptime_s,
                rssi,
                battery,
            },
            None,
        )
    }

    pub fn event(
        &mut self,
        entity: u8,
        channel: u8,
        event: EventKind,
        data: Option<Value>,
    ) -> Message {
        self.message(
            Payload::Event {
                entity,
                channel,
                event,
                data,
            },
            None,
        )
    }

    /// Numbers a payload as the next message from this node.
    pub fn message(&mut self, payload: Payload, reply_to: Option<u32>) -> Message {
        let mut message = Message::new(self.device_id.clone(), self.next_message_id, payload);
        if let Some(request) = reply_to {
            message = message.replying_to(request);
        }
        self.next_message_id = self.next_message_id.wrapping_add(1);
        message
    }

    /// `Ack`, then the entity's new `State` after a `Set`.
    fn command(
        &mut self,
        device: &mut impl Device,
        message: &Message,
        entity: u8,
        channel: u8,
        op: &CommandOp,
    ) -> Vec<Message> {
        let reply_to = Some(message.msg_id);
        let result = validation::check(&device.entities(), entity, channel, op)
            .and_then(|()| device.command(entity, channel, op));
        if let Err(Rejection { code, detail }) = result {
            return vec![self.message(rejection(code, detail), reply_to)];
        }
        let ack = self.message(Payload::Ack { ok: true }, reply_to);
        self.answered_commands.insert(
            message.src.clone(),
            AnsweredCommand {
                message_id: message.msg_id,
                ack: ack.clone(),
            },
        );
        let mut answers = vec![ack];
        if matches!(op, CommandOp::Set(_)) {
            answers.extend(self.state(device, entity));
        }
        answers
    }

    fn hello_replying(&mut self, device: &impl Device, reply_to: Option<u32>) -> Message {
        let payload = Payload::Hello {
            fw_version: self.firmware_version.clone(),
            entities: device.entities(),
        };
        self.message(payload, reply_to)
    }

    fn states_replying(&mut self, device: &impl Device, reply_to: Option<u32>) -> Vec<Message> {
        device
            .entities()
            .iter()
            .filter_map(|entity| {
                device.state(entity.id).map(|values| Payload::State {
                    entity: entity.id,
                    values,
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|payload| self.message(payload, reply_to))
            .collect()
    }
}

fn declares_entity(device: &impl Device, entity: u8) -> bool {
    device
        .entities()
        .iter()
        .any(|declared| declared.id == entity)
}

fn rejection(code: ErrorCode, detail: String) -> Payload {
    Payload::Error {
        code: code.into(),
        detail: Some(detail),
    }
}

#[cfg(test)]
mod tests;
