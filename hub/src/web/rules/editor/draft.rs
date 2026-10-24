//! What the form holds, read from a rule or from the posted fields.

use std::collections::HashMap;

use jiff::SignedDuration;
use jiff::civil::Time;
use oxihome_protocol::EventKind;

use super::catalog::{Catalog, Choice, ChoiceGroup, Control, ungrouped};
use crate::config::rules::{
    Action, Condition, Expected, Limit, Rule, TrafficTrigger, Trigger, ValueTest,
};
use crate::traffic::AlertKind;

pub(super) const DAYS: [(&str, &str); 7] = [
    ("monday", "Mon"),
    ("tuesday", "Tue"),
    ("wednesday", "Wed"),
    ("thursday", "Thu"),
    ("friday", "Fri"),
    ("saturday", "Sat"),
    ("sunday", "Sun"),
];

/// A new `EventKind` fails to compile in `event_label`; add it here too.
pub(super) const EVENTS: [EventKind; 4] = [
    EventKind::Press,
    EventKind::LongPress,
    EventKind::DoublePress,
    EventKind::Received,
];

pub(super) fn event_label(event: EventKind) -> &'static str {
    match event {
        EventKind::Press => "Press",
        EventKind::LongPress => "Long press",
        EventKind::DoublePress => "Double press",
        EventKind::Received => "Received",
    }
}

pub(super) fn event_key(event: EventKind) -> String {
    format!("{event:?}")
}

pub(super) const ALERTS: [AlertKind; 4] = [
    AlertKind::Stopped,
    AlertKind::Noisy,
    AlertKind::Unanswered,
    AlertKind::Reported,
];

/// The `when` key, and what the select shows.
pub(super) fn alert_texts(kind: AlertKind) -> (&'static str, &'static str) {
    match kind {
        AlertKind::Stopped => ("stopped", "A channel stopped reporting"),
        AlertKind::Noisy => ("noisy", "A channel is noisy"),
        AlertKind::Unanswered => ("unanswered", "Commands went unanswered"),
        AlertKind::Reported => ("reported", "The node reported a problem"),
    }
}

pub(super) trait Kind: Copy + PartialEq + Default + 'static {
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
pub(super) enum TriggerKind {
    #[default]
    Event,
    Threshold,
    Schedule,
    Cron,
    Offline,
    Traffic,
    RemoteKey,
    Button,
    Notice,
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
        Self::Notice,
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
            Self::Notice => "notice",
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
            Self::Notice => "A notice from a node",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
pub(super) enum ConditionKind {
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
pub(super) enum ActionKind {
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
pub(super) struct Draft {
    /// The name of the rule being edited; `None` for a new one.
    pub(super) previous: Option<String>,
    pub(super) name: String,
    pub(super) enabled: bool,
    /// A duration such as `1h`; empty for none.
    pub(super) cooldown: String,
    pub(super) trigger: TriggerDraft,
    pub(super) conditions: Vec<ConditionDraft>,
    pub(super) actions: Vec<ActionDraft>,
}

#[derive(Clone, Default)]
pub(super) struct TriggerDraft {
    pub(super) kind: TriggerKind,
    /// A `ChannelKey` as text.
    pub(super) channel: String,
    pub(super) event: String,
    pub(super) data: String,
    /// `above` or `below`.
    pub(super) direction: String,
    pub(super) limit: String,
    /// The `for` of a threshold; empty for none.
    pub(super) lasting: String,
    pub(super) at: String,
    pub(super) days: Vec<String>,
    pub(super) cron: String,
    /// A device id, or `*` for any node.
    pub(super) device: String,
    /// The `when` key of the traffic alert.
    pub(super) alert: String,
    /// Remote and key as a pair.
    pub(super) remote_key: String,
    pub(super) button: String,
}

#[derive(Clone, Default)]
pub(super) struct ConditionDraft {
    pub(super) kind: ConditionKind,
    pub(super) channel: String,
    /// `equals`, `above` or `below`.
    pub(super) test: String,
    pub(super) value: String,
    pub(super) from: String,
    pub(super) to: String,
    pub(super) days: Vec<String>,
    pub(super) device: String,
}

#[derive(Clone, Default)]
pub(super) struct ActionDraft {
    pub(super) kind: ActionKind,
    pub(super) channel: String,
    pub(super) value: String,
    pub(super) title: String,
    pub(super) message: String,
    pub(super) wait: String,
}

#[derive(Clone, PartialEq)]
pub(super) struct ChannelKey {
    pub(super) device: String,
    pub(super) entity: u8,
    pub(super) channel: u8,
}

impl ChannelKey {
    pub(super) fn text(device: &str, entity: u8, channel: u8) -> String {
        serde_json::json!([device, entity, channel]).to_string()
    }

    pub(super) fn parse(text: &str) -> Option<Self> {
        let (device, entity, channel) = serde_json::from_str(text).ok()?;
        Some(Self {
            device,
            entity,
            channel,
        })
    }
}

pub(super) fn pair(first: &str, second: &str) -> String {
    serde_json::json!([first, second]).to_string()
}

pub(super) fn parse_pair(text: &str) -> Option<(String, String)> {
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
    pub(super) fn new() -> Self {
        Self {
            enabled: true,
            actions: vec![ActionDraft::default()],
            ..Self::default()
        }
    }

    pub(super) fn from_rule(rule: &Rule) -> Self {
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
            Trigger::Notice(notice) => {
                trigger.kind = TriggerKind::Notice;
                trigger.device = notice.device.clone().unwrap_or_else(|| "*".into());
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
    pub(super) fn from_fields(fields: &HashMap<String, String>) -> Self {
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
    pub(super) fn settle(&mut self, catalog: &Catalog) {
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
