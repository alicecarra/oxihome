use std::fmt;
use std::sync::Arc;

use anyhow::bail;
use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use jiff::civil::{DateTime, Time, Weekday};
use jiff::{Timestamp, ToSpan, Zoned};
use oxihome_protocol::{Confidence, EventKind, Value};
use serde::Deserialize;
use toml_edit::{DocumentMut, value};

use super::panels::Panels;
use super::remotes::Remotes;
use crate::hub::{CommandOrigin, Hub};
use crate::node::events::EventRecord;
use crate::node::state::ChannelUpdate;

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub when: Trigger,
    /// Checked when the trigger fires; every condition must hold for the actions to run.
    #[serde(default, rename = "if")]
    pub conditions: Vec<Condition>,
    pub then: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "TriggerFields")]
pub enum Trigger {
    Event(EventTrigger),
    Schedule(ScheduleTrigger),
    Cron(CronTrigger),
    Threshold(ThresholdTrigger),
    Offline(OfflineTrigger),
    RemoteKey(RemoteKeyTrigger),
    Button(ButtonTrigger),
}

impl Trigger {
    /// The first run strictly after `now`; `None` for event rules.
    pub fn next_run(&self, now: &Zoned) -> Option<Zoned> {
        match self {
            Trigger::Event(_)
            | Trigger::Threshold(_)
            | Trigger::Offline(_)
            | Trigger::RemoteKey(_)
            | Trigger::Button(_) => None,
            Trigger::Schedule(trigger) => trigger.next_run(now),
            Trigger::Cron(trigger) => trigger.cron.find_next_occurrence(now, false).ok(),
        }
    }

    fn is_due(&self, minute: DateTime) -> bool {
        match self {
            Trigger::Event(_)
            | Trigger::Threshold(_)
            | Trigger::Offline(_)
            | Trigger::RemoteKey(_)
            | Trigger::Button(_) => false,
            Trigger::Schedule(trigger) => trigger.is_due(minute),
            Trigger::Cron(trigger) => trigger.cron.is_time_matching(&minute).unwrap_or(false),
        }
    }
}

/// Every key a `when` may have; which ones are present picks the trigger.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerFields {
    device: Option<String>,
    entity: Option<u8>,
    channel: Option<u8>,
    event: Option<EventKind>,
    data: Option<String>,
    above: Option<f32>,
    below: Option<f32>,
    at: Option<Time>,
    #[serde(default)]
    days: Vec<Day>,
    cron: Option<String>,
    offline: Option<String>,
    remote: Option<String>,
    key: Option<String>,
    panel: Option<String>,
    button: Option<String>,
}

impl TryFrom<TriggerFields> for Trigger {
    type Error = String;

    fn try_from(fields: TriggerFields) -> Result<Self, String> {
        if fields.panel.is_some() || fields.button.is_some() {
            let alone = fields.device.is_none()
                && fields.entity.is_none()
                && fields.channel.is_none()
                && fields.event.is_none()
                && fields.data.is_none()
                && fields.above.is_none()
                && fields.below.is_none()
                && fields.at.is_none()
                && fields.days.is_empty()
                && fields.cron.is_none()
                && fields.offline.is_none()
                && fields.remote.is_none()
                && fields.key.is_none();
            let (Some(panel), Some(button), true) = (fields.panel, fields.button, alone) else {
                return Err("a panel button trigger is `panel` and `button` alone".into());
            };
            return Ok(Trigger::Button(ButtonTrigger { panel, button }));
        }
        if fields.remote.is_some() || fields.key.is_some() {
            let alone = fields.device.is_none()
                && fields.entity.is_none()
                && fields.channel.is_none()
                && fields.event.is_none()
                && fields.data.is_none()
                && fields.above.is_none()
                && fields.below.is_none()
                && fields.at.is_none()
                && fields.days.is_empty()
                && fields.cron.is_none()
                && fields.offline.is_none();
            let (Some(remote), Some(key), true) = (fields.remote, fields.key, alone) else {
                return Err("a remote key trigger is `remote` and `key` alone".into());
            };
            return Ok(Trigger::RemoteKey(RemoteKeyTrigger { remote, key }));
        }
        if let Some(device) = fields.offline {
            let alone = fields.device.is_none()
                && fields.entity.is_none()
                && fields.channel.is_none()
                && fields.event.is_none()
                && fields.data.is_none()
                && fields.above.is_none()
                && fields.below.is_none()
                && fields.at.is_none()
                && fields.days.is_empty()
                && fields.cron.is_none();
            if !alone {
                return Err("`offline` goes alone in `when`".into());
            }
            let device = (device != "*").then_some(device);
            return Ok(Trigger::Offline(OfflineTrigger { device }));
        }
        let channel_kinds = [
            fields.event.is_some(),
            fields.above.is_some(),
            fields.below.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        let is_channel_trigger = channel_kinds > 0
            || fields.device.is_some()
            || fields.entity.is_some()
            || fields.channel.is_some();
        if is_channel_trigger {
            if fields.at.is_some() || fields.cron.is_some() || !fields.days.is_empty() {
                return Err("a channel trigger cannot have `at`, `days` or `cron`".into());
            }
            if channel_kinds > 1 {
                return Err("use only one of `event`, `above` and `below`".into());
            }
            if fields.data.is_some() && fields.event.is_none() {
                return Err("`data` goes with `event`".into());
            }
            let (Some(device), Some(entity), Some(channel)) =
                (fields.device, fields.entity, fields.channel)
            else {
                return Err("a channel trigger needs `device`, `entity` and `channel`".into());
            };
            let limit = match (fields.event, fields.above, fields.below) {
                (Some(event), _, _) => {
                    return Ok(Trigger::Event(EventTrigger {
                        device,
                        entity,
                        channel,
                        event,
                        data: fields.data,
                    }));
                }
                (_, Some(above), _) => Limit::Above(above),
                (_, _, Some(below)) => Limit::Below(below),
                _ => return Err("a channel trigger needs `event`, `above` or `below`".into()),
            };
            return Ok(Trigger::Threshold(ThresholdTrigger {
                device,
                entity,
                channel,
                limit,
            }));
        }
        if fields.at.is_some() && fields.cron.is_some() {
            return Err("use either `at` or `cron`, not both".into());
        }
        if let Some(expression) = fields.cron {
            if !fields.days.is_empty() {
                return Err("`days` goes with `at`; put the days in the cron expression".into());
            }
            return Ok(Trigger::Cron(CronTrigger::parse(&expression)?));
        }
        if let Some(at) = fields.at {
            let days = fields.days;
            return Ok(Trigger::Schedule(ScheduleTrigger { at, days }));
        }
        if !fields.days.is_empty() {
            return Err("`days` needs `at`".into());
        }
        Err("`when` needs an event or a limit on a channel (`device`, `entity`, `channel`), `at`, `cron` or `offline`".into())
    }
}

/// Runs when a confirmed numeric value crosses the limit, not again while it stays past it.
/// The first known value of the channel counts as a crossing.
#[derive(Debug, Clone, PartialEq)]
pub struct ThresholdTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub limit: Limit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Limit {
    Above(f32),
    Below(f32),
}

impl Limit {
    fn crossed(self, previous: Option<&Value>, value: &Value) -> bool {
        let Some(now) = number(value) else {
            return false;
        };
        let before = previous.and_then(number);
        match self {
            Limit::Above(limit) => now > limit && before.is_none_or(|before| before <= limit),
            Limit::Below(limit) => now < limit && before.is_none_or(|before| before >= limit),
        }
    }
}

fn number(value: &Value) -> Option<f32> {
    match value {
        Value::Int(number) => Some(*number as f32),
        Value::Float(number) => Some(*number),
        _ => None,
    }
}

/// Something about the home right now: a channel's value, the local time, the day or a node's
/// presence.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "ConditionFields")]
pub enum Condition {
    Channel(ChannelCondition),
    /// From `from` (included) to `to` (excluded); crosses midnight when `from` is later than `to`.
    Time {
        from: Time,
        to: Time,
    },
    Days(Vec<Day>),
    Online(String),
    Offline(String),
}

/// Holds only while the node is online and its value is known.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelCondition {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub test: ValueTest,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValueTest {
    Equals(Expected),
    Above(f32),
    Below(f32),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Expected {
    Bool(bool),
    Number(f32),
    Text(String),
}

impl Expected {
    fn matches(&self, value: &Value) -> bool {
        match (self, value) {
            (Expected::Bool(expected), Value::Bool(actual)) => expected == actual,
            (Expected::Number(expected), _) => number(value) == Some(*expected),
            (Expected::Text(expected), Value::Text(actual) | Value::Enum(actual)) => {
                expected == actual
            }
            _ => false,
        }
    }
}

impl fmt::Display for Expected {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Expected::Bool(expected) => write!(formatter, "{expected}"),
            Expected::Number(expected) => write!(formatter, "{expected}"),
            Expected::Text(expected) => write!(formatter, "\"{expected}\""),
        }
    }
}

