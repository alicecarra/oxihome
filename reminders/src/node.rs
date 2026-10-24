use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, Zoned};
use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    EventKind, Message, Unit, Value, ValueType,
};
use serde::{Deserialize, Serialize};

use crate::schedule::{Quiet, Schedule};

/// The reminders use the numbers after it.
pub const ALL_REMINDERS_ENTITY: u8 = 1;
const PENDING_COUNT_CHANNEL: u8 = 0;
const REMINDER_CHANNEL: u8 = 1;
const LIST_PENDING_CHANNEL: u8 = 2;
const DONE_BY_TEXT_CHANNEL: u8 = 3;

const PENDING_CHANNEL: u8 = 0;
const DONE_CHANNEL: u8 = 1;
const LAST_DONE_CHANNEL: u8 = 2;
const NEXT_DUE_CHANNEL: u8 = 3;
const DUE_CHANNEL: u8 = 4;

const REMINDER_MAX_LENGTH: u16 = 1024;
/// Telegram's message limit, the usual source of the text.
const DONE_BY_TEXT_MAX_LENGTH: u16 = 4096;
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
    /// Texts a command produced for the reminder channel.
    texts: Vec<String>,
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
                texts: Vec::new(),
            },
            started_at: Instant::now(),
            state_path,
        };
        node.save();
        node
    }

    /// Marks what fell due and sends what is pending, unless in the quiet hours.
    pub fn tick(&mut self, now: Timestamp) -> Vec<Message> {
        self.device.now = now;
        let zone = self.device.zone.clone();
        let quiet = self
            .device
            .quiet
            .is_some_and(|quiet| quiet.contains(now.to_zoned(zone.clone()).time()));
        let mut became_due = Vec::new();
        let mut texts = Vec::new();
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
            let repeat_due = match (saved.last_sent, state.reminder.repeat) {
                (None, _) => true,
                (Some(sent), Some(repeat)) => {
                    sent.checked_add(repeat).is_ok_and(|next| next <= now)
                }
                (Some(_), None) => false,
            };
            if repeat_due && !quiet {
                let text = if saved.last_sent.is_none() {
                    state.reminder.name.clone()
                } else {
                    format!(
                        "{} (pending since {})",
                        state.reminder.name,
                        moment(pending_since, now, &zone)
                    )
                };
                saved.last_sent = Some(now);
                texts.push(text);
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
        if !became_due.is_empty() {
            messages.extend(self.core.state(&self.device, ALL_REMINDERS_ENTITY));
        }
        for text in texts {
            messages.push(self.reminder_text(text));
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
        for text in std::mem::take(&mut self.device.texts) {
            answers.push(self.reminder_text(text));
        }
        answers
    }

    fn reminder_text(&mut self, text: String) -> Message {
        self.core.event(
            ALL_REMINDERS_ENTITY,
            REMINDER_CHANNEL,
            EventKind::Received,
            Some(Value::Text(truncate(&text, REMINDER_MAX_LENGTH))),
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
        println!("{} done", state.reminder.name);
        self.changed.push(entity);
    }

    /// The one reminder whose name is in the text; anything else is ignored.
    fn done_by_text(&mut self, text: &str) {
        let named: Vec<u8> = self
            .reminders
            .iter()
            .filter(|state| text.contains(&state.reminder.name))
            .map(|state| state.reminder.entity)
            .collect();
        match named.as_slice() {
            [entity] => self.done(*entity),
            [] => println!("no reminder named in \"{text}\""),
            _ => println!("more than one reminder named in \"{text}\", none done"),
        }
    }

    fn list_pending(&mut self) {
        let names: Vec<&str> = self
            .pending()
            .map(|state| state.reminder.name.as_str())
            .collect();
        let text = if names.is_empty() {
            "Nothing pending".to_string()
        } else {
            format!("Pending: {}", names.join(", "))
        };
        self.texts.push(text);
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
                    channel(
                        REMINDER_CHANNEL,
                        Access::Event,
                        Some(ValueType::Text {
                            max_length: REMINDER_MAX_LENGTH,
                        }),
                    )
                    .named("Reminder"),
                    channel(LIST_PENDING_CHANNEL, Access::Trigger, None).named("List pending"),
                    channel(
                        DONE_BY_TEXT_CHANNEL,
                        Access::Trigger,
                        Some(ValueType::Text {
                            max_length: DONE_BY_TEXT_MAX_LENGTH,
                        }),
                    )
                    .named("Done by text"),
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
        let next_due = saved.pending_since.or_else(|| {
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
                DONE_BY_TEXT_CHANNEL,
                CommandOp::Invoke(Some(Value::Text(text))),
            ) => self.done_by_text(text),
            (_, DONE_CHANNEL, _) if entity != ALL_REMINDERS_ENTITY => self.done(entity),
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

/// At most `max_length` bytes, cut at a character boundary.
fn truncate(text: &str, max_length: u16) -> String {
    let mut end = text.len().min(max_length as usize);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
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
