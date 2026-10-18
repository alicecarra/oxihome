mod condition;
mod editing;
mod run;
mod trigger;

use std::collections::HashSet;

use anyhow::bail;
use jiff::civil::DateTime;
use jiff::{SignedDuration, Timestamp};
use oxihome_protocol::{Confidence, EventKind, Value};
use serde::{Deserialize, Deserializer};

pub use condition::{ChannelCondition, Condition, Expected, HomeState, ValueTest};
pub use editing::with_rule_enabled;
pub use run::{
    Action, Cooldowns, Holds, Running, Start, Subject, has_placeholders, run_triggered, start,
};
pub use trigger::{
    CronTrigger, Day, EventTrigger, Limit, OfflineTrigger, ScheduleTrigger, ThresholdTrigger,
    TrafficTrigger, Trigger,
};

use super::remotes::Remotes;
use crate::node::events::EventRecord;
use crate::node::state::ChannelUpdate;
use crate::traffic::Alert;

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
    /// `cooldown = "1h"`: after the rule starts, triggers about the same node and channel are
    /// ignored this long. Runs from `/rules` neither wait for it nor start it.
    #[serde(default, deserialize_with = "cooldown")]
    pub cooldown: Option<SignedDuration>,
}

const LONGEST_COOLDOWN: SignedDuration = SignedDuration::from_hours(7 * 24);

