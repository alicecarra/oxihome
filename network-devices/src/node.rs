use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    EventKind, Message, Value, ValueType,
};

use crate::scan::Mac;

/// The whole network, apart from the watched devices; they cannot take this id.
pub const NETWORK_ENTITY: u8 = 100;

const ONLINE_CHANNEL: u8 = 0;
const CHANGE_CHANNEL: u8 = 1;
const ADDRESS_CHANNEL: u8 = 2;
const SINCE_CHANNEL: u8 = 3;

const ANSWERING_CHANNEL: u8 = 0;
const UNKNOWN_CHANNEL: u8 = 1;

/// The most hosts a /22 has.
const MAXIMUM_COUNT: i32 = 1022;

pub struct WatchedDevice {
    pub entity: u8,
    pub name: String,
    pub mac: Mac,
    /// How long without answering before it counts as offline; phones sleep with Wi-Fi off.
    pub offline_after: Duration,
}

/// A device going online or offline, for the log.
pub struct Change {
    pub name: String,
    pub online: bool,
}

/// One scan: who answered, from where, and when.
pub struct Scan<'a> {
    pub answered: &'a HashMap<Mac, Ipv4Addr>,
    pub now: Instant,
    /// Local time as the hub shows it, `2026-10-13 09:15`.
    pub clock: &'a str,
}

pub struct NetworkDevicesNode {
    core: NodeCore,
    device: Devices,
    started_at: Instant,
}

struct Devices {
    watched: Vec<Watch>,
    started_at: Instant,
    /// Devices answering and how many of them are not watched; `None` before the first scan.
    counts: Option<(i32, i32)>,
}

struct Watch {
    device: WatchedDevice,
    last_seen: Option<(Instant, String)>,
    /// `None` until it answers or stays silent for `offline_after` after the start.
    online: Option<bool>,
    address: Option<Ipv4Addr>,
    /// When it came online, or the last time it answered once offline.
    since: Option<String>,
}

impl NetworkDevicesNode {
    pub fn new(device_id: String, watched: Vec<WatchedDevice>, now: Instant) -> Self {
        Self {
            core: NodeCore::new(
                device_id,
                Build::native("network-devices", env!("CARGO_PKG_VERSION")),
            ),
            device: Devices {
                watched: watched
                    .into_iter()
                    .map(|device| Watch {
                        device,
                        last_seen: None,
                        online: None,
                        address: None,
                        since: None,
                    })
                    .collect(),
                started_at: now,
                counts: None,
            },
            started_at: now,
        }
    }

    /// The new state of each entity that changed, and an event when a device known before goes
    /// online or offline.
    pub fn update(&mut self, scan: &Scan) -> (Vec<Message>, Vec<Change>) {
        let mut messages = Vec::new();
        let mut changes = Vec::new();
        for index in 0..self.device.watched.len() {
            let watch = &mut self.device.watched[index];
            let (previous_online, previous_address) = (watch.online, watch.address);
            if let Some(&address) = scan.answered.get(&watch.device.mac) {
                watch.last_seen = Some((scan.now, scan.clock.to_string()));
                watch.address = Some(address);
                watch.online = Some(true);
            } else {
                let silent_since = watch
                    .last_seen
                    .as_ref()
                    .map_or(self.device.started_at, |(instant, _)| *instant);
                if scan.now.duration_since(silent_since) >= watch.device.offline_after {
                    watch.online = Some(false);
                }
            }
            let entity = watch.device.entity;
            let online_changed = watch.online != previous_online;
            if online_changed {
                watch.since = match watch.online {
                    Some(true) => Some(scan.clock.to_string()),
                    _ => watch.last_seen.as_ref().map(|(_, clock)| clock.clone()),
                };
            }
            if !online_changed && watch.address == previous_address {
                continue;
            }
            let online = watch.online;
            messages.extend(self.core.state(&self.device, entity));
            let (Some(online), true) = (online, online_changed) else {
                continue;
            };
            changes.push(Change {
                name: self.device.watched[index].device.name.clone(),
                online,
            });
            if previous_online.is_some() {
                let change = if online { "online" } else { "offline" };
                messages.push(self.core.event(
                    entity,
                    CHANGE_CHANNEL,
                    EventKind::Received,
                    Some(Value::Text(change.into())),
                ));
            }
        }

        let unknown = scan
            .answered
            .keys()
            .filter(|mac| {
                !self
                    .device
                    .watched
                    .iter()
                    .any(|watch| watch.device.mac == **mac)
            })
            .count();
        let counts = Some((scan.answered.len() as i32, unknown as i32));
        if counts != self.device.counts {
            self.device.counts = counts;
            messages.extend(self.core.state(&self.device, NETWORK_ENTITY));
        }
        (messages, changes)
    }
}

