//! Messages per minute over the last hour, by node and by channel, and what stands out.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use jiff::Timestamp;

use super::{Direction, MessageKind, Reading, TrafficEntry};
use crate::commands::TIMEOUT as ANSWER_TIMEOUT;

const MINUTES: usize = 60;
const HOUR: Duration = Duration::from_secs(60 * 60);
/// What the rates on the page are averaged over.
const RECENT: Duration = Duration::from_secs(5 * 60);
/// `State` times kept per channel to find its rhythm.
const STATE_TIMES: usize = 20;

const NOISY_FACTOR: f64 = 3.0;
const NOISY_MINIMUM_PER_MINUTE: f64 = 6.0;
const NOISY_ALWAYS_PER_MINUTE: f64 = 60.0;
/// Minutes of the recent window at `NOISY_MINIMUM_PER_MINUTE` or more: holding a remote key or
/// turning the potentiometer for a few seconds is not noise.
const NOISY_MINUTES: usize = 3;

const STOPPED_FACTOR: u32 = 3;
const STOPPED_MINIMUM: Duration = Duration::from_secs(2 * 60);
const STOPPED_MINIMUM_STATES: usize = 5;
/// A channel is regular when its longer gaps (upper quartile) are at most this many times its
/// shorter ones (lower quartile): a potentiometer that only reports changes is not.
const REGULAR_SPREAD: u32 = 2;

/// A node whose median answer takes this long is slow.
const SLOW: Duration = Duration::from_secs(1);

/// Dropped messages are counted after the message kinds.
const KIND_SLOTS: usize = MessageKind::ALL.len() + 1;
const DROPPED_SLOT: usize = MessageKind::ALL.len();

fn minute_of(at: Timestamp) -> i64 {
    at.as_second().div_euclid(60)
}

/// One `T` per minute of the last hour, in slots reused as the minutes go by.
#[derive(Debug, Clone)]
struct Minutes<T> {
    slots: Vec<(i64, T)>,
}

impl<T: Default + Clone> Default for Minutes<T> {
    fn default() -> Self {
        Self {
            slots: vec![(i64::MIN, T::default()); MINUTES],
        }
    }
}

impl<T: Default + Clone> Minutes<T> {
    fn at(&mut self, minute: i64) -> &mut T {
        let slot = &mut self.slots[minute.rem_euclid(MINUTES as i64) as usize];
        if slot.0 != minute {
            *slot = (minute, T::default());
        }
        &mut slot.1
    }

