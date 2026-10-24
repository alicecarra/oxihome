//! What the pages show, built from a `Hub::snapshot`.

use std::time::Duration;

use jiff::Timestamp;
use oxihome_protocol::management::{self, PARAMETERS, Parameter};
use oxihome_protocol::{Access, Channel, EventKind, Quantity, Unit, Value, ValueType};

use super::format::{channel_label, elapsed, entity_label, unit_symbol};
use crate::config::dashboard::ChannelReference;
use crate::config::nodes::{ConfiguredEntity, NameTarget, Nodes};
use crate::config::remotes::Remotes;
use crate::hub::{Hub, Snapshot};
use crate::node::events::data_label;
use crate::node::registry::{Node, Registry};

mod rules;

pub use rules::{
    action_warning, activity, condition, cron_description, failing_rules, next_run_description,
    rules, rules_summary, schedule_description,
};

#[derive(Clone)]
pub struct NodeOverview {
    pub device_id: String,
    pub name: Option<String>,
    /// What runs on it, e.g. "firmware 0.12.1 (esp32)"; `None` for a node that never sent a `Hello`.
    pub software: Option<String>,
    /// Of its last `Hello`; `None` for a node never seen.
    pub protocol_version: Option<u8>,
    pub presence: Presence,
    /// How long ago the hub last heard from it; `None` when not seen.
    pub last_seen: Option<Duration>,
    pub uptime_seconds: Option<u32>,
    pub rssi: Option<i8>,
    pub channels: Vec<ChannelOverview>,
    /// From `nodes.toml`, for a node not seen yet: its channels are unknown until its `Hello`.
    pub configured_entities: Vec<ConfiguredEntity>,
}

/// In the order the home page lists nodes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Presence {
    Online,
    Offline,
    /// In `nodes.toml` but has never sent a `Hello`.
    NotSeen,
}

/// A channel of a dashboard `channels` block.
pub struct DashboardChannel {
    pub device_id: String,
    /// The node's name, or its id.
    pub node_label: String,
    /// `Entity · Channel`.
    pub label: String,
    /// Why it has no value or controls: the node is unknown or not seen, or does not declare it.
    pub channel: Result<ChannelOverview, &'static str>,
}

pub struct NodeParametersOverview {
    pub device_id: String,
    pub parameters: Vec<ParameterOverview>,
}

pub struct ParameterOverview {
    pub parameter: Parameter,
    /// As reported by the node.
    pub current: Option<i32>,
    /// From `nodes.toml`; `None` leaves the node's value alone.
    pub desired: Option<i32>,
}

#[derive(Clone)]
pub struct ChannelOverview {
    pub entity: u8,
    pub entity_name: Option<String>,
    /// What the node calls it in its `Hello`; `entity_name` is this unless `nodes.toml` has one.
    pub entity_declared_name: Option<String>,
    pub channel: u8,
    pub channel_name: Option<String>,
    pub channel_declared_name: Option<String>,
    pub has_value: bool,
    pub value: Option<Value>,
    /// Kind, data as text and how long ago.
    pub last_event: Option<(EventKind, Option<String>, Duration)>,
    pub switchable: bool,
    /// A number the node measures, worth a history chart.
    pub chartable: bool,
    pub quantity: Option<Quantity>,
    pub unit: Option<Unit>,
    /// `None` unless the channel is a `Trigger`.
    pub invocation: Option<Invocation>,
    /// What the channel is, e.g. `Float 0–50 °C · read only`.
    pub kind: String,
}

/// How the node card fires a `Trigger` channel.
#[derive(Clone)]
pub enum Invocation {
    /// One button per option of the `Enum`.
    Options(Vec<String>),
    /// A text box and a send button.
    Text { max_length: u16 },
    /// A single button, for a channel without a value.
    Plain,
}

pub struct RuleOverview {
    pub name: String,
    pub trigger: String,
    pub conditions: Vec<String>,
    pub actions: Vec<ActionOverview>,
    pub enabled: bool,
    /// Scheduled rules only.
    pub next_run: Option<String>,
    /// How long ago it ran and the errors of the actions that failed.
    pub last_run: Option<(Duration, Vec<String>)>,
    /// Its actions have not finished, e.g. during a wait.
    pub running: bool,
    /// When it runs if what it waits on with `for` stays so, soonest first, with the node for a
    /// rule about any node.
    pub waiting: Vec<String>,
}

/// What the dashboard's `rules` block lists.
pub struct RulesSummary {
    /// Name and when, soonest first.
    pub upcoming: Vec<(String, String)>,
    /// Name, how long ago and the errors, latest first.
    pub failed: Vec<(String, Duration, Vec<String>)>,
}

/// A node event or a notification, for the dashboard's `events` block.
pub struct Activity {
    pub title: String,
    pub detail: String,
    pub ago: Duration,
    /// The node's page, for node events.
    pub link: Option<String>,
}

pub struct ActionOverview {
    pub text: String,
    /// Why the node would refuse it, from what the node declares; `None` when fine or unknown.
    pub warning: Option<String>,
}

