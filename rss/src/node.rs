use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    EventKind, Message, Value, ValueType,
};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::fetch::Item;

/// The feeds use the numbers after it.
pub const ALL_FEEDS_ENTITY: u8 = 1;
const MARK_ALL_READ_CHANNEL: u8 = 0;
const TOTAL_UNREAD_CHANNEL: u8 = 1;

const TITLE_CHANNEL: u8 = 0;
const LINK_CHANNEL: u8 = 1;
const UNREAD_CHANNEL: u8 = 2;
const MARK_READ_CHANNEL: u8 = 3;
const REACHABLE_CHANNEL: u8 = 4;
const NEW_ITEM_CHANNEL: u8 = 5;

const TITLE_MAX_LENGTH: u16 = 256;
const LINK_MAX_LENGTH: u16 = 1024;
const SEPARATOR: &str = " — ";
const NEW_ITEM_MAX_LENGTH: u16 = TITLE_MAX_LENGTH + SEPARATOR.len() as u16 + LINK_MAX_LENGTH;
const UNREAD_MAX: i32 = 99_999;
/// More new items in one fetch only count as unread, so a feed that republishes everything does
/// not flood the rules.
const MAX_EVENTS_PER_FETCH: usize = 10;
const FAILURES_UNTIL_UNREACHABLE: u32 = 3;
/// Item ids remembered per feed besides the ones in the last fetch.
const SEEN_MAX: usize = 500;

pub struct Feed {
    pub entity: u8,
    pub name: String,
    pub url: String,
    /// On the title.
    pub filter: Option<Regex>,
}

/// What survives a restart, in the state file.
#[derive(Default, Serialize, Deserialize)]
pub struct SavedState {
    pub feeds: BTreeMap<u8, SavedFeed>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedFeed {
    /// Another URL for the same entity starts over.
    url: String,
    /// Newest first.
    seen: Vec<String>,
    unread: i32,
    title: String,
    link: String,
}

pub struct RssNode {
    core: NodeCore,
    device: RssDevice,
    started_at: Instant,
    /// `None` keeps nothing, for tests.
    state_path: Option<PathBuf>,
}

struct RssDevice {
    feeds: Vec<FeedState>,
    /// A command changed something to publish and save.
    changed: bool,
}

struct FeedState {
    feed: Feed,
    /// `None` until the first fetch works.
    saved: Option<SavedFeed>,
    failures: u32,
    /// `None` until a fetch works or fails enough.
    reachable: Option<bool>,
}

impl RssNode {
    pub fn new(
        device_id: String,
        feeds: Vec<Feed>,
        mut saved: SavedState,
        state_path: Option<PathBuf>,
    ) -> Self {
        let feeds = feeds
            .into_iter()
            .map(|feed| FeedState {
                saved: saved
                    .feeds
                    .remove(&feed.entity)
                    .filter(|saved| saved.url == feed.url),
                feed,
                failures: 0,
                reachable: None,
            })
            .collect();
        Self {
            core: NodeCore::new(device_id, Build::native("rss", env!("CARGO_PKG_VERSION"))),
            device: RssDevice {
                feeds,
                changed: false,
            },
            started_at: Instant::now(),
            state_path,
        }
    }

