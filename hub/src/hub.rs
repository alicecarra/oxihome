use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime};

use anyhow::{Context, bail, ensure};
use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use oxihome_protocol::hardware::Hardware;
use oxihome_protocol::management::{self, ManagementChannel, Parameter};
use oxihome_protocol::{CommandOp, Message, PROTOCOL_VERSION, Payload, Value};
use tokio::sync::{broadcast, watch};

use crate::commands::Commands;
use crate::config::dashboard::{Dashboard, Dashboards};
use crate::config::nodes::{NameTarget, NodeEdits, Nodes};
use crate::config::remotes::Remotes;
use crate::config::rules::{Rule, RuleRun, RuleUse, Rules, Running};
use crate::config::{
    ConfigFile, ConfigTexts, FILES, HomeConfig, read_file, read_texts, write_file,
};
use crate::mqtt::Mqtt;
use crate::node::events::{EventRecord, Events};
use crate::node::hardware::{HardwareState, HardwareStatus, HardwareSync};
use crate::node::registry::{NodeEvent, Registry};
use crate::node::state::{ChannelUpdate, State};
use crate::notifications::{self, Notification, Notifications};
use crate::ota::{Ota, Update};
use crate::storage::{
    History, Operation, Restored, Storage, StoredCommand, StoredNotification, StoredUpdate,
    TimelineEntry, TimelineQuery, UpdateOutcome, Write,
};
use crate::traffic::{Alert, Traffic};
use crate::web::format::value_label;

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

/// Shared by the MQTT loop service and the web server.
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
    /// Scheduled rules run in this zone's local time.
    time_zone: TimeZone,
    changes: broadcast::Sender<Change>,
    /// `true` once the hub is shutting down; open `/events` streams end then.
    shutting_down: watch::Sender<bool>,
}

