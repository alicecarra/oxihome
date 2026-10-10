use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::bail;
use oxihome_protocol::{Access, CommandOp, EventKind, Message, Payload, Value, ValueType};
use tokio::sync::oneshot;

use crate::events::{EventRecord, Events};
use crate::mqtt::Mqtt;
use crate::names::{NameTarget, Names};
use crate::registry::{NodeEvent, Registry};
use crate::state::{ChannelUpdate, State};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared by the MQTT loop service and the web server.
pub struct Hub {
    mqtt: Mqtt,
    registry: Mutex<Registry>,
    state: Mutex<State>,
    events: Mutex<Events>,
    names: Mutex<Names>,
    /// Commands waiting for their `Ack` or `Error`, by message id.
    pending_commands: Mutex<HashMap<u32, PendingCommand>>,
}

struct PendingCommand {
    answer: oneshot::Sender<Payload>,
    entity: u8,
    channel: u8,
    value: Value,
}

pub struct NodeOverview {
    pub device_id: String,
    pub name: Option<String>,
    pub firmware_version: String,
    pub online: bool,
    pub uptime_seconds: Option<u32>,
    pub rssi: Option<i8>,
    pub channels: Vec<ChannelOverview>,
}

pub struct ChannelOverview {
    pub entity: u8,
    pub entity_name: Option<String>,
    pub channel: u8,
    pub channel_name: Option<String>,
    pub has_value: bool,
    pub value: Option<Value>,
    pub last_event: Option<(EventKind, Duration)>,
    pub switchable: bool,
}

impl Hub {
    pub fn new(mqtt: Mqtt, names: Names) -> Self {
        Self {
            mqtt,
            registry: Mutex::default(),
            state: Mutex::default(),
            events: Mutex::default(),
            names: Mutex::new(names),
            pending_commands: Mutex::default(),
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
        let pending = message
            .reply_to
            .and_then(|reply_to| self.pending_commands.lock().unwrap().remove(&reply_to));
        if let Some(pending) = pending {
            if message.payload == (Payload::Ack { ok: true }) {
                updates.extend(state.assume(
                    &message.src,
                    pending.entity,
                    pending.channel,
                    pending.value,
                ));
            }
            let _ = pending.answer.send(message.payload.clone());
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
        let message_id = self.mqtt.next_message_id();
        let (answer, receiver) = oneshot::channel();
        self.pending_commands.lock().unwrap().insert(
            message_id,
            PendingCommand {
                answer,
                entity,
                channel,
                value: value.clone(),
            },
        );

        let payload = Payload::Command {
            entity,
            channel,
            op: CommandOp::Set(value),
        };
        if let Err(error) = self
            .mqtt
            .publish(message_id, Some(device_id), payload)
            .await
        {
            self.pending_commands.lock().unwrap().remove(&message_id);
            return Err(error);
        }

        match tokio::time::timeout(COMMAND_TIMEOUT, receiver).await {
            Ok(Ok(Payload::Ack { ok: true })) => Ok(()),
            Ok(Ok(Payload::Error { code, detail })) => {
                bail!(
                    "{device_id} refused the command: error {code} {}",
                    detail.unwrap_or_default()
                )
            }
            Ok(Ok(other)) => bail!("unexpected answer from {device_id}: {other:?}"),
            Ok(Err(_)) => bail!("command to {device_id} was dropped"),
            Err(_) => {
                self.pending_commands.lock().unwrap().remove(&message_id);
                bail!(
                    "{device_id} did not answer within {} s",
                    COMMAND_TIMEOUT.as_secs()
                )
            }
        }
    }

    pub fn overview(&self) -> Vec<NodeOverview> {
        let registry = self.registry.lock().unwrap();
        let state = self.state.lock().unwrap();
        let events = self.events.lock().unwrap();
        let names = self.names.lock().unwrap();
        let now = Instant::now();
        let name = |target: NameTarget| names.get(&target).map(String::from);
        registry
            .nodes()
            .into_iter()
            .map(|(device_id, node)| NodeOverview {
                device_id: device_id.clone(),
                name: name(NameTarget::Node(device_id.clone())),
                firmware_version: node.firmware_version.clone(),
                online: node.online,
                uptime_seconds: node.uptime_seconds,
                rssi: node.rssi,
                channels: node
                    .entities
                    .iter()
                    .flat_map(|entity| {
                        entity.channels.iter().map(|channel| ChannelOverview {
                            entity: entity.id,
                            entity_name: name(NameTarget::Entity(device_id.clone(), entity.id)),
                            channel: channel.id,
                            channel_name: name(NameTarget::Channel(
                                device_id.clone(),
                                entity.id,
                                channel.id,
                            )),
                            has_value: channel.value.is_some(),
                            value: state.value(device_id, entity.id, channel.id).cloned(),
                            last_event: events
                                .last(device_id, entity.id, channel.id)
                                .map(|(kind, at)| (kind, now - at)),
                            switchable: channel.value == Some(ValueType::Bool)
                                && matches!(channel.access, Access::Write | Access::ReadWrite),
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    pub fn rename(&self, target: NameTarget, name: &str) -> anyhow::Result<()> {
        self.names.lock().unwrap().set(target, name)?;
        Ok(())
    }
}
