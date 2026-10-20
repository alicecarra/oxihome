use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use jiff::{SignedDuration, Timestamp};
use oxihome_protocol::{Unit, Value};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, parse_query_params, path_param},
    view::{View, attributes, component, view},
};

use super::buttons::{RuleButton, rule_button_view, rule_buttons};
use super::chart::{CHART_BUCKETS, Chart, ChartRange, chart, history_chart, series};
use super::feeds::{FeedList, feed_list, feeds_block};
use super::format::{ago_label, bool_label, rule_anchor, value_label};
use super::job::{JobView, job_block, job_view};
use super::layout::layout;
use super::nodes::{channel_controls, channel_value, presence_badge, protocol_badge};
use super::overview::{self, Activity, DashboardChannel, NodeOverview, Presence, RulesSummary};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::config::dashboard::{Block, BlockKind, ChannelReference, DEFAULT_EVENTS, Dashboard};
use crate::hub::{Hub, node_event};

/// A block with what it shows, read before rendering.
struct BlockView {
    index: usize,
    title: String,
    width: u8,
    /// What makes the block fetch itself again.
    trigger: String,
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
    Feeds {
        list: Option<FeedList>,
        device_id: String,
        limit: usize,
        show_read: bool,
        /// The dashboard query that shows or hides the read items of this block.
        toggle_query: String,
    },
    Job(JobView),
}

const UPCOMING_RULES: usize = 5;

path_param!(index);

#[page("/")]
pub async fn dashboard_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let (blocks, query) = block_views(hub, &Picked::from_query(cx)).await;
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
    let (blocks, query) = block_views(hub, &Picked::from_query(cx)).await;
    Ok(view! { dashboard_root(blocks: blocks, query: query) })
}

/// One block, refreshed alone when what it shows changes.
#[page("/dashboard/block/{index}")]
pub async fn dashboard_block_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let index = path_param::<Index>(cx).to_string().parse::<usize>().ok();
    let picked = Picked::from_query(cx);
    let (block, query) = match (index, hub.dashboard()) {
        (Some(index), Some(dashboard)) if index < dashboard.blocks.len() => {
            let state = PageState::new(&dashboard, &picked);
            let block = state.block_view(hub, &dashboard.blocks[index], index).await;
            (Some(block), state.query())
        }
        _ => (None, String::new()),
    };
    Ok(view! {
        match block {
            Some(block) => dashboard_block(block: block, query: query),
            // dashboard.toml changed under the page: reload all of it.
            None => <div hx-get=(format!("/dashboard/fragment?{query}")) hx-trigger="load" hx-target="#dashboard" hx-swap="outerHTML"></div>,
        }
    })
}

/// What was picked on the page, by block index: `range<block>=6h` for a chart's range,
/// `read<block>=1` for a feeds block showing its read items.
struct Picked {
    ranges: HashMap<usize, String>,
    read: HashSet<usize>,
}

impl Picked {
    fn from_query(cx: &Cx) -> Self {
        let mut picked = Picked {
            ranges: HashMap::new(),
            read: HashSet::new(),
        };
        for (key, value) in parse_query_params::<HashMap<String, String>>(cx).unwrap_or_default() {
            if let Some(index) = key
                .strip_prefix("range")
                .and_then(|index| index.parse().ok())
            {
                picked.ranges.insert(index, value);
            } else if let Some(index) = key
                .strip_prefix("read")
                .and_then(|index| index.parse().ok())
                && value == "1"
            {
                picked.read.insert(index);
            }
        }
        picked
    }
}

/// The query keeping every history block's range and every feeds block showing its read items.
fn dashboard_query(ranges: &[(usize, ChartRange)], read_shown: &[usize]) -> String {
    ranges
        .iter()
        .map(|&(index, range)| format!("range{index}={}", range.key()))
        .chain(read_shown.iter().map(|index| format!("read{index}=1")))
        .collect::<Vec<_>>()
        .join("&")
}

