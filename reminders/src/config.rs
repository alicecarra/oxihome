use std::collections::HashSet;

use jiff::SignedDuration;
use jiff::civil::Time;
use jiff::tz::TimeZone;
use oxihome_node::config::BrokerConfig;
use oxihome_protocol::NAME_MAX_LENGTH;
use serde::Deserialize;

use crate::node::{ALL_REMINDERS_ENTITY, Reminder};
use crate::schedule::{Quiet, Schedule};

const DEFAULT_REPEAT: SignedDuration = SignedDuration::from_mins(30);
const SHORTEST_REPEAT: SignedDuration = SignedDuration::from_mins(5);

#[derive(Deserialize)]
pub struct Config {
    pub mqtt: BrokerConfig,
    pub node: NodeConfig,
    pub reminders: Vec<ReminderConfig>,
}

#[derive(Deserialize)]
pub struct NodeConfig {
    pub id: String,
    pub zone: String,
    /// A duration or "off"; 30 minutes when missing.
    pub repeat: Option<String>,
    pub quiet: Option<QuietConfig>,
}

#[derive(Deserialize)]
pub struct QuietConfig {
    pub from: Time,
    pub to: Time,
}

#[derive(Deserialize)]
pub struct ReminderConfig {
    pub entity: u8,
    pub name: String,
    pub cron: Option<String>,
    pub every: Option<String>,
    pub at: Option<Time>,
    /// The node's when missing.
    pub repeat: Option<String>,
}

pub struct Settings {
    pub zone: TimeZone,
    pub quiet: Option<Quiet>,
    pub reminders: Vec<Reminder>,
}

impl NodeConfig {
    pub fn settings(&self, reminders: Vec<ReminderConfig>) -> Result<Settings, String> {
        let zone = TimeZone::get(&self.zone).map_err(|error| format!("zone: {error}"))?;
        let quiet = match &self.quiet {
            Some(quiet) if quiet.from == quiet.to => {
                return Err("quiet: from and to are the same time".into());
            }
            Some(quiet) => Some(Quiet {
                from: quiet.from,
                to: quiet.to,
            }),
            None => None,
        };
        let default_repeat = match &self.repeat {
            Some(text) => repeat(text).map_err(|error| format!("node: {error}"))?,
            None => Some(DEFAULT_REPEAT),
        };

        let mut entities = HashSet::new();
        let mut checked = Vec::new();
        for config in reminders {
            let name = config.name.clone();
            let reminder = config
                .reminder(default_repeat)
                .map_err(|error| format!("reminder \"{name}\": {error}"))?;
            if !entities.insert(reminder.entity) {
                return Err(format!("entity {} used twice", reminder.entity));
            }
            checked.push(reminder);
        }
        for reminder in &checked {
            if let Some(other) = checked.iter().find(|other| {
                other.entity != reminder.entity && other.name.contains(&reminder.name)
            }) {
                return Err(format!(
                    "reminder \"{}\": its name is inside \"{}\", so a text could not tell them apart",
                    reminder.name, other.name
                ));
            }
        }
        Ok(Settings {
            zone,
            quiet,
            reminders: checked,
        })
    }
}

impl ReminderConfig {
    fn reminder(self, default_repeat: Option<SignedDuration>) -> Result<Reminder, String> {
        if self.entity <= ALL_REMINDERS_ENTITY {
            return Err(format!("entity must be above {ALL_REMINDERS_ENTITY}"));
        }
        if self.name.trim().is_empty() || self.name.len() > NAME_MAX_LENGTH {
            return Err(format!("name of 1 to {NAME_MAX_LENGTH} bytes"));
        }
        let schedule = match (&self.cron, &self.every) {
            (Some(_), Some(_)) | (None, None) => {
                return Err("exactly one of cron or every".into());
            }
            (Some(_), None) if self.at.is_some() => {
                return Err("at goes with every; put the time in the cron".into());
            }
            (Some(cron), None) => Schedule::cron(cron)?,
            (None, Some(every)) => Schedule::every(every, self.at)?,
        };
        let repeat = match &self.repeat {
            Some(text) => repeat(text)?,
            None => default_repeat,
        };
        Ok(Reminder {
            entity: self.entity,
            name: self.name,
            schedule,
            repeat,
        })
    }
}