impl SoftwareNode for NetworkDevicesNode {
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
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        vec![self.core.heartbeat(uptime_s, None, None)]
    }
}

impl Device for Devices {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let text = |id, max_length, access| Channel {
            id,
            name: None,
            access,
            value: Some(ValueType::Text { max_length }),
            quantity: None,
            unit: None,
        };
        let count = |id| Channel {
            id,
            name: None,
            access: Access::Read,
            value: Some(ValueType::Int {
                min: 0,
                max: MAXIMUM_COUNT,
                step: 1,
            }),
            quantity: None,
            unit: None,
        };
        let mut entities: Vec<EntityDescriptor> = self
            .watched
            .iter()
            .map(|watch| {
                EntityDescriptor {
                    id: watch.device.entity,
                    name: None,
                    channels: vec![
                        Channel {
                            id: ONLINE_CHANNEL,
                            name: None,
                            access: Access::Read,
                            value: Some(ValueType::Bool),
                            quantity: None,
                            unit: None,
                        }
                        .named("Online"),
                        text(CHANGE_CHANNEL, 8, Access::Event).named("Change"),
                        text(ADDRESS_CHANNEL, 15, Access::Read).named("Address"),
                        text(SINCE_CHANNEL, 16, Access::Read).named("Since"),
                    ],
                }
                .named(&watch.device.name)
            })
            .collect();
        entities.push(
            EntityDescriptor {
                id: NETWORK_ENTITY,
                name: None,
                channels: vec![
                    count(ANSWERING_CHANNEL).named("Answering"),
                    count(UNKNOWN_CHANNEL).named("Unknown"),
                ],
            }
            .named("Network"),
        );
        entities
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        if entity == NETWORK_ENTITY {
            let (answering, unknown) = self.counts?;
            return Some(vec![
                confirmed(ANSWERING_CHANNEL, Value::Int(answering)),
                confirmed(UNKNOWN_CHANNEL, Value::Int(unknown)),
            ]);
        }
        let watch = self
            .watched
            .iter()
            .find(|watch| watch.device.entity == entity)?;
        let mut values = vec![confirmed(ONLINE_CHANNEL, Value::Bool(watch.online?))];
        if let Some(address) = watch.address {
            values.push(confirmed(ADDRESS_CHANNEL, Value::Text(address.to_string())));
        }
        if let Some(since) = &watch.since {
            values.push(confirmed(SINCE_CHANNEL, Value::Text(since.clone())));
        }
        Some(values)
    }

    /// Every channel is read only or an event, so the core refuses commands before they get here.
    fn command(&mut self, entity: u8, channel: u8, _op: &CommandOp) -> Result<(), Rejection> {
        Err(Rejection::new(
            ErrorCode::InvalidOperation,
            format!("entity {entity} channel {channel} is read only"),
        ))
    }
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::Payload;

    use super::*;

    const PHONE: &str = "aa:bb:cc:00:00:01";
    const TELEVISION: &str = "aa:bb:cc:00:00:02";
    const VISITOR: &str = "aa:bb:cc:00:00:03";

    fn node(start: Instant) -> NetworkDevicesNode {
        NetworkDevicesNode::new(
            "network-devices".into(),
            vec![
                WatchedDevice {
                    entity: 1,
                    name: "Phone".into(),
                    mac: PHONE.parse().unwrap(),
                    offline_after: Duration::from_secs(600),
                },
                WatchedDevice {
                    entity: 2,
                    name: "Television".into(),
                    mac: TELEVISION.parse().unwrap(),
                    offline_after: Duration::from_secs(60),
                },
            ],
            start,
        )
    }

    fn answered(macs: &[&str]) -> HashMap<Mac, Ipv4Addr> {
        macs.iter()
            .zip(1..)
            .map(|(mac, host)| (mac.parse().unwrap(), Ipv4Addr::new(192, 168, 15, host)))
            .collect()
    }

    fn update(
        node: &mut NetworkDevicesNode,
        macs: &[&str],
        at: Instant,
        clock: &str,
    ) -> (Vec<Message>, Vec<Change>) {
        node.update(&Scan {
            answered: &answered(macs),
            now: at,
            clock,
        })
    }

    fn value(node: &NetworkDevicesNode, entity: u8, channel: u8) -> Option<Value> {
        node.device
            .state(entity)?
            .into_iter()
            .find(|value| value.channel == channel)
            .map(|value| value.value)
    }

    fn online(node: &NetworkDevicesNode, entity: u8) -> Option<bool> {
        match value(node, entity, ONLINE_CHANNEL)? {
            Value::Bool(online) => Some(online),
            _ => unreachable!(),
        }
    }

    fn text(node: &NetworkDevicesNode, entity: u8, channel: u8) -> Option<String> {
        match value(node, entity, channel)? {
            Value::Text(text) => Some(text),
            _ => unreachable!(),
        }
    }

    fn events(messages: &[Message]) -> Vec<(u8, Value)> {
        messages
            .iter()
            .filter_map(|message| match &message.payload {
                Payload::Event { entity, data, .. } => Some((*entity, data.clone()?)),
                _ => None,
            })
            .collect()
    }

    fn states(messages: &[Message]) -> Vec<u8> {
        messages
            .iter()
            .filter_map(|message| match &message.payload {
                Payload::State { entity, .. } => Some(*entity),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unknown_until_answering_or_silent_for_the_whole_wait() {
        let start = Instant::now();
        let mut node = node(start);
        assert_eq!(node.announcement().len(), 1, "only the Hello");

        let (messages, changes) = update(&mut node, &[PHONE], start, "2026-10-13 09:00");
        assert_eq!(online(&node, 1), Some(true));
        assert_eq!(online(&node, 2), None);
        assert_eq!(states(&messages), vec![1, NETWORK_ENTITY]);
        assert!(events(&messages).is_empty(), "no event on the first value");
        assert_eq!(changes.len(), 1);

        let later = start + Duration::from_secs(60);
        update(&mut node, &[PHONE], later, "2026-10-13 09:01");
        assert_eq!(online(&node, 2), Some(false));
        assert_eq!(text(&node, 2, SINCE_CHANNEL), None, "never seen");
    }

    #[test]
    fn offline_only_after_the_device_wait_with_an_event() {
        let start = Instant::now();
        let mut node = node(start);
        update(&mut node, &[PHONE, TELEVISION], start, "2026-10-13 09:00");

        let later = start + Duration::from_secs(300);
        let (messages, _) = update(&mut node, &[], later, "2026-10-13 09:05");
        assert_eq!(events(&messages), vec![(2, Value::Text("offline".into()))]);
        assert_eq!(
            online(&node, 1),
            Some(true),
            "a sleeping phone is still home"
        );
        assert_eq!(online(&node, 2), Some(false));

        let later = start + Duration::from_secs(600);
        let (messages, changes) = update(&mut node, &[], later, "2026-10-13 09:10");
        assert_eq!(events(&messages), vec![(1, Value::Text("offline".into()))]);
        assert!(!changes[0].online);
    }

    #[test]
    fn coming_back_is_immediate() {
        let start = Instant::now();
        let mut node = node(start);
        let later = start + Duration::from_secs(600);
        update(&mut node, &[], later, "2026-10-13 09:10");
        assert_eq!(online(&node, 1), Some(false));

        let later = start + Duration::from_secs(630);
        let (messages, _) = update(&mut node, &[PHONE], later, "2026-10-13 09:10");
        assert_eq!(events(&messages), vec![(1, Value::Text("online".into()))]);
        assert_eq!(online(&node, 1), Some(true));
    }

    #[test]
    fn since_is_when_it_came_online_or_last_answered() {
        let start = Instant::now();
        let mut node = node(start);
        update(&mut node, &[TELEVISION], start, "2026-10-13 09:00");
        let later = start + Duration::from_secs(30);
        update(&mut node, &[TELEVISION], later, "2026-10-13 09:00");
        assert_eq!(
            text(&node, 2, SINCE_CHANNEL).as_deref(),
            Some("2026-10-13 09:00")
        );

        let later = start + Duration::from_secs(60);
        update(&mut node, &[TELEVISION], later, "2026-10-13 09:01");
        assert_eq!(
            text(&node, 2, SINCE_CHANNEL).as_deref(),
            Some("2026-10-13 09:00"),
            "still the time it came online"
        );

        let later = start + Duration::from_secs(150);
        update(&mut node, &[], later, "2026-10-13 09:02");
        assert_eq!(online(&node, 2), Some(false));
        assert_eq!(
            text(&node, 2, SINCE_CHANNEL).as_deref(),
            Some("2026-10-13 09:01"),
            "the last answer, not when the wait ran out"
        );
    }

    #[test]
    fn a_new_address_is_sent_while_online() {
        let start = Instant::now();
        let mut node = node(start);
        update(&mut node, &[PHONE], start, "2026-10-13 09:00");
        assert_eq!(
            text(&node, 1, ADDRESS_CHANNEL).as_deref(),
            Some("192.168.15.1")
        );

        let later = start + Duration::from_secs(30);
        let (messages, changes) =
            update(&mut node, &[TELEVISION, PHONE], later, "2026-10-13 09:00");
        assert_eq!(
            text(&node, 1, ADDRESS_CHANNEL).as_deref(),
            Some("192.168.15.2")
        );
        assert!(states(&messages).contains(&1));
        assert!(events(&messages).iter().all(|(entity, _)| *entity != 1));
        assert!(changes.iter().all(|change| change.name != "Phone"));
    }

    #[test]
    fn the_network_counts_every_device_and_the_unknown_ones() {
        let start = Instant::now();
        let mut node = node(start);
        update(&mut node, &[PHONE, VISITOR], start, "2026-10-13 09:00");
        assert_eq!(
            value(&node, NETWORK_ENTITY, ANSWERING_CHANNEL),
            Some(Value::Int(2))
        );
        assert_eq!(
            value(&node, NETWORK_ENTITY, UNKNOWN_CHANNEL),
            Some(Value::Int(1))
        );

        let later = start + Duration::from_secs(30);
        let (messages, _) = update(&mut node, &[PHONE, VISITOR], later, "2026-10-13 09:00");
        assert!(messages.is_empty(), "same counts, nothing sent");
    }

    #[test]
    fn nothing_new_sends_nothing() {
        let start = Instant::now();
        let mut node = node(start);
        update(&mut node, &[PHONE, TELEVISION], start, "2026-10-13 09:00");
        let later = start + Duration::from_secs(30);
        let (messages, changes) =
            update(&mut node, &[PHONE, TELEVISION], later, "2026-10-13 09:00");
        assert!(messages.is_empty());
        assert!(changes.is_empty());
    }
}
