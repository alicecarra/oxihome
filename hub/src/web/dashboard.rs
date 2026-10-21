use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, parse_query_params, path_param},
    view::{View, attributes, component, view},
};

use super::chart::{Chart, ChartRange};
use super::format::query_value;
use super::layout::layout;
use super::overview::{self, Activity, DashboardChannel, NodeOverview, RulesSummary};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::config::dashboard::{Block, BlockKind, ChannelReference, DEFAULT_EVENTS, Dashboard};
use crate::hub::{Hub, node_event};

mod attention;
mod buttons;
mod channels;
mod events;
mod feeds;
mod history;
mod job;
mod nodes;
mod rules;
mod tabs;
mod value;

use buttons::{RuleButton, buttons_block, rule_buttons};
use channels::channels_block;
use events::events_block;
use feeds::{FeedList, feed_list, feeds_block};
use history::{history, history_block};
use job::{JobView, job_block, job_view};
use nodes::node_status;
use rules::rules_block;
use tabs::{TabRow, dashboard_tabs, tab_row};
use value::value_block;

pub use buttons::press;
pub use tabs::dashboard_tabs_fragment;

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
        /// Each range with where it shows this block's chart in it.
        choices: Vec<(ChartRange, Link)>,
    },
    Rules(RulesSummary),
    Events(Vec<Activity>),
    Feeds {
        list: Option<FeedList>,
        device_id: String,
        limit: usize,
        show_read: bool,
        /// Where this block shows or hides its read items.
        toggle: Link,
    },
    Job(JobView),
}

const UPCOMING_RULES: usize = 5;

path_param!(index);
path_param!(name);

/// The first dashboard.
#[page("/")]
pub async fn dashboard_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let shown = shown_dashboard(hub, None, &Picked::from_query(cx)).await;
    Ok(view! {
        layout(
            title: "Oxihome",
            dashboard_root(shown: shown)
            <script src="/static/sticky-header.js"></script>
        )
    })
}

/// Any dashboard by name, the first one too.
#[page("/d/{name}")]
pub async fn named_dashboard_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let name = path_param::<Name>(cx).to_string();
    let title = format!("{name} · Oxihome");
    let shown = shown_dashboard(hub, Some(name), &Picked::from_query(cx)).await;
    Ok(view! {
        layout(
            title: &title,
            dashboard_root(shown: shown)
            <script src="/static/sticky-header.js"></script>
        )
    })
}

#[page("/dashboard/fragment")]
pub async fn dashboard_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let picked = Picked::from_query(cx);
    let shown = shown_dashboard(hub, picked.dashboard.clone(), &picked).await;
    Ok(view! { dashboard_root(shown: shown) })
}

/// One block, refreshed alone when what it shows changes.
#[page("/dashboard/block/{index}")]
pub async fn dashboard_block_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let index = path_param::<Index>(cx).to_string().parse::<usize>().ok();
    let picked = Picked::from_query(cx);
    let place = Place::of(hub, picked.dashboard.clone());
    let dashboard = place
        .as_ref()
        .and_then(|place| hub.dashboard(Some(&place.name)));
    let (block, query) = match (index, &place, dashboard) {
        (Some(index), Some(place), Some(dashboard)) if index < dashboard.blocks.len() => {
            let state = PageState::new(&dashboard, place.clone(), &picked);
            let block = state.block_view(hub, &dashboard.blocks[index], index).await;
            (Some(block), state.query())
        }
        _ => (None, String::new()),
    };
    Ok(view! {
        match (block, place) {
            (Some(block), Some(place)) => dashboard_block(block: block, place: place, query: query),
            // dashboard.toml changed under the page: reload all of it.
            (_, place) => <div
                hx-get=(place.map_or_else(|| "/dashboard/fragment".to_string(), |place| place.fragment("")))
                hx-trigger="load"
                hx-target="#dashboard"
                hx-swap="outerHTML"
            ></div>,
        }
    })
}

/// Where a dashboard lives: `/` for the first one, `/d/<name>` for the others, and its fragments.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    name: String,
    first: bool,
}

impl Place {
    /// The named dashboard's place, or the first one's; `None` without dashboards.
    fn of(hub: &Hub, name: Option<String>) -> Option<Self> {
        let first = hub.dashboard(None)?.name;
        let name = name.unwrap_or_else(|| first.clone());
        Some(Place {
            first: name == first,
            name,
        })
    }

