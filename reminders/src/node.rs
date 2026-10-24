use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan, Zoned};
use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    EventKind, Message, NoticeLevel, Unit, Value, ValueType,
};
use serde::{Deserialize, Serialize};

use crate::schedule::{Quiet, Schedule};

/// The reminders use the numbers after it.
pub const ALL_REMINDERS_ENTITY: u8 = 1;
const PENDING_COUNT_CHANNEL: u8 = 0;
const LIST_PENDING_CHANNEL: u8 = 2;
const REACTION_CHANNEL: u8 = 3;

const PENDING_CHANNEL: u8 = 0;
const DONE_CHANNEL: u8 = 1;
const LAST_DONE_CHANNEL: u8 = 2;
const NEXT_DUE_CHANNEL: u8 = 3;
const DUE_CHANNEL: u8 = 4;
const SNOOZE_CHANNEL: u8 = 5;

/// Telegram's message limit, the usual source of the text.
const REACTION_MAX_LENGTH: u16 = 4096;
/// A reaction starting with it snoozes for an hour; any other marks done.
const SNOOZE_EMOJI: &str = "😴";
const REACTION_SNOOZE: &str = "1h";
const SNOOZE_OPTIONS: [&str; 4] = ["15m", "1h", "3h", "tomorrow"];
/// The first notice says "Due at" until this long after the due time, "Pending since" after.
const JUST_DUE: SignedDuration = SignedDuration::from_mins(5);
const LIST_KEY: &str = "pending";
/// Done this close before a fixed time counts for that time.
const EARLY_DONE: SignedDuration = SignedDuration::from_hours(2);

pub struct Reminder {
    pub entity: u8,
    pub name: String,
    pub schedule: Schedule,
    /// `None` sends it once.
    pub repeat: Option<SignedDuration>,
}

/// What survives a restart, in the state file.
#[derive(Default, Serialize, Deserialize)]
pub struct SavedState {
    pub reminders: BTreeMap<u8, SavedReminder>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedReminder {
    /// The next due time is counted from here: the last done, or the first start.
    since: Timestamp,
    last_done: Option<Timestamp>,
    pending_since: Option<Timestamp>,
    /// The last time it went out while pending.
    last_sent: Option<Timestamp>,
    /// Pending but quiet until then.
    #[serde(default)]
    snoozed_until: Option<Timestamp>,
}

pub struct RemindersNode {
    core: NodeCore,
    device: RemindersDevice,
    started_at: Instant,
    /// `None` keeps nothing, for tests.
    state_path: Option<PathBuf>,
}

struct RemindersDevice {
    reminders: Vec<ReminderState>,
    zone: TimeZone,
    quiet: Option<Quiet>,
    now: Timestamp,
    /// Entities a command changed, to publish and save.
    changed: Vec<u8>,
    /// Notices a command produced.
    notices: Vec<Notice>,
}

struct Notice {
    level: NoticeLevel,
    title: String,
    message: String,
    key: String,
}

struct ReminderState {
    reminder: Reminder,
    saved: SavedReminder,
}

impl RemindersNode {
    pub fn new(
        device_id: String,
        reminders: Vec<Reminder>,
        zone: TimeZone,
        quiet: Option<Quiet>,
        mut saved: SavedState,
        state_path: Option<PathBuf>,
        now: Timestamp,
    ) -> Self {
        let reminders = reminders
            .into_iter()
            .map(|reminder| ReminderState {
                saved: saved
                    .reminders
                    .remove(&reminder.entity)
                    .unwrap_or(SavedReminder {
                        since: now,
                        last_done: None,
                        pending_since: None,
                        last_sent: None,
                        snoozed_until: None,
                    }),
                reminder,
            })
            .collect();
        let node = Self {
            core: NodeCore::new(
                device_id,
                Build::native("reminders", env!("CARGO_PKG_VERSION")),
            ),
            device: RemindersDevice {
                reminders,
                zone,
                quiet,
                now,
                changed: Vec::new(),
                notices: Vec::new(),
            },
            started_at: Instant::now(),
            state_path,
        };
        node.save();
        node
    }

