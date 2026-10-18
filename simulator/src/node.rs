use std::collections::HashMap;
use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::hardware::Hardware;
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, Color, ColorModel, CommandOp, Confidence,
    EntityDescriptor, EventKind, Message, Quantity, Unit, Value, ValueType, management,
};
use serde::Deserialize;

/// Share of a channel's range that its wave covers on each side of the middle.
const WAVE_AMPLITUDE: f32 = 0.05;
const WAVE_PERIOD_SECONDS: f32 = 300.0;
/// Added to the period of each channel after the first, so the waves do not move together.
const WAVE_PERIOD_SPREAD_SECONDS: f32 = 60.0;

/// Ready-made entities, for nodes written by hand in `simulator.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// Channel 0: `Bool`, read and write.
    Lamp,
    /// Channel 0: events without a value.
    Button,
    /// The firmware's DHT11: channel 0 temperature (°C), channel 1 humidity (%), read only.
    ClimateSensor,
}

impl Profile {
    pub fn descriptor(self, id: u8) -> EntityDescriptor {
        let (name, channels) = match self {
            Profile::Lamp => (
                "Lamp",
                vec![channel(0, Access::ReadWrite, Some(ValueType::Bool)).named("Power")],
            ),
            Profile::Button => (
                "Button",
                vec![channel(0, Access::Event, None).named("Press")],
            ),
            Profile::ClimateSensor => (
                "Climate sensor",
                vec![
                    Channel {
                        quantity: Some(Quantity::Temperature),
                        unit: Some(Unit::Celsius),
                        ..channel(
                            0,
                            Access::Read,
                            Some(ValueType::Float {
                                min: 0.0,
                                max: 50.0,
                                step: 1.0,
                            }),
                        )
                    }
                    .named("Temperature"),
                    Channel {
                        quantity: Some(Quantity::Humidity),
                        unit: Some(Unit::Percent),
                        ..channel(
                            1,
                            Access::Read,
                            Some(ValueType::Float {
                                min: 20.0,
                                max: 90.0,
                                step: 1.0,
                            }),
                        )
                    }
                    .named("Humidity"),
                ],
            ),
        };
        EntityDescriptor {
            id,
            name: None,
            channels,
        }
        .named(name)
    }
}

/// A node's behaviour, without the network: messages in, messages out.
pub struct SimulatedNode {
    core: NodeCore,
    device: SimulatedDevice,
}

/// Behaves after the declared channels: what is read follows a wave or a fixed value, what is
/// written is kept, triggers are shown on the console and events come from the console.
struct SimulatedDevice {
    device_id: String,
    entities: Vec<EntityDescriptor>,
    /// Written, fixed from the console or, for triggers, the last value invoked.
    values: HashMap<(u8, u8), Value>,
    booted_at: Instant,
}

impl SimulatedNode {
    pub fn new(device_id: String, entities: Vec<EntityDescriptor>, now: Instant) -> Self {
        Self {
            core: NodeCore::new(
                device_id.clone(),
                Build::native("simulator", env!("CARGO_PKG_VERSION")),
            ),
            device: SimulatedDevice {
                device_id,
                entities,
                values: HashMap::new(),
                booted_at: now,
            },
        }
    }

    /// Like a power cycle: every value back to its default, uptime and message ids from zero.
    pub fn reboot(&mut self, now: Instant) {
        self.core.reset();
        self.device.booted_at = now;
        self.device.values.clear();
    }

    pub fn heartbeat(&mut self, now: Instant) -> Message {
        let uptime_s = (now - self.device.booted_at).as_secs() as u32;
        self.core.heartbeat(uptime_s, Some(-55), None)
    }

    /// The states of the entities with a channel that is only read, the ones that change alone.
    pub fn sensor_states(&mut self) -> Vec<Message> {
        let sensors: Vec<u8> = self
            .device
            .entities
            .iter()
            .filter(|entity| {
                entity
                    .channels
                    .iter()
                    .any(|channel| channel.access == Access::Read)
            })
            .map(|entity| entity.id)
            .collect();
        sensors
            .into_iter()
            .filter_map(|entity| self.core.state(&self.device, entity))
            .collect()
    }

    /// Sets a channel as if the device changed it alone: a fixed reading for a sensor, a new
    /// value for what the hub can also write. Returns the entity's new state.
    pub fn set(&mut self, entity: u8, channel: u8, value: Value) -> Result<Vec<Message>, String> {
        let declared = self.declared(entity, channel)?;
        if !matches!(
            declared.access,
            Access::Read | Access::ReadWrite | Access::Write
        ) {
            return Err(format!("{entity}.{channel} is not read or written"));
        }
        self.device.values.insert((entity, channel), value);
        Ok(self.core.state(&self.device, entity).into_iter().collect())
    }

