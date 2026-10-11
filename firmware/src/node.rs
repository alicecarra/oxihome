use std::collections::HashMap;

use esp_idf_svc::hal::gpio::{Output, PinDriver};
use oxihome_protocol::management::{self, PARAMETERS};
use oxihome_protocol::{
    Access, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode, EventKind,
    Message, Payload, Quantity, QueryKind, Unit, Value, ValueType,
};

use crate::climate::Climate;
use crate::settings::Settings;
use crate::transport::{Transport, TransportEvent};

pub const LED_ENTITY: u8 = 1;
pub const BUTTON_ENTITY: u8 = 2;
pub const CLIMATE_ENTITY: u8 = 3;
pub const EXTERNAL_LED_ENTITY: u8 = 4;
const MAIN_CHANNEL: u8 = 0;
const TEMPERATURE_CHANNEL: u8 = 0;
const HUMIDITY_CHANNEL: u8 = 1;

struct HandledCommand {
    message_id: u32,
    ack: Vec<u8>,
}

/// Speaks `oxihome-protocol` as JSON over a `Transport`.
pub struct Node<T: Transport> {
    device_id: String,
    transport: T,
    led: PinDriver<'static, Output>,
    external_led: PinDriver<'static, Output>,
    climate: Climate,
    settings: Settings,
    next_message_id: u32,
    /// Last command per sender, to answer duplicates without running them again.
    last_commands: HashMap<String, HandledCommand>,
}

impl<T: Transport> Node<T> {
    pub fn new(
        device_id: String,
        transport: T,
        led: PinDriver<'static, Output>,
        external_led: PinDriver<'static, Output>,
        climate: Climate,
        settings: Settings,
    ) -> Self {
        Self {
            device_id,
            transport,
            led,
            external_led,
            climate,
            settings,
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
            self.send_led_state(LED_ENTITY, None)?;
        }
        Ok(())
    }

    pub fn send_heartbeat(&mut self, uptime_seconds: u32, rssi: Option<i8>) -> anyhow::Result<()> {
        self.send(
            Payload::Heartbeat {
                uptime_s: uptime_seconds,
                rssi,
                battery: None,
            },
            None,
        )
    }

