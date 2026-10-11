use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

use oxihome_protocol::{EventKind, Message, Payload, Value};

#[derive(Debug, PartialEq)]
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

/// `Bytes` holding UTF-8 show as text: the IR receiver sends its codes that way until the
/// protocol has a text value.
pub fn data_label(data: &Value) -> String {
    match data {
        Value::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_string(),
            Err(_) => format!("{} bytes", bytes.len()),
        },
        other => format!("{other:?}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LastEvent {
    pub kind: EventKind,
    pub data: Option<Value>,
    pub at: Instant,
}

/// Last event of every (node, entity, channel), since the hub started.
#[derive(Default)]
pub struct Events {
    last: HashMap<(String, u8, u8), LastEvent>,
}

impl Events {
    pub fn apply(&mut self, message: &Message, now: Instant) -> Option<EventRecord> {
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

    pub fn last(&self, device_id: &str, entity: u8, channel: u8) -> Option<&LastEvent> {
        self.last.get(&(device_id.to_string(), entity, channel))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
        let now = Instant::now();
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
        let start = Instant::now();
        events.apply(&event(EventKind::Press), start);
        events.apply(&event(EventKind::LongPress), start + Duration::from_secs(1));

        assert_eq!(
            events.last(DEVICE, 2, 0).map(|last| (last.kind, last.at)),
            Some((EventKind::LongPress, start + Duration::from_secs(1)))
        );
    }

    #[test]
    fn event_data_is_kept_and_text_bytes_show_as_text() {
        let mut events = Events::default();
        let received = message(Payload::Event {
            entity: 6,
            channel: 0,
            event: EventKind::Received,
            data: Some(Value::Bytes(b"NEC 00 45".to_vec())),
        });
        let record = events.apply(&received, Instant::now()).unwrap();

        assert_eq!(
            record.to_string(),
            "esp32-test 6/0 event Received NEC 00 45"
        );
        assert_eq!(
            events.last(DEVICE, 6, 0).and_then(|last| last.data.clone()),
            Some(Value::Bytes(b"NEC 00 45".to_vec()))
        );
        assert_eq!(data_label(&Value::Bytes(vec![0xFF, 0x00])), "2 bytes");
    }

    #[test]
    fn other_messages_are_ignored() {
        let mut events = Events::default();
        let record = events.apply(&message(Payload::Ack { ok: true }), Instant::now());

        assert_eq!(record, None);
        assert_eq!(events.last(DEVICE, 2, 0), None);
    }
}
