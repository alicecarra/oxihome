//! The form that creates and edits a rule. Every change posts the whole form and the server
//! draws it again, so the fields follow the trigger, the channel and its type.

use std::collections::HashMap;
use std::sync::Arc;

use jiff::civil::Time;
use jiff::{SignedDuration, Zoned};
use oxihome_protocol::management;
use oxihome_protocol::{Access, Channel, EventKind, ValueType};
use serde::Deserialize;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, value};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        page, parse_query_params,
        response::{IntoResponse, Response},
        route,
    },
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::format::{channel_label, entity_label, rule_anchor, value_label};
use super::layout::layout;
use super::overview::{
    action_warning, cron_description, next_run_description, schedule_description,
};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::checkbox::checkbox;
use crate::components::input::input;
use crate::components::label::label;
use crate::components::select::select;
use crate::components::switch::switch;
use crate::config::nodes::NameTarget;
use crate::config::rules::{
    Action, Condition, Expected, Limit, Rule, Rules, Subject, TrafficTrigger, Trigger, ValueTest,
};
use crate::hub::Hub;
use crate::traffic::AlertKind;

type Built<T> = std::result::Result<T, String>;

const DAYS: [(&str, &str); 7] = [
    ("monday", "Mon"),
    ("tuesday", "Tue"),
    ("wednesday", "Wed"),
    ("thursday", "Thu"),
    ("friday", "Fri"),
    ("saturday", "Sat"),
    ("sunday", "Sun"),
];

/// A new `EventKind` fails to compile in `event_label`; add it here too.
const EVENTS: [EventKind; 4] = [
    EventKind::Press,
    EventKind::LongPress,
    EventKind::DoublePress,
    EventKind::Received,
];

fn event_label(event: EventKind) -> &'static str {
    match event {
        EventKind::Press => "Press",
        EventKind::LongPress => "Long press",
        EventKind::DoublePress => "Double press",
        EventKind::Received => "Received",
    }
}

fn event_key(event: EventKind) -> String {
    format!("{event:?}")
}

const ALERTS: [AlertKind; 4] = [
    AlertKind::Stopped,
    AlertKind::Noisy,
    AlertKind::Unanswered,
    AlertKind::Reported,
];

/// The `when` key, and what the select shows.
fn alert_texts(kind: AlertKind) -> (&'static str, &'static str) {
    match kind {
        AlertKind::Stopped => ("stopped", "A channel stopped reporting"),
        AlertKind::Noisy => ("noisy", "A channel is noisy"),
        AlertKind::Unanswered => ("unanswered", "Commands went unanswered"),
        AlertKind::Reported => ("reported", "The node reported a problem"),
    }
}

