//! What the pages show, built from a `Hub::snapshot`.

use std::time::{Duration, Instant};

use jiff::Zoned;
use oxihome_protocol::management::{self, PARAMETERS, Parameter};
use oxihome_protocol::{Access, Channel, EventKind, Quantity, Unit, Value, ValueType};

use super::format::{channel_label, entity_label, value_label};
use crate::config::names::{NameTarget, Names};
use crate::config::rules::{
    Action, CronTrigger, Day, EventTrigger, Limit, OfflineTrigger, ScheduleTrigger,
    ThresholdTrigger, Trigger,
};
use crate::hub::{Hub, Snapshot};
use crate::node::events::data_label;
use crate::node::registry::Registry;

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
    /// Kind, data as text and how long ago.
    pub last_event: Option<(EventKind, Option<String>, Duration)>,
    pub switchable: bool,
    pub quantity: Option<Quantity>,
    pub unit: Option<Unit>,
    /// For `Trigger` channels: one button per declared option, or a single one without a value.
    pub invocations: Option<Vec<Option<String>>>,
}

pub struct RuleOverview {
    pub name: String,
    pub trigger: String,
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
                        has_value: channel.value.is_some() && channel.access != Access::Event,
                        value: state.value(device_id, entity.id, channel.id).cloned(),
                        last_event: events.last(device_id, entity.id, channel.id).map(|last| {
                            (last.kind, last.data.as_ref().map(data_label), now - last.at)
                        }),
                        switchable: channel.value == Some(ValueType::Bool)
                            && matches!(channel.access, Access::Write | Access::ReadWrite),
                        quantity: channel.quantity,
                        unit: channel.unit,
                        invocations: invocations(channel),
                    })
                })
                .collect(),
        })
        .collect()
}

