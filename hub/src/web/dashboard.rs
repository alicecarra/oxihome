use std::collections::HashMap;
use std::sync::Arc;

use jiff::{SignedDuration, Timestamp};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, parse_query_params},
    view::{View, attributes, component, view},
};

use super::buttons::{RuleButton, rule_button_view, rule_buttons};
use super::chart::{CHART_BUCKETS, Chart, ChartRange, chart, history_chart, series};
use super::format::{ago_label, rule_anchor, value_label};
use super::layout::layout;
use super::nodes::{channel_controls, channel_value, presence_badge, protocol_badge};
use super::overview::{self, Activity, DashboardChannel, NodeOverview, Presence, RulesSummary};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::config::dashboard::{Block, BlockKind, ChannelReference, DEFAULT_EVENTS};
use crate::hub::Hub;

/// A block with what it shows, read before rendering.
struct BlockView {
    title: String,
    width: u8,
    content: BlockContent,
}

enum BlockContent {
    Nodes(Vec<NodeOverview>),
    Channels {
        channels: Vec<DashboardChannel>,
        /// Whether rows name their node: only when the block mixes nodes.
        several_nodes: bool,
    },
    Buttons(Vec<RuleButton>),
    Value {
        channel: DashboardChannel,
        online: bool,
    },
    History {
        channel: DashboardChannel,
        chart: Option<Chart>,
        range: ChartRange,
        /// Each range with the dashboard query that picks it for this block.
        choices: Vec<(ChartRange, String)>,
    },
    Rules(RulesSummary),
    Events(Vec<Activity>),
}

const UPCOMING_RULES: usize = 5;

#[page("/")]
pub async fn dashboard_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let (blocks, query) = block_views(hub, &picked_ranges(cx)).await;
    Ok(view! {
        layout(
            title: "Oxihome",
            dashboard_root(blocks: blocks, query: query)
        )
    })
}

#[page("/dashboard/fragment")]
pub async fn dashboard_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let (blocks, query) = block_views(hub, &picked_ranges(cx)).await;
    Ok(view! { dashboard_root(blocks: blocks, query: query) })
}

/// The ranges picked on the page, `range<block>=6h` in the query, by block index.
fn picked_ranges(cx: &Cx) -> HashMap<usize, String> {
    parse_query_params::<HashMap<String, String>>(cx)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, value)| Some((key.strip_prefix("range")?.parse().ok()?, value)))
        .collect()
}

