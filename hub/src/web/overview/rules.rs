//! Rules as the pages describe them: triggers, conditions, actions, schedules and recent runs.

use std::time::Duration;

use jiff::{Timestamp, Zoned};
use oxihome_protocol::{Channel, Value, ValueType};

use super::super::format::{elapsed, value_label};
use super::{
    ActionOverview, Activity, RuleOverview, RulesSummary, channel_description, event_data_label,
};
use crate::config::nodes::{NameTarget, Nodes};
use crate::config::rules::{
    Action, ChannelCondition, Condition, CronTrigger, Day, EventTrigger, Expected, Limit,
    NoticeTrigger, OfflineTrigger, ScheduleTrigger, Subject, ThresholdTrigger, TrafficTrigger,
    Trigger, ValueTest, has_placeholders,
};
use crate::hub::Hub;
use crate::node::events::EventRecord;
use crate::node::registry::Registry;
use crate::storage::StoredNotification;
use crate::traffic::AlertKind;

pub fn rules_summary(hub: &Hub, upcoming_count: usize) -> RulesSummary {
    let local_now = Zoned::now().with_time_zone(hub.time_zone().clone());
    let rules = hub.rules();
    let mut upcoming: Vec<(Zoned, String)> = rules
        .iter()
        .filter(|rule| rule.enabled)
        .filter_map(|rule| Some((rule.when.next_run(&local_now)?, rule.name.clone())))
        .collect();
    upcoming.sort();
    let now = Timestamp::now();
    let mut failed: Vec<(String, Duration, Vec<String>)> = hub
        .rule_runs()
        .into_iter()
        .filter(|(name, run)| !run.errors.is_empty() && rules.iter().any(|rule| &rule.name == name))
        .map(|(name, run)| (name, elapsed(run.at, now), run.errors))
        .collect();
    failed.sort_by_key(|(_, ago, _)| *ago);
    RulesSummary {
        upcoming: upcoming
            .into_iter()
            .take(upcoming_count)
            .map(|(run, name)| (name, next_run_description(&local_now, &run)))
            .collect(),
        failed,
    }
}

/// Node events and notifications together, newest first.
pub fn activity(
    hub: &Hub,
    events: Vec<(EventRecord, Timestamp)>,
    notifications: Vec<StoredNotification>,
    limit: usize,
) -> Vec<Activity> {
    let snapshot = hub.snapshot();
    let now = Timestamp::now();
    let mut items: Vec<(Timestamp, Activity)> = events
        .into_iter()
        .map(|(event, at)| {
            let detail = match &event.data {
                Some(data) => format!(
                    "{:?} {}",
                    event.kind,
                    event_data_label(data, &snapshot.remotes)
                ),
                None => format!("{:?}", event.kind),
            };
            let activity = Activity {
                title: channel_description(
                    &snapshot.nodes,
                    &snapshot.registry,
                    &event.device_id,
                    event.entity,
                    event.channel,
                    None,
                ),
                detail,
                ago: elapsed(at, now),
                link: Some(format!("/nodes/{}", event.device_id)),
            };
            (at, activity)
        })
        .chain(notifications.into_iter().map(|notification| {
            let activity = Activity {
                title: notification.title,
                detail: notification.message,
                ago: elapsed(notification.at, now),
                link: None,
            };
            (notification.at, activity)
        }))
        .collect();
    items.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    items
        .into_iter()
        .take(limit)
        .map(|(_, activity)| activity)
        .collect()
}

/// Enabled rules whose last run had a failed action.
pub fn failing_rules(hub: &Hub) -> usize {
    let runs = hub.rule_runs();
    hub.rules()
        .iter()
        .filter(|rule| rule.enabled)
        .filter(|rule| {
            runs.get(&rule.name)
                .is_some_and(|run| !run.errors.is_empty())
        })
        .count()
}