    /// Back to the wave for one read only channel, or for all of them.
    pub fn release(&mut self, channel: Option<(u8, u8)>) -> Vec<Message> {
        let device = &self.device;
        let released: Vec<(u8, u8)> = device
            .values
            .keys()
            .copied()
            .filter(|key| channel.is_none_or(|wanted| wanted == *key))
            .filter(|(entity, channel)| {
                device
                    .channel(*entity, *channel)
                    .is_some_and(|declared| declared.access == Access::Read)
            })
            .collect();
        let mut entities: Vec<u8> = released.iter().map(|(entity, _)| *entity).collect();
        entities.sort_unstable();
        entities.dedup();
        for key in released {
            self.device.values.remove(&key);
        }
        entities
            .into_iter()
            .filter_map(|entity| self.core.state(&self.device, entity))
            .collect()
    }

    /// An event on `channel`, or on the entity's first event channel. Channels without a value
    /// type send `Press`, the others `Received` with the value.
    pub fn event(
        &mut self,
        entity: u8,
        channel: Option<u8>,
        value: Option<Value>,
    ) -> Result<Message, String> {
        let declared = match channel {
            Some(channel) => self.declared(entity, channel)?,
            None => self
                .device
                .entities
                .iter()
                .find(|declared| declared.id == entity)
                .ok_or_else(|| format!("no entity {entity}"))?
                .channels
                .iter()
                .find(|declared| declared.access == Access::Event)
                .ok_or_else(|| format!("entity {entity} has no event channel"))?,
        };
        if declared.access != Access::Event {
            return Err(format!("{entity}.{} is not an event channel", declared.id));
        }
        let kind = match (&declared.value, &value) {
            (None, None) => EventKind::Press,
            (Some(_), Some(_)) => EventKind::Received,
            (None, Some(_)) => return Err(format!("{entity}.{} takes no value", declared.id)),
            (Some(_), None) => return Err(format!("{entity}.{} needs a value", declared.id)),
        };
        let channel = declared.id;
        Ok(self.core.event(entity, channel, kind, value))
    }

    fn declared(&self, entity: u8, channel: u8) -> Result<&Channel, String> {
        self.device
            .channel(entity, channel)
            .ok_or_else(|| format!("no channel {entity}.{channel}"))
    }
}

impl SoftwareNode for SimulatedNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.core.handle(&mut self.device, message)
    }

    fn invalid_message(&mut self, detail: String) -> Vec<Message> {
        vec![self.core.invalid_message(detail)]
    }

    fn announcement(&mut self) -> Vec<Message> {
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let mut messages = vec![self.heartbeat(Instant::now())];
        messages.extend(self.sensor_states());
        messages
    }
}

impl SimulatedDevice {
    fn channel(&self, entity: u8, channel: u8) -> Option<&Channel> {
        self.entities
            .iter()
            .find(|declared| declared.id == entity)?
            .channels
            .iter()
            .find(|declared| declared.id == channel)
    }

    fn values_at(&self, entity: u8, now: Instant) -> Option<Vec<ChannelValue>> {
        let declared = self
            .entities
            .iter()
            .find(|declared| declared.id == entity)?;
        let seconds = (now - self.booted_at).as_secs_f32();
        let values: Vec<ChannelValue> = declared
            .channels
            .iter()
            .filter(|channel| matches!(channel.access, Access::Read | Access::ReadWrite))
            .filter_map(|channel| {
                let key = (entity, channel.id);
                let value = match self.values.get(&key) {
                    Some(value) => value.clone(),
                    None => {
                        let value_type = channel.value.as_ref()?;
                        match channel.access {
                            Access::Read => {
                                let period = WAVE_PERIOD_SECONDS
                                    + WAVE_PERIOD_SPREAD_SECONDS * self.position(key) as f32;
                                wave(value_type, seconds, period)
                            }
                            _ => default_value(entity, channel.id, value_type),
                        }
                    }
                };
                Some(ChannelValue {
                    channel: channel.id,
                    value,
                    confidence: Confidence::Confirmed,
                })
            })
            .collect();
        (!values.is_empty()).then_some(values)
    }

    /// The channel's order among all the node's channels.
    fn position(&self, key: (u8, u8)) -> usize {
        self.entities
            .iter()
            .flat_map(|entity| {
                entity
                    .channels
                    .iter()
                    .map(|channel| (entity.id, channel.id))
            })
            .position(|candidate| candidate == key)
            .unwrap_or(0)
    }
}