trait Kind: Copy + PartialEq + Default + 'static {
    const ALL: &'static [Self];

    fn key(self) -> &'static str;

    fn label(self) -> &'static str;

    fn from_key(key: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.key() == key)
            .unwrap_or_default()
    }

    fn choices() -> Vec<ChoiceGroup> {
        ungrouped(
            Self::ALL
                .iter()
                .map(|kind| Choice::new(kind.key(), kind.label()))
                .collect(),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
enum TriggerKind {
    #[default]
    Event,
    Threshold,
    Schedule,
    Cron,
    Offline,
    Traffic,
    RemoteKey,
    Button,
}

impl Kind for TriggerKind {
    const ALL: &'static [Self] = &[
        Self::Event,
        Self::Threshold,
        Self::Schedule,
        Self::Cron,
        Self::Offline,
        Self::Traffic,
        Self::RemoteKey,
        Self::Button,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Threshold => "threshold",
            Self::Schedule => "schedule",
            Self::Cron => "cron",
            Self::Offline => "offline",
            Self::Traffic => "traffic",
            Self::RemoteKey => "remote_key",
            Self::Button => "button",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Event => "An event of a channel",
            Self::Threshold => "A value crossing a limit",
            Self::Schedule => "At a time of day",
            Self::Cron => "A cron expression",
            Self::Offline => "A node going offline",
            Self::Traffic => "A traffic alert",
            Self::RemoteKey => "A remote key",
            Self::Button => "A button",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
enum ConditionKind {
    #[default]
    Channel,
    Time,
    Days,
    Online,
    Offline,
}

impl Kind for ConditionKind {
    const ALL: &'static [Self] = &[
        Self::Channel,
        Self::Time,
        Self::Days,
        Self::Online,
        Self::Offline,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Channel => "channel",
            Self::Time => "time",
            Self::Days => "days",
            Self::Online => "online",
            Self::Offline => "offline",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Channel => "A channel's value",
            Self::Time => "Time of day",
            Self::Days => "Days of the week",
            Self::Online => "A node is online",
            Self::Offline => "A node is offline",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
enum ActionKind {
    #[default]
    Set,
    Toggle,
    Invoke,
    Notify,
    Wait,
}

impl Kind for ActionKind {
    const ALL: &'static [Self] = &[
        Self::Set,
        Self::Toggle,
        Self::Invoke,
        Self::Notify,
        Self::Wait,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Toggle => "toggle",
            Self::Invoke => "invoke",
            Self::Notify => "notify",
            Self::Wait => "wait",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Set => "Turn on or off",
            Self::Toggle => "Toggle",
            Self::Invoke => "Invoke an action",
            Self::Notify => "Notify",
            Self::Wait => "Wait",
        }
    }
}

/// What the form holds, as text; checked when it becomes TOML.
#[derive(Clone, Default)]
struct Draft {
    /// The name of the rule being edited; `None` for a new one.
    previous: Option<String>,
    name: String,
    enabled: bool,
    /// A duration such as `1h`; empty for none.
    cooldown: String,
    trigger: TriggerDraft,
    conditions: Vec<ConditionDraft>,
    actions: Vec<ActionDraft>,
}

#[derive(Clone, Default)]
struct TriggerDraft {
    kind: TriggerKind,
    /// A `ChannelKey` as text.
    channel: String,
    event: String,
    data: String,
    /// `above` or `below`.
    direction: String,
    limit: String,
    /// The `for` of a threshold; empty for none.
    lasting: String,
    at: String,
    days: Vec<String>,
    cron: String,
    /// A device id, or `*` for any node.
    device: String,
    /// The `when` key of the traffic alert.
    alert: String,
    /// Remote and key as a pair.
    remote_key: String,
    button: String,
}

#[derive(Clone, Default)]
struct ConditionDraft {
    kind: ConditionKind,
    channel: String,
    /// `equals`, `above` or `below`.
    test: String,
    value: String,
    from: String,
    to: String,
    days: Vec<String>,
    device: String,
}

#[derive(Clone, Default)]
struct ActionDraft {
    kind: ActionKind,
    channel: String,
    value: String,
    title: String,
    message: String,
    wait: String,
}

#[derive(Clone, PartialEq)]
struct ChannelKey {
    device: String,
    entity: u8,
    channel: u8,
}

impl ChannelKey {
    fn text(device: &str, entity: u8, channel: u8) -> String {
        serde_json::json!([device, entity, channel]).to_string()
    }

    fn parse(text: &str) -> Option<Self> {
        let (device, entity, channel) = serde_json::from_str(text).ok()?;
        Some(Self {
            device,
            entity,
            channel,
        })
    }
}

fn pair(first: &str, second: &str) -> String {
    serde_json::json!([first, second]).to_string()
}

fn parse_pair(text: &str) -> Option<(String, String)> {
    serde_json::from_str(text).ok()
}

fn day_key(day: crate::config::rules::Day) -> String {
    format!("{day:?}").to_lowercase()
}

fn duration_text(duration: SignedDuration) -> String {
    format!("{duration:#}")
}

fn time_text(time: Time) -> String {
    if time.second() == 0 && time.subsec_nanosecond() == 0 {
        time.strftime("%H:%M").to_string()
    } else {
        time.to_string()
    }
}

impl Draft {
    fn new() -> Self {
        Self {
            enabled: true,
            actions: vec![ActionDraft::default()],
            ..Self::default()
        }
    }

    fn from_rule(rule: &Rule) -> Self {
        let mut trigger = TriggerDraft::default();
        match &rule.when {
            Trigger::Event(event) => {
                trigger.kind = TriggerKind::Event;
                trigger.channel = ChannelKey::text(&event.device, event.entity, event.channel);
                trigger.event = event_key(event.event);
                trigger.data = event.data.clone().unwrap_or_default();
            }
            Trigger::Threshold(threshold) => {
                trigger.kind = TriggerKind::Threshold;
                trigger.channel =
                    ChannelKey::text(&threshold.device, threshold.entity, threshold.channel);
                let (direction, limit) = match threshold.limit {
                    Limit::Above(limit) => ("above", limit),
                    Limit::Below(limit) => ("below", limit),
                };
                trigger.direction = direction.into();
                trigger.limit = limit.to_string();
                trigger.lasting = threshold.lasting.map(duration_text).unwrap_or_default();
            }
            Trigger::Schedule(schedule) => {
                trigger.kind = TriggerKind::Schedule;
                trigger.at = time_text(schedule.at);
                trigger.days = schedule.days.iter().map(|day| day_key(*day)).collect();
            }
            Trigger::Cron(cron) => {
                trigger.kind = TriggerKind::Cron;
                trigger.cron = cron.expression.clone();
            }
            Trigger::Offline(offline) => {
                trigger.kind = TriggerKind::Offline;
                trigger.device = offline.device.clone().unwrap_or_else(|| "*".into());
            }
            Trigger::Traffic(TrafficTrigger { kind, device }) => {
                trigger.kind = TriggerKind::Traffic;
                trigger.alert = alert_texts(*kind).0.into();
                trigger.device = device.clone().unwrap_or_else(|| "*".into());
            }
            Trigger::RemoteKey(remote_key) => {
                trigger.kind = TriggerKind::RemoteKey;
                trigger.remote_key = pair(&remote_key.remote, &remote_key.key);
            }
            Trigger::Button(trigger_button) => {
                trigger.kind = TriggerKind::Button;
                trigger.button = trigger_button.button.clone();
            }
        }
        Self {
            previous: Some(rule.name.clone()),
            name: rule.name.clone(),
            enabled: rule.enabled,
            cooldown: rule.cooldown.map(duration_text).unwrap_or_default(),
            trigger,
            conditions: rule
                .conditions
                .iter()
                .map(ConditionDraft::from_condition)
                .collect(),
            actions: rule.then.iter().map(ActionDraft::from_action).collect(),
        }
    }

    /// `trigger.kind`, `condition.<index>.kind`, `action.<index>.value`... and the `command` of
    /// the button that sent the form.
    fn from_fields(fields: &HashMap<String, String>) -> Self {
        let field = |key: &str| fields.get(key).cloned().unwrap_or_default();
        let days = |prefix: &str| -> Vec<String> {
            DAYS.iter()
                .filter(|(day, _)| fields.contains_key(&format!("{prefix}.day.{day}")))
                .map(|(day, _)| day.to_string())
                .collect()
        };
        let indexes = |prefix: &str| {
            let mut indexes: Vec<usize> = fields
                .keys()
                .filter_map(|key| {
                    key.strip_prefix(prefix)?
                        .strip_suffix(".kind")?
                        .parse()
                        .ok()
                })
                .collect();
            indexes.sort_unstable();
            indexes
        };
        let mut draft = Self {
            previous: Some(field("previous")).filter(|previous| !previous.is_empty()),
            name: field("name"),
            enabled: fields.contains_key("enabled"),
            cooldown: field("cooldown"),
            trigger: TriggerDraft {
                kind: TriggerKind::from_key(&field("trigger.kind")),
                channel: field("trigger.channel"),
                event: field("trigger.event"),
                data: field("trigger.data"),
                direction: field("trigger.direction"),
                limit: field("trigger.limit"),
                lasting: field("trigger.lasting"),
                at: field("trigger.at"),
                days: days("trigger"),
                cron: field("trigger.cron"),
                device: field("trigger.device"),
                alert: field("trigger.alert"),
                remote_key: field("trigger.remote_key"),
                button: field("trigger.button"),
            },
            conditions: indexes("condition.")
                .into_iter()
                .map(|index| {
                    let prefix = format!("condition.{index}");
                    let item_field = |name: &str| field(&format!("{prefix}.{name}"));
                    ConditionDraft {
                        kind: ConditionKind::from_key(&item_field("kind")),
                        channel: item_field("channel"),
                        test: item_field("test"),
                        value: item_field("value"),
                        from: item_field("from"),
                        to: item_field("to"),
                        days: days(&prefix),
                        device: item_field("device"),
                    }
                })
                .collect(),
            actions: indexes("action.")
                .into_iter()
                .map(|index| {
                    let item_field = |name: &str| field(&format!("action.{index}.{name}"));
                    ActionDraft {
                        kind: ActionKind::from_key(&item_field("kind")),
                        channel: item_field("channel"),
                        value: item_field("value"),
                        title: item_field("title"),
                        message: item_field("message"),
                        wait: item_field("wait"),
                    }
                })
                .collect(),
        };
        let command = field("command");
        let removed = |prefix: &str| command.strip_prefix(prefix)?.parse::<usize>().ok();
        if command == "add-condition" {
            draft.conditions.push(ConditionDraft::default());
        } else if command == "add-action" {
            draft.actions.push(ActionDraft::default());
        } else if let Some(index) = removed("remove-condition.")
            && index < draft.conditions.len()
        {
            draft.conditions.remove(index);
        } else if let Some(index) = removed("remove-action.")
            && index < draft.actions.len()
        {
            draft.actions.remove(index);
        }
        draft
    }

    /// Picks what the form shows as chosen when nothing valid is: the browser shows the first
    /// option of a select without a placeholder, so the draft says the same.
    fn settle(&mut self, catalog: &Catalog) {
        if self.trigger.event.is_empty() {
            self.trigger.event = event_key(EVENTS[0]);
        }
        if self.trigger.alert.is_empty() {
            self.trigger.alert = alert_texts(ALERTS[0]).0.into();
        }
        if self.trigger.kind == TriggerKind::Traffic && self.trigger.device.is_empty() {
            self.trigger.device = "*".into();
        }
        if self.trigger.direction != "below" {
            self.trigger.direction = "above".into();
        }
        for condition in &mut self.conditions {
            let control = Control::for_condition(catalog.declared(&condition.channel));
            if !control.allows_limits() || !["above", "below"].contains(&condition.test.as_str()) {
                condition.test = "equals".into();
            }
            if condition.test == "equals" {
                control.settle(&mut condition.value);
            }
        }
        for action in &mut self.actions {
            match action.kind {
                ActionKind::Set => Control::OnOff.settle(&mut action.value),
                ActionKind::Invoke => {
                    Control::for_invoke(catalog.declared(&action.channel)).settle(&mut action.value)
                }
                ActionKind::Toggle | ActionKind::Notify | ActionKind::Wait => {}
            }
        }
    }
}

impl ConditionDraft {
    fn from_condition(condition: &Condition) -> Self {
        let mut draft = Self::default();
        match condition {
            Condition::Channel(channel) => {
                draft.kind = ConditionKind::Channel;
                draft.channel = ChannelKey::text(&channel.device, channel.entity, channel.channel);
                let (test, value) = match &channel.test {
                    ValueTest::Equals(Expected::Bool(expected)) => ("equals", expected.to_string()),
                    ValueTest::Equals(Expected::Number(expected)) => {
                        ("equals", expected.to_string())
                    }
                    ValueTest::Equals(Expected::Text(expected)) => ("equals", expected.clone()),
                    ValueTest::Above(limit) => ("above", limit.to_string()),
                    ValueTest::Below(limit) => ("below", limit.to_string()),
                };
                draft.test = test.into();
                draft.value = value;
            }
            Condition::Time { from, to } => {
                draft.kind = ConditionKind::Time;
                draft.from = time_text(*from);
                draft.to = time_text(*to);
            }
            Condition::Days(days) => {
                draft.kind = ConditionKind::Days;
                draft.days = days.iter().map(|day| day_key(*day)).collect();
            }
            Condition::Online(device) => {
                draft.kind = ConditionKind::Online;
                draft.device = device.clone();
            }
            Condition::Offline(device) => {
                draft.kind = ConditionKind::Offline;
                draft.device = device.clone();
            }
        }
        draft
    }
}

impl ActionDraft {
    fn from_action(action: &Action) -> Self {
        match action {
            Action::Set {
                device,
                entity,
                channel,
                value,
            } => Self {
                kind: ActionKind::Set,
                channel: ChannelKey::text(device, *entity, *channel),
                value: value.to_string(),
                ..Self::default()
            },
            Action::Toggle {
                device,
                entity,
                channel,
            } => Self {
                kind: ActionKind::Toggle,
                channel: ChannelKey::text(device, *entity, *channel),
                ..Self::default()
            },
            Action::Invoke {
                device,
                entity,
                channel,
                value,
            } => Self {
                kind: ActionKind::Invoke,
                channel: ChannelKey::text(device, *entity, *channel),
                value: value.clone().unwrap_or_default(),
                ..Self::default()
            },
            Action::Notify { title, message } => Self {
                kind: ActionKind::Notify,
                title: title.clone(),
                message: message.clone(),
                ..Self::default()
            },
            Action::Wait(time) => Self {
                kind: ActionKind::Wait,
                wait: time.to_string(),
                ..Self::default()
            },
        }
    }
}

/// What the hub knows that a rule can name.
struct Catalog {
    channels: Vec<ChannelChoice>,
    /// Device id and name, by name.
    nodes: Vec<(String, String)>,
    /// Remote and key.
    remote_keys: Vec<(String, String)>,
}

struct ChannelChoice {
    key: ChannelKey,
    node: String,
    /// `LED`, or `Climate sensor · Humidity` for a channel after the first.
    name: String,
    declared: Channel,
    /// The value it has now, with its unit.
    now: Option<String>,
}

/// Which channels fit where.
#[derive(Clone, Copy)]
enum Purpose {
    Event,
    Threshold,
    Condition,
    Switch,
    Invoke,
}

impl Purpose {
    fn fits(self, channel: &Channel) -> bool {
        let readable = matches!(channel.access, Access::Read | Access::ReadWrite);
        match self {
            Purpose::Event => channel.access == Access::Event,
            Purpose::Threshold => {
                readable
                    && matches!(
                        channel.value,
                        Some(ValueType::Int { .. } | ValueType::Float { .. })
                    )
            }
            Purpose::Condition => readable && channel.value.is_some(),
            Purpose::Switch => {
                channel.value == Some(ValueType::Bool)
                    && matches!(channel.access, Access::Write | Access::ReadWrite)
            }
            Purpose::Invoke => channel.access == Access::Trigger,
        }
    }
}

impl Catalog {
    /// Reads everything from one snapshot: no other `Hub` call while it lives.
    fn new(hub: &Hub) -> Self {
        let snapshot = hub.snapshot();
        let name = |target: NameTarget| {
            snapshot
                .nodes
                .name(&target)
                .or_else(|| snapshot.registry.declared_name(&target))
                .map(String::from)
        };
        let mut nodes: Vec<(String, String)> = snapshot
            .registry
            .nodes()
            .into_iter()
            .map(|(device_id, _)| {
                let node_name = name(NameTarget::Node(device_id.clone()));
                (
                    device_id.clone(),
                    node_name.unwrap_or_else(|| device_id.clone()),
                )
            })
            .collect();
        nodes.sort_by(|first, second| first.1.cmp(&second.1));
        let mut channels = Vec::new();
        for (device_id, node_name) in &nodes {
            let Some(node) = snapshot.registry.node(device_id) else {
                continue;
            };
            for entity in node
                .entities
                .iter()
                .filter(|entity| entity.id != management::ENTITY)
            {
                let entity_name = entity_label(
                    entity.id,
                    &name(NameTarget::Entity(device_id.clone(), entity.id)),
                );
                for channel in &entity.channels {
                    let channel_name = if channel.id == 0 {
                        entity_name.clone()
                    } else {
                        let named = name(NameTarget::Channel(
                            device_id.clone(),
                            entity.id,
                            channel.id,
                        ));
                        format!(
                            "{entity_name} · {}",
                            channel_label(channel.id, &named, channel.quantity)
                        )
                    };
                    channels.push(ChannelChoice {
                        key: ChannelKey {
                            device: device_id.clone(),
                            entity: entity.id,
                            channel: channel.id,
                        },
                        node: node_name.clone(),
                        name: channel_name,
                        declared: channel.clone(),
                        now: snapshot
                            .state
                            .value(device_id, entity.id, channel.id)
                            .map(|value| value_label(value, channel.unit)),
                    });
                }
            }
        }
        let remote_keys = snapshot
            .remotes
            .all()
            .iter()
            .flat_map(|(remote_name, remote)| {
                remote
                    .keys
                    .keys()
                    .map(move |key| (remote_name.clone(), key.clone()))
            })
            .collect();
        Self {
            channels,
            nodes,
            remote_keys,
        }
    }

    fn declared(&self, key: &str) -> Option<&Channel> {
        let key = ChannelKey::parse(key)?;
        self.channels
            .iter()
            .find(|choice| choice.key == key)
            .map(|choice| &choice.declared)
    }

    fn now(&self, key: &str) -> Option<String> {
        let key = ChannelKey::parse(key)?;
        self.channels
            .iter()
            .find(|choice| choice.key == key)?
            .now
            .clone()
    }

    fn channel_groups(&self, purpose: Purpose, chosen: &str) -> Vec<ChoiceGroup> {
        let mut groups: Vec<ChoiceGroup> = Vec::new();
        for choice in self
            .channels
            .iter()
            .filter(|choice| purpose.fits(&choice.declared))
        {
            let option = Choice {
                value: ChannelKey::text(&choice.key.device, choice.key.entity, choice.key.channel),
                text: choice.name.clone(),
            };
            match groups.last_mut() {
                Some(group) if group.label == choice.node => group.choices.push(option),
                _ => groups.push(ChoiceGroup {
                    label: choice.node.clone(),
                    choices: vec![option],
                }),
            }
        }
        keep_chosen(groups, chosen, || {
            ChannelKey::parse(chosen).map_or_else(
                || chosen.to_string(),
                |key| {
                    format!(
                        "{} {}/{} (not declared)",
                        key.device, key.entity, key.channel
                    )
                },
            )
        })
    }

    fn node_groups(&self, chosen: &str, any: bool) -> Vec<ChoiceGroup> {
        let mut choices: Vec<Choice> = Vec::new();
        if any {
            choices.push(Choice::new("*", "Any node"));
        }
        choices.extend(
            self.nodes
                .iter()
                .map(|(device_id, node_name)| Choice::new(device_id, node_name)),
        );
        keep_chosen(ungrouped(choices), chosen, || {
            format!("{chosen} (not seen)")
        })
    }
}

/// `(remote, key)` pairs grouped by remote.
fn pair_groups(pairs: &[(String, String)], chosen: &str) -> Vec<ChoiceGroup> {
    let mut groups: Vec<ChoiceGroup> = Vec::new();
    for (group_name, item) in pairs {
        let option = Choice {
            value: pair(group_name, item),
            text: item.clone(),
        };
        match groups.last_mut() {
            Some(group) if &group.label == group_name => group.choices.push(option),
            _ => groups.push(ChoiceGroup {
                label: group_name.clone(),
                choices: vec![option],
            }),
        }
    }
    keep_chosen(groups, chosen, || {
        parse_pair(chosen).map_or_else(
            || chosen.to_string(),
            |(group_name, item)| format!("{group_name} › {item} (missing)"),
        )
    })
}

#[derive(Clone)]
struct Choice {
    value: String,
    text: String,
}

impl Choice {
    fn new(value: &str, text: &str) -> Self {
        Self {
            value: value.into(),
            text: text.into(),
        }
    }
}

/// Shown under a heading, or loose when `label` is empty.
#[derive(Clone)]
struct ChoiceGroup {
    label: String,
    choices: Vec<Choice>,
}

fn ungrouped(choices: Vec<Choice>) -> Vec<ChoiceGroup> {
    vec![ChoiceGroup {
        label: String::new(),
        choices,
    }]
}

/// A rule may name something the hub does not know (any more); it stays chosen.
fn keep_chosen(
    mut groups: Vec<ChoiceGroup>,
    chosen: &str,
    text: impl FnOnce() -> String,
) -> Vec<ChoiceGroup> {
    let known = groups
        .iter()
        .any(|group| group.choices.iter().any(|choice| choice.value == chosen));
    if !chosen.is_empty() && !known {
        groups.insert(
            0,
            ChoiceGroup {
                label: String::new(),
                choices: vec![Choice {
                    value: chosen.into(),
                    text: text(),
                }],
            },
        );
    }
    groups
}

/// How a value is entered, from the channel's type.
#[derive(Clone)]
enum Control {
    OnOff,
    Options(Vec<String>),
    Number,
    Text,
    /// The channel takes no value.
    Nothing,
}

impl Control {
    fn for_condition(declared: Option<&Channel>) -> Self {
        match declared.and_then(|channel| channel.value.as_ref()) {
            Some(ValueType::Bool) => Control::OnOff,
            Some(ValueType::Enum(options)) => Control::Options(options.clone()),
            Some(ValueType::Int { .. } | ValueType::Float { .. }) => Control::Number,
            _ => Control::Text,
        }
    }

    fn for_invoke(declared: Option<&Channel>) -> Self {
        match declared.and_then(|channel| channel.value.as_ref()) {
            Some(ValueType::Enum(options)) => Control::Options(options.clone()),
            Some(ValueType::Text { .. }) => Control::Text,
            Some(_) => Control::Nothing,
            None if declared.is_some() => Control::Nothing,
            None => Control::Text,
        }
    }

    fn allows_limits(&self) -> bool {
        matches!(self, Control::Number | Control::Text)
    }

    fn settle(&self, value: &mut String) {
        match self {
            Control::OnOff if value != "false" => *value = "true".into(),
            Control::Options(options) if !options.contains(value) => {
                *value = options.first().cloned().unwrap_or_default();
            }
            Control::Nothing => value.clear(),
            _ => {}
        }
    }

    fn choices(&self) -> Vec<ChoiceGroup> {
        match self {
            Control::OnOff => {
                ungrouped(vec![Choice::new("true", "on"), Choice::new("false", "off")])
            }
            Control::Options(options) => ungrouped(
                options
                    .iter()
                    .map(|option| Choice::new(option, option))
                    .collect(),
            ),
            _ => Vec::new(),
        }
    }
}

fn required<'a>(text: &'a str, missing: &str) -> Built<&'a str> {
    let text = text.trim();
    if text.is_empty() {
        Err(missing.into())
    } else {
        Ok(text)
    }
}