    /// Marks what fell due and sends what is pending, unless snoozed or in the quiet hours.
    pub fn tick(&mut self, now: Timestamp) -> Vec<Message> {
        self.device.now = now;
        let zone = self.device.zone.clone();
        let quiet = self
            .device
            .quiet
            .is_some_and(|quiet| quiet.contains(now.to_zoned(zone.clone()).time()));
        let mut became_due = Vec::new();
        let mut woke = Vec::new();
        let mut notices = Vec::new();
        let mut changed = false;
        for state in &mut self.device.reminders {
            let saved = &mut state.saved;
            if saved.pending_since.is_none()
                && let Some(due) = state
                    .reminder
                    .schedule
                    .next_due(&saved.since.to_zoned(zone.clone()))
                && due.timestamp() <= now
            {
                saved.pending_since = Some(due.timestamp());
                became_due.push(state.reminder.entity);
                changed = true;
            }
            let Some(pending_since) = saved.pending_since else {
                continue;
            };
            match saved.snoozed_until {
                Some(until) if now < until => continue,
                Some(_) => {
                    saved.snoozed_until = None;
                    saved.last_sent = None;
                    woke.push(state.reminder.entity);
                    changed = true;
                }
                None => {}
            }
            let repeat_due = match (saved.last_sent, state.reminder.repeat) {
                (None, _) => true,
                (Some(sent), Some(repeat)) => {
                    sent.checked_add(repeat).is_ok_and(|next| next <= now)
                }
                (Some(_), None) => false,
            };
            if repeat_due && !quiet {
                let at = moment(pending_since, now, &zone);
                let message = if now.duration_since(pending_since) < JUST_DUE {
                    format!("Due at {at}")
                } else {
                    format!("Pending since {at}")
                };
                saved.last_sent = Some(now);
                notices.push(Notice {
                    level: NoticeLevel::Warning,
                    title: state.reminder.name.clone(),
                    message,
                    key: state.reminder.entity.to_string(),
                });
                changed = true;
            }
        }

        let mut messages = Vec::new();
        for entity in &became_due {
            println!("{} is due", self.device.name(*entity));
            messages.extend(self.core.state(&self.device, *entity));
            messages.push(
                self.core
                    .event(*entity, DUE_CHANNEL, EventKind::Press, None),
            );
        }
        for entity in &woke {
            messages.extend(self.core.state(&self.device, *entity));
        }
        if !became_due.is_empty() {
            messages.extend(self.core.state(&self.device, ALL_REMINDERS_ENTITY));
        }
        for notice in notices {
            messages.push(self.notice(notice));
        }
        if changed {
            self.save();
        }
        messages
    }

    /// Like [`SoftwareNode::handle`] at a given time.
    pub fn handle_at(&mut self, message: &Message, now: Timestamp) -> Vec<Message> {
        self.device.now = now;
        let mut answers = self.core.handle(&mut self.device, message);
        let changed = std::mem::take(&mut self.device.changed);
        for entity in &changed {
            answers.extend(self.core.state(&self.device, *entity));
        }
        if !changed.is_empty() {
            answers.extend(self.core.state(&self.device, ALL_REMINDERS_ENTITY));
            self.save();
        }
        for notice in std::mem::take(&mut self.device.notices) {
            answers.push(self.notice(notice));
        }
        answers
    }

    fn notice(&mut self, notice: Notice) -> Message {
        self.core.notice(
            notice.level,
            &notice.title,
            &notice.message,
            Some(&notice.key),
        )
    }

    fn save(&self) {
        let Some(path) = &self.state_path else {
            return;
        };
        let state = SavedState {
            reminders: self
                .device
                .reminders
                .iter()
                .map(|state| (state.reminder.entity, state.saved.clone()))
                .collect(),
        };
        if let Err(error) = write_state(path, &state) {
            println!("saving {} failed: {error:#}", path.display());
        }
    }
}

impl SoftwareNode for RemindersNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.handle_at(message, Timestamp::now())
    }

    fn invalid_message(&mut self, detail: String) -> Vec<Message> {
        vec![self.core.invalid_message(detail)]
    }

    fn announcement(&mut self) -> Vec<Message> {
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        vec![self.core.heartbeat(uptime_s, None, None)]
    }
}

impl RemindersDevice {
    fn name(&self, entity: u8) -> &str {
        self.reminders
            .iter()
            .find(|state| state.reminder.entity == entity)
            .map_or("", |state| state.reminder.name.as_str())
    }

    fn pending(&self) -> impl Iterator<Item = &ReminderState> {
        self.reminders
            .iter()
            .filter(|state| state.saved.pending_since.is_some())
    }