/// What every block shows, from what was picked on the page.
struct PageState {
    ranges: Vec<(usize, ChartRange)>,
    read_shown: Vec<usize>,
}

impl PageState {
    fn new(dashboard: &Dashboard, picked: &Picked) -> Self {
        let ranges = dashboard
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| block.kind == BlockKind::History)
            .map(|(index, block)| {
                let key = picked
                    .ranges
                    .get(&index)
                    .map(String::as_str)
                    .or(block.range.as_deref())
                    .unwrap_or("24h");
                (index, ChartRange::from_key(Some(key)))
            })
            .collect();
        let read_shown = dashboard
            .blocks
            .iter()
            .enumerate()
            .filter(|(index, block)| block.kind == BlockKind::Feeds && picked.read.contains(index))
            .map(|(index, _)| index)
            .collect();
        PageState { ranges, read_shown }
    }

    /// The query that keeps what was picked on every block.
    fn query(&self) -> String {
        dashboard_query(&self.ranges, &self.read_shown)
    }

    async fn block_view(&self, hub: &Hub, block: &Block, index: usize) -> BlockView {
        let range = self
            .ranges
            .iter()
            .find(|(history, _)| *history == index)
            .map(|(_, range)| *range)
            .unwrap_or(ChartRange::Day);
        let choices = ChartRange::ALL
            .into_iter()
            .map(|choice| {
                let changed: Vec<(usize, ChartRange)> = self
                    .ranges
                    .iter()
                    .map(|&(history, range)| {
                        (history, if history == index { choice } else { range })
                    })
                    .collect();
                (choice, dashboard_query(&changed, &self.read_shown))
            })
            .collect();
        let show_read = self.read_shown.contains(&index);
        let toggled: Vec<usize> = if show_read {
            self.read_shown
                .iter()
                .copied()
                .filter(|shown| *shown != index)
                .collect()
        } else {
            self.read_shown.iter().copied().chain([index]).collect()
        };
        let read = ReadToggle {
            shown: show_read,
            query: dashboard_query(&self.ranges, &toggled),
        };
        let mut view = block_view(hub, block, range, choices, read).await;
        view.index = index;
        view.trigger = block_trigger(block, range);
        view
    }
}

/// The blocks and the query that keeps what was picked on them.
async fn block_views(hub: &Hub, picked: &Picked) -> (Vec<BlockView>, String) {
    let Some(dashboard) = hub.dashboard() else {
        return (Vec::new(), String::new());
    };
    let state = PageState::new(&dashboard, picked);
    let mut views = Vec::new();
    for (index, block) in dashboard.blocks.iter().enumerate() {
        views.push(state.block_view(hub, block, index).await);
    }
    (views, state.query())
}

/// Every block also refreshes after one of its controls and once a minute, in case an event was
/// missed. A chart refreshes at most once per bucket (from 6 s to a minute): it does not change
/// sooner.
fn block_trigger(block: &Block, range: ChartRange) -> String {
    let node = |device_id: &str| {
        node_event(device_id).map_or_else(|| "sse:nodes".to_string(), |name| format!("sse:{name}"))
    };
    let mut triggers: Vec<String> = match block.kind {
        BlockKind::Nodes => vec!["sse:presence".into()],
        BlockKind::Channels => {
            let mut triggers = Vec::new();
            for channel in &block.channels {
                let trigger = node(&channel.device);
                if !triggers.contains(&trigger) {
                    triggers.push(trigger);
                }
            }
            triggers
        }
        BlockKind::Value => block
            .channel
            .iter()
            .map(|channel| node(&channel.device))
            .collect(),
        BlockKind::History => {
            let throttle = range.bucket_length().as_secs().clamp(6, 60);
            block
                .channel
                .iter()
                .map(|channel| format!("{} throttle:{throttle}s", node(&channel.device)))
                .collect()
        }
        BlockKind::Feeds | BlockKind::Job => {
            block.device.iter().map(|device| node(device)).collect()
        }
        BlockKind::Buttons | BlockKind::Rules => {
            vec!["sse:config".into(), "sse:timeline".into()]
        }
        BlockKind::Events => vec!["sse:timeline".into(), "sse:notifications".into()],
    };
    triggers.extend(["every 60s".into(), "refresh".into()]);
    triggers.join(", ")
}