/// The registered nodes and the configured ones not seen yet, by presence then device id.
pub fn nodes(hub: &Hub) -> Vec<NodeOverview> {
    let snapshot = hub.snapshot();
    let now = Timestamp::now();
    let mut nodes: Vec<NodeOverview> = snapshot
        .registry
        .nodes()
        .into_iter()
        .map(|(device_id, node)| node_overview(&snapshot, device_id, node, now))
        .collect();
    nodes.extend(
        snapshot
            .nodes
            .devices_id()
            .filter(|device_id| snapshot.registry.node(device_id).is_none())
            .map(|device_id| NodeOverview {
                device_id: device_id.to_string(),
                name: snapshot
                    .nodes
                    .name(&NameTarget::Node(device_id.to_string()))
                    .map(String::from),
                software: None,
                protocol_version: None,
                presence: Presence::NotSeen,
                last_seen: None,
                uptime_seconds: None,
                rssi: None,
                channels: Vec::new(),
                configured_entities: snapshot.nodes.entities(device_id),
            }),
    );
    nodes.sort_by(|first, second| {
        (first.presence, &first.device_id).cmp(&(second.presence, &second.device_id))
    });
    nodes
}

/// In the order of `references`.
pub fn dashboard_channels(hub: &Hub, references: &[ChannelReference]) -> Vec<DashboardChannel> {
    let nodes = nodes(hub);
    let snapshot = hub.snapshot();
    references
        .iter()
        .map(|reference| {
            let node = nodes.iter().find(|node| node.device_id == reference.device);
            let channel = node.and_then(|node| {
                node.channels.iter().find(|channel| {
                    channel.entity == reference.entity && channel.channel == reference.channel
                })
            });
            let name = |target: NameTarget| name(&snapshot.nodes, &snapshot.registry, &target);
            let entity_name = name(NameTarget::Entity(
                reference.device.clone(),
                reference.entity,
            ));
            let channel_name = name(NameTarget::Channel(
                reference.device.clone(),
                reference.entity,
                reference.channel,
            ));
            DashboardChannel {
                device_id: reference.device.clone(),
                node_label: node
                    .and_then(|node| node.name.clone())
                    .unwrap_or_else(|| reference.device.clone()),
                label: format!(
                    "{} · {}",
                    entity_label(reference.entity, &entity_name),
                    channel_label(
                        reference.channel,
                        &channel_name,
                        channel.and_then(|channel| channel.quantity)
                    )
                ),
                channel: match (node, channel) {
                    (_, Some(channel)) => Ok(channel.clone()),
                    (None, None) => Err("unknown node"),
                    (Some(node), None) if node.presence == Presence::NotSeen => Err("not seen"),
                    (Some(_), None) => Err("unknown channel"),
                },
            }
        })
        .collect()
}

/// `None` for a node that never sent a `Hello`.
pub fn node(hub: &Hub, device_id: &str) -> Option<NodeOverview> {
    let snapshot = hub.snapshot();
    let node = snapshot.registry.node(device_id)?;
    Some(node_overview(&snapshot, device_id, node, Timestamp::now()))
}

fn node_overview(
    snapshot: &Snapshot,
    device_id: &str,
    node: &Node,
    now: Timestamp,
) -> NodeOverview {
    let name = |target: NameTarget| name(&snapshot.nodes, &snapshot.registry, &target);
    let declared_name =
        |target: NameTarget| snapshot.registry.declared_name(&target).map(String::from);
    NodeOverview {
        device_id: device_id.to_string(),
        name: name(NameTarget::Node(device_id.to_string())),
        software: Some(node.build.to_string()),
        protocol_version: Some(node.protocol_version),
        presence: if node.online {
            Presence::Online
        } else {
            Presence::Offline
        },
        last_seen: Some(elapsed(node.last_seen_at, now)),
        uptime_seconds: node.uptime_seconds,
        rssi: node.rssi,
        channels: node
            .entities
            .iter()
            .flat_map(|entity| {
                entity.channels.iter().map(|channel| ChannelOverview {
                    entity: entity.id,
                    entity_name: name(NameTarget::Entity(device_id.to_string(), entity.id)),
                    entity_declared_name: declared_name(NameTarget::Entity(
                        device_id.to_string(),
                        entity.id,
                    )),
                    channel: channel.id,
                    channel_name: name(NameTarget::Channel(
                        device_id.to_string(),
                        entity.id,
                        channel.id,
                    )),
                    channel_declared_name: declared_name(NameTarget::Channel(
                        device_id.to_string(),
                        entity.id,
                        channel.id,
                    )),
                    has_value: channel.value.is_some() && channel.access != Access::Event,
                    value: snapshot
                        .state
                        .value(device_id, entity.id, channel.id)
                        .cloned(),
                    last_event: snapshot.events.last(device_id, entity.id, channel.id).map(
                        |last| {
                            (
                                last.kind,
                                last.data
                                    .as_ref()
                                    .map(|data| event_data_label(data, &snapshot.remotes)),
                                elapsed(last.at, now),
                            )
                        },
                    ),
                    switchable: channel.value == Some(ValueType::Bool)
                        && matches!(channel.access, Access::Write | Access::ReadWrite),
                    chartable: channel.access == Access::Read
                        && matches!(
                            channel.value,
                            Some(ValueType::Int { .. } | ValueType::Float { .. })
                        ),
                    quantity: channel.quantity,
                    unit: channel.unit,
                    invocation: invocation(channel),
                    kind: channel_kind(channel),
                })
            })
            .collect(),
        configured_entities: Vec::new(),
    }
}

