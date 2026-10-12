//! What the pages show, built from a `Hub::snapshot`.

use std::time::Duration;

use jiff::{Timestamp, Zoned};
use oxihome_protocol::management::{self, PARAMETERS, Parameter};
use oxihome_protocol::{Access, Channel, EventKind, Quantity, Unit, Value, ValueType};

use super::format::{channel_label, elapsed, entity_label, unit_symbol, value_label};
use crate::config::dashboard::ChannelReference;
use crate::config::nodes::{ConfiguredEntity, NameTarget, Nodes};
use crate::config::remotes::Remotes;
use crate::config::rules::{
    Action, ChannelCondition, Condition, CronTrigger, Day, EventTrigger, Expected, Limit,
    OfflineTrigger, ScheduleTrigger, ThresholdTrigger, Trigger, ValueTest,
};
use crate::hub::{Hub, Snapshot};
use crate::node::events::data_label;
use crate::node::registry::{Node, Registry};

#[derive(Clone)]
pub struct NodeOverview {
    pub device_id: String,
    pub name: Option<String>,
    /// `None` for a node that never sent a `Hello`.
    pub firmware_version: Option<String>,
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
    pub channel: u8,
    pub channel_name: Option<String>,
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
                firmware_version: None,
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
            let name = |target: NameTarget| snapshot.nodes.name(&target).map(String::from);
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
    let name = |target: NameTarget| snapshot.nodes.name(&target).map(String::from);
    NodeOverview {
        device_id: device_id.to_string(),
        name: name(NameTarget::Node(device_id.to_string())),
        firmware_version: Some(node.firmware_version.clone()),
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
                    channel: channel.id,
                    channel_name: name(NameTarget::Channel(
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

/// Every rule in `rules.toml`, described with friendly names, for the rules page.
pub fn rules(hub: &Hub) -> Vec<RuleOverview> {
    let rules = hub.rules();
    let mut runs = hub.rule_runs();
    let snapshot = hub.snapshot();
    let now = Timestamp::now();
    let local_now = Zoned::now().with_time_zone(hub.time_zone().clone());
    rules
        .into_iter()
        .map(|rule| RuleOverview {
            trigger: trigger_description(&snapshot.nodes, &snapshot.registry, &rule.when),
            next_run: rule
                .when
                .next_run(&local_now)
                .filter(|_| rule.enabled)
                .map(|run| next_run_description(&local_now, &run)),
            conditions: rule
                .conditions
                .iter()
                .map(|condition| {
                    condition_description(&snapshot.nodes, &snapshot.registry, condition)
                })
                .collect(),
            actions: rule
                .then
                .iter()
                .map(|action| ActionOverview {
                    text: action_description(&snapshot.nodes, action),
                    warning: action_warning(&snapshot.registry, action),
                })
                .collect(),
            last_run: runs
                .remove(&rule.name)
                .map(|run| (elapsed(run.at, now), run.errors)),
            name: rule.name,
            enabled: rule.enabled,
        })
        .collect()
}

fn trigger_description(nodes: &Nodes, registry: &Registry, trigger: &Trigger) -> String {
    match trigger {
        Trigger::Event(trigger) => event_description(nodes, trigger),
        Trigger::Threshold(trigger) => threshold_description(nodes, registry, trigger),
        Trigger::Schedule(trigger) => schedule_description(trigger),
        Trigger::Cron(trigger) => cron_description(trigger),
        Trigger::Offline(trigger) => offline_description(nodes, trigger),
        Trigger::RemoteKey(trigger) => format!("{}: key {}", trigger.remote, trigger.key),
        Trigger::Button(trigger) => format!("Panel {} › {}", trigger.panel, trigger.button),
    }
}

fn offline_description(nodes: &Nodes, trigger: &OfflineTrigger) -> String {
    match &trigger.device {
        Some(device) => {
            let node = nodes
                .name(&NameTarget::Node(device.clone()))
                .unwrap_or(device);
            format!("{node} goes offline")
        }
        None => "Any node goes offline".into(),
    }
}

fn event_description(nodes: &Nodes, trigger: &EventTrigger) -> String {
    let target = channel_description(
        nodes,
        &trigger.device,
        trigger.entity,
        trigger.channel,
        None,
    );
    match &trigger.data {
        Some(data) => format!("{target}: {:?} {data}", trigger.event),
        None => format!("{target}: {:?}", trigger.event),
    }
}

const ALL_DAYS: [Day; 7] = [
    Day::Monday,
    Day::Tuesday,
    Day::Wednesday,
    Day::Thursday,
    Day::Friday,
    Day::Saturday,
    Day::Sunday,
];

/// The channel as the node's `Hello` declares it, for its quantity and unit.
fn declared_channel<'a>(
    registry: &'a Registry,
    device_id: &str,
    entity: u8,
    channel: u8,
) -> Option<&'a Channel> {
    registry
        .node(device_id)?
        .entities
        .iter()
        .find(|known| known.id == entity)?
        .channels
        .iter()
        .find(|known| known.id == channel)
}

/// `Living room › Climate sensor: above 28.0 °C`, with the unit the node declares.
fn threshold_description(nodes: &Nodes, registry: &Registry, trigger: &ThresholdTrigger) -> String {
    let declared = declared_channel(registry, &trigger.device, trigger.entity, trigger.channel);
    let target = channel_description(
        nodes,
        &trigger.device,
        trigger.entity,
        trigger.channel,
        declared.and_then(|channel| channel.quantity),
    );
    let unit = declared.and_then(|channel| channel.unit);
    let (word, limit) = match trigger.limit {
        Limit::Above(limit) => ("above", limit),
        Limit::Below(limit) => ("below", limit),
    };
    format!(
        "{target}: {word} {}",
        value_label(&Value::Float(limit), unit)
    )
}

fn condition_description(nodes: &Nodes, registry: &Registry, condition: &Condition) -> String {
    let node = |device: &String| {
        nodes
            .name(&NameTarget::Node(device.clone()))
            .unwrap_or(device)
            .to_string()
    };
    match condition {
        Condition::Channel(condition) => channel_condition_description(nodes, registry, condition),
        Condition::Time { from, to } => {
            format!(
                "From {} to {}",
                from.strftime("%H:%M"),
                to.strftime("%H:%M")
            )
        }
        Condition::Days(days) => days_description(days),
        Condition::Online(device) => format!("{} is online", node(device)),
        Condition::Offline(device) => format!("{} is offline", node(device)),
    }
}

/// `Test board › LED is on`, `Living room › Climate sensor is above 28.0 °C`.
fn channel_condition_description(
    nodes: &Nodes,
    registry: &Registry,
    condition: &ChannelCondition,
) -> String {
    let declared = declared_channel(
        registry,
        &condition.device,
        condition.entity,
        condition.channel,
    );
    let target = channel_description(
        nodes,
        &condition.device,
        condition.entity,
        condition.channel,
        declared.and_then(|channel| channel.quantity),
    );
    let unit = declared.and_then(|channel| channel.unit);
    let number = |number: f32| value_label(&Value::Float(number), unit);
    match &condition.test {
        ValueTest::Equals(Expected::Bool(true)) => format!("{target} is on"),
        ValueTest::Equals(Expected::Bool(false)) => format!("{target} is off"),
        ValueTest::Equals(Expected::Number(expected)) => {
            format!("{target} is {}", number(*expected))
        }
        ValueTest::Equals(Expected::Text(expected)) => format!("{target} is {expected}"),
        ValueTest::Above(limit) => format!("{target} is above {}", number(*limit)),
        ValueTest::Below(limit) => format!("{target} is below {}", number(*limit)),
    }
}

/// `Every day`, `Weekdays`, `Weekends` or the days in week order.
fn days_description(days: &[Day]) -> String {
    let has_exactly = |expected: &[Day]| {
        days.iter().all(|day| expected.contains(day))
            && expected.iter().all(|day| days.contains(day))
    };
    if days.is_empty() || has_exactly(&ALL_DAYS) {
        "Every day".to_string()
    } else if has_exactly(&ALL_DAYS[..5]) {
        "Weekdays".to_string()
    } else if has_exactly(&[Day::Saturday, Day::Sunday]) {
        "Weekends".to_string()
    } else {
        ALL_DAYS
            .iter()
            .filter(|day| days.contains(day))
            .map(|day| format!("{day:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn schedule_description(trigger: &ScheduleTrigger) -> String {
    format!(
        "{} at {}",
        days_description(&trigger.days),
        trigger.at.strftime("%H:%M")
    )
}

/// Croner's English text, e.g. `At 07:00, on Monday, Tuesday.`, with the expression after it.
fn cron_description(trigger: &CronTrigger) -> String {
    let text = trigger.cron.describe();
    format!("{} ({})", text.trim_end_matches('.'), trigger.expression)
}

/// `today 23:00`, `tomorrow 07:00` or `Monday 07:00`.
fn next_run_description(now: &Zoned, run: &Zoned) -> String {
    let day = if run.date() == now.date() {
        "today".to_string()
    } else if now
        .date()
        .tomorrow()
        .is_ok_and(|tomorrow| tomorrow == run.date())
    {
        "tomorrow".to_string()
    } else {
        run.strftime("%A").to_string()
    };
    format!("{day} {}", run.strftime("%H:%M"))
}

fn action_description(nodes: &Nodes, action: &Action) -> String {
    match action {
        Action::Set {
            device,
            entity,
            channel,
            value,
        } => format!(
            "Set {} to {}",
            channel_description(nodes, device, *entity, *channel, None),
            if *value { "on" } else { "off" }
        ),
        Action::Toggle {
            device,
            entity,
            channel,
        } => format!(
            "Toggle {}",
            channel_description(nodes, device, *entity, *channel, None)
        ),
        Action::Invoke {
            device,
            entity,
            channel,
            value,
        } => {
            let target = channel_description(nodes, device, *entity, *channel, None);
            match value {
                Some(value) => format!("Invoke {target} with {value}"),
                None => format!("Invoke {target}"),
            }
        }
        Action::Notify { title, message } => format!("Notify \"{title}\": {message}"),
    }
}

fn action_warning(registry: &Registry, action: &Action) -> Option<String> {
    let Action::Invoke {
        device,
        entity,
        channel,
        value,
    } = action
    else {
        return None;
    };
    registry
        .node(device)?
        .invoke_value(*entity, *channel, value.as_deref())
        .err()
}

/// `Test board › LED`; the channel only shows when it is not the entity's first.
fn channel_description(
    nodes: &Nodes,
    device_id: &str,
    entity: u8,
    channel: u8,
    quantity: Option<Quantity>,
) -> String {
    let name = |target: NameTarget| nodes.name(&target).map(String::from);
    let node =
        name(NameTarget::Node(device_id.to_string())).unwrap_or_else(|| device_id.to_string());
    let entity_name = entity_label(
        entity,
        &name(NameTarget::Entity(device_id.to_string(), entity)),
    );
    if channel == 0 {
        return format!("{node} › {entity_name}");
    }
    let channel_name = channel_label(
        channel,
        &name(NameTarget::Channel(device_id.to_string(), entity, channel)),
        quantity,
    );
    format!("{node} › {entity_name} · {channel_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxihome_protocol::EventKind;

    const NAMES: &str = r#"
        [nodes."esp32-test"]
        name = "Test board"
        entities.1 = { name = "LED", channels.1 = "Brightness" }
        entities.2 = { name = "BOOT button" }
    "#;

    fn nodes() -> Nodes {
        toml::from_str(NAMES).unwrap()
    }

    #[test]
    fn trigger_uses_friendly_names() {
        let trigger = EventTrigger {
            device: "esp32-test".into(),
            entity: 2,
            channel: 0,
            event: EventKind::LongPress,
            data: None,
        };
        assert_eq!(
            event_description(&nodes(), &trigger),
            "Test board › BOOT button: LongPress"
        );
    }

    #[test]
    fn unnamed_targets_fall_back_to_ids() {
        let trigger = EventTrigger {
            device: "sim-unknown".into(),
            entity: 3,
            channel: 0,
            event: EventKind::Press,
            data: None,
        };
        assert_eq!(
            event_description(&nodes(), &trigger),
            "sim-unknown › entity 3: Press"
        );
    }

    #[test]
    fn event_data_shows_after_the_event() {
        let trigger = EventTrigger {
            device: "esp32-test".into(),
            entity: 2,
            channel: 0,
            event: EventKind::Received,
            data: Some("NEC 00 52".into()),
        };
        assert_eq!(
            event_description(&nodes(), &trigger),
            "Test board › BOOT button: Received NEC 00 52"
        );
    }

    fn schedule(days: &[Day]) -> ScheduleTrigger {
        ScheduleTrigger {
            at: "07:00".parse().unwrap(),
            days: days.to_vec(),
        }
    }

    #[test]
    fn schedule_descriptions() {
        assert_eq!(schedule_description(&schedule(&[])), "Every day at 07:00");
        assert_eq!(
            schedule_description(&schedule(&ALL_DAYS)),
            "Every day at 07:00"
        );
        assert_eq!(
            schedule_description(&schedule(&ALL_DAYS[..5])),
            "Weekdays at 07:00"
        );
        assert_eq!(
            schedule_description(&schedule(&[Day::Sunday, Day::Saturday])),
            "Weekends at 07:00"
        );
        assert_eq!(
            schedule_description(&schedule(&[Day::Friday, Day::Monday])),
            "Monday, Friday at 07:00"
        );
    }

    #[test]
    fn threshold_description_uses_the_declared_unit() {
        let mut registry = Registry::new(3);
        let hello = oxihome_protocol::Message::new(
            "esp32-test",
            0,
            oxihome_protocol::Payload::Hello {
                fw_version: "0.6.0".into(),
                entities: vec![oxihome_protocol::EntityDescriptor {
                    id: 3,
                    channels: vec![oxihome_protocol::Channel {
                        id: 1,
                        access: Access::Read,
                        value: None,
                        quantity: Some(Quantity::Humidity),
                        unit: Some(Unit::Percent),
                    }],
                }],
            },
        );
        registry.handle(&hello, std::time::Instant::now());
        let trigger = ThresholdTrigger {
            device: "esp32-test".into(),
            entity: 3,
            channel: 1,
            limit: Limit::Above(85.0),
        };
        assert_eq!(
            threshold_description(&nodes(), &registry, &trigger),
            "Test board › entity 3 · Humidity: above 85.0 %"
        );
    }

    #[test]
    fn cron_description_shows_the_expression() {
        let rules: crate::config::rules::Rules = toml::from_str(
            r#"
            [[rules]]
            name = "Every half hour"
            when = { cron = "*/30 8-22 * * mon-fri" }
            then = []
            "#,
        )
        .unwrap();
        assert_eq!(
            trigger_description(&nodes(), &Registry::new(3), &rules.all()[0].when),
            "At every 30 minutes, of hour 8-22, on Monday, Tuesday, Wednesday, Thursday, and Friday (*/30 8-22 * * mon-fri)"
        );
    }

    #[test]
    fn next_run_is_relative_to_today() {
        let now: Zoned = "2026-10-11T22:00:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        let run = |text: &str| next_run_description(&now, &text.parse().unwrap());
        assert_eq!(
            run("2026-10-11T23:00:00-03:00[America/Sao_Paulo]"),
            "today 23:00"
        );
        assert_eq!(
            run("2026-10-12T07:00:00-03:00[America/Sao_Paulo]"),
            "tomorrow 07:00"
        );
        assert_eq!(
            run("2026-10-14T07:00:00-03:00[America/Sao_Paulo]"),
            "Wednesday 07:00"
        );
    }

    #[test]
    fn channels_after_the_first_are_shown() {
        let toggle = Action::Toggle {
            device: "esp32-test".into(),
            entity: 1,
            channel: 1,
        };
        assert_eq!(
            action_description(&nodes(), &toggle),
            "Toggle Test board › LED · Brightness"
        );
    }

    #[test]
    fn set_and_notify_descriptions() {
        let set = Action::Set {
            device: "esp32-test".into(),
            entity: 1,
            channel: 0,
            value: false,
        };
        let notify = Action::Notify {
            title: "Good night".into(),
            message: "All lamps are off".into(),
        };
        assert_eq!(
            action_description(&nodes(), &set),
            "Set Test board › LED to off"
        );
        assert_eq!(
            action_description(&nodes(), &notify),
            "Notify \"Good night\": All lamps are off"
        );
    }

    #[test]
    fn offline_and_invoke_descriptions() {
        let nodes = nodes();
        let bedroom = OfflineTrigger {
            device: Some("esp32-test".into()),
        };
        assert_eq!(
            offline_description(&nodes, &bedroom),
            "Test board goes offline"
        );
        assert_eq!(
            offline_description(&nodes, &OfflineTrigger { device: None }),
            "Any node goes offline"
        );
        let invoke = Action::Invoke {
            device: "esp32-test".into(),
            entity: 1,
            channel: 0,
            value: Some("Alarm".into()),
        };
        assert_eq!(
            action_description(&nodes, &invoke),
            "Invoke Test board › LED with Alarm"
        );
    }

    #[test]
    fn named_ir_codes_show_the_remote_and_key() {
        let remotes: Remotes =
            toml::from_str("[remotes.\"Kit remote\".keys]\n0 = \"NEC 00 52\"").unwrap();
        let text = |code: &str| Value::Text(code.into());
        assert_eq!(
            event_data_label(&text("NEC 00 52"), &remotes),
            "Kit remote · 0"
        );
        assert_eq!(
            event_data_label(&text("NEC 00 52 repeat"), &remotes),
            "Kit remote · 0 (held)"
        );
        assert_eq!(event_data_label(&text("NEC 00 18"), &remotes), "NEC 00 18");
    }
}