fn number(text: &str, what: &str) -> Built<f64> {
    let text = required(text, &format!("write {what}"))?;
    text.replace(',', ".")
        .parse()
        .map_err(|_| format!("{what} must be a number, not \"{text}\""))
}

fn insert_channel(table: &mut InlineTable, key: &str) -> Built<()> {
    let key = ChannelKey::parse(key).ok_or("choose the channel")?;
    table.insert("device", key.device.as_str().into());
    table.insert("entity", i64::from(key.entity).into());
    table.insert("channel", i64::from(key.channel).into());
    Ok(())
}

fn day_array(days: &[String]) -> Array {
    DAYS.iter()
        .filter(|(day, _)| days.iter().any(|chosen| chosen == day))
        .map(|(day, _)| *day)
        .collect()
}

/// One item per line, like the rules written by hand.
fn list(items: Vec<InlineTable>) -> Array {
    let mut array: Array = items.into_iter().collect();
    if !array.is_empty() {
        for item in array.iter_mut() {
            item.decor_mut().set_prefix("\n    ");
        }
        array.set_trailing_comma(true);
        array.set_trailing("\n");
    }
    array
}

fn trigger_table(trigger: &TriggerDraft) -> Built<InlineTable> {
    let mut table = InlineTable::new();
    match trigger.kind {
        TriggerKind::Event => {
            insert_channel(&mut table, &trigger.channel)?;
            table.insert("event", trigger.event.as_str().into());
            let data = trigger.data.trim();
            if !data.is_empty() {
                table.insert("data", data.into());
            }
        }
        TriggerKind::Threshold => {
            insert_channel(&mut table, &trigger.channel)?;
            let limit = number(&trigger.limit, "the limit")?;
            table.insert(trigger.direction.as_str(), limit.into());
            let lasting = trigger.lasting.trim();
            if !lasting.is_empty() {
                table.insert("for", lasting.into());
            }
        }
        TriggerKind::Schedule => {
            table.insert("at", required(&trigger.at, "choose the time")?.into());
            if !trigger.days.is_empty() {
                table.insert("days", day_array(&trigger.days).into());
            }
        }
        TriggerKind::Cron => {
            let expression = required(&trigger.cron, "write the cron expression")?;
            table.insert("cron", expression.into());
        }
        TriggerKind::Offline => {
            table.insert(
                "offline",
                required(&trigger.device, "choose the node")?.into(),
            );
        }
        TriggerKind::Traffic => {
            table.insert(
                trigger.alert.as_str(),
                required(&trigger.device, "choose the node")?.into(),
            );
        }
        TriggerKind::RemoteKey => {
            let (remote, key) = parse_pair(&trigger.remote_key).ok_or("choose the remote key")?;
            table.insert("remote", remote.into());
            table.insert("key", key.into());
        }
        TriggerKind::Button => {
            table.insert(
                "button",
                required(&trigger.button, "write the button's name")?.into(),
            );
        }
    }
    Ok(table)
}