    /// The first fetch of a feed only marks its items as seen.
    pub fn fetched(&mut self, entity: u8, items: Vec<Item>) -> Vec<Message> {
        let Some(feed) = self.device.feed_mut(entity) else {
            return Vec::new();
        };
        let was_reachable = feed.reachable == Some(true);
        feed.failures = 0;
        feed.reachable = Some(true);

        let matching: Vec<&Item> = items
            .iter()
            .filter(|item| {
                feed.feed
                    .filter
                    .as_ref()
                    .is_none_or(|filter| filter.is_match(&item.title))
            })
            .collect();
        let new_items: Vec<&Item> = match &feed.saved {
            None => Vec::new(),
            Some(saved) => {
                let seen: HashSet<&str> = saved.seen.iter().map(String::as_str).collect();
                matching
                    .iter()
                    .copied()
                    .filter(|item| !seen.contains(item.id.as_str()))
                    .collect()
            }
        };
        let before = feed.saved.clone();
        let saved = feed.saved.get_or_insert_with(|| SavedFeed {
            url: feed.feed.url.clone(),
            seen: Vec::new(),
            unread: 0,
            title: String::new(),
            link: String::new(),
        });
        saved.seen = remembered(&items, &saved.seen);
        let added = i32::try_from(new_items.len()).unwrap_or(UNREAD_MAX);
        saved.unread = saved.unread.saturating_add(added).min(UNREAD_MAX);
        if let Some(latest) = matching.last() {
            saved.title = truncate(&latest.title, TITLE_MAX_LENGTH);
            saved.link = truncate(&latest.link, LINK_MAX_LENGTH);
        }
        let changed = before.as_ref() != Some(saved);

        let name = &feed.feed.name;
        if before.is_none() {
            println!("{name}: first fetch, {} items marked as seen", items.len());
        }
        for item in &new_items {
            println!("{name}: {}", item.title);
        }
        let events: Vec<Value> = new_items[new_items.len().saturating_sub(MAX_EVENTS_PER_FETCH)..]
            .iter()
            .map(|item| Value::Text(new_item_text(item)))
            .collect();

        let mut messages = Vec::new();
        if changed || !was_reachable {
            messages.extend(self.core.state(&self.device, entity));
        }
        if changed {
            messages.extend(self.core.state(&self.device, ALL_FEEDS_ENTITY));
            self.save();
        }
        for event in events {
            messages.push(self.core.event(
                entity,
                NEW_ITEM_CHANNEL,
                EventKind::Received,
                Some(event),
            ));
        }
        messages
    }

    pub fn not_modified(&mut self, entity: u8) -> Vec<Message> {
        let Some(feed) = self.device.feed_mut(entity) else {
            return Vec::new();
        };
        feed.failures = 0;
        if feed.reachable == Some(true) {
            return Vec::new();
        }
        feed.reachable = Some(true);
        self.core.state(&self.device, entity).into_iter().collect()
    }

    pub fn failed(&mut self, entity: u8) -> Vec<Message> {
        let Some(feed) = self.device.feed_mut(entity) else {
            return Vec::new();
        };
        feed.failures += 1;
        if feed.failures < FAILURES_UNTIL_UNREACHABLE || feed.reachable == Some(false) {
            return Vec::new();
        }
        feed.reachable = Some(false);
        self.core.state(&self.device, entity).into_iter().collect()
    }

    fn save(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let state = SavedState {
            feeds: self
                .device
                .feeds
                .iter()
                .filter_map(|feed| Some((feed.feed.entity, feed.saved.clone()?)))
                .collect(),
        };
        if let Err(error) = write_state(path, &state) {
            println!("saving {} failed: {error:#}", path.display());
        }
    }
}

impl SoftwareNode for RssNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        let mut answers = self.core.handle(&mut self.device, message);
        if std::mem::take(&mut self.device.changed) {
            answers.extend(self.core.states(&self.device));
            self.save();
        }
        answers
    }

    fn announcement(&mut self) -> Vec<Message> {
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        vec![self.core.heartbeat(uptime_s, None, None)]
    }
}

impl RssDevice {
    fn feed_mut(&mut self, entity: u8) -> Option<&mut FeedState> {
        self.feeds
            .iter_mut()
            .find(|feed| feed.feed.entity == entity)
    }

    fn mark_read(&mut self, entity: Option<u8>) {
        for feed in &mut self.feeds {
            if entity.is_none_or(|entity| entity == feed.feed.entity)
                && let Some(saved) = &mut feed.saved
            {
                saved.unread = 0;
            }
        }
        self.changed = true;
    }
}

