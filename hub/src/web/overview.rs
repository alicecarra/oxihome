//! What the pages show, built from a `Hub::snapshot`.

use std::time::{Duration, Instant};

use oxihome_protocol::management::{self, PARAMETERS, Parameter};
use oxihome_protocol::{Access, EventKind, Quantity, Unit, Value, ValueType};

use crate::hub::{Hub, Snapshot};
use crate::names::NameTarget;

pub struct NodeOverview {
    pub device_id: String,
    pub name: Option<String>,
    pub firmware_version: String,
    pub online: bool,
    pub uptime_seconds: Option<u32>,
    pub rssi: Option<i8>,
    pub channels: Vec<ChannelOverview>,
}

pub struct NodeParametersOverview {
    pub device_id: String,
    pub name: Option<String>,
    pub parameters: Vec<ParameterOverview>,
}

pub struct ParameterOverview {
    pub parameter: Parameter,
    /// As reported by the node.
    pub current: Option<i32>,
    /// From `nodes.toml`; `None` leaves the node's value alone.
    pub desired: Option<i32>,
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
    pub quantity: Option<Quantity>,
    pub unit: Option<Unit>,
}

/// One entry per registered node, for the nodes page.
pub fn nodes(hub: &Hub) -> Vec<NodeOverview> {
    let Snapshot {
        state,
        registry,
        events,
        names,
        ..
    } = &hub.snapshot();
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
                        quantity: channel.quantity,
                        unit: channel.unit,
                    })
                })
                .collect(),
        })
        .collect()
}

/// Nodes that declare the management entity, for the settings page.
pub fn parameters(hub: &Hub) -> Vec<NodeParametersOverview> {
    let Snapshot {
        state,
        registry,
        names,
        parameters,
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
            let desired = parameters.desired(device_id);
            NodeParametersOverview {
                device_id: device_id.clone(),
                name: names
                    .get(&NameTarget::Node(device_id.clone()))
                    .map(String::from),
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
