use std::collections::HashMap;
use std::time::Instant;

use oxihome_protocol::{
    Access, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode, EventKind,
    Message, Payload, Quantity, QueryKind, Unit, Value, ValueType,
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

/// A node's behaviour, without the network: messages in, payloads out.
pub struct SimulatedNode {
    pub device_id: String,
    /// Entity ids start at 1, in configuration order.
    entities: Vec<(u8, Profile)>,
    lamps: HashMap<u8, bool>,
    booted_at: Instant,
    next_message_id: u32,
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
            device_id,
            entities,
            lamps,
            booted_at: now,
            next_message_id: 0,
        }
    }

    /// Like a power cycle: lamps off, uptime and message ids from zero.
    pub fn reboot(&mut self, now: Instant) {
        self.booted_at = now;
        self.next_message_id = 0;
        for on in self.lamps.values_mut() {
            *on = false;
        }
    }

    pub fn hello(&self) -> Payload {
        Payload::Hello {
            fw_version: format!("simulator {}", env!("CARGO_PKG_VERSION")),
            entities: self
                .entities
                .iter()
                .map(|(id, profile)| EntityDescriptor {
                    id: *id,
                    channels: channels(*profile),
                })
                .collect(),
        }
    }

    pub fn heartbeat(&self, now: Instant) -> Payload {
        Payload::Heartbeat {
            uptime_s: (now - self.booted_at).as_secs() as u32,
            rssi: Some(-55),
            battery: None,
        }
    }

    /// One `State` per entity that has values.
    pub fn states(&self, now: Instant) -> Vec<Payload> {
        self.entities
            .iter()
            .filter_map(|(id, _)| self.state(*id, now))
            .collect()
    }

    pub fn sensor_states(&self, now: Instant) -> Vec<Payload> {
        self.entities
            .iter()
            .filter(|(_, profile)| *profile == Profile::ClimateSensor)
            .filter_map(|(id, _)| self.state(*id, now))
            .collect()
    }

    /// `None` when the entity is not a button.
    pub fn press(&self, entity: u8) -> Option<Payload> {
        (self.profile(entity) == Some(Profile::Button)).then_some(Payload::Event {
            entity,
            channel: 0,
            event: EventKind::Press,
            data: None,
        })
    }

    /// Answers to a message from the hub, each with its `reply_to`.
    pub fn handle(&mut self, message: &Message, now: Instant) -> Vec<(Payload, Option<u32>)> {
        if message
            .dst
            .as_ref()
            .is_some_and(|dst| *dst != self.device_id)
        {
            return Vec::new();
        }
        let reply_to = Some(message.msg_id);
        let reply = |payloads: Vec<Payload>| {
            payloads
                .into_iter()
                .map(|payload| (payload, reply_to))
                .collect()
        };

        match &message.payload {
            Payload::Command {
                entity,
                channel,
                op,
            } => reply(self.command(*entity, *channel, op, now)),
            Payload::Query {
                what: QueryKind::Hello,
            } => reply(vec![self.hello()]),
            Payload::Query {
                what: QueryKind::State { entity: None },
            } => reply(self.states(now)),
            Payload::Query {
                what: QueryKind::State {
                    entity: Some(entity),
                },
            } => reply(vec![
                match (self.profile(*entity), self.state(*entity, now)) {
                    (None, _) => {
                        error(ErrorCode::UnknownEntity, format!("unknown entity {entity}"))
                    }
                    (Some(_), None) => error(
                        ErrorCode::InvalidOperation,
                        format!("entity {entity} has no state"),
                    ),
                    (Some(_), Some(state)) => state,
                },
            ]),
            other => reply(vec![error(
                ErrorCode::Unsupported,
                format!("unsupported message: {other:?}"),
            )]),
        }
    }

    pub fn message(&mut self, payload: Payload, reply_to: Option<u32>) -> Message {
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
        message
    }

    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp, now: Instant) -> Vec<Payload> {
        let Some(profile) = self.profile(entity) else {
            return vec![error(
                ErrorCode::UnknownEntity,
                format!("unknown entity {entity}"),
            )];
        };
        if channel >= channels(profile).len() as u8 {
            return vec![error(
                ErrorCode::UnknownChannel,
                format!("entity {entity} has no channel {channel}"),
            )];
        }
        match (profile, op) {
            (Profile::Lamp, CommandOp::Set(Value::Bool(on))) => {
                self.lamps.insert(entity, *on);
                let mut answers = vec![Payload::Ack { ok: true }];
                answers.extend(self.state(entity, now));
                answers
            }
            (Profile::Lamp, CommandOp::Set(value)) => vec![error(
                ErrorCode::InvalidValue,
                format!("expected Bool, got {value:?}"),
            )],
            _ => vec![error(
                ErrorCode::InvalidOperation,
                format!("{op:?} not allowed on entity {entity} channel {channel}"),
            )],
        }
    }

    fn profile(&self, entity: u8) -> Option<Profile> {
        self.entities
            .iter()
            .find(|(id, _)| *id == entity)
            .map(|(_, profile)| *profile)
    }

    fn state(&self, entity: u8, now: Instant) -> Option<Payload> {
        let values = match self.profile(entity)? {
            Profile::Lamp => vec![confirmed(0, Value::Bool(self.lamps[&entity]))],
            Profile::Button => return None,
            Profile::ClimateSensor => {
                let seconds = (now - self.booted_at).as_secs_f32();
                vec![
                    confirmed(0, Value::Float(wave(seconds, 22.0, 2.0, 300.0))),
                    confirmed(1, Value::Float(wave(seconds, 50.0, 5.0, 420.0))),
                ]
            }
        };
        Some(Payload::State { entity, values })
    }
}