impl Device for RssDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let unread = |id| Channel {
            id,
            name: None,
            access: Access::Read,
            value: Some(ValueType::Int {
                min: 0,
                max: UNREAD_MAX,
                step: 1,
            }),
            quantity: None,
            unit: None,
        };
        let button = |id| Channel {
            id,
            name: None,
            access: Access::Trigger,
            value: None,
            quantity: None,
            unit: None,
        };
        let text = |id, access, max_length| Channel {
            id,
            name: None,
            access,
            value: Some(ValueType::Text { max_length }),
            quantity: None,
            unit: None,
        };
        let mut entities = vec![
            EntityDescriptor {
                id: ALL_FEEDS_ENTITY,
                name: None,
                channels: vec![
                    button(MARK_ALL_READ_CHANNEL).named("Mark all as read"),
                    unread(TOTAL_UNREAD_CHANNEL).named("Unread"),
                ],
            }
            .named("All feeds"),
        ];
        entities.extend(self.feeds.iter().map(|feed| {
            EntityDescriptor {
                id: feed.feed.entity,
                name: None,
                channels: vec![
                    text(TITLE_CHANNEL, Access::Read, TITLE_MAX_LENGTH).named("Title"),
                    text(LINK_CHANNEL, Access::Read, LINK_MAX_LENGTH).named("Link"),
                    unread(UNREAD_CHANNEL).named("Unread"),
                    button(MARK_READ_CHANNEL).named("Mark as read"),
                    Channel {
                        id: REACHABLE_CHANNEL,
                        name: None,
                        access: Access::Read,
                        value: Some(ValueType::Bool),
                        quantity: None,
                        unit: None,
                    }
                    .named("Reachable"),
                    text(NEW_ITEM_CHANNEL, Access::Event, NEW_ITEM_MAX_LENGTH).named("New item"),
                ],
            }
            .named(&feed.feed.name)
        }));
        entities
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        if entity == ALL_FEEDS_ENTITY {
            let total = self
                .feeds
                .iter()
                .filter_map(|feed| feed.saved.as_ref())
                .fold(0, |total: i32, saved| total.saturating_add(saved.unread));
            return Some(vec![confirmed(
                TOTAL_UNREAD_CHANNEL,
                Value::Int(total.min(UNREAD_MAX)),
            )]);
        }
        let feed = self.feeds.iter().find(|feed| feed.feed.entity == entity)?;
        let mut values = Vec::new();
        if let Some(saved) = &feed.saved {
            values.push(confirmed(TITLE_CHANNEL, Value::Text(saved.title.clone())));
            values.push(confirmed(LINK_CHANNEL, Value::Text(saved.link.clone())));
        }
        let unread = feed.saved.as_ref().map_or(0, |saved| saved.unread);
        values.push(confirmed(UNREAD_CHANNEL, Value::Int(unread)));
        if let Some(reachable) = feed.reachable {
            values.push(confirmed(REACHABLE_CHANNEL, Value::Bool(reachable)));
        }
        Some(values)
    }

    fn command(&mut self, entity: u8, channel: u8, _op: &CommandOp) -> Result<(), Rejection> {
        match (entity, channel) {
            (ALL_FEEDS_ENTITY, MARK_ALL_READ_CHANNEL) => self.mark_read(None),
            (_, MARK_READ_CHANNEL) => self.mark_read(Some(entity)),
            _ => {
                return Err(Rejection::new(
                    ErrorCode::InvalidOperation,
                    format!("entity {entity} channel {channel} cannot be invoked"),
                ));
            }
        }
        Ok(())
    }
}

/// Missing or unreadable: every feed starts over, quietly.
pub fn read_state(path: &Path) -> SavedState {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            println!("{} is not valid, starting over: {error}", path.display());
            SavedState::default()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SavedState::default(),
        Err(error) => {
            println!("reading {} failed, starting over: {error}", path.display());
            SavedState::default()
        }
    }
}

/// Through a temporary file, so a crash never leaves half a file.
fn write_state(path: &Path, state: &SavedState) -> anyhow::Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// The ids in this fetch, then the older ones still remembered.
fn remembered(items: &[Item], seen: &[String]) -> Vec<String> {
    let current: Vec<String> = items.iter().rev().map(|item| item.id.clone()).collect();
    let in_fetch: HashSet<&str> = current.iter().map(String::as_str).collect();
    let older: Vec<String> = seen
        .iter()
        .filter(|id| !in_fetch.contains(id.as_str()))
        .cloned()
        .collect();
    let limit = SEEN_MAX.max(current.len());
    current.into_iter().chain(older).take(limit).collect()
}

fn new_item_text(item: &Item) -> String {
    let title = truncate(&item.title, TITLE_MAX_LENGTH);
    if item.link.is_empty() {
        return title;
    }
    format!(
        "{title}{SEPARATOR}{}",
        truncate(&item.link, LINK_MAX_LENGTH)
    )
}

