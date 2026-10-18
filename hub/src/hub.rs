use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::ensure;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use oxihome_protocol::management::{self};
use oxihome_protocol::{CommandOp, Message, PROTOCOL_VERSION, Payload, Value};
use tokio::sync::{broadcast, watch};

use crate::commands::Commands;
use crate::config::dashboard::Dashboards;
use crate::config::nodes::{NameTarget, Nodes};
use crate::config::remotes::Remotes;
use crate::config::rules::{RuleRun, Rules, Running};
use crate::config::{ConfigTexts, HomeConfig};
use crate::mqtt::Mqtt;
use crate::node::events::{EventRecord, Events};
use crate::node::hardware::HardwareSync;
use crate::node::registry::{NodeEvent, Registry};
use crate::node::state::{ChannelUpdate, State};
use crate::notifications::{self, Notification, Notifications};
use crate::ota::Ota;
use crate::storage::{
    History, Restored, Storage, StoredNotification, TimelineEntry, TimelineQuery, Write,
};
use crate::traffic::Traffic;
use crate::web::format::value_label;

mod config_files;
mod control;
mod hardware;
mod rules;
mod updates;

/// Read access for the web interface, see `Hub::snapshot`.
pub struct Snapshot<'a> {
    pub state: MutexGuard<'a, State>,
    pub registry: MutexGuard<'a, Registry>,
    pub events: MutexGuard<'a, Events>,
    pub nodes: MutexGuard<'a, Nodes>,
    pub remotes: MutexGuard<'a, Remotes>,
}

/// A code an IR receiver got, for learning remote keys.
#[derive(Debug, Clone)]
pub struct ReceivedCode {
    pub code: String,
    pub device_id: String,
    pub at: Timestamp,
}

const RECENT_CODES: usize = 10;
/// A reboot noticed this soon after an update ended is the update's own (the first heartbeat of
/// the new image reveals it).
const REBOOT_AFTER_UPDATE: Duration = Duration::from_secs(2 * 60);

/// What changed, announced to the pages listening on `/events`.
#[derive(Debug, Clone, Copy)]
pub enum Change {
    Nodes,
    Notifications,
    Timeline,
}

impl Change {
    pub fn event_name(self) -> &'static str {
        match self {
            Change::Nodes => "nodes",
            Change::Notifications => "notifications",
            Change::Timeline => "timeline",
        }
    }
}

/// Who sends a command. The timeline keeps the ones from the web interface and from rules.
#[derive(Clone)]
pub enum CommandOrigin {
    Web,
    Rule(String),
    /// Management commands: parameters, hardware, updates.
    Hub,
}

/// A newer firmware image for a node, or one being installed.
#[derive(Clone)]
pub enum UpdateStatus {
    Available(String),
    InProgress(String),
}

/// Shared by the MQTT loop service and the web server. Locks are unwrapped everywhere in the hub:
/// a panic while holding one stops the main loop, and Docker restarts the hub with a clean state.
pub struct Hub {
    mqtt: Mqtt,
    registry: Mutex<Registry>,
    state: Mutex<State>,
    events: Mutex<Events>,
    config_directory: PathBuf,
    /// Held across each read, check and write of the config files, so concurrent edits do not
    /// undo one another.
    config_writes: Mutex<()>,
    /// The config files as last applied or rejected; a reread with the same texts does nothing.
    config_seen: Mutex<ConfigTexts>,
    nodes: Mutex<Nodes>,
    rules: Mutex<Rules>,
    /// Last run of each rule, by name.
    rule_runs: Mutex<HashMap<String, RuleRun>>,
    running: Running,
    remotes: Mutex<Remotes>,
    dashboards: Mutex<Dashboards>,
    /// Newest first, without repeats of held keys.
    recent_codes: Mutex<VecDeque<ReceivedCode>>,
    notifications: Mutex<Notifications>,
    commands: Commands,
    hardware: Mutex<HardwareSync>,
    storage: Storage,
    /// `None` without `[ota]` in `hub.toml`.
    ota: Option<Arc<Ota>>,
    /// Nodes already notified for speaking another protocol version, with that version.
    other_protocols: Mutex<HashSet<(String, u8)>>,
    /// When each node's last update ended; the reboot it brings is not notified.
    updates_finished: Mutex<HashMap<String, Instant>>,
    /// Scheduled rules run in this zone's local time.
    time_zone: TimeZone,
    changes: broadcast::Sender<Change>,
    /// `true` once the hub is shutting down; open `/events` streams end then.
    shutting_down: watch::Sender<bool>,
}

