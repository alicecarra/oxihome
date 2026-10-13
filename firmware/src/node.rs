use esp_idf_svc::hal::gpio::{Output, PinDriver};
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::management::{self, ManagementChannel, PARAMETERS};
use oxihome_protocol::{
    Access, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode, EventKind,
    Message, Payload, Quantity, Unit, Value, ValueType,
};

use crate::buzzer::{self, Buzzer};
use crate::climate::Climate;
use crate::ir::{self, IrReceiver};
use crate::ota::Ota;
use crate::potentiometer::Potentiometer;
use crate::settings::Settings;
use crate::transport::{Transport, TransportEvent};

pub const LED_ENTITY: u8 = 1;
pub const BUTTON_ENTITY: u8 = 2;
pub const CLIMATE_ENTITY: u8 = 3;
pub const EXTERNAL_LED_ENTITY: u8 = 4;
pub const BUZZER_ENTITY: u8 = 5;
pub const IR_ENTITY: u8 = 6;
pub const POTENTIOMETER_ENTITY: u8 = 7;
const MAIN_CHANNEL: u8 = 0;
const TEMPERATURE_CHANNEL: u8 = 0;
const HUMIDITY_CHANNEL: u8 = 1;

/// The board's peripherals as `oxihome-node` sees them.
pub struct Board {
    pub settings: Settings,
    pub led: PinDriver<'static, Output>,
    pub external_led: Option<PinDriver<'static, Output>>,
    pub climate: Option<Climate>,
    pub buzzer: Option<Buzzer>,
    pub potentiometer: Option<Potentiometer>,
    pub ir: Option<IrReceiver>,
    /// Set by an `Ota` command, run by `Node::poll` once the `Ack` is out.
    pub pending_update: Option<String>,
}

/// The protocol comes from `oxihome-node`; this ties it to the transport and the event sources.
pub struct Node<T: Transport> {
    core: NodeCore,
    board: Board,
    transport: T,
    ota: Ota,
}

impl<T: Transport> Node<T> {
    pub fn new(device_id: String, transport: T, board: Board, ota: Ota) -> Self {
        Self {
            core: NodeCore::new(device_id, env!("CARGO_PKG_VERSION")),
            board,
            transport,
            ota,
        }
    }

    pub fn poll(&mut self) -> anyhow::Result<()> {
        while let Some(event) = self.transport.poll() {
            let connected = matches!(event, TransportEvent::Connected);
            let messages = match event {
                TransportEvent::Connected => self.core.announcement(&self.board),
                TransportEvent::Received(bytes) => self.handle_bytes(&bytes),
            };
            self.send_all(messages)?;
            if connected {
                self.ota.mark_valid()?;
            }
        }
        if let Some(url) = self.board.pending_update.take() {
            match self.ota.update(&url) {
                Ok(()) => esp_idf_svc::hal::reset::restart(),
                Err(error) => {
                    log::warn!("OTA: {error:#}");
                    let payload = Payload::Error {
                        code: ErrorCode::Internal.into(),
                        detail: Some(format!("OTA: {error:#}")),
                    };
                    let message = self.core.message(payload, None);
                    self.send(message)?;
                }
            }
        }
        if let Some(code) = self.board.ir.as_mut().and_then(IrReceiver::poll) {
            log::info!("IR: {code}");
            let event = self.core.event(
                IR_ENTITY,
                MAIN_CHANNEL,
                EventKind::Received,
                Some(Value::Text(code)),
            );
            self.send(event)?;
        }
        match &mut self.board.buzzer {
            Some(buzzer) => buzzer.poll(),
            None => Ok(()),
        }
    }

    pub fn handle_button(&mut self, event: EventKind) -> anyhow::Result<()> {
        let message = self.core.event(BUTTON_ENTITY, MAIN_CHANNEL, event, None);
        self.send(message)?;

        // Hardcoded local binding: press toggles the LED.
        if event == EventKind::Press {
            self.board.led.toggle()?;
            self.send_state(LED_ENTITY)?;
        }
        Ok(())
    }

    pub fn send_heartbeat(&mut self, uptime_seconds: u32, rssi: Option<i8>) -> anyhow::Result<()> {
        let message = self.core.heartbeat(uptime_seconds, rssi, None);
        self.send(message)
    }

