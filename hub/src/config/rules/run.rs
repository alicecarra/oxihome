use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use anyhow::bail;
use jiff::{SignedDuration, Timestamp};
use oxihome_protocol::Value;
use serde::Deserialize;
use tokio::task::JoinHandle;

use super::{
    Condition, EventTrigger, HomeState, NoticeTrigger, OfflineTrigger, Rule, RuleRun, StateTrigger,
    TrafficTrigger, Trigger,
};
use crate::hub::{CommandOrigin, Hub};
use crate::node::events::{EventRecord, data_label};
use crate::node::state::ChannelUpdate;
use crate::traffic::AlertKind;

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
    /// Pauses before the next action.
    Wait(WaitTime),
}

/// `wait = "2s"`, `"500ms"`, `"1m 30s"`: more than zero, at most 30 minutes.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct WaitTime(SignedDuration);

const LONGEST_WAIT: SignedDuration = SignedDuration::from_mins(30);

impl TryFrom<String> for WaitTime {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let duration: SignedDuration = text
            .parse()
            .map_err(|error| format!("wait \"{text}\": {error}"))?;
        if duration <= SignedDuration::ZERO || duration > LONGEST_WAIT {
            return Err(format!(
                "wait \"{text}\": must be more than zero and at most 30 minutes"
            ));
        }
        Ok(Self(duration))
    }
}

impl fmt::Display for WaitTime {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "{:#}", self.0)
    }
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
            Action::Wait(time) => write!(formatter, "wait {time}"),
        }
    }
}

impl Action {
    /// The texts that may have placeholders: a notification's title and message, an invoke's
    /// value.
    pub fn texts(&self) -> Vec<&str> {
        match self {
            Action::Notify { title, message } => vec![title, message],
            Action::Invoke {
                value: Some(value), ..
            } => vec![value],
            _ => Vec::new(),
        }
    }
}

/// What a rule runs about, for `{node}`, `{channel}`, `{detail}`, `{value}`, `{title}` and
/// `{message}` in the text of its `notify` and `invoke` actions. Which ones a rule may use depends on its trigger.
#[derive(Debug, Clone, Default)]
pub struct Subject {
    pub node: Option<String>,
    pub channel: Option<String>,
    pub detail: Option<String>,
    pub value: Option<String>,
    /// A notice's.
    pub title: Option<String>,
    pub message: Option<String>,
    /// The node that went offline, for an offline rule with `for` to wait on.
    pub device_id: Option<String>,
}

/// Friendly names and values for a subject; the hub in practice.
pub trait Names {
    fn node_name(&self, device_id: &str) -> String;
    fn channel_name(&self, device_id: &str, entity: u8, channel: u8) -> String;
    /// With the channel's unit, e.g. `22.3 °C`.
    fn value_label(&self, device_id: &str, entity: u8, channel: u8, value: &Value) -> String;
}

impl Names for Hub {
    fn node_name(&self, device_id: &str) -> String {
        Hub::node_name(self, device_id)
    }

    fn channel_name(&self, device_id: &str, entity: u8, channel: u8) -> String {
        Hub::channel_name(self, device_id, entity, channel)
    }

    fn value_label(&self, device_id: &str, entity: u8, channel: u8, value: &Value) -> String {
        Hub::value_label(self, device_id, entity, channel, value)
    }
}

impl Subject {
    /// `{value}` is the event's data, e.g. `offline` or an IR code.
    pub fn of_event(event: &EventRecord, names: &impl Names) -> Self {
        Self {
            node: Some(names.node_name(&event.device_id)),
            channel: Some(names.channel_name(&event.device_id, event.entity, event.channel)),
            value: event.data.as_ref().map(data_label),
            ..Self::default()
        }
    }

    /// `{value}` is the new value, with its unit.
    pub fn of_update(update: &ChannelUpdate, names: &impl Names) -> Self {
        let ChannelUpdate {
            device_id,
            entity,
            channel,
            value,
            ..
        } = update;
        Self {
            node: Some(names.node_name(device_id)),
            channel: Some(names.channel_name(device_id, *entity, *channel)),
            value: Some(names.value_label(device_id, *entity, *channel, value)),
            ..Self::default()
        }
    }

