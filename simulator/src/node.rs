use std::collections::HashMap;
use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, EventKind,
    Message, Quantity, Unit, Value, ValueType,
};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    /// Channel 0: `Bool`, read and write.
    Lamp,
    /// Channel 0: events without a value.
    Button,
    /// Channel 0: temperature (°C), channel 1: humidity (%), read only.
    ClimateSensor,
}

/// A node's behaviour, without the network: messages in, messages out.
pub struct SimulatedNode {
    core: NodeCore,
    device: SimulatedDevice,
}

/// What the simulated hardware holds; the protocol side is `oxihome-node`'s.
struct SimulatedDevice {
    /// Entity ids start at 1, in configuration order.
    entities: Vec<(u8, Profile)>,
    lamps: HashMap<u8, bool>,
    /// Set from the console; `None` follows the slow wave.
    fixed_temperature: Option<f32>,
    fixed_humidity: Option<f32>,
    booted_at: Instant,
}

impl SimulatedNode {
    pub fn new(device_id: String, profiles: &[Profile], now: Instant) -> Self {
        let entities: Vec<(u8, Profile)> = profiles
            .iter()
            .enumerate()
            .map(|(index, profile)| (index as u8 + 1, *profile))
            .collect();
        let lamps = entities
            .iter()
            .filter(|(_, profile)| *profile == Profile::Lamp)
            .map(|(id, _)| (*id, false))
            .collect();
        Self {
            core: NodeCore::new(
                device_id,
                Build::native("simulator", env!("CARGO_PKG_VERSION")),
            ),
            device: SimulatedDevice {
                entities,
                lamps,
                fixed_temperature: None,
                fixed_humidity: None,
                booted_at: now,
            },
        }
    }

    /// Like a power cycle: lamps off, uptime and message ids from zero.
    pub fn reboot(&mut self, now: Instant) {
        self.core.reset();
        self.device.booted_at = now;
        for on in self.device.lamps.values_mut() {
            *on = false;
        }
    }

    pub fn heartbeat(&mut self, now: Instant) -> Message {
        let uptime_s = (now - self.device.booted_at).as_secs() as u32;
        self.core.heartbeat(uptime_s, Some(-55), None)
    }

    pub fn sensor_states(&mut self) -> Vec<Message> {
        let sensors: Vec<u8> = self
            .device
            .entities
            .iter()
            .filter(|(_, profile)| *profile == Profile::ClimateSensor)
            .map(|(id, _)| *id)
            .collect();
        sensors
            .into_iter()
            .filter_map(|entity| self.core.state(&self.device, entity))
            .collect()
    }

    /// Fixes the climate readings (`None` goes back to the wave) and returns the new states,
    /// empty when the node has no climate sensor.
    pub fn set_climate(&mut self, temperature: Option<f32>, humidity: Option<f32>) -> Vec<Message> {
        self.device.fixed_temperature = temperature;
        self.device.fixed_humidity = humidity;
        self.sensor_states()
    }

    /// `None` when the entity is not a button.
    pub fn press(&mut self, entity: u8) -> Option<Message> {
        (self.device.profile(entity) == Some(Profile::Button))
            .then(|| self.core.event(entity, 0, EventKind::Press, None))
    }
}

impl SoftwareNode for SimulatedNode {
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
        let mut messages = vec![self.heartbeat(Instant::now())];
        messages.extend(self.sensor_states());
        messages
    }
}

impl SimulatedDevice {
    fn profile(&self, entity: u8) -> Option<Profile> {
        self.entities
            .iter()
            .find(|(id, _)| *id == entity)
            .map(|(_, profile)| *profile)
    }

    fn values_at(&self, entity: u8, now: Instant) -> Option<Vec<ChannelValue>> {
        Some(match self.profile(entity)? {
            Profile::Lamp => vec![confirmed(0, Value::Bool(self.lamps[&entity]))],
            Profile::Button => return None,
            Profile::ClimateSensor => {
                let seconds = (now - self.booted_at).as_secs_f32();
                let temperature = self
                    .fixed_temperature
                    .unwrap_or_else(|| wave(seconds, 22.0, 2.0, 300.0));
                let humidity = self
                    .fixed_humidity
                    .unwrap_or_else(|| wave(seconds, 50.0, 5.0, 420.0));
                vec![
                    confirmed(0, Value::Float(temperature)),
                    confirmed(1, Value::Float(humidity)),
                ]
            }
        })
    }
}