/// What `Hub::new` starts from.
pub struct HubSetup {
    pub mqtt: Mqtt,
    pub missed_heartbeats: u32,
    pub config_directory: PathBuf,
    pub config: HomeConfig,
    pub time_zone: TimeZone,
    pub storage: Storage,
    pub restored: Restored,
    pub ota: Option<Arc<Ota>>,
}

impl Hub {
    pub fn new(setup: HubSetup) -> Self {
        let HubSetup {
            mqtt,
            missed_heartbeats,
            config_directory,
            config,
            time_zone,
            storage,
            restored,
            ota,
        } = setup;
        let now = Instant::now();
        let mut registry = Registry::new(missed_heartbeats);
        for node in restored.nodes {
            registry.restore(node, now);
        }
        let mut state = State::default();
        for (device_id, entity, channel, value, confidence) in restored.values {
            state.restore(device_id, entity, channel, value, confidence);
        }
        let mut events = Events::default();
        for (event, at) in restored.last_events {
            events.restore(event, at);
        }
        let mut recent_codes = VecDeque::new();
        for (event, at) in &restored.received_events {
            if let Some(code) = learnable_code(event)
                && recent_codes.len() < RECENT_CODES
                && !recent_codes
                    .iter()
                    .any(|received: &ReceivedCode| received.code == code)
            {
                recent_codes.push_back(ReceivedCode {
                    code: code.to_string(),
                    device_id: event.device_id.clone(),
                    at: *at,
                });
            }
        }
        let hub = Self {
            mqtt,
            registry: Mutex::new(registry),
            state: Mutex::new(state),
            events: Mutex::new(events),
            config_directory,
            config_writes: Mutex::default(),
            config_seen: Mutex::new(config.texts),
            nodes: Mutex::new(config.nodes),
            rules: Mutex::new(config.rules),
            rule_runs: Mutex::new(restored.rule_runs.into_iter().collect()),
            running: Running::default(),
            remotes: Mutex::new(config.remotes),
            dashboards: Mutex::new(config.dashboards),
            recent_codes: Mutex::new(recent_codes),
            notifications: Mutex::new(Notifications::starting_at(
                u64::try_from(Timestamp::now().as_millisecond()).unwrap_or(0),
            )),
            commands: Commands::default(),
            hardware: Mutex::default(),
            storage,
            ota,
            other_protocols: Mutex::default(),
            updates_finished: Mutex::default(),
            time_zone,
            changes: broadcast::channel(64).0,
            shutting_down: watch::channel(false).0,
        };
        hub.forget_runs_of_removed_rules();
        hub
    }

    pub fn mqtt(&self) -> &Mqtt {
        &self.mqtt
    }

    pub fn traffic(&self) -> &Traffic {
        self.mqtt.traffic()
    }

    pub fn handle(
        &self,
        message: &Message,
        now: Instant,
    ) -> (Vec<NodeEvent>, Vec<ChannelUpdate>, Option<EventRecord>) {
        let mut state = self.state.lock().unwrap();
        let mut updates = Vec::new();
        if let Some(pending) = self.commands.answered_by(message) {
            // Shown until the node's own `State` confirms it.
            if let (Payload::Ack { ok: true }, CommandOp::Set(value)) =
                (&message.payload, &pending.op)
            {
                updates.extend(state.assume(
                    &pending.device_id,
                    pending.entity,
                    pending.channel,
                    value.clone(),
                ));
            }
            pending.answer(message.payload.clone());
        }
        let at = Timestamp::now();
        let node_events = {
            let mut registry = self.registry.lock().unwrap();
            let node_events = registry.handle(message, now);
            let describes_node = matches!(
                message.payload,
                Payload::Hello { .. }
                    | Payload::Heartbeat { .. }
                    | Payload::State {
                        entity: management::ENTITY,
                        ..
                    }
            );
            if describes_node && let Some(stored) = registry.stored(&message.src) {
                self.storage.write(Write::Node(stored));
            }
            node_events
        };
        updates.extend(state.apply(message));
        for update in &updates {
            self.storage.write(Write::Value(update.clone(), at));
        }
        let event = self.events.lock().unwrap().apply(message, at);
        if let Some(event) = &event {
            self.storage.write(Write::Event(event.clone(), at));
            self.announce(Change::Timeline);
        }
        if let Some(code) = event.as_ref().and_then(learnable_code) {
            let mut recent = self.recent_codes.lock().unwrap();
            recent.retain(|received| received.code != code);
            recent.push_front(ReceivedCode {
                code: code.to_string(),
                device_id: message.src.clone(),
                at,
            });
            recent.truncate(RECENT_CODES);
        }
        if !node_events.is_empty() || !updates.is_empty() || event.is_some() {
            self.announce(Change::Nodes);
        }
        (node_events, updates, event)
    }