    /// For a rule run from `/rules`: stand-ins that read well in a message, named after what the
    /// trigger names.
    pub fn by_hand(trigger: &Trigger, names: &impl Names) -> Self {
        let node = |device: &Option<String>| {
            Some(
                device
                    .as_deref()
                    .map_or_else(|| "any node".to_string(), |device| names.node_name(device)),
            )
        };
        match trigger {
            Trigger::Offline(OfflineTrigger { device, .. }) => Self {
                node: node(device),
                ..Self::default()
            },
            Trigger::Notice(NoticeTrigger { device }) => Self {
                node: node(device),
                title: Some("a notice".into()),
                message: Some("run by hand".into()),
                ..Self::default()
            },
            Trigger::Traffic(TrafficTrigger { kind, device }) => Self {
                node: node(device),
                channel: matches!(kind, AlertKind::Stopped | AlertKind::Noisy)
                    .then(|| "a channel".to_string()),
                detail: Some("run by hand".into()),
                ..Self::default()
            },
            Trigger::Event(EventTrigger {
                device,
                entity,
                channel,
                data,
                ..
            }) => Self {
                node: Some(names.node_name(device)),
                channel: Some(names.channel_name(device, *entity, *channel)),
                value: Some(data.clone().unwrap_or_else(|| "a value".into())),
                ..Self::default()
            },
            Trigger::State(StateTrigger {
                device,
                entity,
                channel,
                test,
                ..
            }) => Self {
                node: Some(names.node_name(device)),
                channel: Some(names.channel_name(device, *entity, *channel)),
                value: Some(names.value_label(device, *entity, *channel, &test.example())),
                ..Self::default()
            },
            _ => Self::default(),
        }
    }

    /// The text with each placeholder replaced; one without a value stays as written.
    pub fn fill(&self, text: &str) -> String {
        let Ok(pieces) = pieces(text) else {
            return text.to_string();
        };
        let mut filled = String::new();
        for piece in pieces {
            match piece {
                Piece::Literal(literal) => filled.push_str(literal),
                Piece::Placeholder(name) => match self.placeholder(name) {
                    Some(value) => filled.push_str(value),
                    None => filled.push_str(&format!("{{{name}}}")),
                },
            }
        }
        filled
    }

    fn placeholder(&self, name: &str) -> Option<&str> {
        match name {
            "node" => self.node.as_deref(),
            "channel" => self.channel.as_deref(),
            "detail" => self.detail.as_deref(),
            "value" => self.value.as_deref(),
            "title" => self.title.as_deref(),
            "message" => self.message.as_deref(),
            _ => None,
        }
    }
}

enum Piece<'a> {
    Literal(&'a str),
    Placeholder(&'a str),
}

/// Splits a text into literal parts and `{placeholder}` names; `{{` and `}}` are literal braces.
fn pieces(text: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['{', '}']) {
        pieces.push(Piece::Literal(&rest[..start]));
        let brace = &rest[start..];
        if brace.starts_with("{{") || brace.starts_with("}}") {
            pieces.push(Piece::Literal(&brace[..1]));
            rest = &brace[2..];
        } else if brace.starts_with('}') {
            return Err(format!("a lone `}}` in \"{text}\" (`}}}}` is a brace)"));
        } else {
            let Some(end) = brace.find('}') else {
                return Err(format!(
                    "a `{{` without `}}` in \"{text}\" (`{{{{` is a brace)"
                ));
            };
            pieces.push(Piece::Placeholder(&brace[1..end]));
            rest = &brace[end + 1..];
        }
    }
    pieces.push(Piece::Literal(rest));
    Ok(pieces)
}

pub fn has_placeholders(text: &str) -> bool {
    pieces(text).is_ok_and(|pieces| {
        pieces
            .iter()
            .any(|piece| matches!(piece, Piece::Placeholder(_)))
    })
}

/// Every placeholder in `text` is one of `available`, the ones the rule's trigger fills.
pub fn check_placeholders(text: &str, available: &[&str]) -> Result<(), String> {
    for piece in pieces(text)? {
        if let Piece::Placeholder(name) = piece
            && !available.contains(&name)
        {
            let offer = match available {
                [] => "this trigger has no placeholders".to_string(),
                _ => {
                    let names: Vec<String> =
                        available.iter().map(|name| format!("{{{name}}}")).collect();
                    format!("this trigger has {}", names.join(", "))
                }
            };
            return Err(format!("{{{name}}} in \"{text}\": {offer}"));
        }
    }
    Ok(())
}

/// How starting a rule went.
pub enum Start {
    /// The actions run in the background; the handle ends with them.
    Started(JoinHandle<()>),
    /// This condition did not hold.
    Skipped(Condition),
    /// The rule is still running about the same node and channel.
    Busy,
}