impl Device for SimulatedDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        self.entities
            .iter()
            .map(|(id, profile)| {
                EntityDescriptor {
                    id: *id,
                    name: None,
                    channels: channels(*profile),
                }
                .named(match profile {
                    Profile::Lamp => "Lamp",
                    Profile::Button => "Button",
                    Profile::ClimateSensor => "Climate sensor",
                })
            })
            .collect()
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        self.values_at(entity, Instant::now())
    }

    /// Only lamps take commands, and `oxihome-node` already checked the value is a `Bool`.
    fn command(&mut self, entity: u8, _channel: u8, op: &CommandOp) -> Result<(), Rejection> {
        if let CommandOp::Set(Value::Bool(on)) = op {
            self.lamps.insert(entity, *on);
        }
        Ok(())
    }
}

fn channels(profile: Profile) -> Vec<Channel> {
    match profile {
        Profile::Lamp => vec![
            Channel {
                id: 0,
                name: None,
                access: Access::ReadWrite,
                value: Some(ValueType::Bool),
                quantity: None,
                unit: None,
            }
            .named("Power"),
        ],
        Profile::Button => vec![
            Channel {
                id: 0,
                name: None,
                access: Access::Event,
                value: None,
                quantity: None,
                unit: None,
            }
            .named("Press"),
        ],
        Profile::ClimateSensor => vec![
            Channel {
                id: 0,
                name: None,
                access: Access::Read,
                value: Some(ValueType::Float {
                    min: -40.0,
                    max: 85.0,
                    step: 0.1,
                }),
                quantity: Some(Quantity::Temperature),
                unit: Some(Unit::Celsius),
            }
            .named("Temperature"),
            Channel {
                id: 1,
                name: None,
                access: Access::Read,
                value: Some(ValueType::Float {
                    min: 0.0,
                    max: 100.0,
                    step: 0.1,
                }),
                quantity: Some(Quantity::Humidity),
                unit: Some(Unit::Percent),
            }
            .named("Humidity"),
        ],
    }
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}

/// A smooth value around `center`, rounded to 0.1.
fn wave(seconds: f32, center: f32, amplitude: f32, period_seconds: f32) -> f32 {
    let value = center + amplitude * (seconds / period_seconds * std::f32::consts::TAU).sin();
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oxihome_protocol::{ErrorCode, Payload, QueryKind};

    use super::*;

    const DEVICE: &str = "sim-test";

    fn node(now: Instant) -> SimulatedNode {
        SimulatedNode::new(
            DEVICE.into(),
            &[Profile::Lamp, Profile::Button, Profile::ClimateSensor],
            now,
        )
    }

    fn from_hub(payload: Payload) -> Message {
        Message::new("oxihome-hub", 7, payload).to(DEVICE)
    }

    fn set(entity: u8, value: Value) -> Message {
        from_hub(Payload::Command {
            entity,
            channel: 0,
            op: CommandOp::Set(value),
        })
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
    fn unknown_entity_and_channel() {
        let mut node = node(Instant::now());
        assert_eq!(
            error_code(&node.handle(&set(9, Value::Bool(true)))),
            Some(ErrorCode::UnknownEntity.into())
        );
        let command = from_hub(Payload::Command {
            entity: 1,
            channel: 5,
            op: CommandOp::Set(Value::Bool(true)),
        });
        assert_eq!(
            error_code(&node.handle(&command)),
            Some(ErrorCode::UnknownChannel.into())
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
    fn messages_for_other_nodes_are_ignored() {
        let mut message = set(1, Value::Bool(true));
        message.dst = Some("someone-else".into());
        assert!(node(Instant::now()).handle(&message).is_empty());
    }

    #[test]
    fn reboot_resets_uptime_lamps_and_message_ids() {
        let start = Instant::now();
        let mut node = node(start);
        node.handle(&set(1, Value::Bool(true)));
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
    }

    #[test]
    fn only_buttons_press() {
        let mut node = node(Instant::now());
        assert!(node.press(2).is_some());
        assert!(node.press(1).is_none());
    }

    #[test]
    fn fixed_climate_replaces_the_wave_until_auto() {
        let now = Instant::now();
        let mut node = node(now);
        node.set_climate(Some(30.0), None);
        let values = node.device.values_at(3, now).unwrap();
        assert_eq!(values[0], confirmed(0, Value::Float(30.0)));
        assert_eq!(values[1], confirmed(1, Value::Float(50.0)));
        assert_eq!(node.set_climate(Some(30.0), None).len(), 1);
        node.set_climate(None, None);
        let values = node.device.values_at(3, now).unwrap();
        assert_eq!(values[0], confirmed(0, Value::Float(22.0)));
    }
}