/// Every rule in `rules.toml`, described with friendly names, for the rules page.
pub fn rules(hub: &Hub) -> Vec<RuleOverview> {
    let rules = hub.rules();
    let mut runs = hub.rule_runs();
    // Before the snapshot: the stand-ins ask the hub for names, which locks what it holds.
    let subjects: Vec<Subject> = rules
        .iter()
        .map(|rule| Subject::by_hand(&rule.when, hub))
        .collect();
    let snapshot = hub.snapshot();
    let now = Timestamp::now();
    let local_now = Zoned::now().with_time_zone(hub.time_zone().clone());
    rules
        .into_iter()
        .zip(subjects)
        .map(|(rule, subject)| RuleOverview {
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
                    text: action_description(&snapshot.nodes, &snapshot.registry, action),
                    warning: action_warning(&snapshot.registry, action, &subject),
                })
                .collect(),
            last_run: runs
                .remove(&rule.name)
                .map(|run| (elapsed(run.at, now), run.errors)),
            running: hub.running().includes(&rule.name),
            name: rule.name,
            enabled: rule.enabled,
        })
        .collect()
}

fn trigger_description(nodes: &Nodes, registry: &Registry, trigger: &Trigger) -> String {
    match trigger {
        Trigger::Event(trigger) => event_description(nodes, registry, trigger),
        Trigger::Threshold(trigger) => threshold_description(nodes, registry, trigger),
        Trigger::Schedule(trigger) => schedule_description(trigger),
        Trigger::Cron(trigger) => cron_description(trigger),
        Trigger::Offline(trigger) => offline_description(nodes, trigger),
        Trigger::Traffic(trigger) => traffic_description(nodes, trigger),
        Trigger::RemoteKey(trigger) => format!("{}: key {}", trigger.remote, trigger.key),
        Trigger::Button(trigger) => format!("Button \"{}\"", trigger.button),
        Trigger::Notice(trigger) => notice_description(nodes, trigger),
    }
}