fn condition_table(condition: &ConditionDraft, catalog: &Catalog) -> Built<InlineTable> {
    let mut table = InlineTable::new();
    match condition.kind {
        ConditionKind::Channel => {
            insert_channel(&mut table, &condition.channel)?;
            if condition.test == "equals" {
                let control = Control::for_condition(catalog.declared(&condition.channel));
                table.insert("equals", expected(&control, &condition.value)?);
            } else {
                let limit = number(&condition.value, "the limit")?;
                table.insert(condition.test.as_str(), limit.into());
            }
        }
        ConditionKind::Time => {
            table.insert(
                "from",
                required(&condition.from, "choose the start time")?.into(),
            );
            table.insert("to", required(&condition.to, "choose the end time")?.into());
        }
        ConditionKind::Days => {
            if condition.days.is_empty() {
                return Err("choose at least one day".into());
            }
            table.insert("days", day_array(&condition.days).into());
        }
        ConditionKind::Online | ConditionKind::Offline => {
            let device = required(&condition.device, "choose the node")?;
            table.insert(condition.kind.key(), device.into());
        }
    }
    Ok(table)
}

/// A channel the hub does not know takes `true`/`false`, a number or text, as written.
fn expected(control: &Control, text: &str) -> Built<toml_edit::Value> {
    let text = text.trim();
    match control {
        Control::OnOff => Ok((text == "true").into()),
        Control::Number => number(text, "the value").map(Into::into),
        Control::Options(_) | Control::Nothing => Ok(required(text, "choose the value")?.into()),
        Control::Text => {
            let text = required(text, "write the value")?;
            Ok(match text {
                "true" => true.into(),
                "false" => false.into(),
                _ => match text.parse::<f64>() {
                    Ok(number) => number.into(),
                    Err(_) => text.into(),
                },
            })
        }
    }
}

fn action_table(action: &ActionDraft) -> Built<InlineTable> {
    let mut table = InlineTable::new();
    if action.kind == ActionKind::Wait {
        let wait = required(&action.wait, "write how long, e.g. 2s")?;
        table.insert("wait", wait.into());
        return Ok(table);
    }
    let mut inner = InlineTable::new();
    match action.kind {
        ActionKind::Set => {
            insert_channel(&mut inner, &action.channel)?;
            inner.insert("value", (action.value != "false").into());
        }
        ActionKind::Toggle => insert_channel(&mut inner, &action.channel)?,
        ActionKind::Invoke => {
            insert_channel(&mut inner, &action.channel)?;
            let argument = action.value.trim();
            if !argument.is_empty() {
                inner.insert("value", argument.into());
            }
        }
        ActionKind::Notify => {
            inner.insert("title", required(&action.title, "write the title")?.into());
            inner.insert("message", action.message.trim().into());
        }
        ActionKind::Wait => unreachable!("handled above"),
    }
    table.insert(action.kind.key(), inner.into());
    Ok(table)
}

/// Prefixed by its number, for errors: `Condition 2: choose the node`.
fn numbered<T>(word: &str, items: impl Iterator<Item = Built<T>>) -> Built<Vec<T>> {
    items
        .enumerate()
        .map(|(index, item)| item.map_err(|error| format!("{word} {}: {error}", index + 1)))
        .collect()
}

fn rule_table(draft: &Draft, catalog: &Catalog) -> Built<Table> {
    let mut table = Table::new();
    table.insert(
        "name",
        value(required(&draft.name, "write the rule's name")?),
    );
    if !draft.enabled {
        table.insert("enabled", value(false));
    }
    let when = trigger_table(&draft.trigger).map_err(|error| format!("When: {error}"))?;
    table.insert("when", value(when));
    let cooldown = draft.cooldown.trim();
    if !cooldown.is_empty() && draft.trigger.kind != TriggerKind::Button {
        table.insert("cooldown", value(cooldown));
    }
    let conditions = numbered(
        "Condition",
        draft
            .conditions
            .iter()
            .map(|condition| condition_table(condition, catalog)),
    )?;
    if !conditions.is_empty() {
        table.insert("if", value(list(conditions)));
    }
    let actions = numbered("Action", draft.actions.iter().map(action_table))?;
    table.insert("then", value(list(actions)));
    Ok(table)
}