fn channels(profile: Profile) -> Vec<Channel> {
    match profile {
        Profile::Lamp => vec![Channel {
            id: 0,
            access: Access::ReadWrite,
            value: Some(ValueType::Bool),
            quantity: None,
            unit: None,
        }],
        Profile::Button => vec![Channel {
            id: 0,
            access: Access::Event,
            value: None,
            quantity: None,
            unit: None,
        }],
        Profile::ClimateSensor => vec![
            Channel {
                id: 0,
                access: Access::Read,
                value: Some(ValueType::Float {
                    min: -40.0,
                    max: 85.0,
                    step: 0.1,
                }),
                quantity: Some(Quantity::Temperature),
                unit: Some(Unit::Celsius),
            },
            Channel {
                id: 1,
                access: Access::Read,
                value: Some(ValueType::Float {
                    min: 0.0,
                    max: 100.0,
                    step: 0.1,
                }),
                quantity: Some(Quantity::Humidity),
                unit: Some(Unit::Percent),
            },
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

fn error(code: ErrorCode, detail: String) -> Payload {
    Payload::Error {
        code: code.into(),
        detail: Some(detail),
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
        Message {
            version: 1,
            msg_id: 7,
            src: "oxihome-hub".into(),
            dst: Some(DEVICE.into()),
            ttl: 1,
            reply_to: None,
            timestamp: None,
            payload,
            auth: None,
        }
    }

    fn set(entity: u8, value: Value) -> Message {
        from_hub(Payload::Command {
            entity,
            channel: 0,
            op: CommandOp::Set(value),
        })
    }

    fn error_code(answers: &[(Payload, Option<u32>)]) -> Option<u16> {
        match answers {
            [(Payload::Error { code, .. }, Some(7))] => Some(*code),
            _ => None,
        }
    }

    #[test]
    fn lamp_set_answers_ack_and_state() {
        let now = Instant::now();
        let mut node = node(now);
        let answers = node.handle(&set(1, Value::Bool(true)), now);

        assert_eq!(answers[0], (Payload::Ack { ok: true }, Some(7)));
        assert_eq!(
            answers[1].0,
            Payload::State {
                entity: 1,
                values: vec![confirmed(0, Value::Bool(true))],
            }
        );
    }

    #[test]
    fn wrong_value_type_is_invalid_value() {
        let now = Instant::now();
        let answers = node(now).handle(&set(1, Value::Int(3)), now);
        assert_eq!(error_code(&answers), Some(ErrorCode::InvalidValue.into()));
    }

    #[test]
    fn setting_a_sensor_is_invalid_operation() {
        let now = Instant::now();
        let answers = node(now).handle(&set(3, Value::Float(1.0)), now);
        assert_eq!(
            error_code(&answers),
            Some(ErrorCode::InvalidOperation.into())
        );
    }

    #[test]
    fn unknown_entity_and_channel() {
        let now = Instant::now();
        let mut node = node(now);
        assert_eq!(
            error_code(&node.handle(&set(9, Value::Bool(true)), now)),
            Some(ErrorCode::UnknownEntity.into())
        );
        let command = from_hub(Payload::Command {
            entity: 1,
            channel: 5,
            op: CommandOp::Set(Value::Bool(true)),
        });
        assert_eq!(
            error_code(&node.handle(&command, now)),
            Some(ErrorCode::UnknownChannel.into())
        );
    }

    #[test]
    fn query_state_returns_every_entity_with_values() {
        let now = Instant::now();
        let query = from_hub(Payload::Query {
            what: QueryKind::State { entity: None },
        });
        let answers = node(now).handle(&query, now);

        let entities: Vec<u8> = answers
            .iter()
            .filter_map(|(payload, _)| match payload {
                Payload::State { entity, .. } => Some(*entity),
                _ => None,
            })
            .collect();
        assert_eq!(entities, vec![1, 3]);
    }

    #[test]
    fn messages_for_other_nodes_are_ignored() {
        let now = Instant::now();
        let mut message = set(1, Value::Bool(true));
        message.dst = Some("someone-else".into());
        assert!(node(now).handle(&message, now).is_empty());
    }

    #[test]
    fn reboot_resets_uptime_and_lamps() {
        let start = Instant::now();
        let mut node = node(start);
        node.handle(&set(1, Value::Bool(true)), start);
        let later = start + Duration::from_secs(100);

        node.reboot(later);

        assert_eq!(
            node.heartbeat(later),
            Payload::Heartbeat {
                uptime_s: 0,
                rssi: Some(-55),
                battery: None,
            }
        );
        assert_eq!(node.message(Payload::Ack { ok: true }, None).msg_id, 0);
        assert_eq!(
            node.state(1, later),
            Some(Payload::State {
                entity: 1,
                values: vec![confirmed(0, Value::Bool(false))],
            })
        );
    }

    #[test]
    fn only_buttons_press() {
        let node = node(Instant::now());
        assert!(node.press(2).is_some());
        assert!(node.press(1).is_none());
    }
}