/// Starts a rule its trigger fired: one with `for` waits first, and a rule with a `cooldown`
/// does not start again that soon about the same node and channel.
pub fn run_triggered(hub: Arc<Hub>, rule: Rule, subject: Subject) {
    let wait = match &rule.when {
        Trigger::State(StateTrigger {
            device,
            lasting: Some(lasting),
            ..
        }) => Some((device.clone(), *lasting)),
        Trigger::Offline(OfflineTrigger {
            lasting: Some(lasting),
            ..
        }) => subject.device_id.clone().map(|device| (device, *lasting)),
        _ => None,
    };
    match wait {
        Some((device_id, lasting)) => hold(hub, lasting, rule, device_id, subject),
        None => start_unless_cooling_down(hub, rule, subject),
    }
}

/// Runs the rule after `lasting` unless what started the wait ends first (a value failing the
/// test, the node going offline or coming back), a new start replaces it, or the rule changes.
fn hold(hub: Arc<Hub>, lasting: SignedDuration, rule: Rule, device_id: String, subject: Subject) {
    let deadline = Timestamp::now() + lasting;
    let id = hub.holds().begin(&rule.name, &device_id, deadline);
    println!(
        "rule \"{}\": runs if {device_id} stays so for {lasting:#}",
        rule.name
    );
    tokio::spawn(async move {
        tokio::time::sleep(lasting.unsigned_abs()).await;
        if !hub.holds().finish(&rule.name, &device_id, id) {
            return;
        }
        let current = hub.rules().into_iter().find(|current| {
            current.name == rule.name && current.enabled && current.when == rule.when
        });
        let Some(current) = current else {
            return;
        };
        if !still_so(&current.when, &device_id, hub.as_ref()) {
            println!(
                "rule \"{}\": {device_id} is no longer so, not run",
                current.name
            );
            return;
        }
        start_unless_cooling_down(hub, current, subject);
    });
}

/// Whether what started the wait of a rule with `for` about the node still holds.
pub(super) fn still_so(trigger: &Trigger, device_id: &str, home: &impl HomeState) -> bool {
    match trigger {
        Trigger::State(trigger) => {
            home.is_online(device_id)
                && home
                    .value(device_id, trigger.entity, trigger.channel)
                    .is_some_and(|value| trigger.test.passes(&value))
        }
        Trigger::Offline(_) => !home.is_online(device_id),
        _ => false,
    }
}

fn start_unless_cooling_down(hub: Arc<Hub>, rule: Rule, subject: Subject) {
    let Some(cooldown) = rule.cooldown else {
        start(hub, rule, subject);
        return;
    };
    let key = RunKey::new(&rule, &subject);
    let now = Timestamp::now();
    if hub.cooldowns().cooling(&key, cooldown, now) {
        println!("rule \"{}\": skipped, cooling down", rule.name);
        return;
    }
    if let Start::Started(_) = start(hub.clone(), rule, subject) {
        hub.cooldowns().started(key, now);
    }
}