impl ValueTest {
    fn passes(&self, value: &Value) -> bool {
        match self {
            ValueTest::Equals(expected) => expected.matches(value),
            ValueTest::Above(limit) => number(value).is_some_and(|number| number > *limit),
            ValueTest::Below(limit) => number(value).is_some_and(|number| number < *limit),
        }
    }
}

/// What conditions look at; the hub in practice.
pub trait HomeState {
    fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value>;
    fn is_online(&self, device_id: &str) -> bool;
}

impl Condition {
    /// `now` is the local time in the hub's zone.
    pub fn holds(&self, now: DateTime, home: &impl HomeState) -> bool {
        match self {
            Condition::Channel(condition) => {
                home.is_online(&condition.device)
                    && home
                        .value(&condition.device, condition.entity, condition.channel)
                        .is_some_and(|value| condition.test.passes(&value))
            }
            Condition::Time { from, to } => {
                let time = now.time();
                if from < to {
                    *from <= time && time < *to
                } else {
                    *from <= time || time < *to
                }
            }
            Condition::Days(days) => days.iter().any(|day| Weekday::from(*day) == now.weekday()),
            Condition::Online(device) => home.is_online(device),
            Condition::Offline(device) => !home.is_online(device),
        }
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Condition::Channel(condition) => {
                let ChannelCondition {
                    device,
                    entity,
                    channel,
                    test,
                } = condition;
                match test {
                    ValueTest::Equals(expected) => {
                        write!(formatter, "{device} {entity}/{channel} equals {expected}")
                    }
                    ValueTest::Above(limit) => {
                        write!(formatter, "{device} {entity}/{channel} above {limit}")
                    }
                    ValueTest::Below(limit) => {
                        write!(formatter, "{device} {entity}/{channel} below {limit}")
                    }
                }
            }
            Condition::Time { from, to } => write!(
                formatter,
                "from {} to {}",
                from.strftime("%H:%M"),
                to.strftime("%H:%M")
            ),
            Condition::Days(days) => write!(formatter, "days {days:?}"),
            Condition::Online(device) => write!(formatter, "{device} online"),
            Condition::Offline(device) => write!(formatter, "{device} offline"),
        }
    }
}

/// Every key a condition may have; which ones are present picks the condition.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConditionFields {
    device: Option<String>,
    entity: Option<u8>,
    channel: Option<u8>,
    equals: Option<Expected>,
    above: Option<f32>,
    below: Option<f32>,
    from: Option<Time>,
    to: Option<Time>,
    days: Option<Vec<Day>>,
    online: Option<String>,
    offline: Option<String>,
}

impl TryFrom<ConditionFields> for Condition {
    type Error = String;

    fn try_from(fields: ConditionFields) -> Result<Self, String> {
        let is_channel = fields.device.is_some()
            || fields.entity.is_some()
            || fields.channel.is_some()
            || fields.equals.is_some()
            || fields.above.is_some()
            || fields.below.is_some();
        let is_time = fields.from.is_some() || fields.to.is_some();
        let kinds = [
            is_channel,
            is_time,
            fields.days.is_some(),
            fields.online.is_some(),
            fields.offline.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if kinds != 1 {
            return Err("a condition is one of: a channel value (`device`, `entity`, `channel` with `equals`, `above` or `below`), `from` and `to`, `days`, `online` or `offline`".into());
        }
        if is_channel {
            let (Some(device), Some(entity), Some(channel)) =
                (fields.device, fields.entity, fields.channel)
            else {
                return Err("a channel condition needs `device`, `entity` and `channel`".into());
            };
            let test = match (fields.equals, fields.above, fields.below) {
                (Some(expected), None, None) => ValueTest::Equals(expected),
                (None, Some(limit), None) => ValueTest::Above(limit),
                (None, None, Some(limit)) => ValueTest::Below(limit),
                _ => {
                    return Err(
                        "a channel condition needs one of `equals`, `above` and `below`".into(),
                    );
                }
            };
            return Ok(Condition::Channel(ChannelCondition {
                device,
                entity,
                channel,
                test,
            }));
        }
        if is_time {
            let (Some(from), Some(to)) = (fields.from, fields.to) else {
                return Err("a time condition needs both `from` and `to`".into());
            };
            if from == to {
                return Err("`from` and `to` must differ".into());
            }
            return Ok(Condition::Time { from, to });
        }
        if let Some(days) = fields.days {
            if days.is_empty() {
                return Err("`days` needs at least one day".into());
            }
            return Ok(Condition::Days(days));
        }
        if let Some(device) = fields.online {
            return Ok(Condition::Online(device));
        }
        let device = fields.offline.expect("one kind is present");
        Ok(Condition::Offline(device))
    }
}

/// How a rule involves a node: it runs on something from it, or acts on it.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleUse {
    pub rule: String,
    pub enabled: bool,
    pub role: RuleRole,
    /// `None` for the whole node (offline triggers).
    pub channel: Option<(u8, u8)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleRole {
    /// An event or value of the node, or the node going offline, runs the rule.
    RunsOn,
    /// One of the rule's actions sets, toggles or invokes a channel of the node.
    ActsOn,
    /// One of the rule's conditions looks at a channel of the node, or at whether it is online.
    Checks,
}

/// A button of a panel in `panels.toml`, pressed on the `/panel` page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonTrigger {
    pub panel: String,
    pub button: String,
}