impl Device for SimulatedDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        self.entities.clone()
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        self.values_at(entity, Instant::now())
    }

    /// Commands arrive checked against `entities`: right channel, operation and value.
    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp) -> Result<(), Rejection> {
        match op {
            CommandOp::Set(value) => {
                self.values.insert((entity, channel), value.clone());
            }
            CommandOp::Invoke(value) => {
                println!(
                    "{} {entity}.{channel} invoked{}",
                    self.device_id,
                    value
                        .as_ref()
                        .map(|value| format!(" with {value:?}"))
                        .unwrap_or_default()
                );
                if let Some(value) = value {
                    self.values.insert((entity, channel), value.clone());
                }
            }
        }
        Ok(())
    }
}

/// What a channel holds before anyone sets it: the management entity's documented defaults,
/// otherwise the middle of the range or the first option.
fn default_value(entity: u8, channel: u8, value_type: &ValueType) -> Value {
    if entity == management::ENTITY {
        if let Some(parameter) = management::parameter(channel) {
            return Value::Int(parameter.default);
        }
        if channel == u8::from(management::ManagementChannel::Hardware) {
            return Value::Text(Hardware::default().to_json());
        }
    }
    type_default(value_type)
}

fn type_default(value_type: &ValueType) -> Value {
    match value_type {
        ValueType::Bool => Value::Bool(false),
        ValueType::Int { min, max, step } => {
            Value::Int(snap_int(min + (max - min) / 2, *min, *max, *step))
        }
        ValueType::Float { min, max, step } => {
            Value::Float(snap_float((min + max) / 2.0, *min, *max, *step))
        }
        ValueType::Enum(options) => Value::Enum(options.first().cloned().unwrap_or_default()),
        ValueType::Color(ColorModel::Rgb) => Value::Color(Color::Rgb { r: 0, g: 0, b: 0 }),
        ValueType::Color(ColorModel::Hsv) => Value::Color(Color::Hsv {
            h: 0.0,
            s: 0.0,
            v: 0.0,
        }),
        ValueType::Text { .. } => Value::Text(String::new()),
        ValueType::Bytes => Value::Bytes(Vec::new()),
    }
}

/// A smooth value around the middle of the range for numbers; other types hold their default.
fn wave(value_type: &ValueType, seconds: f32, period_seconds: f32) -> Value {
    let swing = (seconds / period_seconds * std::f32::consts::TAU).sin();
    match value_type {
        ValueType::Int { min, max, step } => {
            let (low, high) = (*min as f32, *max as f32);
            let amplitude = ((high - low) * WAVE_AMPLITUDE).max(*step as f32);
            let value = (low + high) / 2.0 + amplitude * swing;
            Value::Int(snap_int(value.round() as i32, *min, *max, *step))
        }
        ValueType::Float { min, max, step } => {
            let amplitude = ((max - min) * WAVE_AMPLITUDE).max(*step);
            Value::Float(snap_float(
                (min + max) / 2.0 + amplitude * swing,
                *min,
                *max,
                *step,
            ))
        }
        other => type_default(other),
    }
}

fn snap_int(value: i32, min: i32, max: i32, step: i32) -> i32 {
    let snapped = if step > 0 {
        min + (value - min + step / 2).div_euclid(step) * step
    } else {
        value
    };
    snapped.clamp(min, max)
}