/// Whether a feeds block shows its read items, and the query that flips it.
struct ReadToggle {
    shown: bool,
    query: String,
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
    read: ReadToggle,
) -> BlockView {
    let (default_title, content) = match block.kind {
        BlockKind::Nodes => (
            "Nodes".to_string(),
            BlockContent::Nodes(overview::nodes(hub)),
        ),
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
                    index: 0,
                    title: block.title.clone().unwrap_or_default(),
                    width: block.width,
                    trigger: String::new(),
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
            let notifications = hub
                .recent_notifications(limit)
                .await
                .unwrap_or_else(|error| {
                    println!("dashboard: recent notifications: {error:#}");
                    Vec::new()
                });
            (
                "Recent events".to_string(),
                BlockContent::Events(overview::activity(hub, events, notifications, limit)),
            )
        }
        BlockKind::Feeds => {
            let device_id = block.device.clone().unwrap_or_default();
            let list = feed_list(hub, &device_id).await;
            let title = list
                .as_ref()
                .map_or_else(|| device_id.clone(), |list| list.node_label.clone());
            (
                title,
                BlockContent::Feeds {
                    list,
                    device_id,
                    limit: block.limit.unwrap_or(DEFAULT_EVENTS),
                    show_read: read.shown,
                    toggle_query: read.query,
                },
            )
        }
        BlockKind::Job => {
            let (title, job) = job_view(hub, block);
            (title, BlockContent::Job(job))
        }
    };
    BlockView {
        index: 0,
        title: block.title.clone().unwrap_or(default_title),
        width: block.width,
        trigger: String::new(),
        content,
    }
}

/// Each block refreshes itself; the whole dashboard only when a config file changes, since
/// dashboard.toml may have moved its blocks. Both keep the query, so picked chart ranges stay.
#[component]
async fn dashboard_root(blocks: Vec<BlockView>, query: String) -> Result<impl View> {
    Ok(view! {
        <div
            id="dashboard"
            hx-get=(format!("/dashboard/fragment?{query}"))
            hx-trigger="sse:config"
            hx-swap="outerHTML"
            // Buttons and controls inside would swap `#status` out with outerHTML.
            hx-disinherit="*"
        >
            dashboard_blocks(blocks: blocks, query: query)
        </div>
    })
}

#[component]
async fn dashboard_blocks(blocks: Vec<BlockView>, query: String) -> Result<impl View> {
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
                dashboard_block(block: block, query: query.clone())
            }
        </div>
    })
}