fn cooldown<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SignedDuration>, D::Error> {
    let text = String::deserialize(deserializer)?;
    let duration: SignedDuration = text
        .parse()
        .map_err(|error| serde::de::Error::custom(format!("cooldown \"{text}\": {error}")))?;
    if duration <= SignedDuration::ZERO || duration > LONGEST_COOLDOWN {
        return Err(serde::de::Error::custom(format!(
            "cooldown \"{text}\": must be more than zero and at most 168h (7 days)"
        )));
    }
    Ok(Some(duration))
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
    /// buttons and schedules are not tied to a node, so only their actions count.
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
                Trigger::Offline(OfflineTrigger { device })
                | Trigger::Traffic(TrafficTrigger { device, .. })
                    if device.as_deref().is_none_or(|device| device == device_id) =>
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

    /// The rule a button runs.
    pub fn button(&self, button: &str) -> Option<&Rule> {
        self.rules
            .iter()
            .find(|rule| matches!(&rule.when, Trigger::Button(trigger) if trigger.button == button))
    }

    /// Button names are unique.
    /// Rule runs, the timeline and `set_rule_enabled` find rules by name.
    pub fn check_names(&self) -> anyhow::Result<()> {
        let mut names = HashSet::new();
        for rule in &self.rules {
            if !names.insert(&rule.name) {
                bail!("two rules are named \"{}\"", rule.name);
            }
        }
        Ok(())
    }

    pub fn check_buttons(&self) -> anyhow::Result<()> {
        let mut buttons = HashSet::new();
        for rule in &self.rules {
            let Trigger::Button(trigger) = &rule.when else {
                continue;
            };
            if !buttons.insert(&trigger.button) {
                bail!(
                    "rule \"{}\": another rule has the button \"{}\"",
                    rule.name,
                    trigger.button
                );
            }
            if rule.cooldown.is_some() {
                bail!(
                    "rule \"{}\": a button rule cannot have a cooldown",
                    rule.name
                );
            }
        }
        Ok(())
    }

    /// Every placeholder in a rule's texts is one its trigger fills.
    pub fn check_placeholders(&self) -> anyhow::Result<()> {
        for rule in &self.rules {
            for text in rule.then.iter().flat_map(Action::texts) {
                if let Err(error) = run::check_placeholders(text, rule.when.placeholders()) {
                    bail!("rule \"{}\": {error}", rule.name);
                }
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

    /// Threshold rules with `for` whose wait the update ends: a confirmed number back within the
    /// limit.
    pub fn released(&self, update: &ChannelUpdate) -> Vec<&str> {
        if update.confidence != Confidence::Confirmed {
            return Vec::new();
        }
        let Some(value) = number(&update.value) else {
            return Vec::new();
        };
        self.rules
            .iter()
            .filter(|rule| {
                matches!(&rule.when, Trigger::Threshold(trigger)
                    if trigger.lasting.is_some()
                        && trigger.device == update.device_id
                        && trigger.entity == update.entity
                        && trigger.channel == update.channel
                        && !trigger.limit.passed(value))
            })
            .map(|rule| rule.name.as_str())
            .collect()
    }

    /// Threshold rules with `for` on the node's channels; its going offline ends their wait.
    pub fn waiting_on(&self, device_id: &str) -> Vec<&str> {
        self.rules
            .iter()
            .filter(|rule| {
                matches!(&rule.when, Trigger::Threshold(trigger)
                    if trigger.lasting.is_some() && trigger.device == device_id)
            })
            .map(|rule| rule.name.as_str())
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

    pub fn traffic(&self, alert: &Alert) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.enabled
                    && matches!(&rule.when, Trigger::Traffic(trigger) if trigger.matches(alert))
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

pub(super) fn number(value: &Value) -> Option<f32> {
    match value {
        Value::Int(number) => Some(*number as f32),
        Value::Float(number) => Some(*number),
        _ => None,
    }
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
fn minute(text: &str) -> DateTime {
    text.parse().unwrap()
}

#[cfg(test)]
fn when(text: &str) -> Result<Trigger, String> {
    #[derive(Deserialize)]
    struct Wrapper {
        when: Trigger,
    }
    toml::from_str::<Wrapper>(&format!("when = {text}"))
        .map(|wrapper| wrapper.when)
        .map_err(|error| error.message().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Zoned;
    use jiff::civil::Time;

    use crate::traffic::AlertKind;

    impl Rules {
        fn matching_without_remotes(&self, event: &EventRecord) -> Vec<&Rule> {
            self.matching(event, &Remotes::default())
        }
    }

    #[test]
    fn buttons_are_unique() {
        let rules: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "Lights off"
            when = { button = "Good night" }
            then = []
        "#,
        )
        .unwrap();
        assert_eq!(rules.button("Good night").unwrap().name, "Lights off");
        assert!(rules.button("Alarm").is_none());
        assert!(rules.check_buttons().is_ok());
        let twice: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "A"
            when = { button = "Good night" }
            then = []

            [[rules]]
            name = "B"
            when = { button = "Good night" }
            then = []
        "#,
        )
        .unwrap();
        let error = twice.check_buttons().unwrap_err().to_string();
        assert!(error.contains("another rule has the button"), "{error}");
    }

    #[test]
    fn cooldowns_parse_and_buttons_reject_them() {
        let rules = |text: &str| toml::from_str::<Rules>(text);
        let hourly = rules(
            r#"
            [[rules]]
            name = "A"
            when = { offline = "*" }
            cooldown = "1h"
            then = []
            "#,
        )
        .unwrap();
        assert_eq!(
            hourly.all()[0].cooldown,
            Some(SignedDuration::from_hours(1))
        );
        let error = rules(
            r#"
            [[rules]]
            name = "A"
            when = { offline = "*" }
            cooldown = "200h"
            then = []
            "#,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("at most 168h"), "{error}");
        let button = rules(
            r#"
            [[rules]]
            name = "A"
            when = { button = "Go" }
            cooldown = "1m"
            then = []
            "#,
        )
        .unwrap();
        let error = button.check_buttons().unwrap_err().to_string();
        assert!(error.contains("cannot have a cooldown"), "{error}");
    }

    #[test]
    fn rule_names_are_unique() {
        let rules: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "A"
            when = { button = "Good night" }
            then = []

            [[rules]]
            name = "B"
            when = { button = "Alarm" }
            then = []
        "#,
        )
        .unwrap();
        assert!(rules.check_names().is_ok());
        let twice: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "A"
            when = { button = "Good night" }
            then = []

            [[rules]]
            name = "A"
            when = { button = "Alarm" }
            then = []
        "#,
        )
        .unwrap();
        let error = twice.check_names().unwrap_err().to_string();
        assert_eq!(error, "two rules are named \"A\"");
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
        let wake_up = &rules.rules[1].when;
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

    const LASTING: &str = r#"
        [[rules]]
        name = "Hot for a while"
        when = { device = "sim-living-room", entity = 3, channel = 0, above = 28, for = "5m" }
        then = []

        [[rules]]
        name = "Hot"
        when = { device = "sim-living-room", entity = 3, channel = 0, above = 28 }
        then = []
    "#;

    #[test]
    fn only_a_confirmed_value_back_within_the_limit_ends_a_wait() {
        let rules: Rules = toml::from_str(LASTING).unwrap();
        let float = Value::Float;
        assert_eq!(
            rules.released(&reading(float(27.0), Some(float(29.0)))),
            ["Hot for a while"]
        );
        assert!(
            rules
                .released(&reading(float(29.5), Some(float(29.0))))
                .is_empty()
        );
        let mut assumed = reading(float(27.0), Some(float(29.0)));
        assumed.confidence = Confidence::Assumed;
        assert!(rules.released(&assumed).is_empty());
        assert_eq!(rules.waiting_on("sim-living-room"), ["Hot for a while"]);
        assert!(rules.waiting_on("board").is_empty());
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

    #[test]
    fn placeholders_must_be_filled_by_the_trigger() {
        let check = |when: &str, message: &str| {
            let text = format!(
                "[[rules]]\nname = \"R\"\nwhen = {when}\nthen = [{{ notify = {{ title = \"T\", message = \"{message}\" }} }}]"
            );
            let rules: Rules = toml::from_str(&text).unwrap();
            rules
                .check_placeholders()
                .map_err(|error| error.to_string())
        };
        let threshold = r#"{ device = "w", entity = 1, channel = 3, above = 50 }"#;
        assert!(check(threshold, "{node} › {channel}: {value}").is_ok());
        assert!(check(r#"{ offline = "*" }"#, "{node} went offline").is_ok());
        assert!(check(r#"{ stopped = "*" }"#, "{node} › {channel}: {detail}").is_ok());
        let received = r#"{ device = "n", entity = 1, channel = 1, event = "Received" }"#;
        assert!(check(received, "{value}").is_ok());
        let error = check(
            r#"{ device = "b", entity = 2, channel = 0, event = "Press" }"#,
            "{value}",
        )
        .unwrap_err();
        assert!(error.starts_with("rule \"R\": {value}"), "{error}");
        assert!(check(r#"{ unanswered = "*" }"#, "{channel}").is_err());
        assert!(
            check(r#"{ at = "07:00" }"#, "{node}")
                .unwrap_err()
                .contains("no placeholders")
        );
        assert!(check(r#"{ at = "07:00" }"#, "{{node}}").is_ok());
        assert!(check(threshold, "{nde}").is_err());
    }

    #[test]
    fn traffic_rules_match_kind_and_node() {
        let rules: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "Any stopped"
            when = { stopped = "*" }
            then = []

            [[rules]]
            name = "Board noisy"
            when = { noisy = "esp32-test" }
            then = []
            "#,
        )
        .unwrap();
        let alert = |kind, device_id: &str| Alert {
            kind,
            device_id: device_id.into(),
            channel: Some((3, 0)),
        };
        assert_eq!(
            names(rules.traffic(&alert(AlertKind::Stopped, "other"))),
            ["Any stopped"]
        );
        assert_eq!(
            names(rules.traffic(&alert(AlertKind::Noisy, "esp32-test"))),
            ["Board noisy"]
        );
        assert!(rules.traffic(&alert(AlertKind::Noisy, "other")).is_empty());
        assert_eq!(
            rules.involving("esp32-test").len(),
            2,
            "both rules run on the board"
        );
    }
}