fn rule_text(table: &Table) -> String {
    let mut rules = ArrayOfTables::new();
    rules.push(table.clone());
    let mut document = DocumentMut::new();
    document.insert("rules", Item::ArrayOfTables(rules));
    document.to_string()
}

/// Everything the form shows.
struct Editor {
    draft: Draft,
    catalog: Arc<Catalog>,
    /// The rule as it would go in `rules.toml`.
    text: Option<String>,
    /// Why the rule cannot be saved as it is.
    error: Option<String>,
    /// What the trigger fills in the texts of the actions, like `{node}`.
    placeholders: &'static [&'static str],
    schedule: Option<SchedulePreview>,
    /// By action, the card's warning on `/rules`, like an invoke value the node does not take.
    warnings: Vec<Option<String>>,
}

/// What a time trigger means, and when it runs next.
#[derive(Clone)]
struct SchedulePreview {
    description: String,
    runs: Vec<String>,
}

const PREVIEW_RUNS: usize = 5;

impl SchedulePreview {
    fn new(trigger: &Trigger, now: &Zoned) -> Option<Self> {
        let description = match trigger {
            Trigger::Cron(trigger) => cron_description(trigger),
            Trigger::Schedule(trigger) => schedule_description(trigger),
            _ => return None,
        };
        let mut runs = Vec::new();
        let mut after = now.clone();
        while runs.len() < PREVIEW_RUNS
            && let Some(run) = trigger.next_run(&after)
        {
            let within_a_week = now
                .date()
                .until(run.date())
                .is_ok_and(|span| span.get_days() < 7);
            runs.push(if within_a_week {
                next_run_description(now, &run)
            } else {
                run.strftime("%A %Y-%m-%d %H:%M").to_string()
            });
            after = run;
        }
        Some(Self { description, runs })
    }
}

impl Editor {
    /// Checks the rule the way saving does, against the whole configuration.
    fn new(hub: &Hub, catalog: Arc<Catalog>, mut draft: Draft, save_error: Option<String>) -> Self {
        draft.settle(&catalog);
        let (text, error, rule) = match rule_table(&draft, &catalog) {
            Ok(table) => {
                let text = rule_text(&table);
                let rule = toml::from_str::<Rules>(&text)
                    .ok()
                    .and_then(|rules| rules.all().first().cloned());
                let error = hub
                    .check_rule(draft.previous.as_deref(), table)
                    .err()
                    .map(|error| format!("{error:#}"));
                (Some(text), error, rule)
            }
            Err(error) => (None, Some(error), None),
        };
        let now = Zoned::now().with_time_zone(hub.time_zone().clone());
        let schedule = rule
            .as_ref()
            .and_then(|rule| SchedulePreview::new(&rule.when, &now));
        let warnings = rule.as_ref().map_or_else(Vec::new, |rule| {
            // Before the snapshot: the stand-ins ask the hub for names.
            let subject = Subject::by_hand(&rule.when, hub);
            let snapshot = hub.snapshot();
            rule.then
                .iter()
                .map(|action| action_warning(&snapshot.registry, action, &subject))
                .collect()
        });
        Self {
            draft,
            catalog,
            text,
            error: save_error.or(error),
            placeholders: rule.map_or(&[][..], |rule| rule.when.placeholders()),
            schedule,
            warnings,
        }
    }
}

#[derive(Deserialize)]
struct RuleQuery {
    name: Option<String>,
    copy: Option<String>,
}

fn find_rule(hub: &Hub, name: Option<String>) -> Option<Rule> {
    let name = name?;
    hub.rules().into_iter().find(|rule| rule.name == name)
}

#[page("/rules/new")]
pub async fn new_rule(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let copy = parse_query_params::<RuleQuery>(cx)
        .ok()
        .and_then(|query| find_rule(hub, query.copy));
    let draft = match copy {
        Some(rule) => Draft {
            previous: None,
            name: format!("{} (copy)", rule.name),
            ..Draft::from_rule(&rule)
        },
        None => Draft::new(),
    };
    let editor = Editor::new(hub, Arc::new(Catalog::new(hub)), draft, None);
    Ok(view! {
        layout(
            title: "Oxihome new rule",
            editor_page(editor: Some(editor), missing: String::new())
        )
    })
}

#[page("/rules/edit")]
pub async fn edit_rule(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let name = parse_query_params::<RuleQuery>(cx)
        .ok()
        .and_then(|query| query.name);
    let editor = find_rule(hub, name.clone()).map(|rule| {
        Editor::new(
            hub,
            Arc::new(Catalog::new(hub)),
            Draft::from_rule(&rule),
            None,
        )
    });
    Ok(view! {
        layout(
            title: "Oxihome edit rule",
            editor_page(editor: editor, missing: name.unwrap_or_default())
        )
    })
}

#[component]
async fn editor_page(editor: Option<Editor>, missing: String) -> Result<impl View> {
    Ok(view! {
        <div class="space-y-4">
            <div class="flex items-center justify-between gap-4">
                <p class="text-sm text-muted-foreground">"The rule is saved in rules.toml; the other rules and the comments stay as they are."</p>
                <a href="/rules" class="text-sm font-medium whitespace-nowrap hover:underline">"Back to rules"</a>
            </div>
            if let Some(editor) = editor {
                rule_form(editor: editor)
            } else {
                <p class="text-muted-foreground">(format!("No rule named \"{missing}\"."))</p>
            }
        </div>
    })
}

/// Answers with the form drawn again from what it sent.
#[route(POST "/rules/editor")]
pub async fn redraw(cx: &Cx, Form(fields): Form<HashMap<String, String>>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let catalog = Arc::new(Catalog::new(hub));
    let editor = Editor::new(hub, catalog, Draft::from_fields(&fields), None);
    view! { cx => rule_form(editor: editor) }.single().await
}

/// Sends the browser to the rule's card on success; answers with the form and the error otherwise.
#[route(POST "/rules/save")]
pub async fn save(cx: &Cx, Form(fields): Form<HashMap<String, String>>) -> Result<Response> {
    let hub: &Arc<Hub> = app_context(cx);
    let catalog = Arc::new(Catalog::new(hub));
    let mut draft = Draft::from_fields(&fields);
    draft.settle(&catalog);
    let result = rule_table(&draft, &catalog)
        .map_err(anyhow::Error::msg)
        .and_then(|table| hub.save_rule(draft.previous.as_deref(), table));
    match result {
        Ok(()) => {
            let name = draft.name.trim().to_string();
            let location = format!("/rules#{}", rule_anchor(&name));
            hub.notify("Rule saved".into(), name, "rules".into());
            ([("HX-Redirect", location)], ()).into_response(cx)
        }
        Err(error) => {
            let editor = Editor::new(hub, catalog, draft, Some(format!("{error:#}")));
            view! { cx => rule_form(editor: editor) }
                .single()
                .await?
                .into_response(cx)
        }
    }
}

