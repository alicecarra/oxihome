//! Small history charts drawn as inline SVG, without JavaScript.

use std::time::Duration;

use oxihome_protocol::Unit;
use topcoat::{
    Result,
    view::{View, component, view},
};

use super::format::unit_symbol;
use crate::storage::History;

pub const CHART_BUCKETS: usize = 144;
const WIDTH: f64 = 288.0;
const HEIGHT: f64 = 48.0;

/// How far back the charts go, picked on the page and kept in its `range` query.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChartRange {
    #[default]
    Hour,
    SixHours,
    Day,
}

impl ChartRange {
    pub const ALL: [ChartRange; 3] = [ChartRange::Hour, ChartRange::SixHours, ChartRange::Day];

    pub fn span(self) -> Duration {
        let hours = match self {
            ChartRange::Hour => 1,
            ChartRange::SixHours => 6,
            ChartRange::Day => 24,
        };
        Duration::from_secs(hours * 3600)
    }

    pub fn bucket_length(self) -> Duration {
        self.span() / u32::try_from(CHART_BUCKETS).unwrap_or(1)
    }

    /// The default for a missing or unknown key.
    pub fn from_key(key: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|range| Some(range.key()) == key)
            .unwrap_or_default()
    }

    /// As in the query.
    pub fn key(self) -> &'static str {
        match self {
            ChartRange::Hour => "1h",
            ChartRange::SixHours => "6h",
            ChartRange::Day => "24h",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChartRange::Hour => "1 h",
            ChartRange::SixHours => "6 h",
            ChartRange::Day => "24 h",
        }
    }
}

/// Average, minimum and maximum of a bucket.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub average: f64,
    pub minimum: f64,
    pub maximum: f64,
}

/// One point per bucket. Readings are stored only when the value changes, so an empty bucket
/// keeps the last value; buckets before the first known value stay empty.
pub fn series(history: &History, count: usize) -> Vec<Option<Point>> {
    let mut points = vec![None; count];
    let mut last = history.before;
    let mut buckets = history.buckets.iter().peekable();
    for (index, point) in points.iter_mut().enumerate() {
        match buckets.next_if(|(bucket, ..)| *bucket == index) {
            Some(&(_, average, minimum, maximum)) => {
                *point = Some(Point {
                    average,
                    minimum,
                    maximum,
                });
                last = Some(average);
            }
            None => {
                *point = last.map(|value| Point {
                    average: value,
                    minimum: value,
                    maximum: value,
                });
            }
        }
    }
    points
}

#[derive(Clone)]
pub struct Chart {
    /// The average line.
    line: String,
    /// The band between minimum and maximum.
    band: String,
    minimum: f64,
    maximum: f64,
}

/// `None` without any value to draw.
pub fn chart(points: &[Option<Point>]) -> Option<Chart> {
    let known: Vec<(usize, Point)> = points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| point.map(|point| (index, point)))
        .collect();
    let minimum = known
        .iter()
        .map(|(_, point)| point.minimum)
        .fold(f64::INFINITY, f64::min);
    let maximum = known
        .iter()
        .map(|(_, point)| point.maximum)
        .fold(f64::NEG_INFINITY, f64::max);
    if known.is_empty() {
        return None;
    }
    // A flat line sits in the middle.
    let (low, high) = if maximum > minimum {
        (minimum, maximum)
    } else {
        (minimum - 1.0, maximum + 1.0)
    };
    let step = WIDTH / points.len().max(1) as f64;
    // A pixel of margin keeps the stroke inside.
    let y = |value: f64| 1.0 + (high - value) / (high - low) * (HEIGHT - 2.0);
    // Each bucket is a step from its start to its end, so a single bucket still shows.
    let steps = |index: usize, value: f64| {
        let (start, end) = (index as f64 * step, (index + 1) as f64 * step);
        [
            format!("{start:.1},{:.1}", y(value)),
            format!("{end:.1},{:.1}", y(value)),
        ]
    };
    let line = known
        .iter()
        .flat_map(|(index, point)| steps(*index, point.average))
        .collect::<Vec<_>>()
        .join(" ");
    let top = known
        .iter()
        .flat_map(|(index, point)| steps(*index, point.maximum));
    let bottom = known.iter().rev().flat_map(|(index, point)| {
        let [start, end] = steps(*index, point.minimum);
        [end, start]
    });
    let band = top.chain(bottom).collect::<Vec<_>>().join(" ");
    Some(Chart {
        line,
        band,
        minimum,
        maximum,
    })
}

/// The last hours of a channel, with its minimum and maximum.
#[component]
pub async fn history_chart(
    chart: Chart,
    unit: Option<Unit>,
    range: ChartRange,
) -> Result<impl View> {
    let suffix = unit
        .map(|unit| format!(" {}", unit_symbol(unit)))
        .unwrap_or_default();
    let extremes = format!(
        "min {:.1}{suffix} · max {:.1}{suffix}",
        chart.minimum, chart.maximum
    );
    Ok(view! {
        <figure class="w-full space-y-1">
            <svg
                viewBox=(format!("0 0 {WIDTH} {HEIGHT}"))
                preserveAspectRatio="none"
                class="h-12 w-full"
                role="img"
                aria-label=(format!("Last {}: {extremes}", range.label()))
            >
                <polygon points=(chart.band) class="fill-foreground/10"></polygon>
                <polyline
                    points=(chart.line)
                    fill="none"
                    stroke-width="1.5"
                    stroke-linejoin="round"
                    vector-effect="non-scaling-stroke"
                    class="stroke-foreground/70"
                ></polyline>
            </svg>
            <figcaption class="flex justify-between text-xs text-muted-foreground tabular-nums">
                <span>(range.label())</span>
                <span>(extremes)</span>
            </figcaption>
        </figure>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(value: f64) -> Option<Point> {
        Some(Point {
            average: value,
            minimum: value,
            maximum: value,
        })
    }

    #[test]
    fn empty_buckets_keep_the_last_value() {
        let history = History {
            before: None,
            buckets: vec![(1, 20.0, 19.0, 21.0), (3, 22.0, 22.0, 22.0)],
        };
        assert_eq!(
            series(&history, 5),
            vec![
                None,
                Some(Point {
                    average: 20.0,
                    minimum: 19.0,
                    maximum: 21.0
                }),
                flat(20.0),
                flat(22.0),
                flat(22.0),
            ]
        );
    }

    #[test]
    fn value_before_the_window_starts_the_line() {
        let history = History {
            before: Some(18.0),
            buckets: Vec::new(),
        };
        assert_eq!(series(&history, 2), vec![flat(18.0), flat(18.0)]);
    }

    #[test]
    fn nothing_known_draws_nothing() {
        assert!(chart(&[None, None]).is_none());
    }

    #[test]
    fn chart_spans_the_range() {
        let chart = chart(&[flat(10.0), flat(20.0)]).unwrap();
        assert_eq!(chart.line, "0.0,47.0 144.0,47.0 144.0,1.0 288.0,1.0");
        assert_eq!((chart.minimum, chart.maximum), (10.0, 20.0));
    }
}
