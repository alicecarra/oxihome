use std::time::{Duration, Instant};

use esp_idf_svc::hal::gpio::{Output, PinDriver};
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::hardware::Hardware;
use oxihome_protocol::management::{self, ManagementChannel, PARAMETERS};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    EventKind, Message, Payload, Quantity, Unit, Value, ValueType,
};

use crate::buzzer;
use crate::ir;
use crate::ota::Ota;
use crate::peripheral::{Entity, POTENTIOMETER_READ_INTERVAL, Peripheral};
use crate::recovery::SAFE_MODE_BOOTS;
use crate::settings::Settings;
use crate::transport::{Transport, TransportEvent};

const MAIN_CHANNEL: u8 = 0;
const TEMPERATURE_CHANNEL: u8 = 0;
const HUMIDITY_CHANNEL: u8 = 1;
/// Time for the `Ack` to leave before a restart.
const RESTART_DELAY: Duration = Duration::from_secs(1);
/// After a failed DHT11 read, the next try comes this soon instead of after the read interval.
const CLIMATE_RETRY: Duration = Duration::from_secs(2);
/// Failed reads in a row before the hub hears of it; the tries go back to the read interval.
const CLIMATE_FAILURES_REPORTED: u8 = 5;

/// The board's peripherals as `oxihome-node` sees them.
pub struct Board {
    pub settings: Settings,
    /// The hardware in use.
    pub hardware: Hardware,
    /// The stored hardware set aside by the safe mode; refused when the hub sends it again.
    pub failed_hardware: Option<Hardware>,
    /// Sent to the hub as `Error` on each connection.
    pub problems: Vec<String>,
    pub entities: Vec<Entity>,
    /// Set by an `Ota` command, run by `Node::poll` once the `Ack` is out.
    pub pending_update: Option<String>,
    /// Set by a new hardware, run by `Node::poll` once the `Ack` is out.
    pub pending_restart: bool,
    /// Sent to the hub as `Error` on the first connection only.
    pub restart_reason: Option<String>,
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
            core: NodeCore::new(
                device_id,
                Build::new("firmware", env!("CARGO_PKG_VERSION"), env!("MCU")),
            ),
            board,
            transport,
            ota,
        }
    }

    pub fn poll(&mut self) {
        while let Some(event) = self.transport.poll() {
            let connected = matches!(event, TransportEvent::Connected);
            let messages = match event {
                TransportEvent::Connected => self.core.announcement(&self.board),
                TransportEvent::Received(bytes) => self.handle_bytes(&bytes),
            };
            self.send_all(messages);
            if connected {
                if let Err(error) = self.ota.mark_valid() {
                    log::error!("Marking the image valid failed: {error:#}");
                }
                if let Err(error) = self.board.settings.set_failed_boots(0) {
                    log::error!("Resetting the failed boots failed: {error:#}");
                }
                let restart_reason = self.board.restart_reason.take();
                for problem in restart_reason
                    .into_iter()
                    .chain(self.board.problems.clone())
                {
                    self.send_error(ErrorCode::Internal, problem);
                }
            }
        }
        if self.board.pending_restart {
            log::info!("New hardware stored; restarting");
            std::thread::sleep(RESTART_DELAY);
            esp_idf_svc::hal::reset::restart();
        }
        if let Some(url) = self.board.pending_update.take() {
            match self.ota.update(&url) {
                Ok(()) => esp_idf_svc::hal::reset::restart(),
                Err(error) => {
                    log::warn!("OTA: {error:#}");
                    self.send_error(ErrorCode::Internal, format!("OTA: {error:#}"));
                }
            }
        }
        self.poll_peripherals();
    }

    fn poll_peripherals(&mut self) {
        let mut outcomes = Vec::new();
        for entity in &mut self.board.entities {
            let id = entity.id;
            match &mut entity.peripheral {
                Peripheral::Output { .. } => {}
                Peripheral::Button { button, toggles } => {
                    if let Some(event) = button.poll() {
                        outcomes.push(Outcome::Event(id, event, None));
                        if let (EventKind::Press, Some(output)) = (event, *toggles) {
                            outcomes.push(Outcome::Toggle(output));
                        }
                    }
                }
                Peripheral::Climate {
                    climate,
                    read_interval,
                    next_read,
                    failures,
                } => {
                    if Instant::now() >= *next_read {
                        if climate.read().is_some() {
                            *failures = 0;
                            *next_read = Instant::now() + *read_interval;
                            outcomes.push(Outcome::State(id));
                        } else {
                            *failures = failures.saturating_add(1);
                            let retry = if *failures < CLIMATE_FAILURES_REPORTED {
                                CLIMATE_RETRY
                            } else {
                                *read_interval
                            };
                            *next_read = Instant::now() + retry;
                            if *failures == CLIMATE_FAILURES_REPORTED {
                                outcomes.push(Outcome::Error(format!(
                                    "entity {id}: DHT11 not responding ({failures} reads failed)"
                                )));
                            }
                        }
                    }
                }
                Peripheral::Buzzer(buzzer) => {
                    if let Err(error) = buzzer.poll() {
                        log::error!("Entity {id}: buzzer failed: {error:#}");
                    }
                }
                Peripheral::IrTransmitter(_) => {}
                Peripheral::IrReceiver(receiver) => {
                    if let Some(code) = receiver.poll() {
                        log::info!("IR: {code}");
                        outcomes.push(Outcome::Event(
                            id,
                            EventKind::Received,
                            Some(Value::Text(code)),
                        ));
                    }
                }
                Peripheral::Potentiometer {
                    potentiometer,
                    last_read,
                } => {
                    if last_read.elapsed() >= POTENTIOMETER_READ_INTERVAL {
                        *last_read = Instant::now();
                        match potentiometer.poll() {
                            Ok(true) => outcomes.push(Outcome::State(id)),
                            Ok(false) => {}
                            Err(error) => log::error!("Entity {id}: reading failed: {error:#}"),
                        }
                    }
                }
            }
        }

        for outcome in outcomes {
            match outcome {
                Outcome::Event(entity, event, value) => {
                    let message = self.core.event(entity, MAIN_CHANNEL, event, value);
                    self.send(message);
                }
                Outcome::State(entity) => self.send_state(entity),
                Outcome::Error(detail) => self.send_error(ErrorCode::Internal, detail),
                Outcome::Toggle(entity) => {
                    if let Some((pin, _)) = self.board.output(entity) {
                        match pin.toggle() {
                            Ok(()) => self.send_state(entity),
                            Err(error) => log::error!("Entity {entity}: toggle failed: {error}"),
                        }
                    }
                }
            }
        }
    }

    pub fn send_heartbeat(&mut self, uptime_seconds: u32, rssi: Option<i8>) {
        let message = self.core.heartbeat(uptime_seconds, rssi, None);
        self.send(message)
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

    fn send_state(&mut self, entity: u8) {
        if let Some(message) = self.core.state(&self.board, entity) {
            self.send(message);
        }
    }

    fn send_all(&mut self, messages: Vec<Message>) {
        for message in messages {
            self.send(message);
        }
    }

    fn send_error(&mut self, code: ErrorCode, detail: String) {
        let payload = Payload::Error {
            code: code.into(),
            detail: Some(detail),
        };
        let message = self.core.message(payload, None);
        self.send(message)
    }

    /// A message that cannot be queued is dropped: the hub asks for the state again on reconnecting.
    fn send(&mut self, message: Message) {
        let result = message
            .to_json()
            .map_err(anyhow::Error::from)
            .and_then(|json| self.transport.send(json.as_bytes()));
        if let Err(error) = result {
            log::warn!("Message dropped: {error:#}");
        }
    }
}