#[component]
async fn rule_form(editor: Editor) -> Result<impl View> {
    let Editor {
        draft,
        catalog,
        text,
        error,
        placeholders,
        schedule,
        warnings,
    } = editor;
    let placeholders_hint = (!placeholders.is_empty()).then(|| {
        let names: Vec<String> = placeholders
            .iter()
            .map(|name| format!("{{{name}}}"))
            .collect();
        format!("Texts can use {}.", names.join(", "))
    });
    let takes_cooldown = draft.trigger.kind != TriggerKind::Button;
    let conditions: Vec<(usize, ConditionDraft)> =
        draft.conditions.iter().cloned().enumerate().collect();
    let actions: Vec<(usize, ActionDraft, Option<String>)> = draft
        .actions
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, action)| (index, action, warnings.get(index).cloned().flatten()))
        .collect();
    let invalid = error.is_some();
    Ok(view! {
        <form
            id="rule-editor"
            class="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,28rem)]"
            hx-post="/rules/editor"
            hx-trigger="change, submit"
            hx-target="this"
            hx-swap="outerHTML"
        >
            <input type="hidden" name="previous" value=(draft.previous.clone().unwrap_or_default())>
            <div class="min-w-0 space-y-4">
                card(
                    card_header(card_title("Rule"))
                    card_content(
                        <div class="flex items-end gap-4">
                            form_field(title: "Name", name: "name".to_string(),
                                input(attrs: attributes! { id="name" name="name" value=(draft.name.clone()) autocomplete="off" })
                            )
                            <div class="flex h-9 items-center gap-2">
                                switch(attrs: attributes! {
                                    id="enabled"
                                    name="enabled"
                                    value="true"
                                    if draft.enabled {
                                        checked=""
                                    }
                                })
                                label(attrs: attributes! { for="enabled" }, "Enabled")
                            </div>
                        </div>
                        if takes_cooldown {
                            <div class="mt-4 space-y-2">
                                form_field(title: "Cooldown (optional)", name: "cooldown".to_string(),
                                    text_input(name: "cooldown".to_string(), text: draft.cooldown.clone(), placeholder: Some("e.g. 1h"))
                                )
                                <p class="text-sm text-muted-foreground">"After the rule starts, triggers about the same node and channel are ignored this long. At most 7 days."</p>
                            </div>
                        }
                    )
                )
                card(
                    card_header(
                        card_title("When")
                        card_description("What runs the rule.")
                    )
                    card_content(
                        trigger_fields(trigger: draft.trigger.clone(), schedule: schedule, catalog: catalog.clone())
                    )
                )
                card(
                    card_header(
                        <div class="flex items-center justify-between gap-2">
                            card_title("If")
                            add_button(command: "add-condition", text: "Add condition")
                        </div>
                        card_description("Checked when the rule runs; every condition must hold. Optional.")
                    )
                    card_content(
                        if conditions.is_empty() {
                            <p class="text-sm text-muted-foreground">"No conditions: the actions always run."</p>
                        }
                        <div class="space-y-3">
                            for (index, condition) in conditions {
                                condition_fields(index: index, condition: condition, catalog: catalog.clone())
                            }
                        </div>
                    )
                )
                card(
                    card_header(
                        <div class="flex items-center justify-between gap-2">
                            card_title("Then")
                            add_button(command: "add-action", text: "Add action")
                        </div>
                        card_description("Run in order; a failed action does not stop the next ones.")
                        if let Some(hint) = placeholders_hint {
                            card_description((hint))
                        }
                    )
                    card_content(
                        if actions.is_empty() {
                            <p class="text-sm text-muted-foreground">"No actions yet."</p>
                        }
                        <div class="space-y-3">
                            for (index, action, warning) in actions {
                                action_fields(index: index, action: action, warning: warning, catalog: catalog.clone())
                            }
                        </div>
                    )
                )
            </div>
            <div class="min-w-0 space-y-4 lg:sticky lg:top-4">
                card(
                    card_header(
                        card_title("Preview")
                        card_description("As it goes in rules.toml.")
                    )
                    card_content(
                        if let Some(text) = text {
                            <pre class="overflow-x-auto rounded-lg bg-muted p-3 font-mono text-xs">(text)</pre>
                        }
                        if let Some(error) = error {
                            <pre class="mt-3 text-sm whitespace-pre-wrap text-destructive">(error)</pre>
                        }
                    )
                )
                <div class="flex justify-end gap-2">
                    <a
                        href="/rules"
                        class="inline-flex h-8 items-center rounded-lg border border-border px-3 text-sm font-medium hover:bg-foreground/5"
                    >"Cancel"</a>
                    button(
                        size: ButtonSize::Sm,
                        attrs: attributes! {
                            type="button"
                            hx-post="/rules/save"
                            hx-target="#rule-editor"
                            hx-swap="outerHTML"
                            if invalid {
                                disabled=""
                            }
                        },
                        "Save"
                    )
                </div>
            </div>
        </form>
    })
}

#[component]
async fn form_field(
    title: &'static str,
    name: String,
    #[default] child: topcoat::view::Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <div class="flex min-w-0 flex-1 flex-col gap-2">
            label(attrs: attributes! { for=(name) }, (title))
            (child)
        </div>
    })
}

#[component]
async fn add_button(command: &'static str, text: &'static str) -> Result<impl View> {
    Ok(view! {
        button(
            variant: ButtonVariant::Outline,
            size: ButtonSize::Sm,
            attrs: attributes! {
                type="button"
                name="command"
                value=(command)
                hx-post="/rules/editor"
                hx-target="#rule-editor"
                hx-swap="outerHTML"
            },
            (text)
        )
    })
}

#[component]
async fn remove_button(command: String) -> Result<impl View> {
    Ok(view! {
        button(
            variant: ButtonVariant::Ghost,
            size: ButtonSize::Sm,
            attrs: attributes! {
                type="button"
                name="command"
                value=(command)
                hx-post="/rules/editor"
                hx-target="#rule-editor"
                hx-swap="outerHTML"
            },
            "Remove"
        )
    })
}

#[component]
async fn choice_select(
    name: String,
    chosen: String,
    groups: Vec<ChoiceGroup>,
    #[default] placeholder: Option<&'static str>,
) -> Result<impl View> {
    Ok(view! {
        select(
            attrs: attributes! { id=(name.clone()) name=(name) },
            if let Some(placeholder) = placeholder {
                <option value="" if chosen.is_empty() { selected="" }>(placeholder)</option>
            }
            for group in groups {
                if group.label.is_empty() {
                    for choice in group.choices {
                        <option value=(choice.value.clone()) if choice.value == chosen { selected="" }>(choice.text)</option>
                    }
                } else {
                    <optgroup label=(group.label.clone())>
                        <legend>(group.label)</legend>
                        for choice in group.choices {
                            <option value=(choice.value.clone()) if choice.value == chosen { selected="" }>(choice.text)</option>
                        }
                    </optgroup>
                }
            }
        )
    })
}

#[component]
async fn text_input(
    name: String,
    text: String,
    #[default] kind: Option<&'static str>,
    #[default] placeholder: Option<&'static str>,
) -> Result<impl View> {
    Ok(view! {
        input(attrs: attributes! {
            id=(name.clone())
            name=(name)
            value=(text)
            type=(kind.unwrap_or("text"))
            autocomplete="off"
            placeholder=(placeholder.unwrap_or(""))
        })
    })
}