/// Runs when any IR receiver gets the key's code (not its repeats), by the names in
/// `remotes.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteKeyTrigger {
    pub remote: String,
    pub key: String,
}

/// Runs when the hub marks a node offline; `None` for any node (`offline = "*"`).
#[derive(Debug, Clone, PartialEq)]
pub struct OfflineTrigger {
    pub device: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventTrigger {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
    pub event: EventKind,
    /// When set, the event's data must be exactly this text, e.g. an IR code `NEC 00 45`
    /// (`NEC 00 45 repeat`, sent while the key is held, does not match).
    pub data: Option<String>,
}

impl EventTrigger {
    fn matches(&self, event: &EventRecord) -> bool {
        self.device == event.device_id
            && self.entity == event.entity
            && self.channel == event.channel
            && self.event == event.kind
            && self.data.as_ref().is_none_or(|expected| {
                matches!(&event.data, Some(Value::Text(text) | Value::Enum(text)) if text == expected)
            })
    }
}

/// Local time; no `days` means every day.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleTrigger {
    pub at: Time,
    pub days: Vec<Day>,
}

/// Standard five fields (minute, hour, day of month, month, day of week), in local time.
#[derive(Debug, Clone, PartialEq)]
pub struct CronTrigger {
    pub expression: String,
    pub cron: Box<Cron>,
}

impl CronTrigger {
    fn parse(expression: &str) -> Result<Self, String> {
        let parser = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .build();
        parser
            .parse(expression)
            .map(|cron| Self {
                expression: expression.to_string(),
                cron: Box::new(cron),
            })
            .map_err(|error| format!("cron \"{expression}\": {error}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Day {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl From<Day> for Weekday {
    fn from(day: Day) -> Self {
        match day {
            Day::Monday => Weekday::Monday,
            Day::Tuesday => Weekday::Tuesday,
            Day::Wednesday => Weekday::Wednesday,
            Day::Thursday => Weekday::Thursday,
            Day::Friday => Weekday::Friday,
            Day::Saturday => Weekday::Saturday,
            Day::Sunday => Weekday::Sunday,
        }
    }
}

impl ScheduleTrigger {
    fn runs_on(&self, weekday: Weekday) -> bool {
        self.days.is_empty() || self.days.iter().any(|day| Weekday::from(*day) == weekday)
    }

    fn is_due(&self, minute: DateTime) -> bool {
        self.at.hour() == minute.hour()
            && self.at.minute() == minute.minute()
            && self.runs_on(minute.weekday())
    }

    /// `None` only if the time never exists in the zone.
    fn next_run(&self, now: &Zoned) -> Option<Zoned> {
        (0..=7)
            .filter_map(|days| now.date().checked_add(days.days()).ok())
            .filter(|date| self.runs_on(date.weekday()))
            .filter_map(|date| {
                date.to_datetime(self.at)
                    .to_zoned(now.time_zone().clone())
                    .ok()
            })
            .find(|run| run > now)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Set {
        device: String,
        entity: u8,
        channel: u8,
        value: bool,
    },
    Toggle {
        device: String,
        entity: u8,
        channel: u8,
    },
    /// Fires a `Trigger` channel, e.g. a buzzer pattern; `value` is one of the channel's `Enum` options.
    Invoke {
        device: String,
        entity: u8,
        channel: u8,
        value: Option<String>,
    },
    Notify {
        title: String,
        message: String,
    },
}

impl fmt::Display for Action {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Action::Set {
                device,
                entity,
                channel,
                value,
            } => write!(formatter, "set {device} {entity}/{channel} to {value}"),
            Action::Toggle {
                device,
                entity,
                channel,
            } => write!(formatter, "toggle {device} {entity}/{channel}"),
            Action::Invoke {
                device,
                entity,
                channel,
                value: Some(value),
            } => write!(formatter, "invoke {device} {entity}/{channel} with {value}"),
            Action::Invoke {
                device,
                entity,
                channel,
                value: None,
            } => write!(formatter, "invoke {device} {entity}/{channel}"),
            Action::Notify { title, .. } => write!(formatter, "notify \"{title}\""),
        }
    }
}

/// How the last run of a rule went; `errors` is empty when every action worked.
#[derive(Debug, Clone)]
pub struct RuleRun {
    pub at: Timestamp,
    pub errors: Vec<String>,
}

#[derive(Default, Deserialize)]
pub struct Rules {
    #[serde(default)]
    rules: Vec<Rule>,
}

impl Rules {
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn all(&self) -> &[Rule] {
        &self.rules
    }

    pub fn matching(&self, event: &EventRecord, remotes: &Remotes) -> Vec<&Rule> {
        let code = match &event.data {
            Some(Value::Text(code)) if event.kind == EventKind::Received => Some(code.as_str()),
            _ => None,
        };
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && match &rule.when {
                        Trigger::Event(trigger) => trigger.matches(event),
                        Trigger::RemoteKey(trigger) => code.is_some_and(|code| {
                            remotes.code(&trigger.remote, &trigger.key) == Some(code)
                        }),
                        _ => false,
                    }
            })
            .collect()
    }

    /// Rules that run on something from the node or act on it, in file order. Remote keys,
    /// panel buttons and schedules are not tied to a node, so only their actions count.
    pub fn involving(&self, device_id: &str) -> Vec<RuleUse> {
        let mut uses: Vec<RuleUse> = Vec::new();
        for rule in &self.rules {
            let mut add = |role, channel| {
                let entry = RuleUse {
                    rule: rule.name.clone(),
                    enabled: rule.enabled,
                    role,
                    channel,
                };
                if !uses.contains(&entry) {
                    uses.push(entry);
                }
            };
            match &rule.when {
                Trigger::Event(trigger) if trigger.device == device_id => {
                    add(RuleRole::RunsOn, Some((trigger.entity, trigger.channel)));
                }
                Trigger::Threshold(trigger) if trigger.device == device_id => {
                    add(RuleRole::RunsOn, Some((trigger.entity, trigger.channel)));
                }
                Trigger::Offline(trigger)
                    if trigger
                        .device
                        .as_deref()
                        .is_none_or(|device| device == device_id) =>
                {
                    add(RuleRole::RunsOn, None);
                }
                _ => {}
            }
            for condition in &rule.conditions {
                match condition {
                    Condition::Channel(condition) if condition.device == device_id => {
                        add(
                            RuleRole::Checks,
                            Some((condition.entity, condition.channel)),
                        );
                    }
                    Condition::Online(device) | Condition::Offline(device)
                        if device == device_id =>
                    {
                        add(RuleRole::Checks, None);
                    }
                    _ => {}
                }
            }
            for action in &rule.then {
                match action {
                    Action::Set {
                        device,
                        entity,
                        channel,
                        ..
                    }
                    | Action::Toggle {
                        device,
                        entity,
                        channel,
                    }
                    | Action::Invoke {
                        device,
                        entity,
                        channel,
                        ..
                    } if device == device_id => add(RuleRole::ActsOn, Some((*entity, *channel))),
                    _ => {}
                }
            }
        }
        uses
    }