    fn page(&self, query: &str) -> String {
        let path = if self.first {
            "/".to_string()
        } else {
            format!("/d/{}", query_value(&self.name))
        };
        with_query(path, query)
    }

    fn fragment(&self, query: &str) -> String {
        with_query(
            format!("/dashboard/fragment?dashboard={}", query_value(&self.name)),
            query,
        )
    }

    fn block(&self, index: usize, query: &str) -> String {
        with_query(
            format!(
                "/dashboard/block/{index}?dashboard={}",
                query_value(&self.name)
            ),
            query,
        )
    }

    fn tabs(&self) -> String {
        format!("/dashboard/tabs?dashboard={}", query_value(&self.name))
    }

    /// What swaps the dashboard to `query` and the address to go with it.
    fn link(&self, query: &str) -> Link {
        Link {
            fragment: self.fragment(query),
            page: self.page(query),
        }
    }
}

fn with_query(path: String, query: &str) -> String {
    match (query.is_empty(), path.contains('?')) {
        (true, _) => path,
        (false, true) => format!("{path}&{query}"),
        (false, false) => format!("{path}?{query}"),
    }
}

/// A choice on the page: the fragment that shows it and the address pushed for it.
pub struct Link {
    pub fragment: String,
    pub page: String,
}

/// What was picked on the page, by block index: `range<block>=6h` for a chart's range,
/// `read<block>=1` for a feeds block showing its read items. `dashboard=<name>` names the
/// dashboard for the fragments.
struct Picked {
    dashboard: Option<String>,
    ranges: HashMap<usize, String>,
    read: HashSet<usize>,
}

