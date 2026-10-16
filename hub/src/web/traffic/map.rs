//! The map above the live list: messages per minute by node and channel in the last hour, and
//! what stands out.

use std::sync::Arc;

use jiff::Timestamp;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    view::{View, component, view},
};

use super::{Filter, parameters};
use crate::components::badge::{BadgeVariant, badge};
use crate::config::nodes::NameTarget;
use crate::hub::Hub;
use crate::traffic::{Highlight, MessageKind, NodeSummary};
use crate::web::format::{ago_label, duration_label, elapsed};
use crate::web::overview::{self, channel_name};

const BAR_WIDTH: usize = 2;
const BAR_GAP: usize = 1;
const BARS_HEIGHT: u32 = 24;

struct Map {
    since: String,
    nodes: Vec<NodeLine>,
}

struct NodeLine {
    title: String,
    link: Option<String>,
    chosen: bool,
    kinds: String,
    rates: String,
    last: String,
    bars: String,
    dropped: u32,
    /// Channels that stand out, shown even when the node is not chosen.
    highlighted: Vec<ChannelLine>,
    /// Every channel, only for the chosen node.
    channels: Vec<ChannelLine>,
}

#[derive(Clone)]
struct ChannelLine {
    title: String,
    link: String,
    rate: String,
    last: String,
    rhythm: Option<String>,
    bars: String,
    highlight: Option<Highlight>,
}

fn rate_label(per_minute: f64) -> String {
    if per_minute < 10.0 {
        format!("{per_minute:.1}/min")
    } else {
        format!("{per_minute:.0}/min")
    }
}

fn bytes_label(per_minute: f64) -> String {
    if per_minute < 1024.0 {
        format!("{per_minute:.0} B/min")
    } else {
        format!("{:.1} KiB/min", per_minute / 1024.0)
    }
}

fn last_label(at: Option<Timestamp>, now: Timestamp) -> String {
    at.map_or_else(String::new, |at| ago_label(elapsed(at, now)))
}

/// One bar per minute, as a single SVG path; the busiest minute fills the height.
fn bars_path(series: &[u32]) -> String {
    let highest = series.iter().copied().max().unwrap_or(0);
    if highest == 0 {
        return String::new();
    }
    series
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(index, count)| {
            let height = (count * BARS_HEIGHT).div_ceil(highest).max(1);
            let x = index * (BAR_WIDTH + BAR_GAP);
            format!("M{x} {BARS_HEIGHT}v-{height}h{BAR_WIDTH}v{height}z")
        })
        .collect()
}

fn map(hub: &Hub, filter: &Filter) -> Map {
    let now = Timestamp::now();
    let since = hub
        .traffic()
        .counting_since()
        .to_zoned(hub.time_zone().clone())
        .strftime("%H:%M")
        .to_string();
    let summary = hub.traffic().summary();
    let snapshot = hub.snapshot();
    let mut nodes: Vec<NodeLine> = summary
        .into_iter()
        .map(|node| {
            let NodeSummary {
                device_id,
                up_per_minute,
                down_per_minute,
                bytes_per_minute,
                last_at,
                kinds,
                dropped,
                series,
                channels,
            } = node;
            let title = match &device_id {
                Some(device_id) => overview::name(
                    &snapshot.nodes,
                    &snapshot.registry,
                    &NameTarget::Node(device_id.clone()),
                )
                .unwrap_or_else(|| device_id.clone()),
                None => "Every node".into(),
            };
            let chosen = device_id.is_some() && device_id == filter.device_id;
            let channels: Vec<ChannelLine> = match &device_id {
                Some(device_id) => channels
                    .into_iter()
                    .map(|channel| ChannelLine {
                        title: channel_name(
                            &snapshot.nodes,
                            &snapshot.registry,
                            device_id,
                            channel.entity,
                            channel.channel,
                            None,
                        ),
                        link: format!(
                            "/traffic?{}",
                            Filter {
                                device_id: Some(device_id.clone()),
                                channel: Some((channel.entity, channel.channel)),
                                ..filter.clone()
                            }
                            .query()
                        ),
                        rate: rate_label(channel.per_minute),
                        last: last_label(channel.last_at, now),
                        rhythm: channel
                            .typical
                            .map(|typical| format!("State every {}", duration_label(typical))),
                        bars: bars_path(&channel.series),
                        highlight: channel.highlight,
                    })
                    .collect(),
                None => Vec::new(),
            };
            NodeLine {
                link: device_id.as_ref().map(|device_id| {
                    format!(
                        "/traffic?{}",
                        Filter {
                            device_id: Some(device_id.clone()),
                            channel: None,
                            ..filter.clone()
                        }
                        .query()
                    )
                }),
                title,
                chosen,
                kinds: kinds
                    .into_iter()
                    .map(|(kind, count)| {
                        format!("{} {count}", kind.map_or("Dropped", MessageKind::name))
                    })
                    .collect::<Vec<_>>()
                    .join(" · "),
                rates: format!(
                    "↑ {} · ↓ {} · {}",
                    rate_label(up_per_minute),
                    rate_label(down_per_minute),
                    bytes_label(bytes_per_minute)
                ),
                last: last_label(last_at, now),
                bars: bars_path(&series),
                dropped,
                highlighted: channels
                    .iter()
                    .filter(|channel| channel.highlight.is_some())
                    .cloned()
                    .collect(),
                channels: if chosen { channels } else { Vec::new() },
            }
        })
        .collect();
    // Broadcasts last.
    nodes.sort_by_key(|node| (node.link.is_none(), node.title.to_lowercase()));
    Map { since, nodes }
}