fn invocations(channel: &Channel) -> Option<Vec<Option<String>>> {
    if channel.access != Access::Trigger {
        return None;
    }
    match &channel.value {
        Some(ValueType::Enum(options)) => Some(options.iter().cloned().map(Some).collect()),
        _ => Some(vec![None]),
    }
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

/// Every rule in `rules.toml`, described with friendly names, for the rules page.
pub fn rules(hub: &Hub) -> Vec<RuleOverview> {
    let rules = hub.rules();
    let mut runs = hub.rule_runs();
    let snapshot = hub.snapshot();
    let now = Instant::now();
    let local_now = Zoned::now().with_time_zone(hub.time_zone().clone());
    rules
        .into_iter()
        .map(|rule| RuleOverview {
            trigger: trigger_description(&snapshot.names, &snapshot.registry, &rule.when),
            next_run: rule
                .when
                .next_run(&local_now)
                .filter(|_| rule.enabled)
                .map(|run| next_run_description(&local_now, &run)),
            actions: rule
                .then
                .iter()
                .map(|action| ActionOverview {
                    text: action_description(&snapshot.names, action),
                    warning: action_warning(&snapshot.registry, action),
                })
                .collect(),
            last_run: runs
                .remove(&rule.name)
                .map(|run| (now - run.at, run.errors)),
            name: rule.name,
            enabled: rule.enabled,
        })
        .collect()
}

fn trigger_description(names: &Names, registry: &Registry, trigger: &Trigger) -> String {
    match trigger {
        Trigger::Event(trigger) => event_description(names, trigger),
        Trigger::Threshold(trigger) => threshold_description(names, registry, trigger),
        Trigger::Schedule(trigger) => schedule_description(trigger),
        Trigger::Cron(trigger) => cron_description(trigger),
        Trigger::Offline(trigger) => offline_description(names, trigger),
    }
}

fn offline_description(names: &Names, trigger: &OfflineTrigger) -> String {
    match &trigger.device {
        Some(device) => {
            let node = names
                .get(&NameTarget::Node(device.clone()))
                .unwrap_or(device);
            format!("{node} goes offline")
        }
        None => "Any node goes offline".into(),
    }
}

fn event_description(names: &Names, trigger: &EventTrigger) -> String {
    let target = channel_description(
        names,
        &trigger.device,
        trigger.entity,
        trigger.channel,
        None,
    );
    format!("{target}: {:?}", trigger.event)
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

/// `Living room › Climate sensor: above 28.0 °C`, with the unit the node declares.
fn threshold_description(names: &Names, registry: &Registry, trigger: &ThresholdTrigger) -> String {
    let declared = registry
        .node(&trigger.device)
        .and_then(|node| {
            node.entities
                .iter()
                .find(|entity| entity.id == trigger.entity)
        })
        .and_then(|entity| {
            entity
                .channels
                .iter()
                .find(|channel| channel.id == trigger.channel)
        });
    let target = channel_description(
        names,
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

fn schedule_description(trigger: &ScheduleTrigger) -> String {
    let has_exactly = |days: &[Day]| {
        trigger.days.iter().all(|day| days.contains(day))
            && days.iter().all(|day| trigger.days.contains(day))
    };
    let days = if trigger.days.is_empty() || has_exactly(&ALL_DAYS) {
        "Every day".to_string()
    } else if has_exactly(&ALL_DAYS[..5]) {
        "Weekdays".to_string()
    } else if has_exactly(&[Day::Saturday, Day::Sunday]) {
        "Weekends".to_string()
    } else {
        ALL_DAYS
            .iter()
            .filter(|day| trigger.days.contains(day))
            .map(|day| format!("{day:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("{days} at {}", trigger.at.strftime("%H:%M"))
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

fn action_description(names: &Names, action: &Action) -> String {
    match action {
        Action::Set {
            device,
            entity,
            channel,
            value,
        } => format!(
            "Set {} to {}",
            channel_description(names, device, *entity, *channel, None),
            if *value { "on" } else { "off" }
        ),
        Action::Toggle {
            device,
            entity,
            channel,
        } => format!(
            "Toggle {}",
            channel_description(names, device, *entity, *channel, None)
        ),
        Action::Invoke {
            device,
            entity,
            channel,
            value,
        } => {
            let target = channel_description(names, device, *entity, *channel, None);
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
        .check_invoke(*entity, *channel, value.as_deref())
        .err()
}

/// `Test board › LED`; the channel only shows when it is not the entity's first.
fn channel_description(
    names: &Names,
    device_id: &str,
    entity: u8,
    channel: u8,
    quantity: Option<Quantity>,
) -> String {
    let name = |target: NameTarget| names.get(&target).map(String::from);
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

    fn names() -> Names {
        toml::from_str(NAMES).unwrap()
    }

    #[test]
    fn trigger_uses_friendly_names() {
        let trigger = EventTrigger {
            device: "esp32-test".into(),
            entity: 2,
            channel: 0,
            event: EventKind::LongPress,
        };
        assert_eq!(
            event_description(&names(), &trigger),
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
        };
        assert_eq!(
            event_description(&names(), &trigger),
            "sim-unknown › entity 3: Press"
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
        registry.handle(&hello, Instant::now());
        let trigger = ThresholdTrigger {
            device: "esp32-test".into(),
            entity: 3,
            channel: 1,
            limit: Limit::Above(85.0),
        };
        assert_eq!(
            threshold_description(&names(), &registry, &trigger),
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
            trigger_description(&names(), &Registry::new(3), &rules.all()[0].when),
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
            action_description(&names(), &toggle),
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
            action_description(&names(), &set),
            "Set Test board › LED to off"
        );
        assert_eq!(
            action_description(&names(), &notify),
            "Notify \"Good night\": All lamps are off"
        );
    }

    #[test]
    fn offline_and_invoke_descriptions() {
        let names = names();
        let bedroom = OfflineTrigger {
            device: Some("esp32-test".into()),
        };
        assert_eq!(
            offline_description(&names, &bedroom),
            "Test board goes offline"
        );
        assert_eq!(
            offline_description(&names, &OfflineTrigger { device: None }),
            "Any node goes offline"
        );
        let invoke = Action::Invoke {
            device: "esp32-test".into(),
            entity: 1,
            channel: 0,
            value: Some("Alarm".into()),
        };
        assert_eq!(
            action_description(&names, &invoke),
            "Invoke Test board › LED with Alarm"
        );
    }
}