    /// Before a fixed time, done counts for that time when it is close.
    fn done(&mut self, entity: u8) {
        let now = self.now;
        let zone = self.zone.clone();
        let Some(state) = self
            .reminders
            .iter_mut()
            .find(|state| state.reminder.entity == entity)
        else {
            return;
        };
        let saved = &mut state.saved;
        let early = saved.pending_since.is_none() && state.reminder.schedule.is_cron();
        saved.since = state
            .reminder
            .schedule
            .next_due(&saved.since.to_zoned(zone))
            .map(|due| due.timestamp())
            .filter(|due| early && *due > now && due.duration_since(now) <= EARLY_DONE)
            .unwrap_or(now);
        saved.last_done = Some(now);
        saved.pending_since = None;
        saved.last_sent = None;
        saved.snoozed_until = None;
        println!("{} done", state.reminder.name);
        self.changed.push(entity);
    }

    /// A reaction to a message naming exactly one reminder: snooze for 😴, done otherwise.
    /// Anything else is ignored, so a reaction to another message is no error.
    fn reaction(&mut self, text: &str) {
        let named: Vec<u8> = self
            .reminders
            .iter()
            .filter(|state| text.contains(&state.reminder.name))
            .map(|state| state.reminder.entity)
            .collect();
        let [entity] = named.as_slice() else {
            match named.len() {
                0 => println!("no reminder named in \"{text}\""),
                _ => println!("more than one reminder named in \"{text}\", none changed"),
            }
            return;
        };
        if !text.starts_with(SNOOZE_EMOJI) {
            self.done(*entity);
        } else if let Err(rejection) = self.snooze(*entity, REACTION_SNOOZE) {
            println!("{}", rejection.detail);
        }
    }

    /// Only a pending reminder; done or the end of the snooze sends it again.
    fn snooze(&mut self, entity: u8, option: &str) -> Result<(), Rejection> {
        let now = self.now;
        let zone = self.zone.clone();
        let state = self
            .reminders
            .iter_mut()
            .find(|state| state.reminder.entity == entity)
            .ok_or_else(|| {
                Rejection::new(ErrorCode::UnknownEntity, format!("unknown entity {entity}"))
            })?;
        if state.saved.pending_since.is_none() {
            return Err(Rejection::new(
                ErrorCode::InvalidOperation,
                format!("{} is not pending", state.reminder.name),
            ));
        }
        let until = match option {
            "15m" => now.checked_add(SignedDuration::from_mins(15)),
            "1h" => now.checked_add(SignedDuration::from_hours(1)),
            "3h" => now.checked_add(SignedDuration::from_hours(3)),
            "tomorrow" => now
                .to_zoned(zone)
                .checked_add(1.day())
                .map(|tomorrow| tomorrow.timestamp()),
            other => {
                return Err(Rejection::new(
                    ErrorCode::InvalidValue,
                    format!("snooze option {other}"),
                ));
            }
        }
        .map_err(|error| Rejection::new(ErrorCode::InvalidValue, error.to_string()))?;
        state.saved.snoozed_until = Some(until);
        println!("{} snoozed until {until}", state.reminder.name);
        self.changed.push(entity);
        Ok(())
    }

    fn list_pending(&mut self) {
        let names: Vec<&str> = self
            .pending()
            .map(|state| state.reminder.name.as_str())
            .collect();
        let message = if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        };
        self.notices.push(Notice {
            level: NoticeLevel::Info,
            title: "Pending reminders".into(),
            message,
            key: LIST_KEY.into(),
        });
    }
}