fn notice_description(nodes: &Nodes, trigger: &NoticeTrigger) -> String {
    match &trigger.device {
        Some(device) => {
            let node = nodes
                .name(&NameTarget::Node(device.clone()))
                .unwrap_or(device);
            format!("{node} sends a notice")
        }
        None => "Any node sends a notice".into(),
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

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

fn traffic_description(nodes: &Nodes, trigger: &TrafficTrigger) -> String {
    let node = match &trigger.device {
        Some(device) => nodes
            .name(&NameTarget::Node(device.clone()))
            .unwrap_or(device)
            .to_string(),
        None => "any node".into(),
    };
    match trigger.kind {
        AlertKind::Stopped => format!("A channel of {node} stops reporting"),
        AlertKind::Noisy => format!("A channel of {node} gets noisy"),
        AlertKind::Unanswered => format!("Commands to {node} go unanswered"),
        AlertKind::Reported => format!("{} reports a problem", capitalized(&node)),
    }
}

fn event_description(nodes: &Nodes, registry: &Registry, trigger: &EventTrigger) -> String {
    let target = channel_description(
        nodes,
        registry,
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
        registry,
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
    let lasting = trigger
        .lasting
        .map(|lasting| format!(" for {lasting:#}"))
        .unwrap_or_default();
    format!(
        "{target}: {word} {}{lasting}",
        value_label(&Value::Float(limit), unit)
    )
}

/// A condition with friendly names, as the rules page lists it.
pub fn condition(hub: &Hub, condition: &Condition) -> String {
    let snapshot = hub.snapshot();
    condition_description(&snapshot.nodes, &snapshot.registry, condition)
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
        registry,
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

pub fn schedule_description(trigger: &ScheduleTrigger) -> String {
    format!(
        "{} at {}",
        days_description(&trigger.days),
        trigger.at.strftime("%H:%M")
    )
}

/// Croner's English text, e.g. `At 07:00, on Monday, Tuesday.`, with the expression after it.
pub fn cron_description(trigger: &CronTrigger) -> String {
    let text = trigger.cron.describe();
    format!("{} ({})", text.trim_end_matches('.'), trigger.expression)
}

/// `today 23:00`, `tomorrow 07:00` or `Monday 07:00`.
pub fn next_run_description(now: &Zoned, run: &Zoned) -> String {
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

fn action_description(nodes: &Nodes, registry: &Registry, action: &Action) -> String {
    match action {
        Action::Set {
            device,
            entity,
            channel,
            value,
        } => format!(
            "Set {} to {}",
            channel_description(nodes, registry, device, *entity, *channel, None),
            if *value { "on" } else { "off" }
        ),
        Action::Toggle {
            device,
            entity,
            channel,
        } => format!(
            "Toggle {}",
            channel_description(nodes, registry, device, *entity, *channel, None)
        ),
        Action::Invoke {
            device,
            entity,
            channel,
            value,
        } => {
            let target = channel_description(nodes, registry, device, *entity, *channel, None);
            match value {
                Some(value) => format!("Invoke {target} with {value}"),
                None => format!("Invoke {target}"),
            }
        }
        Action::Notify { title, message } => format!("Notify \"{title}\": {message}"),
        Action::Wait(time) => format!("Wait {time}"),
    }
}

/// Checks an invoke's value as the rule would send it, with the placeholders filled with the
/// stand-ins of a run by hand.
pub fn action_warning(registry: &Registry, action: &Action, subject: &Subject) -> Option<String> {
    let Action::Invoke {
        device,
        entity,
        channel,
        value,
    } = action
    else {
        return None;
    };
    let node = registry.node(device)?;
    let takes_options = node
        .channel(*entity, *channel)
        .is_some_and(|declared| matches!(declared.value, Some(ValueType::Enum(_))));
    if takes_options && value.as_deref().is_some_and(has_placeholders) {
        return Some("an option of the channel cannot have placeholders".into());
    }
    let filled = value.as_deref().map(|value| subject.fill(value));
    node.invoke_value(*entity, *channel, filled.as_deref())
        .err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::remotes::Remotes;
    use oxihome_protocol::{Access, EventKind, Quantity, Unit};

    #[tokio::test]
    async fn the_rules_page_does_not_lock_itself() {
        let hub = Hub::for_tests(
            "overview-rules",
            r#"
            [[rules]]
            name = "Hot"
            when = { device = "board", entity = 3, channel = 0, above = 28 }
            then = [{ notify = { title = "{node}", message = "{channel} at {value}" } }]
            "#,
        )
        .await;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(rules(&hub).len()).unwrap());
        assert_eq!(
            receiver.recv_timeout(std::time::Duration::from_secs(5)),
            Ok(1)
        );
    }

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
            event_description(&nodes(), &Registry::default(), &trigger),
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
            event_description(&nodes(), &Registry::default(), &trigger),
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
            event_description(&nodes(), &Registry::default(), &trigger),
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
                build: oxihome_protocol::Build::new("firmware", "0.6.0", "esp32"),
                ip: None,
                entities: vec![oxihome_protocol::EntityDescriptor {
                    id: 3,
                    name: None,
                    channels: vec![oxihome_protocol::Channel {
                        id: 1,
                        name: None,
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
            lasting: Some(jiff::SignedDuration::from_mins(5)),
        };
        assert_eq!(
            threshold_description(&nodes(), &registry, &trigger),
            "Test board › entity 3 · Humidity: above 85.0 % for 5m"
        );
    }

    #[test]
    fn nodes_toml_names_win_over_declared_ones() {
        let mut registry = Registry::new(3);
        let channel = |id: u8| oxihome_protocol::Channel {
            id,
            name: None,
            access: Access::ReadWrite,
            value: Some(ValueType::Bool),
            quantity: None,
            unit: None,
        };
        let hello = oxihome_protocol::Message::new(
            "esp32-test",
            0,
            oxihome_protocol::Payload::Hello {
                build: oxihome_protocol::Build::new("simulator", "0.6.0", "linux-x86_64"),
                ip: None,
                entities: vec![
                    oxihome_protocol::EntityDescriptor {
                        id: 1,
                        name: None,
                        channels: vec![channel(0), channel(1).named("Dimmer")],
                    }
                    .named("Lamp"),
                    oxihome_protocol::EntityDescriptor {
                        id: 4,
                        name: None,
                        channels: vec![channel(0), channel(1).named("Power")],
                    }
                    .named("Fan"),
                ],
            },
        );
        registry.handle(&hello, std::time::Instant::now());
        let description = |entity: u8, channel: u8| {
            channel_description(&nodes(), &registry, "esp32-test", entity, channel, None)
        };

        assert_eq!(description(1, 1), "Test board › LED · Brightness");
        assert_eq!(description(4, 1), "Test board › Fan · Power");
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
            action_description(&nodes(), &Registry::default(), &toggle),
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
            action_description(&nodes(), &Registry::default(), &set),
            "Set Test board › LED to off"
        );
        assert_eq!(
            action_description(&nodes(), &Registry::default(), &notify),
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
            action_description(&nodes, &Registry::default(), &invoke),
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
