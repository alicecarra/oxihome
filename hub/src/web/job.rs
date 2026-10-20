//! The `job` dashboard block: what a node is doing, like a print.

use jiff::{SignedDuration, Timestamp, tz::TimeZone};
use oxihome_protocol::Value;
use topcoat::{
    Result,
    view::{View, component, view},
};

use super::format::{bool_label, css_color, long_duration_label, value_label};
use super::nodes::color_dot;
use super::overview::{self, DashboardChannel};
use crate::config::dashboard::{Block, ChannelReference, JobChannel};
use crate::hub::Hub;

/// States shown in red whatever the block says.
const FAILED: [&str; 2] = ["failed", "error"];
/// Further than this, the end time names the day.
const SAME_DAY: SignedDuration = SignedDuration::from_hours(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Failed,
    Stopped,
}

pub struct JobView {
    state: String,
    phase: Phase,
    online: bool,
    /// 0 to 100.
    progress: Option<f64>,
    /// `1 h 30 min left · ends 15:42`.
    left: Option<String>,
    /// Label, value and the color of a color value.
    details: Vec<(String, String, Option<String>)>,
}

/// The node's label for the title, and what the block shows.
pub fn job_view(hub: &Hub, block: &Block) -> (String, JobView) {
    let device = block.device.clone().unwrap_or_default();
    let reference = |job: &JobChannel| ChannelReference {
        device: device.clone(),
        entity: job.entity,
        channel: job.channel,
    };
    let mut references: Vec<ChannelReference> = block.state.iter().map(reference).collect();
    references.extend(block.progress.iter().map(reference));
    references.extend(block.left.iter().map(reference));
    references.extend(block.details.iter().map(reference));
    let mut channels = overview::dashboard_channels(hub, &references).into_iter();
    let state = channels.next();
    let progress = block.progress.and_then(|_| channels.next());
    let left = block.left.and_then(|_| channels.next());
    let details: Vec<DashboardChannel> = channels.collect();

    let title = state
        .as_ref()
        .map_or_else(|| device.clone(), |state| state.node_label.clone());
    let state_text = state.as_ref().map_or_else(|| "—".to_string(), text);
    let phase = phase(&state_text, &block.active);
    let left = left
        .as_ref()
        .and_then(number)
        .filter(|seconds| *seconds > 0.0)
        .map(|seconds| left_label(seconds as u64, Timestamp::now(), hub.time_zone()));
    let view = JobView {
        state: capitalized(&state_text),
        phase,
        online: hub.is_online(&device),
        progress: progress
            .as_ref()
            .and_then(number)
            .map(|progress| progress.clamp(0.0, 100.0)),
        left,
        details: details
            .iter()
            .map(|channel| {
                // Without the entity: `File`, not `Job · File`.
                let label = channel.label.rsplit(" · ").next().unwrap_or_default();
                let color = channel
                    .channel
                    .as_ref()
                    .ok()
                    .and_then(|overview| overview.value.as_ref())
                    .and_then(css_color);
                (label.to_string(), text(channel), color)
            })
            .collect(),
    };
    (title, view)
}

fn phase(state: &str, active: &[String]) -> Phase {
    if active
        .iter()
        .any(|active| active.eq_ignore_ascii_case(state))
    {
        Phase::Running
    } else if FAILED
        .iter()
        .any(|failed| failed.eq_ignore_ascii_case(state))
    {
        Phase::Failed
    } else {
        Phase::Stopped
    }
}

fn text(channel: &DashboardChannel) -> String {
    match &channel.channel {
        Ok(overview) => match &overview.value {
            Some(Value::Bool(on)) => bool_label(*on, overview.switchable).to_string(),
            Some(value) => value_label(value, overview.unit),
            None => "—".to_string(),
        },
        Err(reason) => (*reason).to_string(),
    }
}

fn number(channel: &DashboardChannel) -> Option<f64> {
    match channel.channel.as_ref().ok()?.value.as_ref()? {
        Value::Int(number) => Some(f64::from(*number)),
        Value::Float(number) => Some(f64::from(*number)),
        _ => None,
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

fn left_label(seconds: u64, now: Timestamp, time_zone: &TimeZone) -> String {
    let left = SignedDuration::from_secs(i64::try_from(seconds).unwrap_or(i64::MAX));
    let ends = now
        .checked_add(left)
        .map(|end| end.to_zoned(time_zone.clone()));
    let format = if left >= SAME_DAY {
        "%a %H:%M"
    } else {
        "%H:%M"
    };
    match ends {
        Ok(ends) => format!(
            "{} left · ends {}",
            long_duration_label(seconds),
            ends.strftime(format)
        ),
        Err(_) => format!("{} left", long_duration_label(seconds)),
    }
}

/// While the job runs: the state, progress and time left, then the details. Otherwise one line.
#[component]
pub async fn job_block(job: JobView) -> Result<impl View> {
    let state_class = match job.phase {
        Phase::Running => "text-2xl font-semibold tracking-tight",
        Phase::Failed => "text-sm font-medium text-destructive",
        Phase::Stopped => "text-sm text-muted-foreground",
    };
    let running = job.phase == Phase::Running;
    let progress = job.progress.filter(|_| running);
    Ok(view! {
        <div class=(if job.online { "space-y-3" } else { "space-y-3 opacity-60" })>
            <div class="flex items-baseline justify-between gap-3">
                <p class=(state_class)>
                    (job.state)
                    if !job.online {
                        " · offline"
                    }
                </p>
                if let Some(progress) = progress {
                    <p class="text-xl font-semibold tabular-nums">(format!("{progress:.0} %"))</p>
                }
            </div>
            if let Some(progress) = progress {
                <div class="h-2 w-full overflow-hidden rounded-full bg-muted-foreground/20" role="presentation">
                    <div class="h-full rounded-full bg-foreground/70" style=(format!("width: {progress:.0}%"))></div>
                </div>
            }
            if running {
                if let Some(left) = job.left {
                    <p class="text-sm text-muted-foreground tabular-nums">(left)</p>
                }
                if !job.details.is_empty() {
                    <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
                        for (label, value, color) in job.details {
                            <dt class="text-muted-foreground">(label)</dt>
                            <dd class="truncate text-right tabular-nums">
                                match color {
                                    Some(color) => color_dot(color: color, (value)),
                                    None => (value),
                                }
                            </dd>
                        }
                    </dl>
                }
            }
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phase_comes_from_the_state() {
        let active = ["printing".to_string(), "paused".to_string()];
        assert_eq!(phase("printing", &active), Phase::Running);
        assert_eq!(phase("Paused", &active), Phase::Running);
        assert_eq!(phase("failed", &active), Phase::Failed);
        assert_eq!(phase("idle", &active), Phase::Stopped);
        assert_eq!(phase("finished", &active), Phase::Stopped);
    }

    #[test]
    fn time_left_names_the_end() {
        let time_zone = TimeZone::get("America/Sao_Paulo").unwrap();
        let now: Timestamp = "2026-10-19T17:00:00Z".parse().unwrap();
        assert_eq!(
            left_label(5423, now, &time_zone),
            "1 h 30 min left · ends 15:30"
        );
        assert_eq!(
            left_label(22 * 3600, now, &time_zone),
            "22 h left · ends Tue 12:00"
        );
    }

    #[test]
    fn states_start_with_a_capital() {
        assert_eq!(capitalized("printing"), "Printing");
        assert_eq!(capitalized(""), "");
    }
}