enum Outcome {
    Event(u8, EventKind, Option<Value>),
    State(u8),
    Toggle(u8),
    Error(String),
}

impl Board {
    fn peripheral(&self, entity: u8) -> Option<&Peripheral> {
        self.entities
            .iter()
            .find(|candidate| candidate.id == entity)
            .map(|found| &found.peripheral)
    }

    fn peripheral_mut(&mut self, entity: u8) -> Option<&mut Peripheral> {
        self.entities
            .iter_mut()
            .find(|candidate| candidate.id == entity)
            .map(|found| &mut found.peripheral)
    }

    /// Stores a new hardware and asks for the restart that mounts it.
    fn set_hardware(&mut self, json: &str) -> Result<(), Rejection> {
        let invalid = |detail: String| Rejection::new(ErrorCode::InvalidValue, detail);
        let hardware = Hardware::from_json(json)
            .map_err(|error| invalid(format!("not a hardware configuration: {error}")))?;
        let problems = hardware.validate(env!("MCU"));
        if !problems.is_empty() {
            let problems: Vec<String> = problems.iter().map(ToString::to_string).collect();
            return Err(invalid(problems.join("; ")));
        }
        if self.failed_hardware.as_ref() == Some(&hardware) {
            return Err(invalid(format!(
                "this hardware did not reach the broker in {SAFE_MODE_BOOTS} boots"
            )));
        }
        let in_use = hardware == self.hardware;
        if in_use && self.failed_hardware.is_none() {
            return Ok(());
        }
        self.settings
            .set_hardware(&hardware.to_json())
            .map_err(|error| Rejection::new(ErrorCode::Internal, format!("NVS: {error}")))?;
        self.failed_hardware = None;
        self.pending_restart = !in_use;
        Ok(())
    }

    fn output(&mut self, entity: u8) -> Option<(&mut PinDriver<'static, Output>, bool)> {
        match self.peripheral_mut(entity)? {
            Peripheral::Output { pin, inverted } => Some((pin, *inverted)),
            _ => None,
        }
    }
}

