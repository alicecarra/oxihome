use esp_idf_svc::hal::delay::NON_BLOCK;
use esp_idf_svc::hal::gpio::{Output, PinDriver};
use esp_idf_svc::hal::uart::UartDriver;
use oxihome_protocol::{
    Access, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, EventKind, Message,
    Payload, QueryKind, Value, ValueType,
};

pub const LED_ENTITY: u8 = 1;
pub const BUTTON_ENTITY: u8 = 2;
const MAIN_CHANNEL: u8 = 0;

// Not defined by the spec yet.
const ERROR_INVALID_MESSAGE: u16 = 1;
const ERROR_UNSUPPORTED: u16 = 2;
const ERROR_UNKNOWN_TARGET: u16 = 3;

const MAX_LINE_LENGTH: usize = 1024;

/// Speaks `oxihome-protocol` as JSON lines over a UART.
pub struct Node {
    device_id: String,
    uart: UartDriver<'static>,
    led: PinDriver<'static, Output>,
    next_message_id: u32,
    line: Vec<u8>,
}

impl Node {
    pub fn new(
        device_id: String,
        uart: UartDriver<'static>,
        led: PinDriver<'static, Output>,
    ) -> Self {
        Self {
            device_id,
            uart,
            led,
            next_message_id: 0,
            line: Vec::new(),
        }
    }

    pub fn announce(&mut self) -> anyhow::Result<()> {
        self.send_hello(None)?;
        self.send_led_state(None)
    }

    /// Reads whatever is available on the UART and handles complete lines.
    pub fn poll_serial(&mut self) -> anyhow::Result<()> {
        let mut buffer = [0u8; 128];
        let length = self.uart.read(&mut buffer, NON_BLOCK).unwrap_or(0);

        for &byte in &buffer[..length] {
            if byte == b'\n' {
                let line = std::mem::take(&mut self.line);
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if !line.is_empty() {
                    self.handle_line(line)?;
                }
            } else if self.line.len() < MAX_LINE_LENGTH {
                self.line.push(byte);
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

    fn handle_line(&mut self, line: &str) -> anyhow::Result<()> {
        let message = match Message::from_json(line) {
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
                self.send(Payload::Ack { ok: true }, reply_to)?;
                self.send_led_state(None)
            }
            Payload::Command {
                entity, channel, ..
            } => self.send_error(
                ERROR_UNKNOWN_TARGET,
                format!("cannot command entity {entity} channel {channel}"),
                reply_to,
            ),
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

        let mut line = message.to_json()?;
        line.push('\n');
        self.uart.write(line.as_bytes())?;
        Ok(())
    }
}
