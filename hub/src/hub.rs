use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Instant, SystemTime};

use anyhow::bail;
use oxihome_protocol::management::{self, Parameter};
use oxihome_protocol::{Message, Payload, Value};

use crate::commands::Commands;
use crate::config::names::{NameTarget, Names};
use crate::config::rules::{Rule, Rules};
use crate::config::{ConfigFile, FILES, HomeConfig, read_file, write_file};
use crate::mqtt::Mqtt;
use crate::node::events::{EventRecord, Events};
use crate::node::parameters::NodeParameters;
use crate::node::registry::{NodeEvent, Registry};
use crate::node::state::{ChannelUpdate, State};
use crate::notifications::{self, Notification, Notifications};

/// Read access for the web interface, see `Hub::snapshot`.
pub struct Snapshot<'a> {
    pub state: MutexGuard<'a, State>,
    pub registry: MutexGuard<'a, Registry>,
    pub events: MutexGuard<'a, Events>,
    pub names: MutexGuard<'a, Names>,
    pub parameters: MutexGuard<'a, NodeParameters>,
}

/// Shared by the MQTT loop service and the web server.
pub struct Hub {
    mqtt: Mqtt,
    registry: Mutex<Registry>,
    state: Mutex<State>,
    events: Mutex<Events>,
    config_directory: PathBuf,
    names: Mutex<Names>,
    rules: Mutex<Rules>,
    parameters: Mutex<NodeParameters>,
    notifications: Mutex<Notifications>,
    commands: Commands,
}

impl Hub {
    pub fn new(
        mqtt: Mqtt,
        missed_heartbeats: u32,
        config_directory: PathBuf,
        config: HomeConfig,
    ) -> Self {
        Self {
            mqtt,
            registry: Mutex::new(Registry::new(missed_heartbeats)),
            state: Mutex::default(),
            events: Mutex::default(),
            config_directory,
            names: Mutex::new(config.names),
            rules: Mutex::new(config.rules),
            parameters: Mutex::new(config.parameters),
            notifications: Mutex::default(),
            commands: Commands::default(),
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
            if message.payload == (Payload::Ack { ok: true }) {
                updates.extend(state.assume(
                    &message.src,
                    pending.entity,
                    pending.channel,
                    pending.value.clone(),
                ));
            }
            pending.answer(message.payload.clone());
        }
        let node_events = self.registry.lock().unwrap().handle(message, now);
        updates.extend(state.apply(message));
        let event = self.events.lock().unwrap().apply(message, now);
        (node_events, updates, event)
    }

    pub fn check_presence(&self, now: Instant) -> Vec<NodeEvent> {
        self.registry.lock().unwrap().check_presence(now)
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
            .set(&self.mqtt, device_id, entity, channel, value)
            .await
    }

    /// Everything the web interface reads, locked together in one fixed order (the same as
    /// `handle`), so readers and the MQTT loop cannot deadlock.
    pub fn snapshot(&self) -> Snapshot<'_> {
        Snapshot {
            state: self.state.lock().unwrap(),
            registry: self.registry.lock().unwrap(),
            events: self.events.lock().unwrap(),
            names: self.names.lock().unwrap(),
            parameters: self.parameters.lock().unwrap(),
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
        *self.names.lock().unwrap() = config.names;
        *self.rules.lock().unwrap() = config.rules;
        *self.parameters.lock().unwrap() = config.parameters;
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
            .parameters
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
        self.rules
            .lock()
            .unwrap()
            .matching(event)
            .into_iter()
            .cloned()
            .collect()
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
    }

    pub fn last_notification_id(&self) -> u64 {
        self.notifications.lock().unwrap().last_id()
    }

    pub fn notifications_since(&self, id: u64) -> Vec<Notification> {
        self.notifications.lock().unwrap().since(id)
    }

    /// The node's friendly name, or its id.
    pub fn node_name(&self, device_id: &str) -> String {
        self.names
            .lock()
            .unwrap()
            .get(&NameTarget::Node(device_id.to_string()))
            .map_or_else(|| device_id.to_string(), String::from)
    }

    pub fn notify_node_event(&self, event: &NodeEvent) {
        let name = self.node_name(event.device_id());
        if let Some((title, message)) = notifications::for_node_event(event, &name) {
            self.notify(title, message, event.device_id().to_string());
        }
    }
}