/// Starts the actions in order when every condition holds and the rule is not still running
/// about the same node and channel; a failed action does not stop the next ones.
pub fn start(hub: Arc<Hub>, rule: Rule, subject: Subject) -> Start {
    let Some(guard) = hub.running().claim(RunKey::new(&rule, &subject)) else {
        println!("rule \"{}\": still running, ignored", rule.name);
        return Start::Busy;
    };
    let at = Timestamp::now();
    let now = at.to_zoned(hub.time_zone().clone()).datetime();
    if let Some(condition) = rule
        .conditions
        .iter()
        .find(|condition| !condition.holds(now, hub.as_ref()))
    {
        println!("rule \"{}\": skipped, {condition} does not hold", rule.name);
        return Start::Skipped(condition.clone());
    }
    Start::Started(tokio::spawn(async move {
        let _guard = guard;
        let mut errors = Vec::new();
        for action in &rule.then {
            match perform(&hub, action, &rule.name, &subject).await {
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
    }))
}

/// Rule runs that have not finished, by rule and what they run about.
#[derive(Default, Clone)]
pub struct Running(Arc<Mutex<HashSet<RunKey>>>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RunKey {
    rule: String,
    node: Option<String>,
    channel: Option<String>,
}

impl RunKey {
    fn new(rule: &Rule, subject: &Subject) -> Self {
        Self {
            rule: rule.name.clone(),
            node: subject.node.clone(),
            channel: subject.channel.clone(),
        }
    }
}

/// Rules waiting out their `for`, by rule and node; a new wait replaces the old one.
#[derive(Default)]
pub struct Holds(Mutex<Waits>);

#[derive(Default)]
struct Waits {
    last_id: u64,
    by_key: HashMap<(String, String), Wait>,
}

struct Wait {
    id: u64,
    deadline: Timestamp,
}

/// A rule waiting out its `for` about a node.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub rule: String,
    pub device_id: String,
    pub deadline: Timestamp,
}

impl Holds {
    fn begin(&self, rule: &str, device_id: &str, deadline: Timestamp) -> u64 {
        let mut waits = self.0.lock().unwrap();
        waits.last_id += 1;
        let id = waits.last_id;
        waits.by_key.insert(
            (rule.to_string(), device_id.to_string()),
            Wait { id, deadline },
        );
        id
    }

    /// Whether this wait was still on; it is over either way.
    fn finish(&self, rule: &str, device_id: &str, id: u64) -> bool {
        let mut waits = self.0.lock().unwrap();
        let key = (rule.to_string(), device_id.to_string());
        let current = waits.by_key.get(&key).is_some_and(|wait| wait.id == id);
        if current {
            waits.by_key.remove(&key);
        }
        current
    }

    pub fn end(&self, rule: &str, device_id: &str) {
        let key = (rule.to_string(), device_id.to_string());
        if self.0.lock().unwrap().by_key.remove(&key).is_some() {
            println!("rule \"{rule}\": {device_id} did not stay so, not run");
        }
    }

    /// Every wait, soonest first.
    pub fn pending(&self) -> Vec<Pending> {
        let mut pending: Vec<Pending> = self
            .0
            .lock()
            .unwrap()
            .by_key
            .iter()
            .map(|((rule, device_id), wait)| Pending {
                rule: rule.clone(),
                device_id: device_id.clone(),
                deadline: wait.deadline,
            })
            .collect();
        pending.sort_by_key(|wait| wait.deadline);
        pending
    }

    #[cfg(test)]
    pub fn includes(&self, rule: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .by_key
            .keys()
            .any(|(waiting, _)| waiting == rule)
    }
}

/// When each rule with a `cooldown` last started, by rule and what it ran about.
#[derive(Default)]
pub struct Cooldowns(Mutex<HashMap<RunKey, Timestamp>>);

impl Cooldowns {
    fn cooling(&self, key: &RunKey, cooldown: SignedDuration, now: Timestamp) -> bool {
        self.0
            .lock()
            .unwrap()
            .get(key)
            .is_some_and(|started| now.duration_since(*started) < cooldown)
    }

    fn started(&self, key: RunKey, now: Timestamp) {
        self.0.lock().unwrap().insert(key, now);
    }
}

impl Running {
    /// `None` while the same run has not finished.
    fn claim(&self, key: RunKey) -> Option<RunningGuard> {
        self.0
            .lock()
            .unwrap()
            .insert(key.clone())
            .then(|| RunningGuard {
                running: self.clone(),
                key,
            })
    }

    pub fn includes(&self, rule: &str) -> bool {
        self.0.lock().unwrap().iter().any(|key| key.rule == rule)
    }
}

/// Ends the run when dropped, also when its task panics.
struct RunningGuard {
    running: Running,
    key: RunKey,
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.running.0.lock().unwrap().remove(&self.key);
    }
}

/// Returns what happened, for the log.
async fn perform(
    hub: &Hub,
    action: &Action,
    rule_name: &str,
    subject: &Subject,
) -> anyhow::Result<String> {
    let (device, entity, channel, value) = match action {
        Action::Wait(WaitTime(duration)) => {
            tokio::time::sleep(duration.unsigned_abs()).await;
            return Ok("done".into());
        }
        Action::Notify { title, message } => {
            hub.notify(
                subject.fill(title),
                subject.fill(message),
                rule_name.to_string(),
            );
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
                value.as_deref().map(|value| subject.fill(value)),
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

#[cfg(test)]
mod tests {
    use oxihome_protocol::EventKind;

    use super::*;
    use crate::config::rules::when;

    struct Board;

    impl Names for Board {
        fn node_name(&self, device_id: &str) -> String {
            format!("Board {device_id}")
        }

        fn channel_name(&self, _: &str, entity: u8, channel: u8) -> String {
            format!("Sensor {entity}.{channel}")
        }

        fn value_label(&self, _: &str, _: u8, _: u8, value: &Value) -> String {
            match value {
                Value::Float(number) => format!("{number:.1} km/h"),
                other => format!("{other:?}"),
            }
        }
    }

    #[test]
    fn wait_takes_a_short_friendly_duration() {
        let wait = |text: &str| WaitTime::try_from(text.to_string());
        assert_eq!(wait("2s").unwrap().to_string(), "2s");
        assert_eq!(wait("500ms").unwrap().to_string(), "500ms");
        assert_eq!(wait("1m 30s").unwrap().to_string(), "1m 30s");
        assert!(wait("30m").is_ok());
        assert!(wait("31m").unwrap_err().contains("at most 30 minutes"));
        assert!(wait("0s").unwrap_err().contains("more than zero"));
        assert!(wait("soon").unwrap_err().starts_with("wait \"soon\""));
        let actions: Vec<Action> = toml::from_str::<toml::Table>(r#"then = [{ wait = "2s" }]"#)
            .unwrap()
            .remove("then")
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(actions[0].to_string(), "wait 2s");
    }

    #[test]
    fn a_run_holds_its_rule_node_and_channel() {
        let running = Running::default();
        let key = |node: &str| RunKey {
            rule: "R".into(),
            node: Some(node.into()),
            channel: None,
        };
        let first = running.claim(key("a")).unwrap();
        assert!(running.claim(key("a")).is_none());
        assert!(running.claim(key("b")).is_some());
        assert!(running.includes("R"));
        drop(first);
        assert!(!running.includes("R"));
        assert!(running.claim(key("a")).is_some());
    }

    #[test]
    fn subject_fills_its_placeholders() {
        let subject = Subject {
            node: Some("Test board".into()),
            ..Subject::default()
        };
        assert_eq!(
            subject.fill("{node} › {channel} stopped"),
            "Test board › {channel} stopped"
        );
        assert_eq!(Subject::default().fill("{node}"), "{node}");
        assert_eq!(subject.fill("{{node}} is {node}"), "{node} is Test board");
        assert_eq!(subject.fill("a lone } stays"), "a lone } stays");
    }

    #[test]
    fn events_and_updates_fill_value() {
        let event = EventRecord {
            device_id: "a".into(),
            entity: 1,
            channel: 1,
            kind: EventKind::Received,
            data: Some(Value::Text("offline".into())),
        };
        assert_eq!(
            Subject::of_event(&event, &Board).fill("{node} › {channel}: {value}"),
            "Board a › Sensor 1.1: offline"
        );
        let update = ChannelUpdate {
            device_id: "weather".into(),
            entity: 1,
            channel: 3,
            value: Value::Float(52.25),
            confidence: oxihome_protocol::Confidence::Confirmed,
            previous: None,
        };
        assert_eq!(
            Subject::of_update(&update, &Board).fill("Wind at {value}"),
            "Wind at 52.2 km/h"
        );
    }

    #[test]
    fn a_rule_run_by_hand_reads_well() {
        let filled = |trigger: &str| {
            Subject::by_hand(&when(trigger).unwrap(), &Board)
                .fill("{node} › {channel}: {detail} {value}")
        };
        assert_eq!(
            filled(r#"{ stopped = "*" }"#),
            "any node › a channel: run by hand {value}"
        );
        assert_eq!(
            filled(r#"{ unanswered = "a" }"#),
            "Board a › {channel}: run by hand {value}"
        );
        assert_eq!(
            filled(r#"{ offline = "a" }"#),
            "Board a › {channel}: {detail} {value}"
        );
        assert_eq!(
            filled(r#"{ device = "w", entity = 1, channel = 3, above = 50 }"#),
            "Board w › Sensor 1.3: {detail} 50.0 km/h"
        );
        assert_eq!(
            filled(
                r#"{ device = "n", entity = 1, channel = 1, event = "Received", data = "online" }"#
            ),
            "Board n › Sensor 1.1: {detail} online"
        );
    }

    #[test]
    fn placeholders_are_checked_against_the_trigger() {
        let check = |text: &str| check_placeholders(text, &["node", "detail"]);
        assert!(check("{node}: {detail}").is_ok());
        assert!(check("{{value}} is literal").is_ok());
        assert_eq!(
            check("{value}").unwrap_err(),
            "{value} in \"{value}\": this trigger has {node}, {detail}"
        );
        assert_eq!(
            check_placeholders("at {node}", &[]).unwrap_err(),
            "{node} in \"at {node}\": this trigger has no placeholders"
        );
        assert!(check("{node").unwrap_err().contains("without `}`"));
        assert!(check("node}").unwrap_err().contains("a lone `}`"));
        assert!(has_placeholders("{node} went offline"));
        assert!(!has_placeholders("Alarm"));
        assert!(!has_placeholders("{{literal}}"));
    }
}