impl Hub {
    pub fn new(
        mqtt: Mqtt,
        missed_heartbeats: u32,
        config_directory: PathBuf,
        config: HomeConfig,
        time_zone: TimeZone,
        storage: Storage,
        restored: Restored,
        ota: Option<Arc<Ota>>,
    ) -> Self {
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
        Self {
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
            time_zone,
            changes: broadcast::channel(64).0,
            shutting_down: watch::channel(false).0,
        }
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

    /// Sends a `Set` and waits for the node's answer.
    pub async fn set(
        &self,
        device_id: &str,
        entity: u8,
        channel: u8,
        value: Value,
        origin: CommandOrigin,
    ) -> anyhow::Result<()> {
        let command = StoredCommand {
            device_id: device_id.to_string(),
            entity,
            channel,
            at: Timestamp::now(),
            operation: Operation::Set,
            value: Some(value.clone()),
            rule: None,
            error: None,
        };
        let result = self
            .commands
            .send(
                &self.mqtt,
                device_id,
                entity,
                channel,
                CommandOp::Set(value),
            )
            .await;
        self.record_command(origin, command, &result);
        result
    }

    /// Sends an `Invoke` and waits for the node's answer. The node's declaration picks `Enum` or
    /// `Text` and refuses values it does not take, without sending; an unknown node gets `Enum`.
    pub async fn invoke(
        &self,
        device_id: &str,
        entity: u8,
        channel: u8,
        value: Option<String>,
        origin: CommandOrigin,
    ) -> anyhow::Result<()> {
        let declared = self
            .registry
            .lock()
            .unwrap()
            .node(device_id)
            .map(|node| node.invoke_value(entity, channel, value.as_deref()));
        let value = match declared {
            Some(Ok(value)) => value,
            Some(Err(error)) => bail!("{device_id}: {error}"),
            None => value.map(Value::Enum),
        };
        let command = StoredCommand {
            device_id: device_id.to_string(),
            entity,
            channel,
            at: Timestamp::now(),
            operation: Operation::Invoke,
            value: value.clone(),
            rule: None,
            error: None,
        };
        let result = self
            .commands
            .send(
                &self.mqtt,
                device_id,
                entity,
                channel,
                CommandOp::Invoke(value),
            )
            .await;
        self.record_command(origin, command, &result);
        result
    }

    fn record_command(
        &self,
        origin: CommandOrigin,
        command: StoredCommand,
        result: &anyhow::Result<()>,
    ) {
        let rule = match origin {
            CommandOrigin::Web => None,
            CommandOrigin::Rule(name) => Some(name),
            CommandOrigin::Hub => return,
        };
        self.storage.write(Write::Command(StoredCommand {
            rule,
            error: result.as_ref().err().map(|error| format!("{error:#}")),
            ..command
        }));
        self.announce(Change::Timeline);
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
        Arc::new(Hub::new(
            Mqtt::offline(),
            3,
            directory.clone(),
            HomeConfig::load(&directory).unwrap(),
            TimeZone::UTC,
            storage,
            crate::storage::Restored::default(),
            None,
        ))
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

    /// Rereads the config directory; on error the current configuration stays.
    pub fn reload_config(&self) -> anyhow::Result<usize> {
        let _writing = self.config_writes.lock().unwrap();
        let config = HomeConfig::load(&self.config_directory)?;
        Ok(self.apply_config(config))
    }

    /// Like `reload_config`, for the file watcher: `None` when the files are as last seen,
    /// which includes the hub's own writes.
    pub fn reload_config_if_changed(&self) -> Option<anyhow::Result<usize>> {
        let _writing = self.config_writes.lock().unwrap();
        let texts = match read_texts(&self.config_directory) {
            Ok(texts) => texts,
            Err(error) => return Some(Err(error)),
        };
        let mut seen = self.config_seen.lock().unwrap();
        if *seen == texts {
            return None;
        }
        *seen = texts.clone();
        drop(seen);
        Some(
            HomeConfig::from_texts(&self.config_directory, texts)
                .map(|config| self.apply_config(config)),
        )
    }

    pub fn config_file(&self, file: &str) -> anyhow::Result<ConfigFile> {
        read_file(&self.config_directory.join(file))
    }

    /// Validates the whole configuration with the new text, then writes and applies it.
    /// Refuses when the file changed on disk since `expected_modified` was read.
    pub fn save_config_file(
        &self,
        file: &str,
        text: &str,
        expected_modified: Option<SystemTime>,
    ) -> anyhow::Result<usize> {
        let _writing = self.config_writes.lock().unwrap();
        self.write_config_file(file, text, expected_modified)
    }

    /// Like `save_config_file`, with the text made from the current one.
    fn edit_config_file(
        &self,
        file: &str,
        edit: impl FnOnce(&str) -> anyhow::Result<String>,
    ) -> anyhow::Result<()> {
        let _writing = self.config_writes.lock().unwrap();
        let current = self.config_file(file)?;
        let text = edit(&current.text)?;
        self.write_config_file(file, &text, current.modified)?;
        Ok(())
    }

    /// Only with `config_writes` held.
    fn write_config_file(
        &self,
        file: &str,
        text: &str,
        expected_modified: Option<SystemTime>,
    ) -> anyhow::Result<usize> {
        if !FILES.contains(&file) {
            bail!("{file} is not an editable config file");
        }
        let path = self.config_directory.join(file);
        if read_file(&path)?.modified != expected_modified {
            bail!("{file} changed on disk since this page was loaded; reload the page");
        }
        let config = HomeConfig::load_with(&self.config_directory, Some((file, text)))?;
        write_file(&path, text)?;
        Ok(self.apply_config(config))
    }

    /// Returns the number of rules.
    fn apply_config(&self, config: HomeConfig) -> usize {
        let rule_count = config.rules.len();
        *self.config_seen.lock().unwrap() = config.texts;
        *self.nodes.lock().unwrap() = config.nodes;
        self.hardware.lock().unwrap().refused.clear();
        *self.rules.lock().unwrap() = config.rules;
        *self.remotes.lock().unwrap() = config.remotes;
        *self.dashboards.lock().unwrap() = config.dashboards;
        self.announce(Change::Nodes);
        rule_count
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

    /// Desired parameters the node declares but reports with another value; empty while offline.
    pub fn parameter_differences(&self, device_id: &str) -> Vec<(Parameter, i32)> {
        let snapshot = self.snapshot();
        let Some(node) = snapshot.registry.node(device_id).filter(|node| node.online) else {
            return Vec::new();
        };
        snapshot
            .nodes
            .desired(device_id)
            .into_iter()
            .filter(|(parameter, value)| {
                let channel = u8::from(parameter.channel);
                node.declares(management::ENTITY, channel)
                    && snapshot.state.value(device_id, management::ENTITY, channel)
                        != Some(&Value::Int(*value))
            })
            .collect()
    }

    /// Locks the snapshot, then the hardware sync: never the other way around.
    pub fn hardware_status(&self, device_id: &str) -> HardwareStatus {
        let snapshot = self.snapshot();
        let desired = snapshot.nodes.hardware(device_id);
        let reported = match snapshot.state.value(
            device_id,
            management::ENTITY,
            ManagementChannel::Hardware.into(),
        ) {
            Some(Value::Text(json)) => Hardware::from_json(json).ok(),
            _ => None,
        };
        let node = snapshot.registry.node(device_id);
        let sync = self.hardware.lock().unwrap();
        let state = match (&desired, node) {
            (None, _) => HardwareState::NotConfigured,
            (Some(_), None) => HardwareState::Offline,
            (Some(_), Some(node)) if !node.online => HardwareState::Offline,
            (Some(desired), Some(node)) => {
                let problems = desired.validate(&node.build.target);
                if !node.declares(management::ENTITY, ManagementChannel::Hardware.into()) {
                    HardwareState::Unsupported
                } else if !problems.is_empty() {
                    HardwareState::Invalid(problems.iter().map(ToString::to_string).collect())
                } else if reported.as_ref() == Some(desired) {
                    HardwareState::Matches
                } else if sync.sending.contains(device_id) {
                    HardwareState::Sending
                } else if let Some((refused, reason)) = sync.refused.get(device_id)
                    && refused == desired
                {
                    HardwareState::Refused(reason.clone())
                } else {
                    HardwareState::Differs
                }
            }
        };
        HardwareStatus {
            desired,
            reported,
            state,
            reports: sync.reports.get(device_id).cloned().unwrap_or_default(),
        }
    }

    /// `false` when a send to the node is already under way.
    pub fn start_hardware_send(&self, device_id: &str) -> bool {
        let started = self
            .hardware
            .lock()
            .unwrap()
            .sending
            .insert(device_id.to_string());
        if started {
            self.announce(Change::Nodes);
        }
        started
    }

    /// Taken, the send lasts until the node's next `Hello`: until it restarts, the node still
    /// reports the hardware it mounts.
    pub fn finish_hardware_send(&self, device_id: &str, sent: Hardware, refusal: Option<String>) {
        let mut sync = self.hardware.lock().unwrap();
        match refusal {
            Some(reason) => {
                sync.sending.remove(device_id);
                sync.refused.insert(device_id.to_string(), (sent, reason));
            }
            None => {
                sync.refused.remove(device_id);
            }
        }
        drop(sync);
        self.announce(Change::Nodes);
    }

    /// An `Error` the node sent on its own, kept until its next `Hello`.
    pub fn record_node_report(&self, device_id: &str, detail: &str) {
        self.hardware
            .lock()
            .unwrap()
            .reports
            .entry(device_id.to_string())
            .or_default()
            .push(detail.to_string());
        self.announce(Change::Nodes);
    }

    /// A `Hello`: the node (re)started, and resends what it has to report.
    pub fn hardware_restarted(&self, device_id: &str) {
        let mut sync = self.hardware.lock().unwrap();
        sync.reports.remove(device_id);
        sync.sending.remove(device_id);
    }

    pub fn config_directory(&self) -> &Path {
        &self.config_directory
    }

    pub fn matching_rules(&self, event: &EventRecord) -> Vec<Rule> {
        let rules = self.rules.lock().unwrap();
        let remotes = self.remotes.lock().unwrap();
        rules
            .matching(event, &remotes)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The one the home page shows.
    pub fn dashboard(&self) -> Option<Dashboard> {
        self.dashboards.lock().unwrap().first().cloned()
    }

    pub fn rules_involving(&self, device_id: &str) -> Vec<RuleUse> {
        self.rules.lock().unwrap().involving(device_id)
    }

    pub fn button_rule(&self, button: &str) -> Option<Rule> {
        self.rules.lock().unwrap().button(button).cloned()
    }

    pub fn recent_codes(&self) -> Vec<ReceivedCode> {
        self.recent_codes.lock().unwrap().iter().cloned().collect()
    }

    /// Adds a key to `remotes.toml` (keeping its comments), validates and applies the configuration.
    pub fn learn_key(&self, remote: &str, key: &str, code: &str) -> anyhow::Result<()> {
        self.edit_config_file("remotes.toml", |text| {
            crate::config::remotes::with_key(text, remote, key, code)
        })
    }

    /// Fails when a rule still uses the key.
    pub fn forget_key(&self, remote: &str, key: &str) -> anyhow::Result<()> {
        self.edit_config_file("remotes.toml", |text| {
            crate::config::remotes::without_key(text, remote, key)
        })
    }

    /// Turns a rule on or off in `rules.toml` (keeping its comments), validates and applies the
    /// configuration.
    pub fn set_rule_enabled(&self, name: &str, enabled: bool) -> anyhow::Result<()> {
        self.edit_config_file("rules.toml", |text| {
            crate::config::rules::with_rule_enabled(text, name, enabled)
        })
    }

    /// Changes a node's names and parameters in `nodes.toml` (keeping its comments), validates
    /// and applies the configuration.
    pub fn edit_node(&self, device_id: &str, edits: &NodeEdits) -> anyhow::Result<()> {
        self.edit_config_file("nodes.toml", |text| {
            crate::config::nodes::with_node_edits(text, device_id, edits)
        })
    }

    pub fn crossed_rules(&self, update: &ChannelUpdate) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .crossed(update)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn offline_rules(&self, device_id: &str) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .offline(device_id)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn traffic_rules(&self, alert: &Alert) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .traffic(alert)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn due_rules(&self, minute: DateTime) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .due(minute)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn time_zone(&self) -> &TimeZone {
        &self.time_zone
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.rules.lock().unwrap().all().to_vec()
    }

    pub fn record_rule_run(&self, name: &str, run: RuleRun) {
        self.storage
            .write(Write::RuleRun(name.to_string(), run.clone()));
        self.rule_runs.lock().unwrap().insert(name.to_string(), run);
        self.announce(Change::Timeline);
    }

    pub fn running(&self) -> &Running {
        &self.running
    }

    pub fn rule_runs(&self) -> HashMap<String, RuleRun> {
        self.rule_runs.lock().unwrap().clone()
    }

    pub fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value> {
        self.state
            .lock()
            .unwrap()
            .value(device_id, entity, channel)
            .cloned()
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

    pub fn is_updating(&self, device_id: &str) -> bool {
        self.ota
            .as_ref()
            .is_some_and(|ota| ota.updating(device_id).is_some())
    }

    /// Only for an online node that declares the `Ota` channel.
    pub fn update_status(&self, device_id: &str) -> Option<UpdateStatus> {
        let ota = self.ota.as_ref()?;
        if let Some(version) = ota.updating(device_id) {
            return Some(UpdateStatus::InProgress(version));
        }
        let running = self.updatable_version(device_id)?;
        let (version, _) = ota.newer_image(device_id, &running)?;
        Some(UpdateStatus::Available(version))
    }

    /// Sends the node a single-use link to its newest image; the outcome comes with its next
    /// `Hello` (see `settle_update`).
    pub async fn start_update(&self, device_id: &str) -> anyhow::Result<()> {
        let ota = self
            .ota
            .as_ref()
            .context("over-the-air updates are off: no [ota] in hub.toml")?;
        ensure!(
            ota.updating(device_id).is_none(),
            "{device_id} is already updating"
        );
        let running = self
            .updatable_version(device_id)
            .with_context(|| format!("{device_id} is offline or does not update over the air"))?;
        let (version, image) = ota
            .newer_image(device_id, &running)
            .with_context(|| format!("no image newer than {running} for {device_id}"))?;
        let url = ota.start(device_id, running.clone(), version.clone(), image)?;
        self.announce(Change::Nodes);
        let channel = u8::from(ManagementChannel::Ota);
        if let Err(error) = self
            .invoke(
                device_id,
                management::ENTITY,
                channel,
                Some(url),
                CommandOrigin::Hub,
            )
            .await
        {
            if let Some(update) = ota.finish(device_id) {
                let detail = format!("{error:#}");
                self.record_update(device_id, update, UpdateOutcome::Failed, Some(detail));
            }
            return Err(error);
        }
        let name = self.node_name(device_id);
        self.notify(
            format!("{name} updating"),
            format!("Firmware {running} -> {version}"),
            device_id.to_string(),
        );
        Ok(())
    }

    /// A `Hello` after an update tells which version the node came back with.
    pub fn settle_update(&self, device_id: &str, running: &str) {
        let Some(update) = self.ota.as_ref().and_then(|ota| ota.finish(device_id)) else {
            return;
        };
        let outcome = if update.version == running {
            UpdateOutcome::Updated
        } else {
            UpdateOutcome::RolledBack
        };
        self.record_update(device_id, update, outcome, None);
    }

    /// The node reported an `Error` about the update (e.g. the download failed).
    pub fn fail_update(&self, device_id: &str, detail: &str) {
        if let Some(update) = self.ota.as_ref().and_then(|ota| ota.finish(device_id)) {
            self.record_update(
                device_id,
                update,
                UpdateOutcome::Failed,
                Some(detail.into()),
            );
        }
    }

    pub fn check_updates(&self, now: Instant) {
        let Some(ota) = &self.ota else {
            return;
        };
        for (device_id, update) in ota.timed_out(now) {
            self.record_update(&device_id, update, UpdateOutcome::NoAnswer, None);
        }
    }

    /// Stored for the node page, and notified.
    fn record_update(
        &self,
        device_id: &str,
        update: Update,
        outcome: UpdateOutcome,
        detail: Option<String>,
    ) {
        let name = self.node_name(device_id);
        let (title, message) = match outcome {
            UpdateOutcome::Updated => (
                format!("{name} updated"),
                format!("Firmware {}", update.version),
            ),
            UpdateOutcome::RolledBack => (
                format!("{name} update failed"),
                format!(
                    "Rolled back to {}; {} did not reach the broker",
                    update.from_version, update.version
                ),
            ),
            UpdateOutcome::Failed => (
                format!("{name} update failed"),
                format!(
                    "{}: {}",
                    update.version,
                    detail.as_deref().unwrap_or("error")
                ),
            ),
            UpdateOutcome::NoAnswer => (
                format!("{name} update failed"),
                format!("{}: no answer after the update", update.version),
            ),
        };
        self.storage.write(Write::Update(StoredUpdate {
            device_id: device_id.to_string(),
            started_at: update.started_at,
            finished_at: Timestamp::now(),
            from_version: update.from_version,
            to_version: update.version,
            outcome,
            detail,
        }));
        self.notify(title, message, device_id.to_string());
        self.announce(Change::Nodes);
    }

    /// Newest first; empty when the database fails (logged).
    pub async fn updates(&self, device_id: &str, limit: usize) -> Vec<StoredUpdate> {
        self.storage
            .updates(device_id, limit)
            .await
            .unwrap_or_else(|error| {
                println!("storage: updates of {device_id}: {error:#}");
                Vec::new()
            })
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

    fn updatable_version(&self, device_id: &str) -> Option<String> {
        let registry = self.registry.lock().unwrap();
        let node = registry.node(device_id)?;
        let declares = node.declares(management::ENTITY, ManagementChannel::Ota.into());
        (node.online && declares).then(|| node.build.version.clone())
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