/// Divides by the inverse of `step` so steps like 0.1 give 22.3 and not 22.300001.
fn snap_float(value: f32, min: f32, max: f32, step: f32) -> f32 {
    let snapped = if step > 0.0 {
        (value / step).round() / step.recip()
    } else {
        value
    };
    snapped.clamp(min, max)
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oxihome_protocol::{ErrorCode, Payload, QueryKind};

    use super::*;

    const DEVICE: &str = "sim-test";

    fn node(now: Instant) -> SimulatedNode {
        let entities = [Profile::Lamp, Profile::Button, Profile::ClimateSensor]
            .iter()
            .zip(1..)
            .map(|(profile, id)| profile.descriptor(id))
            .collect();
        SimulatedNode::new(DEVICE.into(), entities, now)
    }

    fn from_hub(payload: Payload) -> Message {
        Message::new("oxihome-hub", 7, payload).to(DEVICE)
    }

    fn command(entity: u8, channel: u8, op: CommandOp) -> Message {
        from_hub(Payload::Command {
            entity,
            channel,
            op,
        })
    }

    fn set(entity: u8, value: Value) -> Message {
        command(entity, 0, CommandOp::Set(value))
    }

    fn confirmed(channel: u8, value: Value) -> ChannelValue {
        ChannelValue {
            channel,
            value,
            confidence: Confidence::Confirmed,
        }
    }

    fn error_code(answers: &[Message]) -> Option<u16> {
        match answers {
            [
                Message {
                    reply_to: Some(7),
                    payload: Payload::Error { code, .. },
                    ..
                },
            ] => Some(*code),
            _ => None,
        }
    }

    /// What `esp32-14335c2e7270` declared in its last `Hello`, as the hub stores it.
    const REMOTE_BOARD: &str = r#"[{"id":0,"channels":[{"id":5,"access":"ReadWrite","value":{"Int":{"min":5,"max":300,"step":1}},"unit":"Second"},{"id":6,"access":"ReadWrite","value":{"Int":{"min":1,"max":60,"step":1}},"unit":"Second"},{"id":8,"access":"ReadWrite","value":{"Int":{"min":0,"max":2,"step":1}}},{"id":1,"access":"Trigger","value":{"Text":{"max_length":512}}},{"id":7,"access":"ReadWrite","value":{"Text":{"max_length":2048}}}]},{"id":1,"channels":[{"id":0,"access":"ReadWrite","value":"Bool"}]},{"id":2,"channels":[{"id":0,"access":"Event"}]},{"id":3,"channels":[{"id":0,"access":"Trigger","value":{"Text":{"max_length":1024}}}]}]"#;

    #[test]
    fn lamp_set_answers_ack_and_state() {
        let mut node = node(Instant::now());
        let answers = node.handle(&set(1, Value::Bool(true)));

        assert_eq!(answers[0].payload, Payload::Ack { ok: true });
        assert_eq!(answers[0].reply_to, Some(7));
        assert_eq!(
            answers[1].payload,
            Payload::State {
                entity: 1,
                values: vec![confirmed(0, Value::Bool(true))],
            }
        );
    }

    #[test]
    fn wrong_value_type_is_invalid_value() {
        let answers = node(Instant::now()).handle(&set(1, Value::Int(3)));
        assert_eq!(error_code(&answers), Some(ErrorCode::InvalidValue.into()));
    }

    #[test]
    fn setting_a_sensor_is_invalid_operation() {
        let answers = node(Instant::now()).handle(&set(3, Value::Float(1.0)));
        assert_eq!(
            error_code(&answers),
            Some(ErrorCode::InvalidOperation.into())
        );
    }

    #[test]
    fn query_state_returns_every_entity_with_values() {
        let query = from_hub(Payload::Query {
            what: QueryKind::State { entity: None },
        });
        let answers = node(Instant::now()).handle(&query);

        let entities: Vec<u8> = answers
            .iter()
            .filter_map(|message| match message.payload {
                Payload::State { entity, .. } => Some(entity),
                _ => None,
            })
            .collect();
        assert_eq!(entities, vec![1, 3]);
    }

    #[test]
    fn waves_stay_in_range_and_on_the_step() {
        let float = ValueType::Float {
            min: -40.0,
            max: 85.0,
            step: 0.1,
        };
        let int = ValueType::Int {
            min: 0,
            max: 100,
            step: 5,
        };
        for second in (0..1200).step_by(7) {
            let Value::Float(value) = wave(&float, second as f32, 300.0) else {
                panic!("not a float");
            };
            assert!((16.2..=28.8).contains(&value), "{value}");
            assert_eq!(value, (value * 10.0).round() / 10.0);
            let Value::Int(value) = wave(&int, second as f32, 300.0) else {
                panic!("not an int");
            };
            assert!((45..=55).contains(&value) && value % 5 == 0, "{value}");
        }
    }

    #[test]
    fn each_channel_follows_its_own_wave() {
        let start = Instant::now();
        let node = node(start);
        let later = start + Duration::from_secs(400);
        let values = node.device.values_at(3, later).unwrap();
        assert_eq!(values[0], confirmed(0, Value::Float(24.0)));
        assert_eq!(values[1], confirmed(1, Value::Float(52.0)));
    }

    #[test]
    fn fixed_reading_replaces_the_wave_until_released() {
        let now = Instant::now();
        let mut node = node(now);
        let states = node.set(3, 0, Value::Float(30.0)).unwrap();
        assert_eq!(states.len(), 1);
        let values = node.device.values_at(3, now).unwrap();
        assert_eq!(values[0], confirmed(0, Value::Float(30.0)));
        assert_eq!(values[1], confirmed(1, Value::Float(55.0)));

        node.handle(&set(1, Value::Bool(true)));
        assert_eq!(node.release(None).len(), 1);
        let values = node.device.values_at(3, now).unwrap();
        assert_eq!(values[0], confirmed(0, Value::Float(25.0)));
        assert_eq!(
            node.device.values_at(1, now),
            Some(vec![confirmed(0, Value::Bool(true))])
        );
    }

    #[test]
    fn set_refuses_triggers_events_and_unknown_channels() {
        let mut node = node(Instant::now());
        assert!(node.set(2, 0, Value::Bool(true)).is_err());
        assert!(node.set(1, 4, Value::Bool(true)).is_err());
    }

    #[test]
    fn events_without_value_are_presses() {
        let mut node = node(Instant::now());
        let event = node.event(2, None, None).unwrap();
        assert_eq!(
            event.payload,
            Payload::Event {
                entity: 2,
                channel: 0,
                event: EventKind::Press,
                data: None,
            }
        );
        assert!(node.event(2, None, Some(Value::Bool(true))).is_err());
        assert!(node.event(1, None, None).is_err());
    }

    #[test]
    fn events_with_value_are_received() {
        let entity = EntityDescriptor {
            id: 6,
            name: None,
            channels: vec![channel(
                0,
                Access::Event,
                Some(ValueType::Text { max_length: 1024 }),
            )],
        };
        let mut node = SimulatedNode::new(DEVICE.into(), vec![entity], Instant::now());
        let code = Value::Text("NEC 00 45".into());
        let event = node.event(6, Some(0), Some(code.clone())).unwrap();
        assert_eq!(
            event.payload,
            Payload::Event {
                entity: 6,
                channel: 0,
                event: EventKind::Received,
                data: Some(code),
            }
        );
        assert!(node.event(6, Some(0), None).is_err());
    }

    #[test]
    fn write_only_channels_are_kept_out_of_the_state() {
        let entity = EntityDescriptor {
            id: 1,
            name: None,
            channels: vec![
                channel(0, Access::Write, Some(ValueType::Bool)),
                channel(1, Access::ReadWrite, Some(ValueType::Bool)),
            ],
        };
        let now = Instant::now();
        let mut node = SimulatedNode::new(DEVICE.into(), vec![entity], now);
        node.handle(&set(1, Value::Bool(true)));
        assert_eq!(
            node.device.values_at(1, now),
            Some(vec![confirmed(1, Value::Bool(false))])
        );
    }

    #[test]
    fn a_cloned_board_answers_like_the_board() {
        let entities: Vec<EntityDescriptor> = serde_json::from_str(REMOTE_BOARD).unwrap();
        let mut node = SimulatedNode::new("sim-remote".into(), entities, Instant::now());
        let to_clone = |payload| Message::new("oxihome-hub", 7, payload).to("sim-remote");

        let answers = node.handle(&to_clone(Payload::Query {
            what: QueryKind::State { entity: Some(0) },
        }));
        let Payload::State { values, .. } = &answers[0].payload else {
            panic!("no state: {answers:?}");
        };
        assert_eq!(values[0], confirmed(5, Value::Int(30)));
        assert_eq!(values[1], confirmed(6, Value::Int(5)));
        assert_eq!(values[2], confirmed(8, Value::Int(0)));
        assert_eq!(
            values[3],
            confirmed(7, Value::Text(Hardware::default().to_json()))
        );

        // Answered in this order: a repeated message id gets the first answer again.
        let out_of_range = to_clone(Payload::Command {
            entity: 0,
            channel: 5,
            op: CommandOp::Set(Value::Int(1)),
        });
        assert_eq!(
            error_code(&node.handle(&out_of_range)),
            Some(ErrorCode::InvalidValue.into())
        );
        let invoke = to_clone(Payload::Command {
            entity: 3,
            channel: 0,
            op: CommandOp::Invoke(Some(Value::Text("NEC 00 45".into()))),
        });
        assert_eq!(node.handle(&invoke)[0].payload, Payload::Ack { ok: true });
    }

    #[test]
    fn reboot_resets_uptime_values_and_message_ids() {
        let start = Instant::now();
        let mut node = node(start);
        node.handle(&set(1, Value::Bool(true)));
        node.set(3, 0, Value::Float(40.0)).unwrap();
        let later = start + Duration::from_secs(100);

        node.reboot(later);

        let heartbeat = node.heartbeat(later);
        assert_eq!(heartbeat.msg_id, 0);
        assert_eq!(
            heartbeat.payload,
            Payload::Heartbeat {
                uptime_s: 0,
                rssi: Some(-55),
                battery: None,
            }
        );
        assert_eq!(
            node.device.values_at(1, later),
            Some(vec![confirmed(0, Value::Bool(false))])
        );
        assert_eq!(
            node.device.values_at(3, later).unwrap()[0],
            confirmed(0, Value::Float(25.0))
        );
    }
}