impl Device for Board {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let mut management = management::descriptor();
        management.channels.push(management::ota_descriptor());
        management.channels.push(management::hardware_descriptor());
        let mut entities = vec![management];
        entities.extend(
            self.entities
                .iter()
                .map(|entity| descriptor(entity.id, &entity.peripheral)),
        );
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
                    .chain([confirmed(
                        ManagementChannel::Hardware.into(),
                        Value::Text(self.hardware.to_json()),
                    )])
                    .collect(),
            ),
            _ => match self.peripheral(entity)? {
                Peripheral::Output { pin, inverted } => Some(vec![confirmed(
                    MAIN_CHANNEL,
                    Value::Bool(pin.is_set_high() != *inverted),
                )]),
                // Nothing before the first good reading.
                Peripheral::Climate { climate, .. } => climate.last().map(|reading| {
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
                Peripheral::Potentiometer { potentiometer, .. } => potentiometer
                    .percent()
                    .map(|percent| vec![confirmed(MAIN_CHANNEL, Value::Int(percent.into()))]),
                Peripheral::Button { .. }
                | Peripheral::Buzzer(_)
                | Peripheral::IrReceiver(_)
                | Peripheral::IrTransmitter(_) => None,
            },
        }
    }

    fn ip(&self) -> Option<String> {
        crate::wifi::ip().map(|ip| ip.to_string())
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
                if parameter == management::WIFI_POWER_SAVE {
                    crate::wifi::set_power_save(*value);
                }
                Ok(())
            }
            (management::ENTITY, CommandOp::Invoke(Some(Value::Text(url))))
                if channel == u8::from(ManagementChannel::Ota) =>
            {
                self.pending_update = Some(url.clone());
                Ok(())
            }
            (management::ENTITY, CommandOp::Set(Value::Text(json)))
                if channel == u8::from(ManagementChannel::Hardware) =>
            {
                self.set_hardware(json)
            }
            (management::ENTITY, _) => Err(unsupported(entity, channel, op)),
            _ => match (self.peripheral_mut(entity), op) {
                (None, _) => Err(not_wired(entity)),
                (Some(Peripheral::Output { pin, inverted }), CommandOp::Set(Value::Bool(on))) => {
                    let result = if *on != *inverted {
                        pin.set_high()
                    } else {
                        pin.set_low()
                    };
                    result.map_err(|error| internal("output", &error))
                }
                (
                    Some(Peripheral::Buzzer(buzzer)),
                    CommandOp::Invoke(Some(Value::Enum(pattern))),
                ) => match buzzer.play(pattern) {
                    Ok(_) => Ok(()),
                    Err(error) => Err(internal("buzzer", &error)),
                },
                (
                    Some(Peripheral::IrTransmitter(transmitter)),
                    CommandOp::Invoke(Some(Value::Text(code))),
                ) => {
                    log::info!("IR out: {code}");
                    transmitter
                        .send(code)
                        .map_err(|reason| Rejection::new(ErrorCode::InvalidValue, reason))
                }
                _ => Err(unsupported(entity, channel, op)),
            },
        }
    }
}

fn unsupported(entity: u8, channel: u8, op: &CommandOp) -> Rejection {
    Rejection::new(
        ErrorCode::Unsupported,
        format!("{op:?} on entity {entity} channel {channel} is declared but not handled"),
    )
}

fn descriptor(id: u8, peripheral: &Peripheral) -> EntityDescriptor {
    let channels = match peripheral {
        Peripheral::Output { .. } => vec![channel(
            MAIN_CHANNEL,
            Access::ReadWrite,
            Some(ValueType::Bool),
        )],
        Peripheral::Button { .. } => vec![channel(MAIN_CHANNEL, Access::Event, None)],
        Peripheral::Climate { .. } => vec![
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
        Peripheral::Buzzer(_) => vec![channel(
            MAIN_CHANNEL,
            Access::Trigger,
            Some(ValueType::Enum(
                buzzer::PATTERNS
                    .iter()
                    .map(|pattern| pattern.to_string())
                    .collect(),
            )),
        )],
        Peripheral::IrReceiver(_) => vec![channel(
            MAIN_CHANNEL,
            Access::Event,
            Some(ValueType::Text {
                max_length: ir::MAX_CODE_LENGTH,
            }),
        )],
        Peripheral::IrTransmitter(_) => vec![channel(
            MAIN_CHANNEL,
            Access::Trigger,
            Some(ValueType::Text {
                max_length: ir::MAX_CODE_LENGTH,
            }),
        )],
        Peripheral::Potentiometer { .. } => vec![Channel {
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
    };
    EntityDescriptor {
        id,
        name: None,
        channels,
    }
}

fn channel(id: u8, access: Access, value: Option<ValueType>) -> Channel {
    Channel {
        id,
        name: None,
        access,
        value,
        quantity: None,
        unit: None,
    }
}

/// Commands are checked against `entities`, which leaves out what is not wired; kept as a safeguard.
fn not_wired(entity: u8) -> Rejection {
    Rejection::new(
        ErrorCode::UnknownEntity,
        format!("entity {entity} is not wired on this board"),
    )
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}