    /// The rules a panel button runs, with whether each is enabled.
    pub fn for_button(&self, panel: &str, button: &str) -> Vec<(String, bool)> {
        self.rules
            .iter()
            .filter(|rule| {
                matches!(&rule.when, Trigger::Button(trigger)
                    if trigger.panel == panel && trigger.button == button)
            })
            .map(|rule| (rule.name.clone(), rule.enabled))
            .collect()
    }

    pub fn pressed(&self, panel: &str, button: &str) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Button(trigger)
                        if trigger.panel == panel && trigger.button == button)
            })
            .collect()
    }

    /// Every panel button a rule names exists in `panels.toml`.
    pub fn check_panel_buttons(&self, panels: &Panels) -> anyhow::Result<()> {
        for rule in &self.rules {
            if let Trigger::Button(trigger) = &rule.when
                && !panels.has(&trigger.panel, &trigger.button)
            {
                bail!(
                    "rule \"{}\": panel \"{}\" has no button \"{}\" in panels.toml",
                    rule.name,
                    trigger.panel,
                    trigger.button
                );
            }
        }
        Ok(())
    }

    /// Every `remote`/`key` a rule names exists in `remotes.toml`.
    pub fn check_remote_keys(&self, remotes: &Remotes) -> anyhow::Result<()> {
        for rule in &self.rules {
            if let Trigger::RemoteKey(trigger) = &rule.when
                && remotes.code(&trigger.remote, &trigger.key).is_none()
            {
                bail!(
                    "rule \"{}\": {} has no key \"{}\" in remotes.toml",
                    rule.name,
                    trigger.remote,
                    trigger.key
                );
            }
        }
        Ok(())
    }

    pub fn crossed(&self, update: &ChannelUpdate) -> Vec<&Rule> {
        if update.confidence != Confidence::Confirmed {
            return Vec::new();
        }
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Threshold(trigger)
                        if trigger.device == update.device_id
                            && trigger.entity == update.entity
                            && trigger.channel == update.channel
                            && trigger.limit.crossed(update.previous.as_ref(), &update.value))
            })
            .collect()
    }

    pub fn offline(&self, device_id: &str) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Offline(trigger)
                        if trigger.device.as_deref().is_none_or(|device| device == device_id))
            })
            .collect()
    }

    /// Scheduled rules for a local minute (seconds are ignored).
    pub fn due(&self, minute: DateTime) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| rule.enabled && rule.when.is_due(minute))
            .collect()
    }
}

impl HomeState for Hub {
    fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value> {
        Hub::value(self, device_id, entity, channel)
    }

    fn is_online(&self, device_id: &str) -> bool {
        Hub::is_online(self, device_id)
    }
}

/// Runs the actions in order when every condition holds; a failed action does not stop the
/// next ones.
pub async fn run(hub: Arc<Hub>, rule: Rule) {
    let at = Timestamp::now();
    let now = at.to_zoned(hub.time_zone().clone()).datetime();
    if let Some(condition) = rule
        .conditions
        .iter()
        .find(|condition| !condition.holds(now, hub.as_ref()))
    {
        println!("rule \"{}\": skipped, {condition} does not hold", rule.name);
        return;
    }
    let mut errors = Vec::new();
    for action in &rule.then {
        match perform(&hub, action, &rule.name).await {
            Ok(outcome) => println!("rule \"{}\": {action} -> {outcome}", rule.name),
            Err(error) => {
                println!("rule \"{}\": {action} failed: {error}", rule.name);
                hub.notify(
                    "Rule failed".into(),
                    format!("{}: {action}: {error}", rule.name),
                    rule.name.clone(),
                );
                errors.push(format!("{action}: {error}"));
            }
        }
    }
    hub.record_rule_run(&rule.name, RuleRun { at, errors });
}

/// Returns what happened, for the log.
async fn perform(hub: &Hub, action: &Action, rule_name: &str) -> anyhow::Result<String> {
    let (device, entity, channel, value) = match action {
        Action::Notify { title, message } => {
            hub.notify(title.clone(), message.clone(), rule_name.to_string());
            return Ok("shown".into());
        }
        Action::Invoke {
            device,
            entity,
            channel,
            value,
        } => {
            hub.invoke(
                device,
                *entity,
                *channel,
                value.clone(),
                CommandOrigin::Rule(rule_name.to_string()),
            )
            .await?;
            return Ok("done".into());
        }
        Action::Set {
            device,
            entity,
            channel,
            value,
        } => (device, *entity, *channel, *value),
        Action::Toggle {
            device,
            entity,
            channel,
        } => match hub.value(device, *entity, *channel) {
            Some(Value::Bool(current)) => (device, *entity, *channel, !current),
            Some(other) => bail!("current value is {other:?}, not a Bool"),
            None => bail!("current value is unknown"),
        },
    };
    hub.set(
        device,
        entity,
        channel,
        Value::Bool(value),
        CommandOrigin::Rule(rule_name.to_string()),
    )
    .await?;
    Ok(value.to_string())
}

const fn default_true() -> bool {
    true
}