impl Picked {
    fn from_query(cx: &Cx) -> Self {
        let mut picked = Picked {
            dashboard: None,
            ranges: HashMap::new(),
            read: HashSet::new(),
        };
        for (key, value) in parse_query_params::<HashMap<String, String>>(cx).unwrap_or_default() {
            if key == "dashboard" {
                picked.dashboard = Some(value);
            } else if let Some(index) = key
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
    place: Place,
    ranges: Vec<(usize, ChartRange)>,
    read_shown: Vec<usize>,
}

impl PageState {
    fn new(dashboard: &Dashboard, place: Place, picked: &Picked) -> Self {
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
        PageState {
            place,
            ranges,
            read_shown,
        }
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
                let query = dashboard_query(&changed, &self.read_shown);
                (choice, self.place.link(&query))
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
            link: self.place.link(&dashboard_query(&self.ranges, &toggled)),
        };
        let mut view = block_view(hub, block, range, choices, read).await;
        view.index = index;
        view.trigger = block_trigger(block, range);
        view
    }
}

/// A dashboard as the page shows it.
struct Shown {
    /// `None` without any dashboard.
    place: Option<Place>,
    /// The name asked for is not in dashboard.toml.
    missing: bool,
    /// `None` with a single dashboard.
    tabs: Option<TabRow>,
    blocks: Vec<BlockView>,
    /// What was picked on the blocks, kept by every refresh.
    query: String,
}

async fn shown_dashboard(hub: &Hub, name: Option<String>, picked: &Picked) -> Shown {
    let place = Place::of(hub, name);
    let dashboard = place
        .as_ref()
        .and_then(|place| hub.dashboard(Some(&place.name)));
    let tabs = place.as_ref().and_then(|place| tab_row(hub, place));
    let (Some(place), Some(dashboard)) = (&place, dashboard) else {
        return Shown {
            missing: place.is_some(),
            place,
            tabs,
            blocks: Vec::new(),
            query: String::new(),
        };
    };
    let state = PageState::new(&dashboard, place.clone(), picked);
    let mut blocks = Vec::new();
    for (index, block) in dashboard.blocks.iter().enumerate() {
        blocks.push(state.block_view(hub, block, index).await);
    }
    Shown {
        place: Some(place.clone()),
        missing: false,
        tabs,
        blocks,
        query: state.query(),
    }
}

fn node_trigger(device_id: &str) -> String {
    node_event(device_id).map_or_else(|| "sse:nodes".to_string(), |name| format!("sse:{name}"))
}

/// Every block also refreshes after one of its controls and once a minute, in case an event was
/// missed. A chart refreshes at most once per bucket (from 6 s to a minute): it does not change
/// sooner.
fn block_trigger(block: &Block, range: ChartRange) -> String {
    let mut triggers: Vec<String> = match block.kind {
        BlockKind::Nodes => vec!["sse:presence".into()],
        BlockKind::Channels => {
            let mut triggers = Vec::new();
            for channel in &block.channels {
                let trigger = node_trigger(&channel.device);
                if !triggers.contains(&trigger) {
                    triggers.push(trigger);
                }
            }
            triggers
        }
        BlockKind::Value => block
            .channel
            .iter()
            .map(|channel| node_trigger(&channel.device))
            .collect(),
        BlockKind::History => {
            let throttle = range.bucket_length().as_secs().clamp(6, 60);
            block
                .channel
                .iter()
                .map(|channel| format!("{} throttle:{throttle}s", node_trigger(&channel.device)))
                .collect()
        }
        BlockKind::Feeds | BlockKind::Job => block
            .device
            .iter()
            .map(|device| node_trigger(device))
            .collect(),
        BlockKind::Buttons | BlockKind::Rules => {
            vec!["sse:config".into(), "sse:timeline".into()]
        }
        BlockKind::Events => vec!["sse:timeline".into(), "sse:notifications".into()],
    };
    triggers.extend(["every 60s".into(), "refresh".into()]);
    triggers.join(", ")
}

/// Whether a feeds block shows its read items, and where it flips.
struct ReadToggle {
    shown: bool,
    link: Link,
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
    choices: Vec<(ChartRange, Link)>,
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
                    toggle: read.link,
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
/// dashboard.toml may have moved its blocks or renamed the tabs. Both keep the query, so picked
/// chart ranges stay.
#[component]
async fn dashboard_root(shown: Shown) -> Result<impl View> {
    let Shown {
        place,
        missing,
        tabs,
        blocks,
        query,
    } = shown;
    let refresh = place.as_ref().map_or_else(
        || "/dashboard/fragment".to_string(),
        |place| place.fragment(&query),
    );
    Ok(view! {
        <div
            id="dashboard"
            hx-get=(refresh)
            hx-trigger="sse:config"
            hx-swap="outerHTML"
            // Buttons and controls inside would swap `#status` out with outerHTML.
            hx-disinherit="*"
        >
            if let Some(tabs) = tabs {
                dashboard_tabs(tabs: tabs)
            }
            match place {
                Some(place) if missing => <p class="text-muted-foreground">(format!("No dashboard named \"{}\".", place.name))</p>,
                Some(place) => dashboard_blocks(blocks: blocks, place: place, query: query),
                None => no_dashboard(),
            }
        </div>
    })
}

#[component]
async fn no_dashboard() -> Result<impl View> {
    Ok(view! {
        <p class="text-muted-foreground">
            "No dashboard yet: add blocks to dashboard.toml in "
            <a href="/settings/dashboard.toml" class="font-medium hover:underline">"Settings"</a>
            ", or see all nodes in "
            <a href="/nodes" class="font-medium hover:underline">"Nodes"</a>
            "."
        </p>
    })
}

#[component]
async fn dashboard_blocks(
    blocks: Vec<BlockView>,
    place: Place,
    query: String,
) -> Result<impl View> {
    Ok(view! {
        if blocks.is_empty() {
            <p class="text-muted-foreground">
                "No blocks in this dashboard yet: add some to dashboard.toml in "
                <a href="/settings/dashboard.toml" class="font-medium hover:underline">"Settings"</a>
                "."
            </p>
        }
        // Cards of a row share its height.
        <div class="grid gap-4 lg:grid-cols-3">
            for block in blocks {
                dashboard_block(block: block, place: place.clone(), query: query.clone())
            }
        </div>
    })
}

#[component]
async fn dashboard_block(block: BlockView, place: Place, query: String) -> Result<impl View> {
    Ok(view! {
        <div
            class=(width_class(block.width))
            hx-get=(place.block(block.index, &query))
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
                        BlockContent::Channels { channels, several_nodes } => channels_block(channels: channels, several_nodes: several_nodes),
                        BlockContent::Value { channel, online } => value_block(channel: channel, online: online),
                        BlockContent::History { channel, chart, range, choices } => history_block(channel: channel, chart: chart, range: range, choices: choices),
                        BlockContent::Rules(summary) => rules_block(summary: summary),
                        BlockContent::Events(items) => events_block(items: items),
                        BlockContent::Feeds { list, device_id, limit, show_read, toggle } => feeds_block(
                            list: list,
                            device_id: device_id,
                            limit: limit,
                            show_read: show_read,
                            toggle: toggle
                        ),
                        BlockContent::Job(job) => job_block(job: job),
                        BlockContent::Buttons(buttons) => buttons_block(buttons: buttons),
                    }
                )
            )
        </div>
    })
}

