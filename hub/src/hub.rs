use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Instant, SystemTime};

use anyhow::bail;
use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use oxihome_protocol::management::{self, Parameter};
use oxihome_protocol::{CommandOp, Message, Payload, Value};
use tokio::sync::{broadcast, watch};

use crate::commands::Commands;
use crate::config::dashboard::{Dashboard, Dashboards};
use crate::config::nodes::{NameTarget, NodeEdits, Nodes};
use crate::config::panels::{Panel, Panels};
use crate::config::remotes::Remotes;
use crate::config::rules::{Rule, RuleRun, RuleUse, Rules};
use crate::config::{ConfigFile, FILES, HomeConfig, read_file, write_file};
use crate::mqtt::Mqtt;
use crate::node::events::{EventRecord, Events};
use crate::node::registry::{NodeEvent, Registry};
use crate::node::state::{ChannelUpdate, State};
use crate::notifications::{self, Notification, Notifications};
use crate::storage::{Restored, Storage, Write};

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
}

impl Change {
    pub fn event_name(self) -> &'static str {
        match self {
            Change::Nodes => "nodes",
            Change::Notifications => "notifications",
        }
    }
}

/// Shared by the MQTT loop service and the web server.
pub struct Hub {
    mqtt: Mqtt,
    registry: Mutex<Registry>,
    state: Mutex<State>,
    events: Mutex<Events>,
    config_directory: PathBuf,
    nodes: Mutex<Nodes>,
    rules: Mutex<Rules>,
    /// Last run of each rule, by name.
    rule_runs: Mutex<HashMap<String, RuleRun>>,
    remotes: Mutex<Remotes>,
    panels: Mutex<Panels>,
    dashboards: Mutex<Dashboards>,
    /// Newest first, without repeats of held keys.
    recent_codes: Mutex<VecDeque<ReceivedCode>>,
    notifications: Mutex<Notifications>,
    commands: Commands,
    storage: Storage,
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
            nodes: Mutex::new(config.nodes),
            rules: Mutex::new(config.rules),
            rule_runs: Mutex::new(restored.rule_runs.into_iter().collect()),
            remotes: Mutex::new(config.remotes),
            panels: Mutex::new(config.panels),
            dashboards: Mutex::new(config.dashboards),
            recent_codes: Mutex::new(recent_codes),
            notifications: Mutex::default(),
            commands: Commands::default(),
            storage,
            time_zone,
            changes: broadcast::channel(64).0,
            shutting_down: watch::channel(false).0,
        }
    }

    pub fn mqtt(&self) -> &Mqtt {
        &self.mqtt
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
                    &message.src,
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
    ) -> anyhow::Result<()> {
        self.commands
            .send(
                &self.mqtt,
                device_id,
                entity,
                channel,
                CommandOp::Set(value),
            )
            .await
    }

    /// Sends an `Invoke` and waits for the node's answer. The node's declaration picks `Enum` or
    /// `Text` and refuses values it does not take, without sending; an unknown node gets `Enum`.
    pub async fn invoke(
        &self,
        device_id: &str,
        entity: u8,
        channel: u8,
        value: Option<String>,
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
        self.commands
            .send(
                &self.mqtt,
                device_id,
                entity,
                channel,
                CommandOp::Invoke(value),
            )
            .await
    }

    /// Everything the web interface reads, locked together in one fixed order (the same as
    /// `handle`), so readers and the MQTT loop cannot deadlock.
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
        let config = HomeConfig::load(&self.config_directory)?;
        Ok(self.apply_config(config))
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
        *self.nodes.lock().unwrap() = config.nodes;
        *self.rules.lock().unwrap() = config.rules;
        *self.remotes.lock().unwrap() = config.remotes;
        *self.panels.lock().unwrap() = config.panels;
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

    pub fn panels(&self) -> Vec<Panel> {
        self.panels.lock().unwrap().all().to_vec()
    }

    /// The one the home page shows.
    pub fn dashboard(&self) -> Option<Dashboard> {
        self.dashboards.lock().unwrap().first().cloned()
    }

    pub fn rules_involving(&self, device_id: &str) -> Vec<RuleUse> {
        self.rules.lock().unwrap().involving(device_id)
    }

    pub fn rules_for_button(&self, panel: &str, button: &str) -> Vec<(String, bool)> {
        self.rules.lock().unwrap().for_button(panel, button)
    }

    pub fn pressed_rules(&self, panel: &str, button: &str) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .pressed(panel, button)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn recent_codes(&self) -> Vec<ReceivedCode> {
        self.recent_codes.lock().unwrap().iter().cloned().collect()
    }

    /// Adds a key to `remotes.toml` (keeping its comments), validates and applies the configuration.
    pub fn learn_key(&self, remote: &str, key: &str, code: &str) -> anyhow::Result<()> {
        let file = "remotes.toml";
        let current = self.config_file(file)?;
        let text = crate::config::remotes::with_key(&current.text, remote, key, code)?;
        self.save_config_file(file, &text, current.modified)?;
        Ok(())
    }

    /// Turns a rule on or off in `rules.toml` (keeping its comments), validates and applies the
    /// configuration.
    pub fn set_rule_enabled(&self, name: &str, enabled: bool) -> anyhow::Result<()> {
        let file = "rules.toml";
        let current = self.config_file(file)?;
        let text = crate::config::rules::with_rule_enabled(&current.text, name, enabled)?;
        self.save_config_file(file, &text, current.modified)?;
        Ok(())
    }

    /// Changes a node's names and parameters in `nodes.toml` (keeping its comments), validates
    /// and applies the configuration.
    pub fn edit_node(&self, device_id: &str, edits: &NodeEdits) -> anyhow::Result<()> {
        let file = "nodes.toml";
        let current = self.config_file(file)?;
        let text = crate::config::nodes::with_node_edits(&current.text, device_id, edits)?;
        self.save_config_file(file, &text, current.modified)?;
        Ok(())
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

    pub fn notify(&self, title: String, message: String, source: String) {
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

    pub fn notify_node_event(&self, event: &NodeEvent) {
        let name = self.node_name(event.device_id());
        if let Some((title, message)) = notifications::for_node_event(event, &name) {
            self.notify(title, message, event.device_id().to_string());
        }
    }
}

/// Codes worth naming: decoded IR codes, not the repeats of a held key nor undecoded `raw` timings.
fn learnable_code(event: &EventRecord) -> Option<&str> {
    match &event.data {
        Some(Value::Text(code))
            if event.kind == oxihome_protocol::EventKind::Received
                && !code.ends_with(" repeat")
                && !code.starts_with("raw") =>
        {
            Some(code)
        }
        _ => None,
    }
}
