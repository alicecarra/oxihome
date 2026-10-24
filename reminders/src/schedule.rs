use croner::Cron;
use croner::parser::{CronParser, Seconds, Year};
use jiff::civil::Time;
use jiff::{Span, ToSpan, Zoned};

/// When a reminder falls due, counted from the last time it was done.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// Standard five fields (minute, hour, day of month, month, day of week), in local time.
    Cron(Box<Cron>),
    /// This long after it was done; with `at`, on that day at that time.
    Every { span: Span, at: Option<Time> },
}

impl Schedule {
    pub fn cron(expression: &str) -> Result<Self, String> {
        let parser = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .build();
        parser
            .parse(expression)
            .map(|cron| Self::Cron(Box::new(cron)))
            .map_err(|error| format!("cron \"{expression}\": {error}"))
    }

    /// `at` needs a whole number of days or weeks.
    pub fn every(text: &str, at: Option<Time>) -> Result<Self, String> {
        let span: Span = text
            .parse()
            .map_err(|error| format!("every \"{text}\": {error}"))?;
        if span.is_negative() || span.is_zero() {
            return Err(format!("every \"{text}\": must be more than zero"));
        }
        if at.is_some() {
            let days = whole_days(span)
                .ok_or_else(|| format!("every \"{text}\": with at, only days or weeks"))?;
            return Ok(Self::Every {
                span: days.days(),
                at,
            });
        }
        if span.get_years() == 0
            && span.get_months() == 0
            && span.get_weeks() == 0
            && span.get_days() == 0
            && span.get_hours() == 0
            && span.get_minutes() == 0
        {
            return Err(format!("every \"{text}\": at least one minute"));
        }
        Ok(Self::Every { span, at })
    }

    /// The first time it falls due after `since`; `None` when it never does.
    pub fn next_due(&self, since: &Zoned) -> Option<Zoned> {
        match self {
            Schedule::Cron(cron) => cron.find_next_occurrence(since, false).ok(),
            Schedule::Every { span, at: None } => since.checked_add(*span).ok(),
            Schedule::Every { span, at: Some(at) } => since
                .date()
                .checked_add(*span)
                .ok()?
                .to_datetime(*at)
                .to_zoned(since.time_zone().clone())
                .ok(),
        }
    }

    pub fn is_cron(&self) -> bool {
        matches!(self, Schedule::Cron(_))
    }
}

fn whole_days(span: Span) -> Option<i64> {
    let calendar_free = span.get_years() == 0 && span.get_months() == 0;
    let no_time = span.get_hours() == 0
        && span.get_minutes() == 0
        && span.get_seconds() == 0
        && span.get_milliseconds() == 0
        && span.get_microseconds() == 0
        && span.get_nanoseconds() == 0;
    (calendar_free && no_time).then(|| i64::from(span.get_weeks()) * 7 + i64::from(span.get_days()))
}

/// Hours when nothing is sent: from `from` (included) to `to` (excluded), across midnight when
/// `from` is later than `to`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quiet {
    pub from: Time,
    pub to: Time,
}

impl Quiet {
    pub fn contains(&self, time: Time) -> bool {
        if self.from <= self.to {
            self.from <= time && time < self.to
        } else {
            time >= self.from || time < self.to
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::{date, time};
    use jiff::tz::TimeZone;

    use super::*;

    fn at(text: &str) -> Zoned {
        text.parse::<jiff::civil::DateTime>()
            .unwrap()
            .to_zoned(TimeZone::get("America/Sao_Paulo").unwrap())
            .unwrap()
    }

    #[test]
    fn cron_falls_due_at_the_next_occurrence() {
        let schedule = Schedule::cron("30 8 * * *").unwrap();
        assert_eq!(
            schedule.next_due(&at("2026-10-23T07:50")),
            Some(at("2026-10-23T08:30"))
        );
        assert_eq!(
            schedule.next_due(&at("2026-10-23T08:30")),
            Some(at("2026-10-24T08:30")),
            "an occurrence at the same minute already passed"
        );
        assert!(Schedule::cron("30 8 * *").is_err());
        assert!(Schedule::cron("0 30 8 * * *").is_err(), "no seconds");
    }

    #[test]
    fn every_counts_from_the_last_time() {
        let schedule = Schedule::every("3d", None).unwrap();
        assert_eq!(
            schedule.next_due(&at("2026-10-23T21:15")),
            Some(at("2026-10-26T21:15"))
        );
        let schedule = Schedule::every("90m", None).unwrap();
        assert_eq!(
            schedule.next_due(&at("2026-10-23T21:15")),
            Some(at("2026-10-23T22:45"))
        );
    }

    #[test]
    fn every_with_at_falls_due_on_the_day_at_that_time() {
        let schedule = Schedule::every("3d", Some(time(9, 0, 0, 0))).unwrap();
        assert_eq!(
            schedule.next_due(&at("2026-10-23T21:15")),
            Some(at("2026-10-26T09:00"))
        );
        let schedule = Schedule::every("1w", Some(time(9, 0, 0, 0))).unwrap();
        assert_eq!(
            schedule.next_due(&at("2026-10-23T08:00")),
            Some(at("2026-10-30T09:00"))
        );
        assert!(Schedule::every("36h", Some(time(9, 0, 0, 0))).is_err());
        assert!(Schedule::every("1d 2h", Some(time(9, 0, 0, 0))).is_err());
    }

    #[test]
    fn every_needs_at_least_a_minute() {
        assert!(Schedule::every("0m", None).is_err());
        assert!(Schedule::every("30s", None).is_err());
        assert!(Schedule::every("-1d", None).is_err());
        assert!(Schedule::every("soon", None).is_err());
        assert!(Schedule::every("1m", None).is_ok());
    }

    #[test]
    fn days_follow_the_calendar_across_a_daylight_saving_change() {
        let zone = TimeZone::get("America/New_York").unwrap();
        let before = date(2026, 3, 7)
            .at(9, 0, 0, 0)
            .to_zoned(zone.clone())
            .unwrap();
        let schedule = Schedule::every("1d", None).unwrap();
        let next = schedule.next_due(&before).unwrap();
        assert_eq!((next.hour(), next.minute()), (9, 0));
        assert_eq!(next.date(), date(2026, 3, 8));
    }

    #[test]
    fn quiet_hours_may_cross_midnight() {
        let night = Quiet {
            from: time(22, 0, 0, 0),
            to: time(8, 0, 0, 0),
        };
        assert!(night.contains(time(23, 30, 0, 0)));
        assert!(night.contains(time(7, 59, 0, 0)));
        assert!(!night.contains(time(8, 0, 0, 0)));
        assert!(!night.contains(time(21, 59, 0, 0)));
        let lunch = Quiet {
            from: time(12, 0, 0, 0),
            to: time(13, 0, 0, 0),
        };
        assert!(lunch.contains(time(12, 30, 0, 0)));
        assert!(!lunch.contains(time(13, 0, 0, 0)));
    }
}