impl Device for RemindersDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let channel = |id, access, value| Channel {
            id,
            name: None,
            access,
            value,
            quantity: None,
            unit: None,
        };
        let moment = |id| Channel {
            unit: Some(Unit::UnixTime),
            ..channel(
                id,
                Access::Read,
                Some(ValueType::Int {
                    min: 0,
                    max: i32::MAX,
                    step: 1,
                }),
            )
        };
        let mut entities = vec![
            EntityDescriptor {
                id: ALL_REMINDERS_ENTITY,
                name: None,
                channels: vec![
                    channel(
                        PENDING_COUNT_CHANNEL,
                        Access::Read,
                        Some(ValueType::Int {
                            min: 0,
                            max: i32::from(u8::MAX),
                            step: 1,
                        }),
                    )
                    .named("Pending"),
                    channel(LIST_PENDING_CHANNEL, Access::Trigger, None).named("List pending"),
                    channel(
                        REACTION_CHANNEL,
                        Access::Trigger,
                        Some(ValueType::Text {
                            max_length: REACTION_MAX_LENGTH,
                        }),
                    )
                    .named("Reaction"),
                ],
            }
            .named("All reminders"),
        ];
        entities.extend(self.reminders.iter().map(|state| {
            EntityDescriptor {
                id: state.reminder.entity,
                name: None,
                channels: vec![
                    channel(PENDING_CHANNEL, Access::Read, Some(ValueType::Bool)).named("Pending"),
                    channel(DONE_CHANNEL, Access::Trigger, None).named("Done"),
                    moment(LAST_DONE_CHANNEL).named("Last done"),
                    moment(NEXT_DUE_CHANNEL).named("Next due"),
                    channel(DUE_CHANNEL, Access::Event, None).named("Due"),
                    channel(
                        SNOOZE_CHANNEL,
                        Access::Trigger,
                        Some(ValueType::Enum(
                            SNOOZE_OPTIONS
                                .iter()
                                .map(|option| option.to_string())
                                .collect(),
                        )),
                    )
                    .named("Snooze"),
                ],
            }
            .named(&state.reminder.name)
        }));
        entities
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        if entity == ALL_REMINDERS_ENTITY {
            let count = i32::try_from(self.pending().count()).unwrap_or(i32::MAX);
            return Some(vec![confirmed(PENDING_COUNT_CHANNEL, Value::Int(count))]);
        }
        let state = self
            .reminders
            .iter()
            .find(|state| state.reminder.entity == entity)?;
        let saved = &state.saved;
        let mut values = vec![confirmed(
            PENDING_CHANNEL,
            Value::Bool(saved.pending_since.is_some()),
        )];
        if let Some(last_done) = saved.last_done {
            values.push(confirmed(LAST_DONE_CHANNEL, unix_time(last_done)));
        }
        let next_due = saved.snoozed_until.or(saved.pending_since).or_else(|| {
            state
                .reminder
                .schedule
                .next_due(&saved.since.to_zoned(self.zone.clone()))
                .map(|due| due.timestamp())
        });
        if let Some(next_due) = next_due {
            values.push(confirmed(NEXT_DUE_CHANNEL, unix_time(next_due)));
        }
        Some(values)
    }

    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp) -> Result<(), Rejection> {
        match (entity, channel, op) {
            (ALL_REMINDERS_ENTITY, LIST_PENDING_CHANNEL, _) => self.list_pending(),
            (
                ALL_REMINDERS_ENTITY,
                REACTION_CHANNEL,
                CommandOp::Invoke(Some(Value::Text(text))),
            ) => self.reaction(text),
            (_, DONE_CHANNEL, _) if entity != ALL_REMINDERS_ENTITY => self.done(entity),
            (_, SNOOZE_CHANNEL, CommandOp::Invoke(Some(Value::Enum(option))))
                if entity != ALL_REMINDERS_ENTITY =>
            {
                self.snooze(entity, option)?
            }
            _ => {
                return Err(Rejection::new(
                    ErrorCode::InvalidOperation,
                    format!("entity {entity} channel {channel} cannot be invoked"),
                ));
            }
        }
        Ok(())
    }
}

/// `08:30` today, `22/10 08:30` on another day.
fn moment(at: Timestamp, now: Timestamp, zone: &TimeZone) -> String {
    let at: Zoned = at.to_zoned(zone.clone());
    if at.date() == now.to_zoned(zone.clone()).date() {
        at.strftime("%H:%M").to_string()
    } else {
        at.strftime("%d/%m %H:%M").to_string()
    }
}

fn unix_time(at: Timestamp) -> Value {
    Value::Int(i32::try_from(at.as_second()).unwrap_or(i32::MAX))
}

/// Missing or unreadable: every reminder starts counting again, quietly.
pub fn read_state(path: &Path) -> SavedState {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            println!("{} is not valid, starting over: {error}", path.display());
            SavedState::default()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SavedState::default(),
        Err(error) => {
            println!("reading {} failed, starting over: {error}", path.display());
            SavedState::default()
        }
    }
}

/// Through a temporary file, so a crash never leaves half a file.
fn write_state(path: &Path, state: &SavedState) -> anyhow::Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}

#[cfg(test)]
mod tests;
