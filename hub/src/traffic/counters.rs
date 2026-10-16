//! Messages per minute over the last hour, by node and by channel, and what stands out.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use jiff::Timestamp;

use super::{Direction, MessageKind, Reading, TrafficEntry};

const MINUTES: usize = 60;
/// What the rates on the page are averaged over.
const RECENT: Duration = Duration::from_secs(5 * 60);
/// `State` times kept per channel to find its rhythm.
const STATE_TIMES: usize = 20;

const NOISY_FACTOR: f64 = 3.0;
const NOISY_MINIMUM_PER_MINUTE: f64 = 6.0;
const NOISY_ALWAYS_PER_MINUTE: f64 = 60.0;

const STOPPED_FACTOR: u32 = 3;
const STOPPED_MINIMUM: Duration = Duration::from_secs(2 * 60);
const STOPPED_MINIMUM_STATES: usize = 5;

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
    channels: BTreeMap<(u8, u8), ChannelCounters>,
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

    fn recent_rate(&self, series: &[u64]) -> f64 {
        let recent: u64 = series[series.len() - self.recent_slots..].iter().sum();
        Self::rate(self.recent_minutes, recent)
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
        let tally = node.minutes.at(minute);
        match entry.direction {
            Direction::Up => tally.up += 1,
            Direction::Down => tally.down += 1,
        }
        tally.bytes += entry.text.len() as u64;
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
            entity: Some(entity),
            channels,
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
                    channels: node
                        .channels
                        .iter()
                        .map(|(&(entity, channel), counters)| {
                            channel_summary(entity, channel, counters, &window, minute, now)
                        })
                        .collect(),
                }
            })
            .collect()
    }
}

fn channel_summary(
    entity: u8,
    channel: u8,
    counters: &ChannelCounters,
    window: &Window,
    minute: i64,
    now: Timestamp,
) -> ChannelSummary {
    let series = counters.minutes.series(minute);
    let counts: Vec<u64> = series.iter().map(|count| u64::from(*count)).collect();
    let per_minute = window.recent_rate(&counts);
    let typical = typical_gap(&counters.states);
    let noisy = per_minute >= NOISY_ALWAYS_PER_MINUTE
        || window.earlier_rate(&counts).is_some_and(|earlier| {
            per_minute >= NOISY_MINIMUM_PER_MINUTE && per_minute >= NOISY_FACTOR * earlier
        });
    let stopped = match (typical, counters.states.back()) {
        (Some(typical), Some(last)) if counters.states.len() >= STOPPED_MINIMUM_STATES => {
            elapsed(*last, now) > (typical * STOPPED_FACTOR).max(STOPPED_MINIMUM)
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

/// The median time between `times`.
fn typical_gap(times: &VecDeque<Timestamp>) -> Option<Duration> {
    let mut gaps: Vec<Duration> = times
        .iter()
        .zip(times.iter().skip(1))
        .map(|(earlier, later)| elapsed(*earlier, *later))
        .collect();
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
        for second in 0..60 {
            counters.record(&entry(second, MessageKind::Event, Some(0)));
            counters.record(&entry(second, MessageKind::Event, Some(0)));
        }
        assert_eq!(
            only_channel(&counters, 59).highlight,
            Some(Highlight::Noisy)
        );
    }

    #[test]
    fn a_regular_channel_that_goes_quiet_stopped() {
        let mut counters = Counters::new(at(0));
        for reading in 0..10 {
            counters.record(&entry(reading * 30, MessageKind::State, Some(0)));
        }
        let last = 9 * 30;
        let channel = only_channel(&counters, last + 100);
        assert_eq!(channel.typical, Some(Duration::from_secs(30)));
        // 3 × 30 s is under the 2 minute minimum.
        assert_eq!(channel.highlight, None);
        assert_eq!(
            only_channel(&counters, last + 121).highlight,
            Some(Highlight::Stopped {
                typical: Duration::from_secs(30)
            })
        );
    }

    #[test]
    fn events_alone_never_stop() {
        let mut counters = Counters::new(at(0));
        for press in 0..10 {
            counters.record(&entry(press * 30, MessageKind::Event, Some(0)));
        }
        assert_eq!(only_channel(&counters, 3000).highlight, None);
    }
}