fn width_class(width: u8) -> &'static str {
    match width {
        2 => "lg:col-span-2",
        3 => "lg:col-span-3",
        _ => "",
    }
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
    fn places_name_their_dashboard() {
        let home = Place {
            name: "Home".into(),
            first: true,
        };
        let printer = Place {
            name: "3D printer".into(),
            first: false,
        };
        assert_eq!(home.page(""), "/");
        assert_eq!(home.page("range4=6h"), "/?range4=6h");
        assert_eq!(printer.page(""), "/d/3D%20printer");
        assert_eq!(
            printer.fragment("read2=1"),
            "/dashboard/fragment?dashboard=3D%20printer&read2=1"
        );
        assert_eq!(
            printer.block(3, ""),
            "/dashboard/block/3?dashboard=3D%20printer"
        );
        assert_eq!(printer.tabs(), "/dashboard/tabs?dashboard=3D%20printer");
    }

    /// A fetch without the dashboard's name would fall back to the first dashboard.
    #[tokio::test]
    async fn every_fetch_names_the_dashboard() {
        use super::attention::Attention;
        use super::tabs::{Tab, tabs_trigger};
        use topcoat::view::ViewExt;

        let place = Place {
            name: "Printer".into(),
            first: false,
        };
        let channel = DashboardChannel {
            device_id: "weather".into(),
            node_label: "Weather".into(),
            label: "Outside · Temperature".into(),
            channel: Err("not seen yet"),
        };
        let picked = Picked {
            dashboard: Some("Printer".into()),
            ranges: HashMap::new(),
            read: HashSet::from([1]),
        };
        let dashboard: Dashboard = toml::from_str(
            "name = \"Printer\"\n\
             [[blocks]]\ntype = \"history\"\nchannel = { device = \"weather\", entity = 1, channel = 0 }\n\
             [[blocks]]\ntype = \"feeds\"\ndevice = \"rss\"",
        )
        .unwrap();
        let state = PageState::new(&dashboard, place.clone(), &picked);
        let choices = ChartRange::ALL
            .into_iter()
            .map(|range| (range, place.link(&format!("range0={}", range.key()))))
            .collect();
        let block = |index, content| BlockView {
            index,
            title: String::new(),
            width: 1,
            trigger: "refresh".into(),
            content,
        };
        let shown = Shown {
            place: Some(place.clone()),
            missing: false,
            tabs: Some(TabRow {
                tabs: vec![Tab {
                    name: "Home".into(),
                    link: Place {
                        name: "Home".into(),
                        first: true,
                    }
                    .link(""),
                    current: false,
                    attention: Attention::Alert,
                }],
                refresh: place.tabs(),
                trigger: tabs_trigger(&[]),
            }),
            blocks: vec![
                block(
                    0,
                    BlockContent::History {
                        channel,
                        chart: None,
                        range: ChartRange::Day,
                        choices,
                    },
                ),
                block(
                    1,
                    BlockContent::Feeds {
                        list: Some(FeedList {
                            node_label: "RSS".into(),
                            total_unread: 0,
                            items: Vec::new(),
                        }),
                        device_id: "rss".into(),
                        limit: 10,
                        show_read: true,
                        toggle: place.link(&dashboard_query(&state.ranges, &[])),
                    },
                ),
            ],
            query: state.query(),
        };
        let cx = Cx::default();
        let html = view! { cx => dashboard_root(shown: shown) }
            .first()
            .await
            .unwrap()
            .render(&cx);
        let fetches: Vec<&str> = html
            .split("hx-get=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        // The dashboard, the tab row, a tab, 4 chart ranges, 2 blocks and the feeds toggle.
        assert_eq!(fetches.len(), 10, "{html}");
        for fetch in fetches {
            assert!(fetch.contains("dashboard="), "{fetch}");
        }
        assert!(html.contains("hx-push-url=\"/d/Printer?"), "{html}");
        assert!(html.contains("bg-destructive"), "{html}");
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