#[component]
async fn dashboard_block(block: BlockView, query: String) -> Result<impl View> {
    Ok(view! {
        <div
            class=(width_class(block.width))
            hx-get=(format!("/dashboard/block/{}?{query}", block.index))
            hx-trigger=(block.trigger)
            hx-swap="outerHTML"
            hx-disinherit="*"
        >
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
                        BlockContent::Feeds { list, device_id, limit, show_read, toggle_query } => feeds_block(
                            list: list,
                            device_id: device_id,
                            limit: limit,
                            show_read: show_read,
                            toggle_query: toggle_query
                        ),
                        BlockContent::Job(job) => job_block(job: job),
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
async fn dashboard_channel_row(
    channel: DashboardChannel,
    several_nodes: bool,
) -> Result<impl View> {
    let label = if several_nodes {
        format!("{} · {}", channel.node_label, channel.label)
    } else {
        channel.label.clone()
    };
    let percent = percent(&channel);
    Ok(view! {
        <li class="flex items-center justify-between gap-4 py-3">
            <div class="min-w-0 flex-1 space-y-1">
                <p class="text-sm font-medium">(label.clone())</p>
                match channel.channel.clone() {
                    Ok(overview) => channel_value(channel: overview),
                    Err(reason) => <p class="text-sm text-muted-foreground">(reason)</p>,
                }
                if let Some(percent) = percent {
                    percent_bar(percent: percent)
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
        Ok(overview) => match &overview.value {
            Some(Value::Bool(on)) => bool_label(*on, overview.switchable).to_string(),
            Some(value) => value_label(value, overview.unit),
            None => "—".to_string(),
        },
        Err(reason) => (*reason).to_string(),
    }
}

/// From 0 to 100, for a number in percent.
fn percent(channel: &DashboardChannel) -> Option<f64> {
    let overview = channel.channel.as_ref().ok()?;
    if overview.unit != Some(Unit::Percent) {
        return None;
    }
    match overview.value.as_ref()? {
        Value::Int(number) => Some(f64::from(*number).clamp(0.0, 100.0)),
        Value::Float(number) => Some(f64::from(*number).clamp(0.0, 100.0)),
        _ => None,
    }
}

#[component]
async fn percent_bar(percent: f64) -> Result<impl View> {
    Ok(view! {
        <div class="h-1.5 w-full max-w-48 overflow-hidden rounded-full bg-muted-foreground/20" role="presentation">
            <div class="h-full rounded-full bg-foreground/70" style=(format!("width: {percent:.0}%"))></div>
        </div>
    })
}

#[component]
async fn value_block(channel: DashboardChannel, online: bool) -> Result<impl View> {
    let value = current_value(&channel);
    let percent = percent(&channel);
    Ok(view! {
        <div class=(if online { "space-y-1" } else { "space-y-1 opacity-60" })>
            <p class="text-4xl font-semibold tracking-tight tabular-nums">(value)</p>
            if let Some(percent) = percent {
                percent_bar(percent: percent)
            }
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
    let unit = channel
        .channel
        .as_ref()
        .ok()
        .and_then(|overview| overview.unit);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> Block {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn blocks_listen_to_what_they_show() {
        let channels = block(
            "type = \"channels\"\nchannels = [\
             { device = \"printer\", entity = 1, channel = 0 },\
             { device = \"weather\", entity = 1, channel = 0 },\
             { device = \"printer\", entity = 2, channel = 0 },\
             { device = \"lamp.1\", entity = 1, channel = 0 }]",
        );
        assert_eq!(
            block_trigger(&channels, ChartRange::Day),
            "sse:node-printer, sse:node-weather, sse:nodes, every 60s, refresh"
        );
        assert_eq!(
            block_trigger(&block("type = \"nodes\""), ChartRange::Day),
            "sse:presence, every 60s, refresh"
        );
        assert_eq!(
            block_trigger(&block("type = \"events\""), ChartRange::Day),
            "sse:timeline, sse:notifications, every 60s, refresh"
        );
        assert_eq!(
            block_trigger(
                &block("type = \"feeds\"\ndevice = \"youtube\""),
                ChartRange::Day
            ),
            "sse:node-youtube, every 60s, refresh"
        );
    }

    #[test]
    fn charts_refresh_at_most_once_per_bucket() {
        let history = block(
            "type = \"history\"\nchannel = { device = \"weather\", entity = 1, channel = 0 }",
        );
        let trigger = |range| block_trigger(&history, range);
        assert!(trigger(ChartRange::FifteenMinutes).starts_with("sse:node-weather throttle:6s,"));
        assert!(trigger(ChartRange::Hour).starts_with("sse:node-weather throttle:25s,"));
        assert!(trigger(ChartRange::Day).starts_with("sse:node-weather throttle:60s,"));
    }
}