    /// The hour up to `minute`, oldest first; minutes without messages are empty.
    fn series(&self, minute: i64) -> Vec<T> {
        (minute - MINUTES as i64 + 1..=minute)
            .map(|wanted| {
                let slot = &self.slots[wanted.rem_euclid(MINUTES as i64) as usize];
                if slot.0 == wanted {
                    slot.1.clone()
                } else {
                    T::default()
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
struct NodeMinute {
    up: u32,
    down: u32,
    bytes: u64,
    largest: usize,
    kinds: [u32; KIND_SLOTS],
}

#[derive(Debug, Default)]
struct ChannelCounters {
    minutes: Minutes<u32>,
    last_at: Option<Timestamp>,
    /// Newest last.
    states: VecDeque<Timestamp>,
}

#[derive(Debug, Default)]
struct NodeCounters {
    minutes: Minutes<NodeMinute>,
    last_at: Option<Timestamp>,
    /// The last message from the node itself.
    last_up_at: Option<Timestamp>,
    channels: BTreeMap<(u8, u8), ChannelCounters>,
    /// Commands sent, by message id, until answered or given up on.
    waiting: HashMap<u32, Timestamp>,
    /// When each answer came and how long it took, over the last hour.
    answers: VecDeque<(Timestamp, Duration)>,
    /// When each command given up on was sent, over the last hour.
    unanswered: VecDeque<Timestamp>,
}

impl NodeCounters {
    /// Commands past the hub's wait count as unanswered; an answer after that is ignored, like
    /// the hub does.
    fn give_up(&mut self, now: Timestamp) {
        let late: Vec<u32> = self
            .waiting
            .iter()
            .filter(|(_, sent)| elapsed(**sent, now) > ANSWER_TIMEOUT)
            .map(|(message_id, _)| *message_id)
            .collect();
        for message_id in late {
            if let Some(sent) = self.waiting.remove(&message_id) {
                self.unanswered.push_back(sent);
            }
        }
        self.unanswered.retain(|sent| elapsed(*sent, now) <= HOUR);
        self.answers.retain(|(at, _)| elapsed(*at, now) <= HOUR);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Highlight {
    /// Well above its own average, or plainly too much.
    Noisy,
    /// Reported regularly, then went quiet.
    Stopped { typical: Duration },
}

#[derive(Debug)]
pub struct ChannelSummary {
    pub entity: u8,
    pub channel: u8,
    pub per_minute: f64,
    pub last_at: Option<Timestamp>,
    /// The usual time between its `State` messages.
    pub typical: Option<Duration>,
    pub series: Vec<u32>,
    pub highlight: Option<Highlight>,
}

/// How fast a node answered commands in the last hour.
#[derive(Debug, PartialEq)]
pub struct Answers {
    pub count: usize,
    pub median: Duration,
    pub worst: Duration,
    pub slow: bool,
}

#[derive(Debug)]
pub struct NodeSummary {
    /// `None` for broadcasts.
    pub device_id: Option<String>,
    pub up_per_minute: f64,
    pub down_per_minute: f64,
    pub bytes_per_minute: f64,
    pub last_at: Option<Timestamp>,
    /// Over the last hour, the kinds seen; `None` stands for dropped messages.
    pub kinds: Vec<(Option<MessageKind>, u32)>,
    pub dropped: u32,
    /// Messages per minute, both ways, oldest first.
    pub series: Vec<u32>,
    pub channels: Vec<ChannelSummary>,
    /// `None` without answers in the last hour.
    pub answers: Option<Answers>,
    /// Commands without an answer in the last hour, the ones still waiting past the hub's
    /// patience included.
    pub unanswered: usize,
    /// The largest message of the last hour, in bytes.
    pub largest: usize,
}

pub struct Counters {
    started: Timestamp,
    nodes: BTreeMap<Option<String>, NodeCounters>,
}

/// How the last minutes compare with the ones before.
struct Window {
    /// Of the recent window, in minutes, from `started` on.
    recent_minutes: f64,
    /// Of the rest of the hour before it.
    earlier_minutes: f64,
    /// Whole minutes in the recent window, the current one included.
    recent_slots: usize,
}

impl Window {
    fn new(started: Timestamp, now: Timestamp) -> Self {
        let counted = elapsed(started, now);
        let into_minute = Duration::from_secs(now.as_second().rem_euclid(60) as u64);
        let recent_slots = (RECENT.as_secs() / 60) as usize;
        let recent = (RECENT - Duration::from_secs(60) + into_minute).min(counted);
        let hour = Duration::from_secs((MINUTES as u64 - 1) * 60) + into_minute;
        let earlier = hour.min(counted).saturating_sub(recent);
        Self {
            recent_minutes: recent.as_secs_f64() / 60.0,
            earlier_minutes: earlier.as_secs_f64() / 60.0,
            recent_slots,
        }
    }

    fn rate(minutes: f64, count: u64) -> f64 {
        if minutes > 0.0 {
            count as f64 / minutes
        } else {
            0.0
        }
    }

    /// The slots of the recent window.
    fn recent<'a>(&self, series: &'a [u64]) -> &'a [u64] {
        &series[series.len() - self.recent_slots..]
    }

    fn recent_rate(&self, series: &[u64]) -> f64 {
        Self::rate(self.recent_minutes, self.recent(series).iter().sum())
    }

    fn earlier_rate(&self, series: &[u64]) -> Option<f64> {
        (self.earlier_minutes >= 1.0).then(|| {
            let earlier: u64 = series[..series.len() - self.recent_slots].iter().sum();
            Self::rate(self.earlier_minutes, earlier)
        })
    }
}

fn elapsed(from: Timestamp, to: Timestamp) -> Duration {
    Duration::try_from(to.duration_since(from)).unwrap_or_default()
}

impl Counters {
    pub fn new(started: Timestamp) -> Self {
        Self {
            started,
            nodes: BTreeMap::new(),
        }
    }

    pub fn started(&self) -> Timestamp {
        self.started
    }

    pub fn record(&mut self, entry: &TrafficEntry) {
        let minute = minute_of(entry.at);
        let node = self.nodes.entry(entry.device_id.clone()).or_default();
        node.last_at = Some(entry.at);
        node.give_up(entry.at);
        let tally = node.minutes.at(minute);
        match entry.direction {
            Direction::Up => {
                tally.up += 1;
                node.last_up_at = Some(entry.at);
            }
            Direction::Down => tally.down += 1,
        }
        tally.bytes += entry.text.len() as u64;
        tally.largest = tally.largest.max(entry.text.len());
        let slot = match entry.kind() {
            Some(kind) => MessageKind::ALL
                .iter()
                .position(|own| *own == kind)
                .unwrap(),
            None => DROPPED_SLOT,
        };
        tally.kinds[slot] += 1;
        if let Reading::Read {
            kind,
            message_id,
            reply_to,
            ..
        } = &entry.reading
        {
            match (entry.direction, kind, reply_to) {
                (Direction::Down, MessageKind::Command, _) => {
                    node.waiting.insert(*message_id, entry.at);
                }
                (Direction::Up, MessageKind::Ack | MessageKind::Error, Some(reply_to)) => {
                    if let Some(sent) = node.waiting.remove(reply_to) {
                        node.answers.push_back((entry.at, elapsed(sent, entry.at)));
                    }
                }
                _ => {}
            }
        }
        if let Reading::Read {
            kind,
            entity: Some(entity),
            channels,
            ..
        } = &entry.reading
        {
            for channel in channels {
                let counters = node.channels.entry((*entity, *channel)).or_default();
                *counters.minutes.at(minute) += 1;
                counters.last_at = Some(entry.at);
                if *kind == MessageKind::State {
                    if counters.states.len() == STATE_TIMES {
                        counters.states.pop_front();
                    }
                    counters.states.push_back(entry.at);
                }
            }
        }
    }

    pub fn summary(&self, now: Timestamp) -> Vec<NodeSummary> {
        let minute = minute_of(now);
        let window = Window::new(self.started, now);
        self.nodes
            .iter()
            .map(|(device_id, node)| {
                let series = node.minutes.series(minute);
                let both: Vec<u64> = series
                    .iter()
                    .map(|tally| u64::from(tally.up + tally.down))
                    .collect();
                let field = |pick: fn(&NodeMinute) -> u64| -> Vec<u64> {
                    series.iter().map(pick).collect()
                };
                let mut kinds = [0u32; KIND_SLOTS];
                for tally in &series {
                    for (total, count) in kinds.iter_mut().zip(tally.kinds) {
                        *total += count;
                    }
                }
                NodeSummary {
                    device_id: device_id.clone(),
                    up_per_minute: window.recent_rate(&field(|tally| tally.up.into())),
                    down_per_minute: window.recent_rate(&field(|tally| tally.down.into())),
                    bytes_per_minute: window.recent_rate(&field(|tally| tally.bytes)),
                    last_at: node.last_at,
                    kinds: MessageKind::ALL
                        .iter()
                        .map(|kind| Some(*kind))
                        .chain([None])
                        .zip(kinds)
                        .filter(|(_, count)| *count > 0)
                        .collect(),
                    dropped: kinds[DROPPED_SLOT],
                    series: both.iter().map(|count| *count as u32).collect(),
                    answers: answers(&node.answers, now),
                    unanswered: node
                        .unanswered
                        .iter()
                        .filter(|sent| elapsed(**sent, now) <= HOUR)
                        .count()
                        + node
                            .waiting
                            .values()
                            .filter(|sent| elapsed(**sent, now) > ANSWER_TIMEOUT)
                            .count(),
                    largest: series.iter().map(|tally| tally.largest).max().unwrap_or(0),
                    channels: node
                        .channels
                        .iter()
                        .map(|(&(entity, channel), counters)| {
                            channel_summary(
                                (entity, channel),
                                counters,
                                node.last_up_at,
                                &window,
                                minute,
                                now,
                            )
                        })
                        .collect(),
                }
            })
            .collect()
    }
}

/// `node_last_up_at` tells a quiet channel from a quiet node: when the whole node went silent,
/// presence (offline) covers it.
fn channel_summary(
    (entity, channel): (u8, u8),
    counters: &ChannelCounters,
    node_last_up_at: Option<Timestamp>,
    window: &Window,
    minute: i64,
    now: Timestamp,
) -> ChannelSummary {
    let series = counters.minutes.series(minute);
    let counts: Vec<u64> = series.iter().map(|count| u64::from(*count)).collect();
    let per_minute = window.recent_rate(&counts);
    let typical = typical_gap(&counters.states);
    let busy_minutes = window
        .recent(&counts)
        .iter()
        .filter(|count| **count as f64 >= NOISY_MINIMUM_PER_MINUTE)
        .count();
    let noisy = busy_minutes >= NOISY_MINUTES
        && (per_minute >= NOISY_ALWAYS_PER_MINUTE
            || window.earlier_rate(&counts).is_some_and(|earlier| {
                per_minute >= NOISY_MINIMUM_PER_MINUTE && per_minute >= NOISY_FACTOR * earlier
            }));
    let stopped = match (typical, counters.states.back()) {
        (Some(typical), Some(last))
            if counters.states.len() >= STOPPED_MINIMUM_STATES && regular(&counters.states) =>
        {
            let limit = (typical * STOPPED_FACTOR).max(STOPPED_MINIMUM);
            elapsed(*last, now) > limit
                && node_last_up_at.is_some_and(|up| elapsed(up, now) <= limit)
        }
        _ => false,
    };
    ChannelSummary {
        entity,
        channel,
        per_minute,
        last_at: counters.last_at,
        typical,
        series,
        highlight: if noisy {
            Some(Highlight::Noisy)
        } else if stopped {
            typical.map(|typical| Highlight::Stopped { typical })
        } else {
            None
        },
    }
}

fn answers(answers: &VecDeque<(Timestamp, Duration)>, now: Timestamp) -> Option<Answers> {
    let mut took: Vec<Duration> = answers
        .iter()
        .filter(|(at, _)| elapsed(*at, now) <= HOUR)
        .map(|(_, took)| *took)
        .collect();
    took.sort();
    let median = *took.get(took.len() / 2)?;
    Some(Answers {
        count: took.len(),
        median,
        worst: *took.last()?,
        slow: median >= SLOW,
    })
}

/// `times` cover at least `STOPPED_MINIMUM`, so a short even burst is not a rhythm, and their gaps
/// are even (`REGULAR_SPREAD`).
fn regular(times: &VecDeque<Timestamp>) -> bool {
    let (Some(first), Some(last)) = (times.front(), times.back()) else {
        return false;
    };
    if elapsed(*first, *last) < STOPPED_MINIMUM {
        return false;
    }
    let mut gaps = gaps(times);
    gaps.sort();
    let lower = gaps[gaps.len() / 4];
    let upper = gaps[gaps.len() * 3 / 4];
    upper <= lower * REGULAR_SPREAD
}

fn gaps(times: &VecDeque<Timestamp>) -> Vec<Duration> {
    times
        .iter()
        .zip(times.iter().skip(1))
        .map(|(earlier, later)| elapsed(*earlier, *later))
        .collect()
}

/// The median time between `times`.
fn typical_gap(times: &VecDeque<Timestamp>) -> Option<Duration> {
    let mut gaps = gaps(times);
    if gaps.is_empty() {
        return None;
    }
    gaps.sort();
    Some(gaps[gaps.len() / 2])
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: i64 = 1_800_000_000;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(START + seconds).unwrap()
    }

    fn entry(seconds: i64, kind: MessageKind, channel: Option<u8>) -> TrafficEntry {
        TrafficEntry {
            at: at(seconds),
            direction: Direction::Up,
            topic: "oxihome/a/up".into(),
            device_id: Some("a".into()),
            reading: Reading::Read {
                kind,
                entity: channel.map(|_| 1),
                channels: channel.into_iter().collect(),
                message_id: 0,
                reply_to: None,
            },
            text: "0123456789".into(),
        }
    }

    fn only_channel(counters: &Counters, now: i64) -> ChannelSummary {
        let mut nodes = counters.summary(at(now));
        nodes.pop().unwrap().channels.pop().unwrap()
    }

    #[test]
    fn minutes_forget_the_previous_hour() {
        let mut minutes = Minutes::<u32>::default();
        *minutes.at(10) += 1;
        *minutes.at(10 + MINUTES as i64) += 2;
        let series = minutes.series(10 + MINUTES as i64);
        assert_eq!(series.len(), MINUTES);
        assert_eq!(series.iter().sum::<u32>(), 2);
        assert_eq!(series.last(), Some(&2));
    }

    #[test]
    fn counts_kinds_dropped_and_rates() {
        let mut counters = Counters::new(at(0));
        for second in 0..30 {
            counters.record(&entry(second * 10, MessageKind::Heartbeat, None));
        }
        let mut dropped = entry(5, MessageKind::Hello, None);
        dropped.reading = Reading::Dropped("invalid".into());
        counters.record(&dropped);
        let node = counters.summary(at(299)).pop().unwrap();
        assert_eq!(
            node.kinds,
            vec![(Some(MessageKind::Heartbeat), 30), (None, 1)]
        );
        assert_eq!(node.dropped, 1);
        // 31 messages since the start, 299 s ago.
        assert!((node.up_per_minute - 31.0 * 60.0 / 299.0).abs() < 0.001);
        assert!((node.bytes_per_minute - 310.0 * 60.0 / 299.0).abs() < 0.001);
    }

    #[test]
    fn a_burst_is_noisy() {
        let mut counters = Counters::new(at(0));
        // One a minute for 50 minutes, then 10 a minute.
        for minute in 0..50 {
            counters.record(&entry(minute * 60, MessageKind::State, Some(0)));
        }
        assert_eq!(only_channel(&counters, 50 * 60).highlight, None);
        for second in (0..300).step_by(6) {
            counters.record(&entry(50 * 60 + second, MessageKind::State, Some(0)));
        }
        assert_eq!(
            only_channel(&counters, 55 * 60 - 1).highlight,
            Some(Highlight::Noisy)
        );
    }

    #[test]
    fn plenty_is_noisy_from_the_start() {
        let mut counters = Counters::new(at(0));
        for second in 0..180 {
            counters.record(&entry(second, MessageKind::Event, Some(0)));
            counters.record(&entry(second, MessageKind::Event, Some(0)));
        }
        assert_eq!(
            only_channel(&counters, 179).highlight,
            Some(Highlight::Noisy)
        );
    }

    #[test]
    fn holding_a_remote_key_is_not_noise() {
        let mut counters = Counters::new(at(0));
        // One code a minute for 50 minutes, then a key held for 30 s: 9 repeats a second.
        for minute in 0..50 {
            counters.record(&entry(minute * 60, MessageKind::Event, Some(0)));
        }
        for repeat in 0..270 {
            counters.record(&entry(50 * 60 + repeat / 9, MessageKind::Event, Some(0)));
        }
        assert_eq!(only_channel(&counters, 50 * 60 + 40).highlight, None);
    }

    #[test]
    fn a_regular_channel_that_goes_quiet_stopped() {
        let mut counters = Counters::new(at(0));
        for reading in 0..10 {
            counters.record(&entry(reading * 30, MessageKind::State, Some(0)));
        }
        let last = 9 * 30;
        // The node keeps sending heartbeats, up to each check.
        for seconds in (10..=100).step_by(10) {
            counters.record(&entry(last + seconds, MessageKind::Heartbeat, None));
        }
        let channel = only_channel(&counters, last + 100);
        assert_eq!(channel.typical, Some(Duration::from_secs(30)));
        // 3 × 30 s is under the 2 minute minimum.
        assert_eq!(channel.highlight, None);
        for seconds in [110, 120] {
            counters.record(&entry(last + seconds, MessageKind::Heartbeat, None));
        }
        assert_eq!(
            only_channel(&counters, last + 121).highlight,
            Some(Highlight::Stopped {
                typical: Duration::from_secs(30)
            })
        );
    }

    /// Records `State` after each gap and heartbeats every 10 s up to `quiet` seconds after the
    /// last one, which it returns.
    fn states_then_quiet(counters: &mut Counters, gaps: &[i64], quiet: i64) -> i64 {
        let mut second = 0;
        counters.record(&entry(second, MessageKind::State, Some(0)));
        for gap in gaps {
            second += gap;
            counters.record(&entry(second, MessageKind::State, Some(0)));
        }
        for heartbeat in (10..=quiet).step_by(10) {
            counters.record(&entry(second + heartbeat, MessageKind::Heartbeat, None));
        }
        second + quiet
    }

    #[test]
    fn a_potentiometer_reporting_noise_never_stops() {
        let mut counters = Counters::new(at(0));
        // The gaps seen on 2026-10-17 before a false alert: the ADC noise at rest, then a few
        // quick ones.
        let now = states_then_quiet(
            &mut counters,
            &[41, 200, 80, 130, 65, 175, 59, 2, 2, 27, 4],
            600,
        );
        assert_eq!(only_channel(&counters, now).highlight, None);
    }

    #[test]
    fn a_periodic_channel_with_an_extra_state_still_stops() {
        let mut counters = Counters::new(at(0));
        // Every 30 s, with one change reported halfway through a period.
        let mut gaps = vec![30; 17];
        gaps.splice(8..9, [12, 18]);
        let now = states_then_quiet(&mut counters, &gaps, 130);
        assert_eq!(
            only_channel(&counters, now + 1).highlight,
            Some(Highlight::Stopped {
                typical: Duration::from_secs(30)
            })
        );
    }

    #[test]
    fn a_short_even_burst_is_not_a_rhythm() {
        let mut counters = Counters::new(at(0));
        // Turning the knob slowly: a change every 4 s for 40 s.
        let now = states_then_quiet(&mut counters, &[4; 10], 300);
        assert_eq!(only_channel(&counters, now).highlight, None);
    }

    #[test]
    fn a_silent_node_is_offline_not_stopped() {
        let mut counters = Counters::new(at(0));
        for reading in 0..10 {
            counters.record(&entry(reading * 30, MessageKind::State, Some(0)));
            counters.record(&entry(reading * 30 + 15, MessageKind::Heartbeat, None));
        }
        assert_eq!(only_channel(&counters, 9 * 30 + 300).highlight, None);
    }

    #[test]
    fn events_alone_never_stop() {
        let mut counters = Counters::new(at(0));
        for press in 0..10 {
            counters.record(&entry(press * 30, MessageKind::Event, Some(0)));
        }
        assert_eq!(only_channel(&counters, 3000).highlight, None);
    }

    fn command(millisecond: i64, message_id: u32) -> TrafficEntry {
        TrafficEntry {
            at: Timestamp::from_millisecond(START * 1000 + millisecond).unwrap(),
            direction: Direction::Down,
            topic: "oxihome/a/down".into(),
            device_id: Some("a".into()),
            reading: Reading::Read {
                kind: MessageKind::Command,
                entity: Some(1),
                channels: vec![0],
                message_id,
                reply_to: None,
            },
            text: "x".repeat(3500),
        }
    }

    fn ack(millisecond: i64, reply_to: u32) -> TrafficEntry {
        TrafficEntry {
            at: Timestamp::from_millisecond(START * 1000 + millisecond).unwrap(),
            direction: Direction::Up,
            topic: "oxihome/a/up".into(),
            device_id: Some("a".into()),
            reading: Reading::Read {
                kind: MessageKind::Ack,
                entity: None,
                channels: Vec::new(),
                message_id: 0,
                reply_to: Some(reply_to),
            },
            text: "{}".into(),
        }
    }

    #[test]
    fn answers_are_timed() {
        let mut counters = Counters::new(at(0));
        for (sent, took, message_id) in [(0, 100, 1), (1000, 300, 2), (2000, 200, 3)] {
            counters.record(&command(sent, message_id));
            counters.record(&ack(sent + took, message_id));
        }
        let node = counters.summary(at(10)).pop().unwrap();
        assert_eq!(
            node.answers,
            Some(Answers {
                count: 3,
                median: Duration::from_millis(200),
                worst: Duration::from_millis(300),
                slow: false,
            })
        );
        assert_eq!(node.unanswered, 0);
        assert_eq!(node.largest, 3500);
    }

    #[test]
    fn a_late_answer_counts_as_none() {
        let mut counters = Counters::new(at(0));
        counters.record(&command(0, 1));
        assert_eq!(counters.summary(at(1)).pop().unwrap().unanswered, 0);
        assert_eq!(counters.summary(at(6)).pop().unwrap().unanswered, 1);
        counters.record(&ack(7000, 1));
        let node = counters.summary(at(8)).pop().unwrap();
        assert_eq!(node.unanswered, 1);
        assert_eq!(node.answers, None);
    }
}