    /// Sends the reading as `State`; a failed read sends nothing.
    pub fn read_climate(&mut self) -> anyhow::Result<()> {
        match self.board.climate.as_mut().and_then(Climate::read) {
            Some(_) => self.send_state(CLIMATE_ENTITY),
            None => Ok(()),
        }
    }

    pub fn read_potentiometer(&mut self) -> anyhow::Result<()> {
        let Some(potentiometer) = &mut self.board.potentiometer else {
            return Ok(());
        };
        if potentiometer.poll()? {
            self.send_state(POTENTIOMETER_ENTITY)?;
        }
        Ok(())
    }

    pub fn update_unverified(&self) -> bool {
        self.ota.unverified()
    }

    pub fn settings(&self) -> &Settings {
        &self.board.settings
    }

    fn handle_bytes(&mut self, bytes: &[u8]) -> Vec<Message> {
        match Message::from_json(&String::from_utf8_lossy(bytes)) {
            Ok(message) => self.core.handle(&mut self.board, &message),
            Err(error) => {
                log::warn!("Invalid message: {error}");
                let payload = Payload::Error {
                    code: ErrorCode::InvalidMessage.into(),
                    detail: Some(error.to_string()),
                };
                vec![self.core.message(payload, None)]
            }
        }
    }

    fn send_state(&mut self, entity: u8) -> anyhow::Result<()> {
        match self.core.state(&self.board, entity) {
            Some(message) => self.send(message),
            None => Ok(()),
        }
    }

    fn send_all(&mut self, messages: Vec<Message>) -> anyhow::Result<()> {
        for message in messages {
            self.send(message)?;
        }
        Ok(())
    }

    fn send(&mut self, message: Message) -> anyhow::Result<()> {
        self.transport.send(message.to_json()?.as_bytes())
    }
}

impl Board {
    /// Only called with `LED_ENTITY` or `EXTERNAL_LED_ENTITY`.
    fn led_pin(&mut self, entity: u8) -> Option<&mut PinDriver<'static, Output>> {
        if entity == EXTERNAL_LED_ENTITY {
            self.external_led.as_mut()
        } else {
            Some(&mut self.led)
        }
    }
}