/// The query keeping every history block's range, with `changed` set to `range`.
fn ranges_query(ranges: &[(usize, ChartRange)], changed: Option<(usize, ChartRange)>) -> String {
    ranges
        .iter()
        .map(|&(index, range)| {
            let range = match changed {
                Some((changed_index, picked)) if changed_index == index => picked,
                _ => range,
            };
            format!("range{index}={}", range.key())
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The blocks and the query that keeps their picked ranges.
async fn block_views(hub: &Hub, picked: &HashMap<usize, String>) -> (Vec<BlockView>, String) {
    let Some(dashboard) = hub.dashboard() else {
        return (Vec::new(), String::new());
    };
    let ranges: Vec<(usize, ChartRange)> = dashboard
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| block.kind == BlockKind::History)
        .map(|(index, block)| {
            let key = picked
                .get(&index)
                .map(String::as_str)
                .or(block.range.as_deref())
                .unwrap_or("24h");
            (index, ChartRange::from_key(Some(key)))
        })
        .collect();
    let mut views = Vec::new();
    for (index, block) in dashboard.blocks.iter().enumerate() {
        let range = ranges
            .iter()
            .find(|(history, _)| *history == index)
            .map(|(_, range)| *range)
            .unwrap_or(ChartRange::Day);
        let choices = ChartRange::ALL
            .into_iter()
            .map(|choice| (choice, ranges_query(&ranges, Some((index, choice)))))
            .collect();
        views.push(block_view(hub, block, range, choices).await);
    }
    (views, ranges_query(&ranges, None))
}

/// Validation makes sure `value` and `history` blocks have a channel.
fn single_channel(hub: &Hub, reference: Option<&ChannelReference>) -> Option<DashboardChannel> {
    overview::dashboard_channels(hub, std::slice::from_ref(reference?))
        .into_iter()
        .next()
}

async fn block_view(
    hub: &Hub,
    block: &Block,
    range: ChartRange,
    choices: Vec<(ChartRange, String)>,
) -> BlockView {
    let (default_title, content) = match block.kind {
        BlockKind::Nodes => ("Nodes".to_string(), BlockContent::Nodes(overview::nodes(hub))),
        BlockKind::Channels => {
            let channels = overview::dashboard_channels(hub, &block.channels);
            let several_nodes = channels
                .iter()
                .any(|channel| channel.device_id != channels[0].device_id);
            let title = if several_nodes {
                "Channels".to_string()
            } else {
                channels[0].node_label.clone()
            };
            (
                title,
                BlockContent::Channels {
                    channels,
                    several_nodes,
                },
            )
        }
        BlockKind::Buttons => (
            "Buttons".to_string(),
            BlockContent::Buttons(rule_buttons(hub, &block.buttons)),
        ),
        BlockKind::Value | BlockKind::History => {
            let Some(channel) = single_channel(hub, block.channel.as_ref()) else {
                return BlockView {
                    title: block.title.clone().unwrap_or_default(),
                    width: block.width,
                    content: BlockContent::Events(Vec::new()),
                };
            };
            let title = channel.node_label.clone();
            if block.kind == BlockKind::Value {
                let online = hub.is_online(&channel.device_id);
                (title, BlockContent::Value { channel, online })
            } else {
                let chart = history(hub, block.channel.as_ref(), range).await;
                (
                    title,
                    BlockContent::History {
                        channel,
                        chart,
                        range,
                        choices,
                    },
                )
            }
        }
        BlockKind::Rules => (
            "Rules".to_string(),
            BlockContent::Rules(overview::rules_summary(hub, UPCOMING_RULES)),
        ),
        BlockKind::Events => {
            let limit = block.limit.unwrap_or(DEFAULT_EVENTS);
            let events = hub.recent_events(limit).await.unwrap_or_else(|error| {
                println!("dashboard: recent events: {error:#}");
                Vec::new()
            });
            let notifications = hub.recent_notifications(limit).await.unwrap_or_else(|error| {
                println!("dashboard: recent notifications: {error:#}");
                Vec::new()
            });
            (
                "Recent events".to_string(),
                BlockContent::Events(overview::activity(hub, events, notifications, limit)),
            )
        }
    };
    BlockView {
        title: block.title.clone().unwrap_or(default_title),
        width: block.width,
        content,
    }
}

/// Refreshes itself with the same query, so picked chart ranges stay.
#[component]
async fn dashboard_root(blocks: Vec<BlockView>, query: String) -> Result<impl View> {
    Ok(view! {
        <div
            id="dashboard"
            hx-get=(format!("/dashboard/fragment?{query}"))
            hx-trigger="sse:nodes, sse:notifications, every 30s, refresh"
            hx-swap="outerHTML"
            // Buttons and controls inside would swap `#status` out with outerHTML.
            hx-disinherit="*"
        >
            dashboard_blocks(blocks: blocks)
        </div>
    })
}

#[component]
async fn dashboard_blocks(blocks: Vec<BlockView>) -> Result<impl View> {
    Ok(view! {
        if blocks.is_empty() {
            <p class="text-muted-foreground">
                "No dashboard yet: add blocks to dashboard.toml in "
                <a href="/settings/dashboard.toml" class="font-medium hover:underline">"Settings"</a>
                ", or see all nodes in "
                <a href="/nodes" class="font-medium hover:underline">"Nodes"</a>
                "."
            </p>
        }
        // Cards of a row share its height.
        <div class="grid gap-4 lg:grid-cols-3">
            for block in blocks {
                <div class=(width_class(block.width))>
                    card(
                        attrs: attributes! { class="h-full" },
                        card_header(card_title((block.title)))
                        card_content(
                            match block.content {
                                BlockContent::Nodes(nodes) => node_status(nodes: nodes),
                                BlockContent::Channels { channels, several_nodes } => {
                                    <ul class="divide-y divide-border">
                                        for channel in channels {
                                            dashboard_channel_row(channel: channel, several_nodes: several_nodes)
                                        }
                                    </ul>
                                },
                                BlockContent::Value { channel, online } => value_block(channel: channel, online: online),
                                BlockContent::History { channel, chart, range, choices } => history_block(channel: channel, chart: chart, range: range, choices: choices),
                                BlockContent::Rules(summary) => rules_block(summary: summary),
                                BlockContent::Events(items) => events_block(items: items),
                                BlockContent::Buttons(buttons) => {
                                    // In one row while they fit, sharing the width.
                                    <div class="flex flex-wrap gap-3">
                                        for rule_button in buttons {
                                            <div class="min-w-32 flex-1">
                                                rule_button_view(rule_button: rule_button)
                                            </div>
                                        }
                                    </div>
                                },
                            }
                        )
                    )
                </div>
            }
        </div>
    })
}

async fn history(
    hub: &Hub,
    reference: Option<&ChannelReference>,
    range: ChartRange,
) -> Option<Chart> {
    let reference = reference?;
    let since = Timestamp::now() - SignedDuration::try_from(range.span()).ok()?;
    match hub
        .history(
            &reference.device,
            reference.entity,
            reference.channel,
            since,
            range.bucket_length(),
        )
        .await
    {
        Ok(history) => chart(&series(&history, CHART_BUCKETS)),
        Err(error) => {
            println!("dashboard: history of {}: {error:#}", reference.device);
            None
        }
    }
}

fn width_class(width: u8) -> &'static str {
    match width {
        2 => "lg:col-span-2",
        3 => "lg:col-span-3",
        _ => "",
    }
}

/// The nodes grouped by presence; empty groups are left out.
#[component]
async fn node_status(nodes: Vec<NodeOverview>) -> Result<impl View> {
    let groups: Vec<(Presence, Vec<NodeOverview>)> =
        [Presence::Online, Presence::Offline, Presence::NotSeen]
            .into_iter()
            .map(|presence| {
                let members = nodes
                    .iter()
                    .filter(|node| node.presence == presence)
                    .cloned()
                    .collect();
                (presence, members)
            })
            .filter(|(_, members): &(Presence, Vec<NodeOverview>)| !members.is_empty())
            .collect();
    Ok(view! {
        if groups.is_empty() {
            <p class="text-sm text-muted-foreground">"No nodes yet."</p>
        }
        <div class="space-y-4">
            for (presence, members) in groups {
                <div class="space-y-2">
                    presence_badge(presence: presence)
                    <ul class="space-y-1">
                        for node in members {
                            <li>
                                <a href=(format!("/nodes/{}", node.device_id)) class="text-sm font-medium hover:underline">
                                    (node.name.clone().unwrap_or_else(|| node.device_id.clone()))
                                </a>
                                if node.name.is_some() {
                                    <span class="ml-2 text-xs text-muted-foreground">(node.device_id.clone())</span>
                                }
                                <span class="ml-2">protocol_badge(version: node.protocol_version)</span>
                            </li>
                        }
                    </ul>
                </div>
            }
        </div>
    })
}

#[component]
async fn dashboard_channel_row(channel: DashboardChannel, several_nodes: bool) -> Result<impl View> {
    let label = if several_nodes {
        format!("{} · {}", channel.node_label, channel.label)
    } else {
        channel.label.clone()
    };
    Ok(view! {
        <li class="flex items-center justify-between gap-4 py-3">
            <div>
                <p class="text-sm font-medium">(label.clone())</p>
                match channel.channel.clone() {
                    Ok(overview) => channel_value(channel: overview),
                    Err(reason) => <p class="text-sm text-muted-foreground">(reason)</p>,
                }
            </div>
            if let Ok(overview) = channel.channel {
                channel_controls(device_id: channel.device_id, channel: overview, label: label)
            }
        </li>
    })
}

/// The channel's current value as text, or why there is none.
fn current_value(channel: &DashboardChannel) -> String {
    match &channel.channel {
        Ok(overview) => overview
            .value
            .as_ref()
            .map_or_else(|| "—".to_string(), |value| value_label(value, overview.unit)),
        Err(reason) => (*reason).to_string(),
    }
}

#[component]
async fn value_block(channel: DashboardChannel, online: bool) -> Result<impl View> {
    let value = current_value(&channel);
    Ok(view! {
        <div class=(if online { "space-y-1" } else { "space-y-1 opacity-60" })>
            <p class="text-4xl font-semibold tracking-tight tabular-nums">(value)</p>
            <p class="text-sm text-muted-foreground">
                (channel.label)
                if !online {
                    " · offline"
                }
            </p>
        </div>
    })
}

#[component]
async fn history_block(
    channel: DashboardChannel,
    chart: Option<Chart>,
    range: ChartRange,
    choices: Vec<(ChartRange, String)>,
) -> Result<impl View> {
    let value = current_value(&channel);
    let unit = channel.channel.as_ref().ok().and_then(|overview| overview.unit);
    Ok(view! {
        <div class="space-y-3">
            <div class="flex items-baseline justify-between gap-3">
                <p class="text-sm text-muted-foreground">(channel.label)</p>
                <p class="text-xl font-semibold tabular-nums">(value)</p>
            </div>
            <div class="inline-flex gap-0.5 rounded-lg border border-border p-0.5" role="group" aria-label="Chart range">
                for (choice, query) in choices {
                    button(
                        variant: if choice == range { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                        size: ButtonSize::Sm,
                        attrs: attributes! {
                            type="button"
                            aria-pressed=(if choice == range { "true" } else { "false" })
                            hx-get=(format!("/dashboard/fragment?{query}"))
                            hx-target="#dashboard"
                            hx-swap="outerHTML"
                            hx-push-url=(format!("/?{query}"))
                        },
                        (choice.label())
                    )
                }
            </div>
            match chart {
                Some(chart) => history_chart(chart: chart, unit: unit, range: range),
                None => <p class="text-sm text-muted-foreground">(format!("No readings in the last {}.", range.label()))</p>,
            }
        </div>
    })
}

#[component]
async fn rules_block(summary: RulesSummary) -> Result<impl View> {
    Ok(view! {
        <div class="space-y-4 text-sm">
            <div class="space-y-2">
                <p class="font-medium">"Next"</p>
                if summary.upcoming.is_empty() {
                    <p class="text-muted-foreground">"No scheduled rules."</p>
                }
                <ul class="space-y-1">
                    for (name, when) in summary.upcoming {
                        <li class="flex justify-between gap-3">
                            <a href=(format!("/rules#{}", rule_anchor(&name))) class="hover:underline">(name)</a>
                            <span class="shrink-0 text-muted-foreground tabular-nums">(when)</span>
                        </li>
                    }
                </ul>
            </div>
            if !summary.failed.is_empty() {
                <div class="space-y-2">
                    <p class="font-medium text-destructive">"Failed"</p>
                    <ul class="space-y-2">
                        for (name, ago, errors) in summary.failed {
                            <li>
                                <div class="flex justify-between gap-3">
                                    <a href=(format!("/rules#{}", rule_anchor(&name))) class="hover:underline">(name)</a>
                                    <span class="shrink-0 text-muted-foreground">(ago_label(ago))</span>
                                </div>
                                <p class="text-xs text-destructive">(errors.join("; "))</p>
                            </li>
                        }
                    </ul>
                </div>
            }
        </div>
    })
}

#[component]
async fn events_block(items: Vec<Activity>) -> Result<impl View> {
    Ok(view! {
        if items.is_empty() {
            <p class="text-sm text-muted-foreground">"Nothing yet."</p>
        }
        <ul class="divide-y divide-border text-sm">
            for item in items {
                <li class="flex justify-between gap-3 py-2 first:pt-0 last:pb-0">
                    <div class="min-w-0">
                        match item.link {
                            Some(link) => <a href=(link) class="font-medium hover:underline">(item.title)</a>,
                            None => <p class="font-medium">(item.title)</p>,
                        }
                        <p class="truncate text-muted-foreground">(item.detail)</p>
                    </div>
                    <span class="shrink-0 text-xs text-muted-foreground">(ago_label(item.ago))</span>
                </li>
            }
        </ul>
    })
}
