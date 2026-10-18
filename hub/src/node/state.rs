use std::collections::HashMap;
use std::fmt;

use oxihome_protocol::{Confidence, Message, Payload, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelUpdate {
    pub device_id: String,
    pub entity: u8,
    pub channel: u8,
    pub value: Value,
    pub confidence: Confidence,
    /// The value before this one, whatever its confidence; `None` for the first.
    pub previous: Option<Value>,
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
    pub fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<&Value> {
        self.values
            .get(&(device_id.to_string(), entity, channel))
            .map(|(value, _)| value)
    }

    /// A value from before the restart.
    pub fn forget(&mut self, device_id: &str) {
        self.values.retain(|(known, _, _), _| known != device_id);
    }

    pub fn restore(
        &mut self,
        device_id: String,
        entity: u8,
        channel: u8,
        value: Value,
        confidence: Confidence,
    ) {
        self.values
            .insert((device_id, entity, channel), (value, confidence));
    }

    /// Returns the channels whose value or confidence changed.
    pub fn apply(&mut self, message: &Message) -> Vec<ChannelUpdate> {
        let Payload::State { entity, values } = &message.payload else {
            return Vec::new();
        };

        values
            .iter()
            .filter_map(|channel_value| {
                self.record(
                    &message.src,
                    *entity,
                    channel_value.channel,
                    channel_value.value.clone(),
                    channel_value.confidence,
                )
            })
            .collect()
    }

    /// Records a value the node accepted but has not reported yet.
    pub fn assume(
        &mut self,
        device_id: &str,
        entity: u8,
        channel: u8,
        value: Value,
    ) -> Option<ChannelUpdate> {
        self.record(device_id, entity, channel, value, Confidence::Assumed)
    }

    fn record(
        &mut self,
        device_id: &str,
        entity: u8,
        channel: u8,
        value: Value,
        confidence: Confidence,
    ) -> Option<ChannelUpdate> {
        let key = (device_id.to_string(), entity, channel);
        let current = (value.clone(), confidence);
        if self.values.get(&key) == Some(&current) {
            return None;
        }
        let previous = self.values.insert(key, current).map(|(value, _)| value);
        Some(ChannelUpdate {
            device_id: device_id.to_string(),
            entity,
            channel,
            value,
            confidence,
            previous,
        })
    }
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::ChannelValue;

    use super::*;

    const DEVICE: &str = "esp32-test";

    fn state_message(on: bool) -> Message {
        Message::new(
            DEVICE,
            0,
            Payload::State {
                entity: 1,
                values: vec![ChannelValue {
                    channel: 0,
                    value: Value::Bool(on),
                    confidence: Confidence::Confirmed,
                }],
            },
        )
    }

    fn update(on: bool, previous: Option<bool>) -> ChannelUpdate {
        ChannelUpdate {
            device_id: DEVICE.into(),
            entity: 1,
            channel: 0,
            value: Value::Bool(on),
            confidence: Confidence::Confirmed,
            previous: previous.map(Value::Bool),
        }
    }

    #[test]
    fn first_value_is_reported() {
        let mut state = State::default();
        assert_eq!(state.apply(&state_message(true)), vec![update(true, None)]);
    }

    #[test]
    fn repeated_value_is_not_reported() {
        let mut state = State::default();
        state.apply(&state_message(true));
        assert!(state.apply(&state_message(true)).is_empty());
    }

    #[test]
    fn assumed_value_is_confirmed_by_the_node() {
        let mut state = State::default();
        state.apply(&state_message(false));

        let assumed = state.assume(DEVICE, 1, 0, Value::Bool(true)).unwrap();
        assert_eq!(assumed.confidence, Confidence::Assumed);
        assert_eq!(state.value(DEVICE, 1, 0), Some(&Value::Bool(true)));
        assert_eq!(
            state.apply(&state_message(true)),
            vec![update(true, Some(true))]
        );
    }

    #[test]
    fn changed_value_is_reported() {
        let mut state = State::default();
        state.apply(&state_message(true));
        assert_eq!(
            state.apply(&state_message(false)),
            vec![update(false, Some(true))]
        );
    }
}