/// The map again, every few seconds.
#[page("/traffic/map")]
pub async fn traffic_map_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let filter = Filter::from_query(&parameters(cx));
    Ok(view! { traffic_map(map: map(hub, &filter)) })
}

/// Polls `/traffic/map` with the page's filter.
#[component]
pub(super) async fn live_map(cx: &Cx, filter: Filter) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let query = filter.query();
    Ok(view! {
        <section
            hx-get=(format!("/traffic/map?{query}"))
            hx-trigger="every 10s"
            class="space-y-2"
        >
            traffic_map(map: map(hub, &filter))
        </section>
    })
}

#[component]
async fn traffic_map(map: Map) -> Result<impl View> {
    Ok(view! {
        <p class="text-xs text-muted-foreground">
            (format!("Last hour, counting since {}. Rates are over the last 5 minutes.", map.since))
        </p>
        if map.nodes.is_empty() {
            <p class="text-muted-foreground">"No messages yet."</p>
        } else {
            <ul class="divide-y divide-border rounded-lg border border-border bg-card text-sm">
                for node in map.nodes {
                    node_line(node: node)
                }
            </ul>
        }
    })
}

#[component]
async fn node_line(node: NodeLine) -> Result<impl View> {
    let highlighted = node.highlighted;
    let channels = node.channels;
    Ok(view! {
        <li class=(if node.chosen { "bg-foreground/5" } else { "" })>
            <div class="flex flex-wrap items-center gap-x-4 gap-y-1 px-3 py-2">
                <div class="min-w-0 flex-1">
                    <div class="flex flex-wrap items-center gap-2">
                        match node.link {
                            Some(link) => <a href=(link) class="font-medium hover:underline">(node.title)</a>,
                            None => <span class="font-medium">(node.title)</span>,
                        }
                        if node.dropped > 0 {
                            badge(variant: BadgeVariant::Destructive, (format!("{} dropped", node.dropped)))
                        }
                    </div>
                    <p class="text-xs text-muted-foreground">(node.kinds)</p>
                </div>
                bars(path: node.bars)
                <div class="w-56 text-right text-xs tabular-nums text-muted-foreground">
                    <p>(node.rates)</p>
                    <p>(node.last)</p>
                </div>
            </div>
            if channels.is_empty() {
                if !highlighted.is_empty() {
                    <ul class="space-y-1 px-3 pb-2">
                        for channel in highlighted {
                            <li class="flex flex-wrap items-center gap-2 text-xs">
                                highlight_badge(highlight: channel.highlight)
                                <a href=(channel.link) class="hover:underline">(channel.title)</a>
                                <span class="text-muted-foreground">(channel.rate)</span>
                            </li>
                        }
                    </ul>
                }
            } else {
                <ul class="divide-y divide-border border-t border-border">
                    for channel in channels {
                        channel_line(channel: channel)
                    }
                </ul>
            }
        </li>
    })
}

#[component]
async fn channel_line(channel: ChannelLine) -> Result<impl View> {
    Ok(view! {
        <li class="flex flex-wrap items-center gap-x-4 gap-y-1 py-2 pr-3 pl-8">
            <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-2">
                    <a href=(channel.link) class="hover:underline">(channel.title)</a>
                    highlight_badge(highlight: channel.highlight)
                </div>
                if let Some(rhythm) = channel.rhythm {
                    <p class="text-xs text-muted-foreground">(rhythm)</p>
                }
            </div>
            bars(path: channel.bars)
            <div class="w-56 text-right text-xs tabular-nums text-muted-foreground">
                <p>(channel.rate)</p>
                <p>(channel.last)</p>
            </div>
        </li>
    })
}

#[component]
async fn highlight_badge(highlight: Option<Highlight>) -> Result<impl View> {
    let label = match highlight {
        Some(Highlight::Noisy) => Some("Noisy".to_string()),
        Some(Highlight::Stopped { typical }) => Some(format!(
            "Stopped, used to report every {}",
            duration_label(typical)
        )),
        None => None,
    };
    Ok(view! {
        if let Some(label) = label {
            badge(variant: BadgeVariant::Destructive, (label))
        }
    })
}

#[component]
async fn bars(path: String) -> Result<impl View> {
    let width = 60 * (BAR_WIDTH + BAR_GAP) - BAR_GAP;
    Ok(view! {
        <svg
            viewBox=(format!("0 0 {width} {BARS_HEIGHT}"))
            class="hidden h-6 w-[179px] shrink-0 text-foreground/60 sm:block"
            aria-hidden="true"
        >
            <path d=(path) fill="currentColor"></path>
        </svg>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_the_height_at_the_busiest_minute() {
        assert_eq!(bars_path(&[0, 0]), "");
        assert_eq!(bars_path(&[4, 0, 1]), "M0 24v-24h2v24zM6 24v-6h2v6z");
    }

    #[test]
    fn rates_keep_one_decimal_when_small() {
        assert_eq!(rate_label(0.5), "0.5/min");
        assert_eq!(rate_label(42.4), "42/min");
        assert_eq!(bytes_label(2048.0), "2.0 KiB/min");
    }
}