fn repeat(text: &str) -> Result<Option<SignedDuration>, String> {
    if text == "off" {
        return Ok(None);
    }
    let duration: SignedDuration = text
        .parse()
        .map_err(|error| format!("repeat \"{text}\": {error}"))?;
    if duration < SHORTEST_REPEAT {
        return Err(format!("repeat \"{text}\": at least 5 minutes, or \"off\""));
    }
    Ok(Some(duration))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(text: &str) -> Result<Settings, String> {
        let config: Config = toml::from_str(text).unwrap();
        config.node.settings(config.reminders)
    }

    const HEAD: &str = r#"
        [mqtt]
        host = "localhost"
        port = 1883

        [node]
        id = "reminders"
        zone = "America/Sao_Paulo"
    "#;

    fn with(reminders: &str) -> Result<Settings, String> {
        settings(&format!("{HEAD}\n{reminders}"))
    }

    #[test]
    fn reads_both_kinds_of_schedule_and_the_repeats() {
        let settings = settings(&format!(
            "{HEAD}repeat = \"1h\"\nquiet = {{ from = \"22:00\", to = \"08:00\" }}\n{}",
            r#"
            [[reminders]]
            entity = 2
            name = "Morning medicine"
            cron = "30 8 * * *"
            repeat = "15m"

            [[reminders]]
            entity = 3
            name = "Water the plants"
            every = "3d"
            at = "09:00"

            [[reminders]]
            entity = 4
            name = "Pay the rent"
            cron = "0 9 10 * *"
            repeat = "off"
            "#
        ))
        .unwrap();
        assert!(settings.quiet.is_some());
        let repeats: Vec<_> = settings.reminders.iter().map(|r| r.repeat).collect();
        assert_eq!(
            repeats,
            [
                Some(SignedDuration::from_mins(15)),
                Some(SignedDuration::from_hours(1)),
                None
            ]
        );
    }

    #[test]
    fn repeat_defaults_to_half_an_hour() {
        let settings = with("[[reminders]]\nentity = 2\nname = \"A\"\nevery = \"1d\"").unwrap();
        assert_eq!(settings.reminders[0].repeat, Some(DEFAULT_REPEAT));
    }

    #[test]
    fn refuses_what_would_not_work() {
        let error = |reminders: &str| with(reminders).err().unwrap();
        assert!(
            error("[[reminders]]\nentity = 1\nname = \"A\"\nevery = \"1d\"").contains("above 1")
        );
        assert!(error("[[reminders]]\nentity = 2\nname = \"A\"").contains("exactly one"));
        assert!(
            error("[[reminders]]\nentity = 2\nname = \"A\"\nevery = \"1d\"\ncron = \"0 8 * * *\"")
                .contains("exactly one")
        );
        assert!(
            error("[[reminders]]\nentity = 2\nname = \"A\"\ncron = \"0 8 * * *\"\nat = \"08:00\"")
                .contains("at goes with every")
        );
        assert!(
            error("[[reminders]]\nentity = 2\nname = \"A\"\nevery = \"1d\"\nrepeat = \"1m\"")
                .contains("at least 5 minutes")
        );
        assert!(
            error("[[reminders]]\nentity = 2\nname = \"A\"\nevery = \"1d\"\n[[reminders]]\nentity = 2\nname = \"B\"\nevery = \"1d\"")
                .contains("used twice")
        );
        assert!(
            error("[[reminders]]\nentity = 2\nname = \"Medicine\"\nevery = \"1d\"\n[[reminders]]\nentity = 3\nname = \"Medicine at night\"\nevery = \"1d\"")
                .contains("inside")
        );
        assert!(
            settings(&format!(
                "{}\n[[reminders]]\nentity = 2\nname = \"A\"\nevery = \"1d\"",
                HEAD.replace("America/Sao_Paulo", "Mars/Olympus")
            ))
            .err()
            .unwrap()
            .starts_with("zone")
        );
    }
}