/// `Float 0–50 °C · read only`, `Enum Short, Long, Alarm · action`, `Bool · read and write`...
fn channel_kind(channel: &Channel) -> String {
    let unit = channel
        .unit
        .map(|unit| format!(" {}", unit_symbol(unit)))
        .unwrap_or_default();
    let value = match &channel.value {
        None => None,
        Some(ValueType::Bool) => Some("on/off".to_string()),
        Some(ValueType::Int { min, max, .. }) => Some(format!("whole number {min}–{max}{unit}")),
        Some(ValueType::Float { min, max, .. }) => Some(format!("number {min}–{max}{unit}")),
        Some(ValueType::Enum(options)) => Some(format!("one of {}", options.join(", "))),
        Some(ValueType::Color(model)) => Some(format!("{model:?} color")),
        Some(ValueType::Text { max_length }) => Some(format!("text up to {max_length} bytes")),
        Some(ValueType::Bytes) => Some("bytes".to_string()),
    };
    let access = match channel.access {
        Access::Read => "read only",
        Access::Write => "write only",
        Access::ReadWrite => "read and write",
        Access::Trigger => "action",
        Access::Event => "event",
    };
    match value {
        Some(value) => format!("{value} · {access}"),
        None => access.to_string(),
    }
}

/// An IR code named in `remotes.toml` shows as `Kit remote · 0`, `(held)` for its repeats.
pub fn event_data_label(data: &Value, remotes: &Remotes) -> String {
    if let Value::Text(code) = data {
        let (code, held) = match code.strip_suffix(" repeat") {
            Some(code) => (code, " (held)"),
            None => (code.as_str(), ""),
        };
        if let Some(named) = remotes.key_for(code) {
            return format!("{} · {}{held}", named.remote, named.key);
        }
    }
    data_label(data)
}

fn invocation(channel: &Channel) -> Option<Invocation> {
    if channel.access != Access::Trigger {
        return None;
    }
    Some(match &channel.value {
        Some(ValueType::Enum(options)) => Invocation::Options(options.clone()),
        Some(ValueType::Text { max_length }) => Invocation::Text {
            max_length: *max_length,
        },
        _ => Invocation::Plain,
    })
}

/// Nodes that declare the management entity, with their parameters.
pub fn parameters(hub: &Hub) -> Vec<NodeParametersOverview> {
    let Snapshot {
        state,
        registry,
        nodes,
        ..
    } = &hub.snapshot();
    registry
        .nodes()
        .into_iter()
        .filter(|(_, node)| {
            node.entities
                .iter()
                .any(|entity| entity.id == management::ENTITY)
        })
        .map(|(device_id, node)| {
            let desired = nodes.desired(device_id);
            NodeParametersOverview {
                device_id: device_id.clone(),
                parameters: PARAMETERS
                    .into_iter()
                    .filter(|parameter| node.declares(management::ENTITY, parameter.channel.into()))
                    .map(|parameter| ParameterOverview {
                        parameter,
                        current: match state.value(
                            device_id,
                            management::ENTITY,
                            parameter.channel.into(),
                        ) {
                            Some(Value::Int(value)) => Some(*value),
                            _ => None,
                        },
                        desired: desired
                            .iter()
                            .find(|(known, _)| *known == parameter)
                            .map(|(_, value)| *value),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// `nodes.toml` first, then what the node declares in its `Hello`.
pub fn name(nodes: &Nodes, registry: &Registry, target: &NameTarget) -> Option<String> {
    nodes
        .name(target)
        .or_else(|| registry.declared_name(target))
        .map(String::from)
}

/// `Test board › LED`; the channel only shows when it is not the entity's first.
pub fn channel_description(
    nodes: &Nodes,
    registry: &Registry,
    device_id: &str,
    entity: u8,
    channel: u8,
    quantity: Option<Quantity>,
) -> String {
    let node = name(nodes, registry, &NameTarget::Node(device_id.to_string()))
        .unwrap_or_else(|| device_id.to_string());
    let channel = channel_name(nodes, registry, device_id, entity, channel, quantity);
    format!("{node} › {channel}")
}

/// `LED · Brightness`, `LED`: `channel_description` without the node.
pub fn channel_name(
    nodes: &Nodes,
    registry: &Registry,
    device_id: &str,
    entity: u8,
    channel: u8,
    quantity: Option<Quantity>,
) -> String {
    let name = |target: NameTarget| name(nodes, registry, &target);
    let entity_name = entity_label(
        entity,
        &name(NameTarget::Entity(device_id.to_string(), entity)),
    );
    if channel == 0 {
        return entity_name;
    }
    let channel_name = channel_label(
        channel,
        &name(NameTarget::Channel(device_id.to_string(), entity, channel)),
        quantity,
    );
    format!("{entity_name} · {channel_name}")
}
