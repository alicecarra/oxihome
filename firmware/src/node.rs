use std::collections::HashMap;

use esp_idf_svc::hal::gpio::{Output, PinDriver};
use oxihome_protocol::{
    Access, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, EventKind, Message,
    Payload, QueryKind, Value, ValueType,
};

use crate::transport::{Transport, TransportEvent};

pub const LED_ENTITY: u8 = 1;
pub const BUTTON_ENTITY: u8 = 2;
const MAIN_CHANNEL: u8 = 0;

// Not defined by the spec yet.
const ERROR_INVALID_MESSAGE: u16 = 1;
const ERROR_UNSUPPORTED: u16 = 2;
const ERROR_UNKNOWN_TARGET: u16 = 3;

struct HandledCommand {
    message_id: u32,
    ack: Vec<u8>,
}

/// Speaks `oxihome-protocol` as JSON over a `Transport`.
pub struct Node<T: Transport> {
    device_id: String,
    transport: T,
    led: PinDriver<'static, Output>,
    next_message_id: u32,
    /// Last command per sender, to answer duplicates without running them again.
    last_commands: HashMap<String, HandledCommand>,
}

impl<T: Transport> Node<T> {
    pub fn new(device_id: String, transport: T, led: PinDriver<'static, Output>) -> Self {
        Self {
            device_id,
            transport,
            led,
            next_message_id: 0,
            last_commands: HashMap::new(),
        }
    }

    pub fn poll(&mut self) -> anyhow::Result<()> {
        while let Some(event) = self.transport.poll() {
            match event {
                TransportEvent::Connected => self.announce()?,
                TransportEvent::Received(bytes) => self.handle_bytes(&bytes)?,
            }
        }
        Ok(())
    }

    pub fn handle_button(&mut self, event: EventKind) -> anyhow::Result<()> {
        self.send(
            Payload::Event {
                entity: BUTTON_ENTITY,
                channel: MAIN_CHANNEL,
                event,
                data: None,
            },
            None,
        )?;

        // Hardcoded local binding: press toggles the LED.
        if event == EventKind::Press {
            self.led.toggle()?;
            self.send_led_state(None)?;
        }
        Ok(())
    }

    fn announce(&mut self) -> anyhow::Result<()> {
        self.send_hello(None)?;
        self.send_led_state(None)
    }

    fn handle_bytes(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let text = String::from_utf8_lossy(bytes);
        let message = match Message::from_json(&text) {
            Ok(message) => message,
            Err(error) => {
                log::warn!("Invalid message: {error}");
                return self.send_error(ERROR_INVALID_MESSAGE, error.to_string(), None);
            }
        };

        if message
            .dst
            .as_ref()
            .is_some_and(|dst| *dst != self.device_id)
        {
            return Ok(());
        }

        let reply_to = Some(message.msg_id);
        match message.payload {
            Payload::Command { .. } => {
                let duplicate_ack = self
                    .last_commands
                    .get(&message.src)
                    .filter(|handled| handled.message_id == message.msg_id)
                    .map(|handled| handled.ack.clone());
                if let Some(ack) = duplicate_ack {
                    log::info!("Duplicate command #{} from {}", message.msg_id, message.src);
                    return self.transport.send(&ack);
                }
                self.handle_command(message)
            }
            Payload::Query {
                what: QueryKind::Hello,
            } => self.send_hello(reply_to),
            Payload::Query {
                what:
                    QueryKind::State {
                        entity: None | Some(LED_ENTITY),
                    },
            } => self.send_led_state(reply_to),
            Payload::Query {
                what: QueryKind::State {
                    entity: Some(entity),
                },
            } => self.send_error(
                ERROR_UNKNOWN_TARGET,
                format!("entity {entity} has no state"),
                reply_to,
            ),
            other => self.send_error(
                ERROR_UNSUPPORTED,
                format!("unsupported message: {other:?}"),
                reply_to,
            ),
        }
    }

    fn handle_command(&mut self, message: Message) -> anyhow::Result<()> {
        let reply_to = Some(message.msg_id);
        match message.payload {
            Payload::Command {
                entity: LED_ENTITY,
                channel: MAIN_CHANNEL,
                op: CommandOp::Set(Value::Bool(on)),
            } => {
                if on {
                    self.led.set_high()?;
                } else {
                    self.led.set_low()?;
                }
                let ack = self.encode(Payload::Ack { ok: true }, reply_to)?;
                self.transport.send(&ack)?;
                self.last_commands.insert(
                    message.src,
                    HandledCommand {
                        message_id: message.msg_id,
                        ack,
                    },
                );
                self.send_led_state(None)
            }
            Payload::Command {
                entity, channel, ..
            } => self.send_error(
                ERROR_UNKNOWN_TARGET,
                format!("cannot command entity {entity} channel {channel}"),
                reply_to,
            ),
            _ => unreachable!("only called with commands"),
        }
    }

    fn send_hello(&mut self, reply_to: Option<u32>) -> anyhow::Result<()> {
        let entities = vec![
            EntityDescriptor {
                id: LED_ENTITY,
                channels: vec![Channel {
                    id: MAIN_CHANNEL,
                    access: Access::ReadWrite,
                    value: ValueType::Bool,
                    quantity: None,
                    unit: None,
                }],
            },
            EntityDescriptor {
                id: BUTTON_ENTITY,
                channels: vec![Channel {
                    id: MAIN_CHANNEL,
                    access: Access::Event,
                    // The spec has no value type for event channels without data yet.
                    value: ValueType::Bool,
                    quantity: None,
                    unit: None,
                }],
            },
        ];

        self.send(
            Payload::Hello {
                fw_version: env!("CARGO_PKG_VERSION").into(),
                entities,
            },
            reply_to,
        )
    }

    fn send_led_state(&mut self, reply_to: Option<u32>) -> anyhow::Result<()> {
        self.send(
            Payload::State {
                entity: LED_ENTITY,
                values: vec![ChannelValue {
                    channel: MAIN_CHANNEL,
                    value: Value::Bool(self.led.is_set_high()),
                    confidence: Confidence::Confirmed,
                }],
            },
            reply_to,
        )
    }

    fn send_error(
        &mut self,
        code: u16,
        detail: String,
        reply_to: Option<u32>,
    ) -> anyhow::Result<()> {
        self.send(
            Payload::Error {
                code,
                detail: Some(detail),
            },
            reply_to,
        )
    }

    fn send(&mut self, payload: Payload, reply_to: Option<u32>) -> anyhow::Result<()> {
        let bytes = self.encode(payload, reply_to)?;
        self.transport.send(&bytes)
    }

    fn encode(&mut self, payload: Payload, reply_to: Option<u32>) -> anyhow::Result<Vec<u8>> {
        let message = Message {
            version: 1,
            msg_id: self.next_message_id,
            src: self.device_id.clone(),
            dst: None,
            ttl: 1,
            reply_to,
            timestamp: None,
            payload,
            auth: None,
        };
        self.next_message_id = self.next_message_id.wrapping_add(1);
        Ok(message.to_json()?.into_bytes())
    }
}