    pub fn check_presence(&self, now: Instant) -> Vec<NodeEvent> {
        let node_events = self.registry.lock().unwrap().check_presence(now);
        if !node_events.is_empty() {
            self.announce(Change::Nodes);
        }
        node_events
    }

    pub fn shut_down(&self) {
        self.shutting_down.send_replace(true);
    }

    /// A channel's numeric readings since `since`, grouped by `bucket`.
    pub async fn history(
        &self,
        device_id: &str,
        entity: u8,
        channel: u8,
        since: Timestamp,
        bucket: std::time::Duration,
    ) -> anyhow::Result<History> {
        self.storage
            .history(device_id, entity, channel, since, bucket)
            .await
    }

    /// Newest first.
    pub async fn recent_events(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<(EventRecord, Timestamp)>> {
        self.storage.recent_events(limit).await
    }

    /// One node's events, newest first.
    pub async fn events_of(
        &self,
        device_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(EventRecord, Timestamp)>> {
        self.storage.events_of(device_id, limit).await
    }

    /// Newest first.
    pub async fn recent_notifications(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredNotification>> {
        self.storage.recent_notifications(limit).await
    }

    /// Newest first.
    pub async fn timeline(&self, query: &TimelineQuery) -> anyhow::Result<Vec<TimelineEntry>> {
        self.storage.timeline(query).await
    }

    /// Waits until everything that happened is in the database.
    pub async fn flush_storage(&self) {
        self.storage.flush().await;
    }

    /// Resolves once `shut_down` is called.
    pub fn shutdown(&self) -> impl Future<Output = ()> + use<> {
        let mut receiver = self.shutting_down.subscribe();
        async move {
            let _ = receiver.wait_for(|shutting_down| *shutting_down).await;
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// Nobody listening is fine.
    fn announce(&self, change: Change) {
        let _ = self.changes.send(change);
    }

    /// Everything the web interface reads, locked together in one fixed order (the same as
    /// `handle`), so readers and the MQTT loop cannot deadlock.
    /// A hub with no broker, its files in a fresh temporary directory named after `name`, and
    /// `rules` as its rules.toml.
    #[cfg(test)]
    pub async fn for_tests(name: &str, rules: &str) -> Arc<Hub> {
        let directory =
            std::env::temp_dir().join(format!("oxihome-hub-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("rules.toml"), rules).unwrap();
        let (storage, _) = crate::storage::Storage::open(
            &directory.join("hub.sqlite"),
            std::time::Duration::from_secs(3600),
        )
        .await
        .unwrap();
        Arc::new(Hub::new(HubSetup {
            mqtt: Mqtt::offline(),
            missed_heartbeats: 3,
            config_directory: directory.clone(),
            config: HomeConfig::load(&directory).unwrap(),
            time_zone: TimeZone::UTC,
            storage,
            restored: crate::storage::Restored::default(),
            ota: Some(Arc::new(Ota::new(&crate::ota::OtaConfig {
                listen: "127.0.0.1:0".into(),
                public_url: "http://localhost".into(),
                images: directory.join("images"),
            }))),
        }))
    }

    #[cfg(test)]
    pub fn ota_for_tests(&self) -> &Ota {
        self.ota.as_deref().unwrap()
    }

    pub fn snapshot(&self) -> Snapshot<'_> {
        Snapshot {
            state: self.state.lock().unwrap(),
            registry: self.registry.lock().unwrap(),
            events: self.events.lock().unwrap(),
            nodes: self.nodes.lock().unwrap(),
            remotes: self.remotes.lock().unwrap(),
        }
    }

    pub fn device_ids(&self) -> Vec<String> {
        self.registry
            .lock()
            .unwrap()
            .nodes()
            .into_iter()
            .map(|(device_id, _)| device_id.clone())
            .collect()
    }

    pub fn time_zone(&self) -> &TimeZone {
        &self.time_zone
    }

    pub fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value> {
        self.state
            .lock()
            .unwrap()
            .value(device_id, entity, channel)
            .cloned()
    }

    /// Drops an offline node from memory and from the `nodes`, `channel_values` and `updates`
    /// tables; its history ages out with the rest. A node that speaks again comes back.
    pub fn forget_node(&self, device_id: &str) -> anyhow::Result<()> {
        ensure!(
            !self.is_online(device_id),
            "{device_id} is online; only an offline node can be forgotten"
        );
        ensure!(
            self.registry.lock().unwrap().forget(device_id),
            "{device_id} is not a known node"
        );
        self.state.lock().unwrap().forget(device_id);
        self.events.lock().unwrap().forget(device_id);
        let mut hardware = self.hardware.lock().unwrap();
        hardware.sending.remove(device_id);
        hardware.refused.remove(device_id);
        hardware.reports.remove(device_id);
        drop(hardware);
        self.other_protocols
            .lock()
            .unwrap()
            .retain(|(known, _)| known != device_id);
        self.storage.write(Write::Forget(device_id.to_string()));
        self.announce(Change::Nodes);
        Ok(())
    }

    pub fn is_online(&self, device_id: &str) -> bool {
        self.registry
            .lock()
            .unwrap()
            .node(device_id)
            .is_some_and(|node| node.online)
    }

    pub fn notify(&self, title: String, message: String, source: String) {
        self.storage.write(Write::Notification(StoredNotification {
            title: title.clone(),
            message: message.clone(),
            source: source.clone(),
            at: Timestamp::now(),
        }));
        self.notifications
            .lock()
            .unwrap()
            .push(title, message, source);
        self.announce(Change::Notifications);
    }

    pub fn last_notification_id(&self) -> u64 {
        self.notifications.lock().unwrap().last_id()
    }

    pub fn notifications_since(&self, id: u64) -> Vec<Notification> {
        self.notifications.lock().unwrap().since(id)
    }

    /// The node's friendly name, or its id.
    pub fn node_name(&self, device_id: &str) -> String {
        self.nodes
            .lock()
            .unwrap()
            .name(&NameTarget::Node(device_id.to_string()))
            .map_or_else(|| device_id.to_string(), String::from)
    }

    /// `LED · Brightness`, `LED`: from `nodes.toml`, or what the node declares.
    pub fn channel_name(&self, device_id: &str, entity: u8, channel: u8) -> String {
        let registry = self.registry.lock().unwrap();
        let nodes = self.nodes.lock().unwrap();
        let name = |target: NameTarget| {
            nodes
                .name(&target)
                .or_else(|| registry.declared_name(&target))
                .map(String::from)
        };
        let entity_name = name(NameTarget::Entity(device_id.to_string(), entity))
            .unwrap_or_else(|| format!("entity {entity}"));
        if channel == 0 {
            return entity_name;
        }
        let channel_name = name(NameTarget::Channel(device_id.to_string(), entity, channel))
            .unwrap_or_else(|| format!("channel {channel}"));
        format!("{entity_name} · {channel_name}")
    }

    /// `22.3 °C`: with the unit the node declares for the channel.
    pub fn value_label(&self, device_id: &str, entity: u8, channel: u8, value: &Value) -> String {
        let unit = self
            .registry
            .lock()
            .unwrap()
            .node(device_id)
            .and_then(|node| node.channel(entity, channel))
            .and_then(|channel| channel.unit);
        value_label(value, unit)
    }

    /// Newest first; empty when the database fails (logged).
    pub async fn notifications_about(
        &self,
        device_id: &str,
        limit: usize,
    ) -> Vec<StoredNotification> {
        self.storage
            .notifications_about(device_id, limit)
            .await
            .unwrap_or_else(|error| {
                println!("storage: notifications about {device_id}: {error:#}");
                Vec::new()
            })
    }

    /// Once per node and version: its messages are dropped until it speaks the hub's version.
    pub fn notify_other_protocol(&self, device_id: &str, version: u8) {
        let first = self
            .other_protocols
            .lock()
            .unwrap()
            .insert((device_id.to_string(), version));
        if !first {
            return;
        }
        println!("{device_id} speaks protocol {version}, the hub {PROTOCOL_VERSION}: ignored");
        let name = self.node_name(device_id);
        self.notify(
            format!("{name} speaks another protocol"),
            format!(
                "Protocol {version}, the hub speaks {PROTOCOL_VERSION}: its messages are ignored until it is updated"
            ),
            device_id.to_string(),
        );
    }

    pub fn notify_node_event(&self, event: &NodeEvent) {
        if let NodeEvent::Rebooted { device_id, .. } = event
            && self
                .updates_finished
                .lock()
                .unwrap()
                .get(device_id)
                .is_some_and(|finished| finished.elapsed() < REBOOT_AFTER_UPDATE)
        {
            return;
        }
        let name = self.node_name(event.device_id());
        if let Some((title, message)) = notifications::for_node_event(event, &name) {
            self.notify(title, message, event.device_id().to_string());
        }
    }
}

/// Codes worth naming: decoded IR codes, not the text other nodes receive (feeds, devices).
fn learnable_code(event: &EventRecord) -> Option<&str> {
    match &event.data {
        Some(Value::Text(code))
            if event.kind == oxihome_protocol::EventKind::Received
                && crate::config::remotes::is_decoded_code(code) =>
        {
            Some(code)
        }
        _ => None,
    }
}