#[component]
async fn day_checkboxes(prefix: String, days: Vec<String>) -> Result<impl View> {
    let boxes: Vec<(String, &'static str, bool)> = DAYS
        .iter()
        .map(|(day, short)| {
            (
                format!("{prefix}.day.{day}"),
                *short,
                days.iter().any(|chosen| chosen == day),
            )
        })
        .collect();
    Ok(view! {
        <div class="flex flex-wrap gap-x-4 gap-y-2">
            for (name, short, checked) in boxes {
                <div class="flex items-center gap-2">
                    checkbox(attrs: attributes! {
                        id=(name.clone())
                        name=(name.clone())
                        value="true"
                        if checked {
                            checked=""
                        }
                    })
                    label(attrs: attributes! { for=(name) }, (short))
                </div>
            }
        </div>
    })
}

/// The select for a value of the channel's type, or a text box.
#[component]
async fn value_control(name: String, text: String, control: Control) -> Result<impl View> {
    let choices = control.choices();
    Ok(view! {
        match control {
            Control::OnOff | Control::Options(_) => {
                choice_select(name: name, chosen: text, groups: choices)
            },
            Control::Number => {
                text_input(name: name, text: text, kind: Some("number"), placeholder: Some("e.g. 28"))
            },
            Control::Text => {
                text_input(name: name, text: text)
            },
            Control::Nothing => {
                <p class="flex h-9 items-center text-sm text-muted-foreground">"Takes no value."</p>
            },
        }
    })
}

#[component]
async fn trigger_fields(
    trigger: TriggerDraft,
    schedule: Option<SchedulePreview>,
    catalog: Arc<Catalog>,
) -> Result<impl View> {
    let now = catalog.now(&trigger.channel);
    let purpose = match trigger.kind {
        TriggerKind::Threshold => Purpose::Threshold,
        _ => Purpose::Event,
    };
    let channels = catalog.channel_groups(purpose, &trigger.channel);
    let events = ungrouped(
        EVENTS
            .iter()
            .map(|event| Choice::new(&event_key(*event), event_label(*event)))
            .collect(),
    );
    let directions = ungrouped(vec![
        Choice::new("above", "rises above"),
        Choice::new("below", "falls below"),
    ]);
    let nodes = catalog.node_groups(&trigger.device, true);
    let alerts = ungrouped(
        ALERTS
            .iter()
            .map(|kind| {
                let (key, text) = alert_texts(*kind);
                Choice::new(key, text)
            })
            .collect(),
    );
    let remote_keys = pair_groups(&catalog.remote_keys, &trigger.remote_key);
    Ok(view! {
        <div class="space-y-4">
            form_field(title: "Trigger", name: "trigger.kind".to_string(),
                choice_select(name: "trigger.kind".to_string(), chosen: trigger.kind.key().to_string(), groups: TriggerKind::choices())
            )
            match trigger.kind {
                TriggerKind::Event => {
                    form_field(title: "Channel", name: "trigger.channel".to_string(),
                        choice_select(name: "trigger.channel".to_string(), chosen: trigger.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                    <div class="flex gap-4">
                        form_field(title: "Event", name: "trigger.event".to_string(),
                            choice_select(name: "trigger.event".to_string(), chosen: trigger.event, groups: events)
                        )
                        form_field(title: "Data (optional)", name: "trigger.data".to_string(),
                            text_input(name: "trigger.data".to_string(), text: trigger.data, placeholder: Some("exact text, e.g. NEC 00 45"))
                        )
                    </div>
                },
                TriggerKind::Threshold => {
                    form_field(title: "Channel", name: "trigger.channel".to_string(),
                        choice_select(name: "trigger.channel".to_string(), chosen: trigger.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                    <div class="flex gap-4">
                        form_field(title: "When the value", name: "trigger.direction".to_string(),
                            choice_select(name: "trigger.direction".to_string(), chosen: trigger.direction, groups: directions)
                        )
                        form_field(title: "Limit", name: "trigger.limit".to_string(),
                            text_input(name: "trigger.limit".to_string(), text: trigger.limit, kind: Some("number"), placeholder: Some("e.g. 28"))
                        )
                    </div>
                    if let Some(now) = now {
                        <p class="text-sm text-muted-foreground">(format!("Now: {now}"))</p>
                    }
                    form_field(title: "For (optional)", name: "trigger.lasting".to_string(),
                        text_input(name: "trigger.lasting".to_string(), text: trigger.lasting, placeholder: Some("e.g. 5m"))
                    )
                    <p class="text-sm text-muted-foreground">"Runs once when the value crosses the limit, not again while it stays past it. With For, only if it stays past the limit that long (at most 24 hours)."</p>
                },
                TriggerKind::Schedule => {
                    form_field(title: "Time", name: "trigger.at".to_string(),
                        text_input(name: "trigger.at".to_string(), text: trigger.at, kind: Some("time"))
                    )
                    <div class="space-y-2">
                        <p class="text-sm font-medium">"Days"</p>
                        day_checkboxes(prefix: "trigger".to_string(), days: trigger.days)
                        <p class="text-sm text-muted-foreground">"None checked means every day."</p>
                    </div>
                    schedule_preview(schedule: schedule)
                },
                TriggerKind::Cron => {
                    form_field(title: "Expression", name: "trigger.cron".to_string(),
                        text_input(name: "trigger.cron".to_string(), text: trigger.cron, placeholder: Some("*/5 * * * *"))
                    )
                    <p class="text-sm text-muted-foreground">"Minute, hour, day of month, month and day of week, in local time."</p>
                    schedule_preview(schedule: schedule)
                },
                TriggerKind::Offline => {
                    form_field(title: "Node", name: "trigger.device".to_string(),
                        choice_select(name: "trigger.device".to_string(), chosen: trigger.device, groups: nodes, placeholder: Some("Choose a node…"))
                    )
                },
                TriggerKind::Traffic => {
                    <div class="flex gap-4">
                        form_field(title: "Alert", name: "trigger.alert".to_string(),
                            choice_select(name: "trigger.alert".to_string(), chosen: trigger.alert, groups: alerts)
                        )
                        form_field(title: "Node", name: "trigger.device".to_string(),
                            choice_select(name: "trigger.device".to_string(), chosen: trigger.device, groups: nodes)
                        )
                    </div>
                    <p class="text-sm text-muted-foreground">"Alerts come from the Traffic page."</p>
                },
                TriggerKind::RemoteKey => {
                    form_field(title: "Key", name: "trigger.remote_key".to_string(),
                        choice_select(name: "trigger.remote_key".to_string(), chosen: trigger.remote_key, groups: remote_keys, placeholder: Some("Choose a key…"))
                    )
                    <p class="text-sm text-muted-foreground">"Keys come from the Remotes page."</p>
                },
                TriggerKind::Button => {
                    form_field(title: "Button", name: "trigger.button".to_string(),
                        text_input(name: "trigger.button".to_string(), text: trigger.button, placeholder: Some("e.g. Good night"))
                    )
                    <p class="text-sm text-muted-foreground">"The button shows on the Rules page; dashboard.toml can show it too. Its name is unique among rules."</p>
                },
            }
        </div>
    })
}

/// Nothing while the trigger does not parse; the error shows in the preview.
#[component]
async fn schedule_preview(schedule: Option<SchedulePreview>) -> Result<impl View> {
    Ok(view! {
        if let Some(schedule) = schedule {
            <div class="space-y-1 rounded-lg bg-muted p-3 text-sm">
                <p>(schedule.description)</p>
                if schedule.runs.is_empty() {
                    <p class="text-muted-foreground">"Never runs."</p>
                } else {
                    <p class="text-muted-foreground">"Next runs:"</p>
                    <ul class="list-inside list-disc text-muted-foreground">
                        for run in schedule.runs {
                            <li>(run)</li>
                        }
                    </ul>
                }
            </div>
        }
    })
}

#[component]
async fn condition_fields(
    index: usize,
    condition: ConditionDraft,
    catalog: Arc<Catalog>,
) -> Result<impl View> {
    let prefix = format!("condition.{index}");
    let name = {
        let prefix = prefix.clone();
        move |part: &str| format!("{prefix}.{part}")
    };
    let control = Control::for_condition(catalog.declared(&condition.channel));
    let now = catalog.now(&condition.channel);
    let mut tests = vec![Choice::new("equals", "is")];
    if control.allows_limits() {
        tests.push(Choice::new("above", "is above"));
        tests.push(Choice::new("below", "is below"));
    }
    let shown_control = if condition.test == "equals" {
        control
    } else {
        Control::Number
    };
    let channels = catalog.channel_groups(Purpose::Condition, &condition.channel);
    let nodes = catalog.node_groups(&condition.device, false);
    Ok(view! {
        <div class="space-y-3 rounded-lg border border-border p-3">
            <div class="flex items-end gap-2">
                form_field(title: "Condition", name: name("kind"),
                    choice_select(name: name("kind"), chosen: condition.kind.key().to_string(), groups: ConditionKind::choices())
                )
                remove_button(command: format!("remove-{prefix}"))
            </div>
            match condition.kind {
                ConditionKind::Channel => {
                    form_field(title: "Channel", name: name("channel"),
                        choice_select(name: name("channel"), chosen: condition.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                    <div class="flex gap-4">
                        form_field(title: "Test", name: name("test"),
                            choice_select(name: name("test"), chosen: condition.test, groups: ungrouped(tests))
                        )
                        form_field(title: "Value", name: name("value"),
                            value_control(name: name("value"), text: condition.value, control: shown_control)
                        )
                    </div>
                    if let Some(now) = now {
                        <p class="text-sm text-muted-foreground">(format!("Now: {now}"))</p>
                    }
                    <p class="text-sm text-muted-foreground">"Does not hold while the node is offline or its value is unknown."</p>
                },
                ConditionKind::Time => {
                    <div class="flex gap-4">
                        form_field(title: "From", name: name("from"),
                            text_input(name: name("from"), text: condition.from, kind: Some("time"))
                        )
                        form_field(title: "To", name: name("to"),
                            text_input(name: name("to"), text: condition.to, kind: Some("time"))
                        )
                    </div>
                    <p class="text-sm text-muted-foreground">"From included, to excluded; may cross midnight."</p>
                },
                ConditionKind::Days => {
                    day_checkboxes(prefix: prefix.clone(), days: condition.days)
                },
                ConditionKind::Online | ConditionKind::Offline => {
                    form_field(title: "Node", name: name("device"),
                        choice_select(name: name("device"), chosen: condition.device, groups: nodes, placeholder: Some("Choose a node…"))
                    )
                },
            }
        </div>
    })
}

#[component]
async fn action_fields(
    index: usize,
    action: ActionDraft,
    warning: Option<String>,
    catalog: Arc<Catalog>,
) -> Result<impl View> {
    let prefix = format!("action.{index}");
    let name = {
        let prefix = prefix.clone();
        move |part: &str| format!("{prefix}.{part}")
    };
    let purpose = match action.kind {
        ActionKind::Invoke => Purpose::Invoke,
        _ => Purpose::Switch,
    };
    let channels = catalog.channel_groups(purpose, &action.channel);
    let invoke_control = Control::for_invoke(catalog.declared(&action.channel));
    Ok(view! {
        <div class="space-y-3 rounded-lg border border-border p-3">
            <div class="flex items-end gap-2">
                form_field(title: "Action", name: name("kind"),
                    choice_select(name: name("kind"), chosen: action.kind.key().to_string(), groups: ActionKind::choices())
                )
                remove_button(command: format!("remove-{prefix}"))
            </div>
            match action.kind {
                ActionKind::Set => {
                    <div class="flex gap-4">
                        form_field(title: "Channel", name: name("channel"),
                            choice_select(name: name("channel"), chosen: action.channel, groups: channels, placeholder: Some("Choose a channel…"))
                        )
                        <div class="w-28">
                            form_field(title: "To", name: name("value"),
                                value_control(name: name("value"), text: action.value, control: Control::OnOff)
                            )
                        </div>
                    </div>
                },
                ActionKind::Toggle => {
                    form_field(title: "Channel", name: name("channel"),
                        choice_select(name: name("channel"), chosen: action.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                },
                ActionKind::Invoke => {
                    <div class="flex gap-4">
                        form_field(title: "Channel", name: name("channel"),
                            choice_select(name: name("channel"), chosen: action.channel, groups: channels, placeholder: Some("Choose a channel…"))
                        )
                        form_field(title: "Value", name: name("value"),
                            value_control(name: name("value"), text: action.value, control: invoke_control)
                        )
                    </div>
                },
                ActionKind::Notify => {
                    form_field(title: "Title", name: name("title"),
                        text_input(name: name("title"), text: action.title)
                    )
                    form_field(title: "Message", name: name("message"),
                        text_input(name: name("message"), text: action.message)
                    )
                },
                ActionKind::Wait => {
                    form_field(title: "How long", name: name("wait"),
                        text_input(name: name("wait"), text: action.wait, placeholder: Some("e.g. 2s"))
                    )
                    <p class="text-sm text-muted-foreground">"Pauses before the next action; at most 30 minutes."</p>
                },
            }
            if let Some(warning) = warning {
                <p class="text-sm text-destructive">(warning)</p>
            }
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_KIND: &str = r#"
        [[rules]]
        name = "Schedule with conditions"
        enabled = false
        when = { at = "07:30", days = ["monday", "friday"] }
        cooldown = "1h"
        if = [
            { device = "board", entity = 3, channel = 0, above = 28.5 },
            { device = "board", entity = 1, channel = 0, equals = true },
            { device = "board", entity = 2, channel = 0, equals = "auto" },
            { from = "22:00", to = "06:00" },
            { days = ["saturday", "sunday"] },
            { online = "board" },
            { offline = "phone" },
        ]
        then = [
            { set = { device = "board", entity = 1, channel = 0, value = true } },
            { toggle = { device = "board", entity = 1, channel = 0 } },
            { invoke = { device = "board", entity = 4, channel = 0, value = "beep" } },
            { invoke = { device = "board", entity = 4, channel = 1 } },
            { wait = "1m 30s" },
            { notify = { title = "Hi", message = "" } },
        ]

        [[rules]]
        name = "Cron"
        when = { cron = "*/5 * * * *" }
        then = []

        [[rules]]
        name = "Event"
        when = { device = "board", entity = 2, channel = 0, event = "Received", data = "NEC 00 45" }
        then = [{ notify = { title = "{node}", message = "{channel}: {value}" } }]

        [[rules]]
        name = "Threshold"
        when = { device = "board", entity = 3, channel = 0, below = 10, for = "5m" }
        then = []

        [[rules]]
        name = "Any offline"
        when = { offline = "*" }
        then = []

        [[rules]]
        name = "Board offline"
        when = { offline = "board" }
        then = []

        [[rules]]
        name = "Stopped"
        when = { stopped = "*" }
        then = []

        [[rules]]
        name = "Unanswered"
        when = { unanswered = "board" }
        then = []

        [[rules]]
        name = "Remote key"
        when = { remote = "TV", key = "Power" }
        then = []

        [[rules]]
        name = "Button"
        when = { button = "Good night" }
        then = []
    "#;

    /// A field the editor does not know fails here instead of being dropped when the rule is
    /// saved from the form.
    #[test]
    fn rules_come_back_the_same_from_the_form() {
        let catalog = Catalog {
            channels: Vec::new(),
            nodes: Vec::new(),
            remote_keys: Vec::new(),
        };
        for text in [EVERY_KIND, include_str!("../../config/rules.toml")] {
            let rules: Rules = toml::from_str(text).unwrap();
            for rule in rules.all() {
                let mut draft = Draft::from_rule(rule);
                draft.settle(&catalog);
                let table = rule_table(&draft, &catalog).unwrap();
                let saved: Rules = toml::from_str(&rule_text(&table)).unwrap();
                let saved = &saved.all()[0];
                assert_eq!(saved.name, rule.name);
                assert_eq!(saved.enabled, rule.enabled, "{}", rule.name);
                assert_eq!(saved.when, rule.when, "{}", rule.name);
                assert_eq!(saved.cooldown, rule.cooldown, "{}", rule.name);
                assert_eq!(saved.conditions, rule.conditions, "{}", rule.name);
                assert_eq!(saved.then, rule.then, "{}", rule.name);
            }
        }
    }

    #[test]
    fn time_triggers_preview_their_next_runs() {
        let rules: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "Weekly"
            when = { cron = "0 7 * * 1" }
            then = []

            [[rules]]
            name = "Daily"
            when = { at = "07:30" }
            then = []
            "#,
        )
        .unwrap();
        let now: Zoned = "2026-10-19T08:00:00-03:00[-03:00]".parse().unwrap();
        let weekly = SchedulePreview::new(&rules.all()[0].when, &now).unwrap();
        assert!(weekly.description.contains("0 7 * * 1"));
        assert_eq!(weekly.runs.len(), 5);
        assert_eq!(weekly.runs[0], "Monday 2026-10-26 07:00");
        let daily = SchedulePreview::new(&rules.all()[1].when, &now).unwrap();
        assert_eq!(daily.runs[..2], ["tomorrow 07:30", "Wednesday 07:30"]);
    }

    #[tokio::test]
    async fn channels_show_the_names_the_node_declares() {
        let hub = Hub::for_tests("rule-editor-names", "").await;
        let event = |id: u8| Channel {
            id,
            name: None,
            access: Access::Event,
            value: None,
            quantity: None,
            unit: None,
        };
        let hello = oxihome_protocol::Message::new(
            "phone",
            0,
            oxihome_protocol::Payload::Hello {
                build: oxihome_protocol::Build::new("node", "0.1.0", "linux"),
                ip: None,
                entities: vec![oxihome_protocol::EntityDescriptor {
                    id: 1,
                    name: Some("Phone".into()),
                    channels: vec![event(0), event(1).named("Presence")],
                }],
            },
        );
        crate::dispatch::on_message(&hub, &hello);
        let names: Vec<String> = Catalog::new(&hub)
            .channels
            .into_iter()
            .map(|choice| choice.name)
            .collect();
        assert_eq!(names, ["Phone", "Phone · Presence"]);
    }

    #[tokio::test]
    async fn the_editor_does_not_lock_itself_and_checks_names() {
        let hub = Hub::for_tests(
            "rule-editor",
            r#"
            [[rules]]
            name = "Hot"
            when = { device = "board", entity = 3, channel = 0, above = 28 }
            then = [{ notify = { title = "{node}", message = "{channel} at {value}" } }]
            "#,
        )
        .await;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rule = hub.rules().remove(0);
            let edited = Editor::new(
                &hub,
                Arc::new(Catalog::new(&hub)),
                Draft::from_rule(&rule),
                None,
            );
            let copy = Draft {
                previous: None,
                ..Draft::from_rule(&rule)
            };
            let copied = Editor::new(&hub, Arc::new(Catalog::new(&hub)), copy, None);
            sender
                .send((edited.error, edited.placeholders, copied.error))
                .unwrap();
        });
        let (edited, placeholders, copied) = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(edited, None);
        assert_eq!(placeholders, ["node", "channel", "value"]);
        assert!(copied.unwrap().contains("already a rule named \"Hot\""));
    }
}
