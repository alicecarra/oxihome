use jiff::civil::{DateTime, DateTimeRound};
use jiff::tz::TimeZone;
use jiff::{RoundMode, Timestamp, ToSpan, Unit};

/// Longer gaps (hub suspended, big clock jump) run only the current minute.
const MAX_CATCH_UP_MINUTES: i64 = 5;

/// Turns clock ticks into local minutes, each one at most once.
pub struct Schedule {
    time_zone: TimeZone,
    last_minute: DateTime,
}

impl Schedule {
    /// The current minute counts as already seen: starting the hub does not run its rules.
    pub fn new(time_zone: &TimeZone, now: Timestamp) -> Self {
        Self {
            time_zone: time_zone.clone(),
            last_minute: minute_of(time_zone, now),
        }
    }

    /// Minutes skipped by a short gap (late tick, small clock jump) come too, in order.
    /// A clock going back (end of daylight saving time) repeats no minute.
    pub fn new_minutes(&mut self, now: Timestamp) -> Vec<DateTime> {
        let minute = minute_of(&self.time_zone, now);
        if minute <= self.last_minute {
            return Vec::new();
        }
        let first = self.last_minute.saturating_add(1.minute());
        self.last_minute = minute;
        if first.saturating_add(MAX_CATCH_UP_MINUTES.minutes()) < minute {
            return vec![minute];
        }
        first
            .series(1.minute())
            .take_while(|next| *next <= minute)
            .collect()
    }
}

fn minute_of(time_zone: &TimeZone, now: Timestamp) -> DateTime {
    let local = time_zone.to_datetime(now);
    local
        .round(
            DateTimeRound::new()
                .smallest(Unit::Minute)
                .mode(RoundMode::Trunc),
        )
        .unwrap_or(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn zone() -> TimeZone {
        TimeZone::get("America/Sao_Paulo").unwrap()
    }

    fn texts(minutes: Vec<DateTime>) -> Vec<String> {
        minutes.iter().map(|minute| minute.to_string()).collect()
    }

    #[test]
    fn starting_minute_does_not_run() {
        let mut schedule = Schedule::new(&zone(), at("2026-10-11T23:00:05-03:00"));
        assert!(
            schedule
                .new_minutes(at("2026-10-11T23:00:40-03:00"))
                .is_empty()
        );
    }

    #[test]
    fn each_minute_comes_once_in_local_time() {
        let mut schedule = Schedule::new(&zone(), at("2026-10-11T22:59:30-03:00"));
        let minutes = schedule.new_minutes(at("2026-10-12T02:00:01Z"));
        assert_eq!(texts(minutes), ["2026-10-11T23:00:00"]);
        assert!(
            schedule
                .new_minutes(at("2026-10-11T23:00:59-03:00"))
                .is_empty()
        );
    }

    #[test]
    fn short_gaps_run_the_skipped_minutes() {
        let mut schedule = Schedule::new(&zone(), at("2026-10-11T22:58:59-03:00"));
        let minutes = schedule.new_minutes(at("2026-10-11T23:01:00-03:00"));
        assert_eq!(
            texts(minutes),
            [
                "2026-10-11T22:59:00",
                "2026-10-11T23:00:00",
                "2026-10-11T23:01:00"
            ]
        );
    }

    #[test]
    fn long_gaps_run_only_the_current_minute() {
        let mut schedule = Schedule::new(&zone(), at("2026-10-11T22:00:00-03:00"));
        let minutes = schedule.new_minutes(at("2026-10-11T23:30:00-03:00"));
        assert_eq!(texts(minutes), ["2026-10-11T23:30:00"]);
    }

    #[test]
    fn clock_going_back_repeats_nothing() {
        let mut schedule = Schedule::new(&zone(), at("2026-10-11T23:00:00-03:00"));
        assert!(
            schedule
                .new_minutes(at("2026-10-11T22:30:00-03:00"))
                .is_empty()
        );
    }
}