    /// Sends the reading as `State`; a failed read sends nothing.
    pub fn read_climate(&mut self) -> anyhow::Result<()> {
        match self.climate.read() {
            Some(_) => self.send_climate_state(None),
            None => Ok(()),
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    fn announce(&mut self) -> anyhow::Result<()> {
        self.send_hello(None)?;
        self.send_management_state(None)?;
        self.send_led_state(LED_ENTITY, None)?;
        self.send_led_state(EXTERNAL_LED_ENTITY, None)?;
        self.send_climate_state(None)
    }

    fn handle_bytes(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let text = String::from_utf8_lossy(bytes);
        let message = match Message::from_json(&text) {
            Ok(message) => message,
            Err(error) => {
                log::warn!("Invalid message: {error}");
                return self.send_error(ErrorCode::InvalidMessage, error.to_string(), None);
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
                what: QueryKind::State { entity: None },
            } => {
                self.send_management_state(reply_to)?;
                self.send_led_state(LED_ENTITY, reply_to)?;
                self.send_led_state(EXTERNAL_LED_ENTITY, reply_to)?;
                self.send_climate_state(reply_to)
            }
            Payload::Query {
                what:
                    QueryKind::State {
                        entity: Some(management::ENTITY),
                    },
            } => self.send_management_state(reply_to),
            Payload::Query {
                what:
                    QueryKind::State {
                        entity: Some(entity @ (LED_ENTITY | EXTERNAL_LED_ENTITY)),
                    },
            } => self.send_led_state(entity, reply_to),
            Payload::Query {
                what:
                    QueryKind::State {
                        entity: Some(CLIMATE_ENTITY),
                    },
            } if self.climate.last().is_none() => {
                self.send_error(ErrorCode::Internal, "no DHT11 reading yet".into(), reply_to)
            }
            Payload::Query {
                what:
                    QueryKind::State {
                        entity: Some(CLIMATE_ENTITY),
                    },
            } => self.send_climate_state(reply_to),
            Payload::Query {
                what:
                    QueryKind::State {
                        entity: Some(BUTTON_ENTITY),
                    },
            } => self.send_error(
                ErrorCode::InvalidOperation,
                format!("entity {BUTTON_ENTITY} has no state"),
                reply_to,
            ),
            Payload::Query {
                what: QueryKind::State {
                    entity: Some(entity),
                },
            } => self.send_error(
                ErrorCode::UnknownEntity,
                format!("unknown entity {entity}"),
                reply_to,
            ),
            other => self.send_error(
                ErrorCode::Unsupported,
                format!("unsupported message: {other:?}"),
                reply_to,
            ),
        }
    }

    fn handle_command(&mut self, message: Message) -> anyhow::Result<()> {
        let reply_to = Some(message.msg_id);
        let Payload::Command {
            entity,
            channel,
            op,
        } = message.payload
        else {
            unreachable!("only called with commands");
        };

        let rejection = match (entity, channel, op) {
            (management::ENTITY, channel, op) => self.configure(channel, op),
            (LED_ENTITY | EXTERNAL_LED_ENTITY, MAIN_CHANNEL, CommandOp::Set(Value::Bool(on))) => {
                let pin = self.led_pin(entity);
                let result = if on { pin.set_high() } else { pin.set_low() };
                match result {
                    Ok(()) => None,
                    Err(error) => Some((ErrorCode::Internal, format!("LED: {error}"))),
                }
            }
            (LED_ENTITY | EXTERNAL_LED_ENTITY, MAIN_CHANNEL, CommandOp::Set(value)) => Some((
                ErrorCode::InvalidValue,
                format!("expected Bool, got {value:?}"),
            )),
            (LED_ENTITY | BUTTON_ENTITY | EXTERNAL_LED_ENTITY, MAIN_CHANNEL, op) => Some((
                ErrorCode::InvalidOperation,
                format!("{op:?} not allowed on entity {entity} channel {channel}"),
            )),
            (CLIMATE_ENTITY, TEMPERATURE_CHANNEL | HUMIDITY_CHANNEL, op) => Some((
                ErrorCode::InvalidOperation,
                format!("{op:?} not allowed on entity {entity} channel {channel}"),
            )),
            (LED_ENTITY | BUTTON_ENTITY | CLIMATE_ENTITY | EXTERNAL_LED_ENTITY, _, _) => Some((
                ErrorCode::UnknownChannel,
                format!("entity {entity} has no channel {channel}"),
            )),
            _ => Some((ErrorCode::UnknownEntity, format!("unknown entity {entity}"))),
        };

        if let Some((code, detail)) = rejection {
            return self.send_error(code, detail, reply_to);
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
        if entity == management::ENTITY {
            self.send_management_state(None)
        } else {
            self.send_led_state(entity, None)
        }
    }

    /// Only called with `LED_ENTITY` or `EXTERNAL_LED_ENTITY`.
    fn led_pin(&mut self, entity: u8) -> &mut PinDriver<'static, Output> {
        if entity == EXTERNAL_LED_ENTITY {
            &mut self.external_led
        } else {
            &mut self.led
        }
    }

    fn configure(&mut self, channel: u8, op: CommandOp) -> Option<(ErrorCode, String)> {
        let Some(parameter) = management::parameter(channel) else {
            return Some((
                ErrorCode::UnknownChannel,
                format!("entity {} has no channel {channel}", management::ENTITY),
            ));
        };
        match op {
            CommandOp::Set(Value::Int(value)) if parameter.accepts(value) => {
                match self.settings.set(parameter, value) {
                    Ok(()) => {
                        log::info!("{:?} = {value}", parameter.channel);
                        None
                    }
                    Err(error) => Some((ErrorCode::Internal, format!("NVS: {error}"))),
                }
            }
            CommandOp::Set(Value::Int(value)) => Some((
                ErrorCode::InvalidValue,
                format!("{value} outside {}..={}", parameter.min, parameter.max),
            )),
            CommandOp::Set(value) => Some((
                ErrorCode::InvalidValue,
                format!("expected Int, got {value:?}"),
            )),
            op => Some((
                ErrorCode::InvalidOperation,
                format!(
                    "{op:?} not allowed on entity {} channel {channel}",
                    management::ENTITY
                ),
            )),
        }
    }

    fn send_hello(&mut self, reply_to: Option<u32>) -> anyhow::Result<()> {
        let entities = vec![
            management::descriptor(),
            led_descriptor(LED_ENTITY),
            EntityDescriptor {
                id: BUTTON_ENTITY,
                channels: vec![Channel {
                    id: MAIN_CHANNEL,
                    access: Access::Event,
                    value: None,
                    quantity: None,
                    unit: None,
                }],
            },
            EntityDescriptor {
                id: CLIMATE_ENTITY,
                channels: vec![
                    Channel {
                        id: TEMPERATURE_CHANNEL,
                        access: Access::Read,
                        value: Some(ValueType::Float {
                            min: 0.0,
                            max: 50.0,
                            step: 1.0,
                        }),
                        quantity: Some(Quantity::Temperature),
                        unit: Some(Unit::Celsius),
                    },
                    Channel {
                        id: HUMIDITY_CHANNEL,
                        access: Access::Read,
                        value: Some(ValueType::Float {
                            min: 20.0,
                            max: 90.0,
                            step: 1.0,
                        }),
                        quantity: Some(Quantity::Humidity),
                        unit: Some(Unit::Percent),
                    },
                ],
            },
            led_descriptor(EXTERNAL_LED_ENTITY),
        ];

        self.send(
            Payload::Hello {
                fw_version: env!("CARGO_PKG_VERSION").into(),
                entities,
            },
            reply_to,
        )
    }

    fn send_management_state(&mut self, reply_to: Option<u32>) -> anyhow::Result<()> {
        let values = PARAMETERS
            .into_iter()
            .map(|parameter| ChannelValue {
                channel: parameter.channel.into(),
                value: Value::Int(self.settings.get(parameter)),
                confidence: Confidence::Confirmed,
            })
            .collect();
        self.send(
            Payload::State {
                entity: management::ENTITY,
                values,
            },
            reply_to,
        )
    }

    fn send_led_state(&mut self, entity: u8, reply_to: Option<u32>) -> anyhow::Result<()> {
        let on = self.led_pin(entity).is_set_high();
        self.send(
            Payload::State {
                entity,
                values: vec![ChannelValue {
                    channel: MAIN_CHANNEL,
                    value: Value::Bool(on),
                    confidence: Confidence::Confirmed,
                }],
            },
            reply_to,
        )
    }

    /// Nothing to send before the first good reading.
    fn send_climate_state(&mut self, reply_to: Option<u32>) -> anyhow::Result<()> {
        let Some(reading) = self.climate.last() else {
            return Ok(());
        };
        self.send(
            Payload::State {
                entity: CLIMATE_ENTITY,
                values: vec![
                    ChannelValue {
                        channel: TEMPERATURE_CHANNEL,
                        value: Value::Float(reading.temperature.into()),
                        confidence: Confidence::Confirmed,
                    },
                    ChannelValue {
                        channel: HUMIDITY_CHANNEL,
                        value: Value::Float(reading.relative_humidity.into()),
                        confidence: Confidence::Confirmed,
                    },
                ],
            },
            reply_to,
        )
    }

    fn send_error(
        &mut self,
        code: ErrorCode,
        detail: String,
        reply_to: Option<u32>,
    ) -> anyhow::Result<()> {
        self.send(
            Payload::Error {
                code: code.into(),
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
        let mut message = Message::new(self.device_id.clone(), self.next_message_id, payload);
        if let Some(request) = reply_to {
            message = message.replying_to(request);
        }
        self.next_message_id = self.next_message_id.wrapping_add(1);
        Ok(message.to_json()?.into_bytes())
    }
}

fn led_descriptor(entity: u8) -> EntityDescriptor {
    EntityDescriptor {
        id: entity,
        channels: vec![Channel {
            id: MAIN_CHANNEL,
            access: Access::ReadWrite,
            value: Some(ValueType::Bool),
            quantity: None,
            unit: None,
        }],
    }
}
