use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

use oxihome_protocol::{EventKind, Message, Payload};

#[derive(Debug, PartialEq)]
pub struct EventRecord {
    pub device_id: String,
    pub entity: u8,
    pub channel: u8,
    pub kind: EventKind,
}

impl fmt::Display for EventRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(
            formatter,
            "{} {}/{} event {:?}",
            self.device_id, self.entity, self.channel, self.kind
        )
    }
}

/// Last event of every (node, entity, channel), since the hub started.
#[derive(Default)]
pub struct Events {
    last: HashMap<(String, u8, u8), (EventKind, Instant)>,
}

impl Events {
    pub fn apply(&mut self, message: &Message, now: Instant) -> Option<EventRecord> {
        let Payload::Event {
            entity,
            channel,
            event,
            ..
        } = &message.payload
        else {
            return None;
        };
        self.last
            .insert((message.src.clone(), *entity, *channel), (*event, now));
        Some(EventRecord {
            device_id: message.src.clone(),
            entity: *entity,
            channel: *channel,
            kind: *event,
        })
    }

    pub fn last(&self, device_id: &str, entity: u8, channel: u8) -> Option<(EventKind, Instant)> {
        self.last
            .get(&(device_id.to_string(), entity, channel))
            .copied()
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
        assert_eq!(events.last(DEVICE, 2, 0), Some((EventKind::Press, now)));
    }

    #[test]
    fn newer_event_replaces_the_previous_one() {
        let mut events = Events::default();
        let start = Instant::now();
        events.apply(&event(EventKind::Press), start);
        events.apply(&event(EventKind::LongPress), start + Duration::from_secs(1));

        assert_eq!(
            events.last(DEVICE, 2, 0),
            Some((EventKind::LongPress, start + Duration::from_secs(1)))
        );
    }

    #[test]
    fn other_messages_are_ignored() {
        let mut events = Events::default();
        let record = events.apply(&message(Payload::Ack { ok: true }), Instant::now());

        assert_eq!(record, None);
        assert_eq!(events.last(DEVICE, 2, 0), None);
    }
}