impl Device for Board {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let mut management = management::descriptor();
        management.channels.push(management::ota_descriptor());
        let mut entities = vec![
            management,
            led_descriptor(LED_ENTITY),
            EntityDescriptor {
                id: BUTTON_ENTITY,
                channels: vec![channel(MAIN_CHANNEL, Access::Event, None)],
            },
        ];
        if self.climate.is_some() {
            entities.push(EntityDescriptor {
                id: CLIMATE_ENTITY,
                channels: vec![
                    Channel {
                        quantity: Some(Quantity::Temperature),
                        unit: Some(Unit::Celsius),
                        ..channel(
                            TEMPERATURE_CHANNEL,
                            Access::Read,
                            Some(ValueType::Float {
                                min: 0.0,
                                max: 50.0,
                                step: 1.0,
                            }),
                        )
                    },
                    Channel {
                        quantity: Some(Quantity::Humidity),
                        unit: Some(Unit::Percent),
                        ..channel(
                            HUMIDITY_CHANNEL,
                            Access::Read,
                            Some(ValueType::Float {
                                min: 20.0,
                                max: 90.0,
                                step: 1.0,
                            }),
                        )
                    },
                ],
            });
        }
        if self.external_led.is_some() {
            entities.push(led_descriptor(EXTERNAL_LED_ENTITY));
        }
        if self.buzzer.is_some() {
            entities.push(EntityDescriptor {
                id: BUZZER_ENTITY,
                channels: vec![channel(
                    MAIN_CHANNEL,
                    Access::Trigger,
                    Some(ValueType::Enum(
                        buzzer::PATTERNS
                            .iter()
                            .map(|pattern| pattern.to_string())
                            .collect(),
                    )),
                )],
            });
        }
        if self.ir.is_some() {
            entities.push(EntityDescriptor {
                id: IR_ENTITY,
                channels: vec![channel(
                    MAIN_CHANNEL,
                    Access::Event,
                    Some(ValueType::Text {
                        max_length: ir::MAX_CODE_LENGTH,
                    }),
                )],
            });
        }
        if self.potentiometer.is_some() {
            entities.push(EntityDescriptor {
                id: POTENTIOMETER_ENTITY,
                channels: vec![Channel {
                    unit: Some(Unit::Percent),
                    ..channel(
                        MAIN_CHANNEL,
                        Access::Read,
                        Some(ValueType::Int {
                            min: 0,
                            max: 100,
                            step: 1,
                        }),
                    )
                }],
            });
        }
        entities
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        match entity {
            management::ENTITY => Some(
                PARAMETERS
                    .into_iter()
                    .map(|parameter| {
                        confirmed(
                            parameter.channel.into(),
                            Value::Int(self.settings.get(parameter)),
                        )
                    })
                    .collect(),
            ),
            LED_ENTITY => Some(vec![confirmed(
                MAIN_CHANNEL,
                Value::Bool(self.led.is_set_high()),
            )]),
            EXTERNAL_LED_ENTITY => self
                .external_led
                .as_ref()
                .map(|pin| vec![confirmed(MAIN_CHANNEL, Value::Bool(pin.is_set_high()))]),
            // Nothing before the first good reading.
            CLIMATE_ENTITY => self.climate.as_ref().and_then(Climate::last).map(|reading| {
                vec![
                    confirmed(
                        TEMPERATURE_CHANNEL,
                        Value::Float(reading.temperature.into()),
                    ),
                    confirmed(
                        HUMIDITY_CHANNEL,
                        Value::Float(reading.relative_humidity.into()),
                    ),
                ]
            }),
            POTENTIOMETER_ENTITY => self
                .potentiometer
                .as_ref()
                .and_then(Potentiometer::percent)
                .map(|percent| vec![confirmed(MAIN_CHANNEL, Value::Int(percent.into()))]),
            _ => None,
        }
    }

    /// Commands arrive checked against `entities`: right channel, operation and value.
    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp) -> Result<(), Rejection> {
        let internal = |what: &str, error: &dyn std::fmt::Display| {
            Rejection::new(ErrorCode::Internal, format!("{what}: {error}"))
        };
        match (entity, op) {
            (management::ENTITY, CommandOp::Set(Value::Int(value))) => {
                let Some(parameter) = management::parameter(channel) else {
                    return Ok(());
                };
                self.settings
                    .set(parameter, *value)
                    .map_err(|error| internal("NVS", &error))?;
                log::info!("{:?} = {value}", parameter.channel);
                Ok(())
            }
            (management::ENTITY, CommandOp::Invoke(Some(Value::Text(url))))
                if channel == u8::from(ManagementChannel::Ota) =>
            {
                self.pending_update = Some(url.clone());
                Ok(())
            }
            (LED_ENTITY | EXTERNAL_LED_ENTITY, CommandOp::Set(Value::Bool(on))) => {
                let Some(pin) = self.led_pin(entity) else {
                    return Err(not_wired(entity));
                };
                let result = if *on { pin.set_high() } else { pin.set_low() };
                result.map_err(|error| internal("LED", &error))
            }
            (BUZZER_ENTITY, CommandOp::Invoke(Some(Value::Enum(pattern)))) => {
                let Some(buzzer) = &mut self.buzzer else {
                    return Err(not_wired(entity));
                };
                match buzzer.play(pattern) {
                    Ok(_) => Ok(()),
                    Err(error) => Err(internal("buzzer", &error)),
                }
            }
            _ => Err(Rejection::new(
                ErrorCode::Unsupported,
                format!("{op:?} on entity {entity} channel {channel} is declared but not handled"),
            )),
        }
    }
}

fn channel(id: u8, access: Access, value: Option<ValueType>) -> Channel {
    Channel {
        id,
        access,
        value,
        quantity: None,
        unit: None,
    }
}

/// Commands are checked against `entities`, which leaves out what is not wired; kept as a safeguard.
fn not_wired(entity: u8) -> Rejection {
    Rejection::new(ErrorCode::UnknownEntity, format!("entity {entity} is not wired on this board"))
}

fn led_descriptor(entity: u8) -> EntityDescriptor {
    EntityDescriptor {
        id: entity,
        channels: vec![channel(
            MAIN_CHANNEL,
            Access::ReadWrite,
            Some(ValueType::Bool),
        )],
    }
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}
