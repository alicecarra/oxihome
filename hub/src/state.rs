use std::collections::HashMap;
use std::fmt;

use oxihome_protocol::{Confidence, Message, Payload, Value};

#[derive(Debug, PartialEq)]
pub struct ChannelUpdate {
    pub device_id: String,
    pub entity: u8,
    pub channel: u8,
    pub value: Value,
    pub confidence: Confidence,
}

impl fmt::Display for ChannelUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "{} {}/{} = {:?} ({:?})",
            self.device_id, self.entity, self.channel, self.value, self.confidence
        )
    }
}

/// Last known value of every (node, entity, channel).
#[derive(Default)]
pub struct State {
    values: HashMap<(String, u8, u8), (Value, Confidence)>,
}

impl State {
    /// Returns the channels whose value or confidence changed.
    pub fn apply(&mut self, message: &Message) -> Vec<ChannelUpdate> {
        let Payload::State { entity, values } = &message.payload else {
            return Vec::new();
        };

        let mut updates = Vec::new();
        for channel_value in values {
            let key = (message.src.clone(), *entity, channel_value.channel);
            let current = (channel_value.value.clone(), channel_value.confidence);
            if self.values.get(&key) == Some(&current) {
                continue;
            }
            self.values.insert(key, current);
            updates.push(ChannelUpdate {
                device_id: message.src.clone(),
                entity: *entity,
                channel: channel_value.channel,
                value: channel_value.value.clone(),
                confidence: channel_value.confidence,
            });
        }
        updates
    }
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::ChannelValue;

    use super::*;

    const DEVICE: &str = "esp32-test";

    fn state_message(on: bool) -> Message {
        Message {
            version: 1,
            msg_id: 0,
            src: DEVICE.into(),
            dst: None,
            ttl: 1,
            reply_to: None,
            timestamp: None,
            payload: Payload::State {
                entity: 1,
                values: vec![ChannelValue {
                    channel: 0,
                    value: Value::Bool(on),
                    confidence: Confidence::Confirmed,
                }],
            },
            auth: None,
        }
    }

    fn update(on: bool) -> ChannelUpdate {
        ChannelUpdate {
            device_id: DEVICE.into(),
            entity: 1,
            channel: 0,
            value: Value::Bool(on),
            confidence: Confidence::Confirmed,
        }
    }

    #[test]
    fn first_value_is_reported() {
        let mut state = State::default();
        assert_eq!(state.apply(&state_message(true)), vec![update(true)]);
    }

    #[test]
    fn repeated_value_is_not_reported() {
        let mut state = State::default();
        state.apply(&state_message(true));
        assert!(state.apply(&state_message(true)).is_empty());
    }

    #[test]
    fn changed_value_is_reported() {
        let mut state = State::default();
        state.apply(&state_message(true));
        assert_eq!(state.apply(&state_message(false)), vec![update(false)]);
    }
}