/// At most `max_length` bytes, cut at a character boundary.
fn truncate(text: &str, max_length: u16) -> String {
    let mut end = text.len().min(max_length as usize);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
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

    const RELEASES: u8 = 2;
    const BLOG: u8 = 3;

    fn feed(entity: u8, filter: Option<&str>) -> Feed {
        Feed {
            entity,
            name: format!("feed {entity}"),
            url: format!("https://example.com/{entity}.atom"),
            filter: filter.map(|filter| Regex::new(filter).unwrap()),
        }
    }

    fn node(feeds: Vec<Feed>) -> RssNode {
        RssNode::new("rss".into(), feeds, SavedState::default(), None)
    }

    fn item(id: u32, title: &str) -> Item {
        Item {
            id: id.to_string(),
            title: title.into(),
            link: format!("https://example.com/{id}"),
            date: Some(id.into()),
        }
    }

    fn items(ids: std::ops::RangeInclusive<u32>) -> Vec<Item> {
        ids.map(|id| item(id, &format!("item {id}"))).collect()
    }

    fn unread(node: &RssNode, entity: u8) -> Value {
        let channel = if entity == ALL_FEEDS_ENTITY {
            TOTAL_UNREAD_CHANNEL
        } else {
            UNREAD_CHANNEL
        };
        node.device
            .state(entity)
            .unwrap()
            .into_iter()
            .find(|value| value.channel == channel)
            .unwrap()
            .value
    }

    fn events(messages: &[Message]) -> Vec<&Value> {
        messages
            .iter()
            .filter_map(|message| match &message.payload {
                Payload::Event {
                    channel: NEW_ITEM_CHANNEL,
                    data: Some(data),
                    ..
                } => Some(data),
                _ => None,
            })
            .collect()
    }

    fn invoke(node: &mut RssNode, id: u32, entity: u8, channel: u8) -> Vec<Message> {
        let message = Message::new(
            "oxihome-hub",
            id,
            Payload::Command {
                entity,
                channel,
                op: CommandOp::Invoke(None),
            },
        )
        .to("rss");
        node.handle(&message)
    }

    #[test]
    fn the_first_fetch_only_marks_items_as_seen() {
        let mut node = node(vec![feed(RELEASES, None)]);
        let messages = node.fetched(RELEASES, items(1..=3));
        assert!(events(&messages).is_empty());
        assert_eq!(unread(&node, RELEASES), Value::Int(0));
        let state = node.device.state(RELEASES).unwrap();
        assert_eq!(state[0].value, Value::Text("item 3".into()));
        assert_eq!(state[1].value, Value::Text("https://example.com/3".into()));
    }

    #[test]
    fn new_items_become_events_oldest_first_and_unread() {
        let mut node = node(vec![feed(RELEASES, None), feed(BLOG, None)]);
        node.fetched(RELEASES, items(1..=3));
        node.fetched(BLOG, items(1..=1));
        let messages = node.fetched(RELEASES, items(2..=5));
        assert_eq!(
            events(&messages),
            [
                &Value::Text("item 4 — https://example.com/4".into()),
                &Value::Text("item 5 — https://example.com/5".into()),
            ]
        );
        assert_eq!(unread(&node, RELEASES), Value::Int(2));
        assert_eq!(unread(&node, ALL_FEEDS_ENTITY), Value::Int(2));
        assert!(
            events(&node.fetched(RELEASES, items(2..=5))).is_empty(),
            "nothing new the second time"
        );
        assert_eq!(unread(&node, RELEASES), Value::Int(2));
    }

    #[test]
    fn a_flood_counts_everything_but_sends_only_the_newest_events() {
        let mut node = node(vec![feed(RELEASES, None)]);
        node.fetched(RELEASES, items(1..=1));
        let messages = node.fetched(RELEASES, items(1..=26));
        let events = events(&messages);
        assert_eq!(events.len(), MAX_EVENTS_PER_FETCH);
        assert_eq!(
            events.last().unwrap(),
            &&Value::Text("item 26 — https://example.com/26".into())
        );
        assert_eq!(unread(&node, RELEASES), Value::Int(25));
    }

    #[test]
    fn marking_as_read_per_feed_and_for_all() {
        let mut node = node(vec![feed(RELEASES, None), feed(BLOG, None)]);
        node.fetched(RELEASES, items(1..=1));
        node.fetched(BLOG, items(1..=1));
        node.fetched(RELEASES, items(1..=3));
        node.fetched(BLOG, items(1..=4));
        assert_eq!(unread(&node, ALL_FEEDS_ENTITY), Value::Int(5));

        let answers = invoke(&mut node, 1, BLOG, MARK_READ_CHANNEL);
        assert!(matches!(answers[0].payload, Payload::Ack { ok: true }));
        assert!(answers.iter().any(|answer| matches!(
            &answer.payload,
            Payload::State { entity: ALL_FEEDS_ENTITY, values } if values[0].value == Value::Int(2)
        )));
        assert_eq!(unread(&node, BLOG), Value::Int(0));
        assert_eq!(unread(&node, RELEASES), Value::Int(2));

        invoke(&mut node, 2, ALL_FEEDS_ENTITY, MARK_ALL_READ_CHANNEL);
        assert_eq!(unread(&node, RELEASES), Value::Int(0));
        assert_eq!(unread(&node, ALL_FEEDS_ENTITY), Value::Int(0));
    }

    #[test]
    fn the_filter_keeps_only_matching_titles() {
        let mut node = node(vec![feed(RELEASES, Some("(?i)^V1\\."))]);
        node.fetched(RELEASES, vec![item(1, "v1.0")]);
        let messages = node.fetched(
            RELEASES,
            vec![item(1, "v1.0"), item(2, "v2.0-rc1"), item(3, "v1.1")],
        );
        assert_eq!(
            events(&messages),
            [&Value::Text("v1.1 — https://example.com/3".into())]
        );
        assert_eq!(unread(&node, RELEASES), Value::Int(1));
    }

    #[test]
    fn a_feed_is_unreachable_after_three_failures_in_a_row() {
        let mut node = node(vec![feed(RELEASES, None)]);
        let reachable = |node: &RssNode| {
            node.device
                .state(RELEASES)
                .unwrap()
                .into_iter()
                .find(|value| value.channel == REACHABLE_CHANNEL)
                .map(|value| value.value)
        };
        assert!(node.failed(RELEASES).is_empty());
        assert!(node.failed(RELEASES).is_empty());
        assert_eq!(reachable(&node), None);
        assert_eq!(node.failed(RELEASES).len(), 1);
        assert_eq!(reachable(&node), Some(Value::Bool(false)));
        assert!(node.failed(RELEASES).is_empty(), "already unreachable");
        assert_eq!(node.not_modified(RELEASES).len(), 1);
        assert_eq!(reachable(&node), Some(Value::Bool(true)));
        node.failed(RELEASES);
        node.failed(RELEASES);
        assert_eq!(
            reachable(&node),
            Some(Value::Bool(true)),
            "the count restarted"
        );
    }

    #[test]
    fn the_state_survives_a_restart_unless_the_url_changed() {
        let directory = std::env::temp_dir().join(format!("oxihome-rss-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("rss-state.json");

        let mut first = RssNode::new(
            "rss".into(),
            vec![feed(RELEASES, None), feed(BLOG, None)],
            SavedState::default(),
            Some(path.clone()),
        );
        first.fetched(RELEASES, items(1..=1));
        first.fetched(RELEASES, items(1..=3));
        first.fetched(BLOG, items(1..=1));
        first.fetched(BLOG, items(1..=2));

        let mut moved = feed(BLOG, None);
        moved.url = "https://example.com/moved.atom".into();
        let mut second = RssNode::new(
            "rss".into(),
            vec![feed(RELEASES, None), moved],
            read_state(&path),
            None,
        );
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(unread(&second, RELEASES), Value::Int(2));
        assert_eq!(unread(&second, BLOG), Value::Int(0));
        assert!(events(&second.fetched(RELEASES, items(1..=3))).is_empty());
        assert!(
            events(&second.fetched(BLOG, items(1..=2))).is_empty(),
            "a new URL starts with a quiet first fetch"
        );
    }

    #[test]
    fn long_titles_are_cut_at_a_character_boundary() {
        let title = "ç".repeat(200);
        assert_eq!(truncate(&title, TITLE_MAX_LENGTH).len(), 256);
        assert_eq!(truncate("abc", 2), "ab");
        assert_eq!(truncate("aç", 2), "a");
    }
}
