use jiff::Timestamp;
use oxihome_protocol::{EventKind, Message, Payload, Value};
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    pub device_id: String,
    pub entity: u8,
    pub channel: u8,
    pub kind: EventKind,
    pub data: Option<Value>,
}

impl fmt::Display for EventRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "{} {}/{} event {:?}",
            self.device_id, self.entity, self.channel, self.kind
        )?;
        match &self.data {
            Some(data) => write!(formatter, " {}", data_label(data)),
            None => Ok(()),
        }
    }
}

pub fn data_label(data: &Value) -> String {
    match data {
        Value::Text(text) | Value::Enum(text) => text.clone(),
        Value::Bytes(bytes) => format!("{} bytes", bytes.len()),
        other => format!("{other:?}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LastEvent {
    pub kind: EventKind,
    pub data: Option<Value>,
    pub at: Timestamp,
}

/// Last event of every (node, entity, channel).
#[derive(Default)]
pub struct Events {
    last: HashMap<(String, u8, u8), LastEvent>,
}

impl Events {
    pub fn apply(&mut self, message: &Message, now: Timestamp) -> Option<EventRecord> {
        let Payload::Event {
            entity,
            channel,
            event,
            data,
        } = &message.payload
        else {
            return None;
        };
        self.last.insert(
            (message.src.clone(), *entity, *channel),
            LastEvent {
                kind: *event,
                data: data.clone(),
                at: now,
            },
        );
        Some(EventRecord {
            device_id: message.src.clone(),
            entity: *entity,
            channel: *channel,
            kind: *event,
            data: data.clone(),
        })
    }

    /// An event from before the restart.
    pub fn restore(&mut self, event: EventRecord, at: Timestamp) {
        self.last.insert(
            (event.device_id, event.entity, event.channel),
            LastEvent {
                kind: event.kind,
                data: event.data,
                at,
            },
        );
    }

    pub fn last(&self, device_id: &str, entity: u8, channel: u8) -> Option<&LastEvent> {
        self.last.get(&(device_id.to_string(), entity, channel))
    }
}

#[cfg(test)]
mod tests {
    use jiff::SignedDuration;

    use super::*;

    const DEVICE: &str = "esp32-test";

    fn message(payload: Payload) -> Message {
        Message::new(DEVICE, 0, payload)
    }

    fn event(kind: EventKind) -> Message {
        message(Payload::Event {
            entity: 2,
            channel: 0,
            event: kind,
            data: None,
        })
    }

    #[test]
    fn records_the_last_event() {
        let mut events = Events::default();
        let now = Timestamp::now();
        let record = events.apply(&event(EventKind::Press), now);

        assert_eq!(record.map(|record| record.kind), Some(EventKind::Press));
        assert_eq!(
            events.last(DEVICE, 2, 0),
            Some(&LastEvent {
                kind: EventKind::Press,
                data: None,
                at: now
            })
        );
    }

    #[test]
    fn newer_event_replaces_the_previous_one() {
        let mut events = Events::default();
        let start = Timestamp::now();
        events.apply(&event(EventKind::Press), start);
        events.apply(
            &event(EventKind::LongPress),
            start + SignedDuration::from_secs(1),
        );

        assert_eq!(
            events.last(DEVICE, 2, 0).map(|last| (last.kind, last.at)),
            Some((EventKind::LongPress, start + SignedDuration::from_secs(1)))
        );
    }

    #[test]
    fn event_data_is_kept_and_shown_as_text() {
        let mut events = Events::default();
        let received = message(Payload::Event {
            entity: 6,
            channel: 0,
            event: EventKind::Received,
            data: Some(Value::Text("NEC 00 45".into())),
        });
        let record = events.apply(&received, Timestamp::now()).unwrap();

        assert_eq!(
            record.to_string(),
            "esp32-test 6/0 event Received NEC 00 45"
        );
        assert_eq!(
            events.last(DEVICE, 6, 0).and_then(|last| last.data.clone()),
            Some(Value::Text("NEC 00 45".into()))
        );
        assert_eq!(data_label(&Value::Bytes(vec![0xFF, 0x00])), "2 bytes");
    }

    #[test]
    fn other_messages_are_ignored() {
        let mut events = Events::default();
        let record = events.apply(&message(Payload::Ack { ok: true }), Timestamp::now());

        assert_eq!(record, None);
        assert_eq!(events.last(DEVICE, 2, 0), None);
    }
}