/// Turns the rule named `name` on or off in the text of `rules.toml`, keeping comments: off is
/// `enabled = false` right after the name, on removes the line (rules are on by default).
pub fn with_rule_enabled(text: &str, name: &str, enabled: bool) -> anyhow::Result<String> {
    let mut document: DocumentMut = text.parse()?;
    let rule = document
        .get_mut("rules")
        .and_then(|rules| rules.as_array_of_tables_mut())
        .and_then(|rules| {
            rules
                .iter_mut()
                .find(|rule| rule.get("name").and_then(|item| item.as_str()) == Some(name))
        })
        .ok_or_else(|| anyhow::anyhow!("no rule named \"{name}\""))?;
    if enabled {
        rule.remove("enabled");
    } else {
        rule.insert("enabled", value(false));
        let rank = |key: &str| match key {
            "name" => 0,
            "enabled" => 1,
            _ => 2,
        };
        rule.sort_values_by(|first, _, second, _| rank(first.get()).cmp(&rank(second.get())));
    }
    Ok(document.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Rules {
        fn matching_without_remotes(&self, event: &EventRecord) -> Vec<&Rule> {
            self.matching(event, &Remotes::default())
        }
    }

    const BUTTONS: &str = r#"
        [[rules]]
        name = "LED"
        when = { panel = "Test board", button = "LED" }
        then = []

        [[rules]]
        name = "LED also beeps"
        when = { panel = "Test board", button = "LED" }
        then = []

        [[rules]]
        name = "LED, disabled"
        enabled = false
        when = { panel = "Test board", button = "LED" }
        then = []
    "#;

    #[test]
    fn panel_buttons_run_their_enabled_rules() {
        let rules: Rules = toml::from_str(BUTTONS).unwrap();
        assert_eq!(
            rules.for_button("Test board", "LED"),
            [
                ("LED".to_string(), true),
                ("LED also beeps".to_string(), true),
                ("LED, disabled".to_string(), false),
            ]
        );
        assert_eq!(
            names(rules.pressed("Test board", "LED")),
            ["LED", "LED also beeps"]
        );
        assert!(rules.pressed("Test board", "Alarm").is_empty());
        assert!(rules.for_button("Other", "LED").is_empty());
        let panels: Panels =
            toml::from_str("[[panels]]\nname = \"Test board\"\nbuttons = [\"LED\"]").unwrap();
        assert!(rules.check_panel_buttons(&panels).is_ok());
        let error = rules
            .check_panel_buttons(&Panels::default())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("panel \"Test board\" has no button \"LED\""),
            "{error}"
        );
    }

    #[test]
    fn button_triggers_need_panel_and_button_alone() {
        assert!(
            when(r#"{ button = "A" }"#)
                .unwrap_err()
                .contains("`panel` and `button`")
        );
        assert!(
            when(r#"{ panel = "P", button = "A", at = "07:00" }"#)
                .unwrap_err()
                .contains("alone")
        );
    }

    #[test]
    fn rules_involving_a_node() {
        let rules: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "Button lights the lamp"
            when = { device = "board", entity = 2, channel = 0, event = "Press" }
            then = [
                { toggle = { device = "lamp-node", entity = 1, channel = 0 } },
                { set = { device = "board", entity = 1, channel = 0, value = true } },
            ]

            [[rules]]
            name = "Any node offline"
            when = { offline = "*" }
            then = [{ invoke = { device = "board", entity = 5, channel = 0, value = "Alarm" } }]

            [[rules]]
            name = "Hot"
            enabled = false
            when = { device = "other", entity = 3, channel = 0, above = 28 }
            then = [{ notify = { title = "Hot", message = "" } }]
            "#,
        )
        .unwrap();
        type Role = (String, RuleRole, Option<(u8, u8)>);
        let roles = |device: &str| -> Vec<Role> {
            rules
                .involving(device)
                .into_iter()
                .map(|use_| (use_.rule, use_.role, use_.channel))
                .collect()
        };
        assert_eq!(
            roles("board"),
            [
                (
                    "Button lights the lamp".into(),
                    RuleRole::RunsOn,
                    Some((2, 0))
                ),
                (
                    "Button lights the lamp".into(),
                    RuleRole::ActsOn,
                    Some((1, 0))
                ),
                ("Any node offline".into(), RuleRole::RunsOn, None),
                ("Any node offline".into(), RuleRole::ActsOn, Some((5, 0))),
            ]
        );
        assert_eq!(
            roles("lamp-node"),
            [
                (
                    "Button lights the lamp".into(),
                    RuleRole::ActsOn,
                    Some((1, 0))
                ),
                ("Any node offline".into(), RuleRole::RunsOn, None),
            ]
        );
        let other = rules.involving("other");
        assert_eq!(other.len(), 2);
        assert!(!other[1].enabled);
    }

    const REMOTE_KEYS: &str = r#"
        [[rules]]
        name = "Key 0"
        when = { remote = "Kit remote", key = "0" }
        then = []
    "#;

    #[test]
    fn remote_key_rules_match_the_code_from_any_receiver() {
        let rules: Rules = toml::from_str(REMOTE_KEYS).unwrap();
        let remotes: Remotes =
            toml::from_str("[remotes.\"Kit remote\".keys]\n0 = \"NEC 00 52\"").unwrap();
        let received = |device: &str, code: &str| EventRecord {
            device_id: device.into(),
            entity: 9,
            channel: 0,
            kind: EventKind::Received,
            data: Some(Value::Text(code.into())),
        };
        assert_eq!(
            rules
                .matching(&received("board-a", "NEC 00 52"), &remotes)
                .len(),
            1
        );
        assert_eq!(
            rules
                .matching(&received("board-b", "NEC 00 52"), &remotes)
                .len(),
            1
        );
        assert!(
            rules
                .matching(&received("board-a", "NEC 00 52 repeat"), &remotes)
                .is_empty()
        );
        assert!(
            rules
                .matching(&received("board-a", "NEC 00 18"), &remotes)
                .is_empty()
        );
        assert!(rules.check_remote_keys(&remotes).is_ok());
        let error = rules
            .check_remote_keys(&Remotes::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("Kit remote has no key \"0\""), "{error}");
    }

    #[test]
    fn remote_key_triggers_go_alone() {
        assert!(
            when(r#"{ remote = "A" }"#)
                .unwrap_err()
                .contains("`remote` and `key` alone")
        );
        assert!(
            when(r#"{ remote = "A", key = "0", device = "x" }"#)
                .unwrap_err()
                .contains("alone")
        );
    }

    const EXAMPLE: &str = r#"
        [[rules]]
        name = "Living room button toggles bedroom lamp"
        when = { device = "sim-living-room", entity = 2, channel = 0, event = "Press" }
        then = [{ toggle = { device = "sim-bedroom", entity = 1, channel = 0 } }]

        [[rules]]
        name = "Long press turns everything off"
        when = { device = "sim-living-room", entity = 2, channel = 0, event = "LongPress" }
        then = [
            { set = { device = "sim-bedroom", entity = 1, channel = 0, value = false } },
            { set = { device = "sim-living-room", entity = 1, channel = 0, value = false } },
            { notify = { title = "Good night", message = "All lamps are off" } },
        ]

        [[rules]]
        name = "Disabled"
        enabled = false
        when = { device = "sim-living-room", entity = 2, channel = 0, event = "LongPress" }
        then = [{ notify = { title = "Error", message = "if this is disabled, why are you seeing this?" } }]
    "#;

    fn event(kind: EventKind, entity: u8) -> EventRecord {
        EventRecord {
            device_id: "sim-living-room".into(),
            entity,
            channel: 0,
            kind,
            data: None,
        }
    }

    #[test]
    fn example_file_parses() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(
            rules.rules[1].then[0],
            Action::Set {
                device: "sim-bedroom".into(),
                entity: 1,
                channel: 0,
                value: false,
            }
        );
    }

    #[test]
    fn notify_action_parses() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert_eq!(
            rules.rules[1].then[2],
            Action::Notify {
                title: "Good night".into(),
                message: "All lamps are off".into(),
            }
        );
    }

    #[test]
    fn matching_event_selects_its_rules() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        let matched = rules.matching_without_remotes(&event(EventKind::Press, 2));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "Living room button toggles bedroom lamp");
    }

    #[test]
    fn other_events_select_nothing() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert!(
            rules
                .matching_without_remotes(&event(EventKind::DoublePress, 2))
                .is_empty()
        );
        assert!(
            rules
                .matching_without_remotes(&event(EventKind::Press, 3))
                .is_empty()
        );
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        let matched = rules.matching_without_remotes(&event(EventKind::LongPress, 2));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "Long press turns everything off");
    }

    #[test]
    fn empty_file_has_no_rules() {
        let rules: Rules = toml::from_str("").unwrap();
        assert_eq!(rules.len(), 0);
    }

    const SCHEDULED: &str = r#"
        [[rules]]
        name = "Lights off at night"
        when = { at = "23:00" }
        then = [{ set = { device = "sim-bedroom", entity = 1, channel = 0, value = false } }]

        [[rules]]
        name = "Wake up"
        when = { at = "07:00", days = ["monday", "tuesday", "wednesday", "thursday", "friday"] }
        then = [{ set = { device = "sim-bedroom", entity = 1, channel = 0, value = true } }]
    "#;

    fn minute(text: &str) -> DateTime {
        text.parse().unwrap()
    }

    fn names(rules: Vec<&Rule>) -> Vec<&str> {
        rules.into_iter().map(|rule| rule.name.as_str()).collect()
    }

    #[test]
    fn scheduled_rules_parse() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        assert_eq!(
            rules.rules[1].when,
            Trigger::Schedule(ScheduleTrigger {
                at: Time::constant(7, 0, 0, 0),
                days: vec![
                    Day::Monday,
                    Day::Tuesday,
                    Day::Wednesday,
                    Day::Thursday,
                    Day::Friday
                ],
            })
        );
    }

    #[test]
    fn rules_are_due_at_their_minute_and_days() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        // 2026-10-12 is a Monday, 2026-10-17 a Saturday.
        assert_eq!(
            names(rules.due(minute("2026-10-12T23:00"))),
            ["Lights off at night"]
        );
        assert_eq!(names(rules.due(minute("2026-10-12T07:00"))), ["Wake up"]);
        assert!(rules.due(minute("2026-10-17T07:00")).is_empty());
        assert!(rules.due(minute("2026-10-12T07:01")).is_empty());
    }

    #[test]
    fn events_do_not_run_scheduled_rules() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        assert!(
            rules
                .matching_without_remotes(&event(EventKind::Press, 2))
                .is_empty()
        );
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert!(rules.due(minute("2026-10-12T23:00")).is_empty());
    }

    #[test]
    fn next_run_skips_to_the_next_allowed_day() {
        let rules: Rules = toml::from_str(SCHEDULED).unwrap();
        let Trigger::Schedule(wake_up) = &rules.rules[1].when else {
            panic!("scheduled rule");
        };
        let friday_night: Zoned = "2026-10-16T22:00:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        let run = wake_up.next_run(&friday_night).unwrap();
        assert_eq!(run.datetime(), minute("2026-10-19T07:00"));
        let monday_at_seven: Zoned = "2026-10-19T07:00:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        let run = wake_up.next_run(&monday_at_seven).unwrap();
        assert_eq!(run.datetime(), minute("2026-10-20T07:00"));
    }

    fn when(text: &str) -> Result<Trigger, String> {
        #[derive(Deserialize)]
        struct Wrapper {
            when: Trigger,
        }
        toml::from_str::<Wrapper>(&format!("when = {text}"))
            .map(|wrapper| wrapper.when)
            .map_err(|error| error.message().to_string())
    }

    #[test]
    fn cron_rules_are_due_when_the_expression_matches() {
        let trigger = when(r#"{ cron = "*/30 8-22 * * mon-fri" }"#).unwrap();
        assert!(trigger.is_due(minute("2026-10-12T08:30")));
        assert!(trigger.is_due(minute("2026-10-12T22:00")));
        assert!(!trigger.is_due(minute("2026-10-12T08:15")));
        assert!(!trigger.is_due(minute("2026-10-12T23:00")));
        assert!(!trigger.is_due(minute("2026-10-17T08:30")));
    }

    #[test]
    fn cron_next_run_skips_the_weekend() {
        let trigger = when(r#"{ cron = "0 7 * * 1-5" }"#).unwrap();
        let friday_night: Zoned = "2026-10-16T22:00:00-03:00[America/Sao_Paulo]"
            .parse()
            .unwrap();
        let run = trigger.next_run(&friday_night).unwrap();
        assert_eq!(run.datetime(), minute("2026-10-19T07:00"));
    }

    #[test]
    fn invalid_triggers_explain_what_is_wrong() {
        let error = |text: &str| when(text).unwrap_err();
        assert!(error(r#"{ cron = "61 * * * *" }"#).contains("cron \"61 * * * *\""));
        assert!(error(r#"{ cron = "0 0 7 * * *" }"#).contains("cron"));
        assert!(error(r#"{ at = "07:00", cron = "0 7 * * *" }"#).contains("not both"));
        assert!(error(r#"{ cron = "0 7 * * *", days = ["monday"] }"#).contains("cron expression"));
        assert!(error(r#"{ days = ["monday"] }"#).contains("needs `at`"));
        assert!(error(r#"{ device = "sim-bedroom", event = "Press" }"#).contains("needs `device`"));
        assert!(error(r#"{ device = "sim-bedroom", entity = 1, channel = 0, event = "Press", at = "07:00" }"#).contains("cannot have"));
        assert!(error(r#"{ at = "07:00", day = ["monday"] }"#).contains("unknown field"));
        assert!(error("{}").contains("needs an event"));
    }

    const OFFLINE: &str = r#"
        [[rules]]
        name = "Any node offline"
        when = { offline = "*" }
        then = [{ invoke = { device = "esp32-08d1f9ca41e4", entity = 5, channel = 0, value = "Alarm" } }]

        [[rules]]
        name = "Bedroom offline"
        when = { offline = "sim-bedroom" }
        then = []
    "#;

    #[test]
    fn offline_rules_match_their_node_or_any() {
        let rules: Rules = toml::from_str(OFFLINE).unwrap();
        assert_eq!(
            names(rules.offline("sim-bedroom")),
            ["Any node offline", "Bedroom offline"]
        );
        assert_eq!(
            names(rules.offline("sim-living-room")),
            ["Any node offline"]
        );
        assert!(
            rules
                .matching_without_remotes(&event(EventKind::Press, 2))
                .is_empty()
        );
        assert!(rules.due(minute("2026-10-12T23:00")).is_empty());
    }

    #[test]
    fn invoke_action_parses() {
        let rules: Rules = toml::from_str(OFFLINE).unwrap();
        assert_eq!(
            rules.rules[0].then[0],
            Action::Invoke {
                device: "esp32-08d1f9ca41e4".into(),
                entity: 5,
                channel: 0,
                value: Some("Alarm".into()),
            }
        );
    }

    #[test]
    fn offline_trigger_goes_alone() {
        assert_eq!(
            when(r#"{ offline = "*" }"#),
            Ok(Trigger::Offline(OfflineTrigger { device: None }))
        );
        assert!(
            when(r#"{ offline = "sim-bedroom", at = "07:00" }"#)
                .unwrap_err()
                .contains("goes alone")
        );
    }

    const IR_CODES: &str = r#"
        [[rules]]
        name = "Key 0 toggles the external LED"
        when = { device = "esp32-test", entity = 6, channel = 0, event = "Received", data = "NEC 00 52" }
        then = []

        [[rules]]
        name = "Any IR code"
        when = { device = "esp32-test", entity = 6, channel = 0, event = "Received" }
        then = []
    "#;

    fn received(data: Option<Value>) -> EventRecord {
        EventRecord {
            device_id: "esp32-test".into(),
            entity: 6,
            channel: 0,
            kind: EventKind::Received,
            data,
        }
    }

    #[test]
    fn event_data_must_match_exactly() {
        let rules: Rules = toml::from_str(IR_CODES).unwrap();
        let text = |code: &str| Some(Value::Text(code.into()));
        assert_eq!(
            names(rules.matching_without_remotes(&received(text("NEC 00 52")))),
            ["Key 0 toggles the external LED", "Any IR code"]
        );
        assert_eq!(
            names(rules.matching_without_remotes(&received(text("NEC 00 52 repeat")))),
            ["Any IR code"]
        );
        assert_eq!(
            names(rules.matching_without_remotes(&received(text("NEC 00 18")))),
            ["Any IR code"]
        );
        assert_eq!(
            names(rules.matching_without_remotes(&received(None))),
            ["Any IR code"]
        );
    }

    #[test]
    fn data_goes_with_event() {
        let error = |text: &str| when(text).unwrap_err();
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, above = 28, data = "x" }"#)
                .contains("`data` goes with `event`")
        );
        assert!(error(r#"{ offline = "*", data = "x" }"#).contains("goes alone"));
    }

    const THRESHOLDS: &str = r#"
        [[rules]]
        name = "Hot"
        when = { device = "sim-living-room", entity = 3, channel = 0, above = 28 }
        then = []

        [[rules]]
        name = "Cooled down"
        when = { device = "sim-living-room", entity = 3, channel = 0, below = 27 }
        then = []

        [[rules]]
        name = "Disabled"
        enabled = false
        when = { device = "sim-living-room", entity = 3, channel = 0, above = 28 }
        then = []
    "#;

    fn reading(value: Value, previous: Option<Value>) -> ChannelUpdate {
        ChannelUpdate {
            device_id: "sim-living-room".into(),
            entity: 3,
            channel: 0,
            value,
            confidence: Confidence::Confirmed,
            previous,
        }
    }

    fn crossed(update: ChannelUpdate) -> Vec<String> {
        let rules: Rules = toml::from_str(THRESHOLDS).unwrap();
        rules
            .crossed(&update)
            .into_iter()
            .map(|rule| rule.name.clone())
            .collect()
    }

    #[test]
    fn limits_run_when_the_value_crosses_them() {
        let float = Value::Float;
        assert_eq!(crossed(reading(float(29.0), Some(float(27.5)))), ["Hot"]);
        assert_eq!(
            crossed(reading(float(26.0), Some(float(27.5)))),
            ["Cooled down"]
        );
        assert_eq!(
            crossed(reading(float(28.0), Some(float(27.0)))),
            Vec::<String>::new()
        );
    }

    #[test]
    fn staying_past_the_limit_runs_nothing() {
        let float = Value::Float;
        assert!(crossed(reading(float(30.0), Some(float(29.0)))).is_empty());
        assert!(crossed(reading(float(25.0), Some(float(26.0)))).is_empty());
    }

    #[test]
    fn hysteresis_band_runs_nothing() {
        let float = Value::Float;
        assert!(crossed(reading(float(27.5), Some(float(28.5)))).is_empty());
        assert!(crossed(reading(float(27.5), Some(float(26.5)))).is_empty());
    }

    #[test]
    fn first_known_value_counts_as_crossing() {
        assert_eq!(crossed(reading(Value::Float(30.0), None)), ["Hot"]);
        assert_eq!(crossed(reading(Value::Int(20), None)), ["Cooled down"]);
    }

    #[test]
    fn assumed_and_non_numeric_values_run_nothing() {
        let mut assumed = reading(Value::Float(30.0), Some(Value::Float(20.0)));
        assumed.confidence = Confidence::Assumed;
        assert!(crossed(assumed).is_empty());
        assert!(crossed(reading(Value::Bool(true), None)).is_empty());
    }

    #[test]
    fn threshold_triggers_parse_and_reject_mixtures() {
        assert_eq!(
            when(r#"{ device = "sim-living-room", entity = 3, channel = 1, below = 40.5 }"#),
            Ok(Trigger::Threshold(ThresholdTrigger {
                device: "sim-living-room".into(),
                entity: 3,
                channel: 1,
                limit: Limit::Below(40.5),
            }))
        );
        let error = |text: &str| when(text).unwrap_err();
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, above = 28, below = 27 }"#)
                .contains("only one of")
        );
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0, above = 28, at = "07:00" }"#)
                .contains("cannot have")
        );
        assert!(error(r#"{ above = 28 }"#).contains("needs `device`"));
        assert!(
            error(r#"{ device = "a", entity = 3, channel = 0 }"#)
                .contains("needs `event`, `above` or `below`")
        );
    }

    #[test]
    fn rules_are_turned_off_and_on_in_the_file() {
        let text = r#"# Lights.
[[rules]]
name = "Lamp at seven"
when = { at = "07:00" }
then = [{ toggle = { device = "a", entity = 1, channel = 0 } }]

[[rules]]
name = "Other"
when = { at = "08:00" }
then = []
"#;
        let off = with_rule_enabled(text, "Lamp at seven", false).unwrap();
        assert!(off.starts_with(
            "# Lights.\n[[rules]]\nname = \"Lamp at seven\"\nenabled = false\nwhen = "
        ));
        let rules: Rules = toml::from_str(&off).unwrap();
        assert!(!rules.all()[0].enabled);
        assert!(rules.all()[1].enabled);
        assert_eq!(
            with_rule_enabled(&off, "Lamp at seven", true).unwrap(),
            text
        );
        assert!(
            with_rule_enabled(text, "Missing", false)
                .unwrap_err()
                .to_string()
                .contains("no rule named")
        );
    }

    const CONDITIONS: &str = r#"
        [[rules]]
        name = "Hot at night while someone is home"
        when = { remote = "Kit remote", key = "0" }
        if = [
            { device = "sim-living-room", entity = 3, channel = 0, above = 25 },
            { device = "board", entity = 1, channel = 0, equals = false },
            { from = "22:00", to = "06:00" },
            { days = ["friday", "saturday"] },
            { online = "phone" },
            { offline = "tv" },
        ]
        then = []
    "#;

    #[derive(Default)]
    struct Home {
        values: Vec<(&'static str, u8, u8, Value)>,
        online: Vec<&'static str>,
    }

    impl HomeState for Home {
        fn value(&self, device_id: &str, entity: u8, channel: u8) -> Option<Value> {
            self.values
                .iter()
                .find(|(device, known_entity, known_channel, _)| {
                    *device == device_id && *known_entity == entity && *known_channel == channel
                })
                .map(|(_, _, _, value)| value.clone())
        }

        fn is_online(&self, device_id: &str) -> bool {
            self.online.contains(&device_id)
        }
    }

    fn condition(text: &str) -> Result<Condition, String> {
        #[derive(Deserialize)]
        struct Wrapper {
            condition: Condition,
        }
        toml::from_str::<Wrapper>(&format!("condition = {text}"))
            .map(|wrapper| wrapper.condition)
            .map_err(|error| error.message().to_string())
    }

    #[test]
    fn conditions_parse() {
        let rules: Rules = toml::from_str(CONDITIONS).unwrap();
        let conditions = &rules.all()[0].conditions;
        assert_eq!(conditions.len(), 6);
        assert_eq!(
            conditions[1],
            Condition::Channel(ChannelCondition {
                device: "board".into(),
                entity: 1,
                channel: 0,
                test: ValueTest::Equals(Expected::Bool(false)),
            })
        );
        assert_eq!(
            conditions[2],
            Condition::Time {
                from: Time::constant(22, 0, 0, 0),
                to: Time::constant(6, 0, 0, 0),
            }
        );
        let rules: Rules = toml::from_str(EXAMPLE).unwrap();
        assert!(rules.all()[0].conditions.is_empty());
    }

    #[test]
    fn invalid_conditions_explain_what_is_wrong() {
        let error = |text: &str| condition(text).unwrap_err();
        assert!(error(r#"{ device = "a", entity = 1, channel = 0 }"#).contains("one of `equals`"));
        assert!(
            error(r#"{ device = "a", entity = 1, channel = 0, above = 1, below = 2 }"#)
                .contains("one of `equals`")
        );
        assert!(error(r#"{ device = "a", above = 1 }"#).contains("needs `device`"));
        assert!(error(r#"{ from = "22:00" }"#).contains("both `from` and `to`"));
        assert!(error(r#"{ from = "22:00", to = "22:00" }"#).contains("must differ"));
        assert!(error(r#"{ days = [] }"#).contains("at least one day"));
        assert!(error(r#"{ online = "a", days = ["monday"] }"#).contains("a condition is one of"));
        assert!(error("{}").contains("a condition is one of"));
        assert!(error(r#"{ at = "07:00" }"#).contains("unknown field"));
    }

    #[test]
    fn channel_conditions_compare_the_current_value() {
        let home = Home {
            values: vec![
                ("a", 1, 0, Value::Float(26.5)),
                ("a", 2, 0, Value::Bool(true)),
                ("a", 3, 0, Value::Enum("On".into())),
                ("a", 4, 0, Value::Int(3)),
            ],
            online: vec!["a"],
        };
        let holds = |text: &str| {
            condition(text)
                .unwrap()
                .holds(minute("2026-10-12T12:00"), &home)
        };
        assert!(holds(
            r#"{ device = "a", entity = 1, channel = 0, above = 25 }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 1, channel = 0, below = 25 }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 1, channel = 0, above = 26.5 }"#
        ));
        assert!(holds(
            r#"{ device = "a", entity = 2, channel = 0, equals = true }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 2, channel = 0, equals = false }"#
        ));
        assert!(holds(
            r#"{ device = "a", entity = 3, channel = 0, equals = "On" }"#
        ));
        assert!(holds(
            r#"{ device = "a", entity = 4, channel = 0, equals = 3 }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 4, channel = 0, equals = true }"#
        ));
        assert!(!holds(
            r#"{ device = "a", entity = 9, channel = 0, equals = true }"#
        ));
    }

    #[test]
    fn channel_conditions_do_not_hold_while_the_node_is_offline() {
        let home = Home {
            values: vec![("a", 2, 0, Value::Bool(true))],
            online: Vec::new(),
        };
        let condition =
            condition(r#"{ device = "a", entity = 2, channel = 0, equals = true }"#).unwrap();
        assert!(!condition.holds(minute("2026-10-12T12:00"), &home));
    }

    #[test]
    fn time_ranges_include_the_start_and_may_cross_midnight() {
        let home = Home::default();
        let night = condition(r#"{ from = "22:00", to = "06:00" }"#).unwrap();
        let day = condition(r#"{ from = "08:00", to = "18:00" }"#).unwrap();
        let at = |text: &str| minute(text);
        assert!(night.holds(at("2026-10-12T22:00"), &home));
        assert!(night.holds(at("2026-10-12T03:00"), &home));
        assert!(!night.holds(at("2026-10-12T06:00"), &home));
        assert!(!night.holds(at("2026-10-12T12:00"), &home));
        assert!(day.holds(at("2026-10-12T08:00"), &home));
        assert!(!day.holds(at("2026-10-12T18:00"), &home));
        assert!(!day.holds(at("2026-10-12T03:00"), &home));
    }

    #[test]
    fn day_and_presence_conditions() {
        let home = Home {
            values: Vec::new(),
            online: vec!["phone"],
        };
        let weekend = condition(r#"{ days = ["saturday", "sunday"] }"#).unwrap();
        // 2026-10-12 is a Monday, 2026-10-17 a Saturday.
        assert!(weekend.holds(minute("2026-10-17T12:00"), &home));
        assert!(!weekend.holds(minute("2026-10-12T12:00"), &home));
        let noon = minute("2026-10-12T12:00");
        assert!(
            condition(r#"{ online = "phone" }"#)
                .unwrap()
                .holds(noon, &home)
        );
        assert!(
            !condition(r#"{ online = "tv" }"#)
                .unwrap()
                .holds(noon, &home)
        );
        assert!(
            condition(r#"{ offline = "tv" }"#)
                .unwrap()
                .holds(noon, &home)
        );
        assert!(
            !condition(r#"{ offline = "phone" }"#)
                .unwrap()
                .holds(noon, &home)
        );
    }

    #[test]
    fn conditions_count_as_checking_the_node() {
        let rules: Rules = toml::from_str(CONDITIONS).unwrap();
        let roles = |device: &str| -> Vec<(RuleRole, Option<(u8, u8)>)> {
            rules
                .involving(device)
                .into_iter()
                .map(|use_| (use_.role, use_.channel))
                .collect()
        };
        assert_eq!(roles("sim-living-room"), [(RuleRole::Checks, Some((3, 0)))]);
        assert_eq!(roles("phone"), [(RuleRole::Checks, None)]);
        assert_eq!(roles("tv"), [(RuleRole::Checks, None)]);
    }
}
