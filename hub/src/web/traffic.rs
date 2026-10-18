//! `/traffic`: the MQTT messages the hub receives and sends, live. Left out of the menu.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;

use futures_util::stream::{self, Stream};
use tokio::sync::broadcast::error::RecvError;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::{IconData, icon},
    router::{
        content::sse::{Event, KeepAlive, Sse},
        page, parse_query_params, route,
    },
    view::{View, ViewExt, component, view},
};

use super::format::query_value;
use super::icons;
use super::layout::layout;
use super::overview::{self, channel_description};
use crate::components::badge::{BadgeVariant, badge};
use crate::config::nodes::NameTarget;
use crate::hub::Hub;
use crate::node::registry::Registry;
use crate::traffic::{
    BOARD_LIMIT, BROKER_LIMIT, Direction, MessageKind, Reading, TrafficEntry, is_large,
};

mod map;

pub use map::traffic_map_fragment;

/// Older rows are removed from the page as new ones arrive.
const SHOWN: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    /// Heartbeats are most of the traffic.
    AllButHeartbeats,
    All,
    Kind(MessageKind),
    Dropped,
}

impl Show {
    fn key(self) -> String {
        match self {
            Show::AllButHeartbeats => String::new(),
            Show::All => "all".into(),
            Show::Kind(kind) => kind.name().to_lowercase(),
            Show::Dropped => "dropped".into(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Show::AllButHeartbeats => "All but heartbeats",
            Show::All => "Everything",
            Show::Kind(kind) => kind.name(),
            Show::Dropped => "Dropped",
        }
    }

    fn options() -> Vec<Show> {
        let mut options = vec![Show::AllButHeartbeats, Show::All];
        options.extend(MessageKind::ALL.map(Show::Kind));
        options.push(Show::Dropped);
        options
    }

    fn from_key(key: &str) -> Self {
        Show::options()
            .into_iter()
            .find(|show| show.key() == key)
            .unwrap_or(Show::AllButHeartbeats)
    }
}

#[derive(Clone)]
struct Filter {
    show: Show,
    device_id: Option<String>,
    /// Entity and channel; only with a node.
    channel: Option<(u8, u8)>,
}

impl Filter {
    fn from_query(parameters: &HashMap<String, String>) -> Self {
        let device_id = parameters
            .get("node")
            .filter(|device_id| !device_id.is_empty())
            .cloned();
        let channel = parameters
            .get("channel")
            .filter(|_| device_id.is_some())
            .and_then(|channel| parse_channel(channel));
        Self {
            show: Show::from_key(parameters.get("show").map_or("", String::as_str)),
            device_id,
            channel,
        }
    }

    fn query(&self) -> String {
        let mut parts = Vec::new();
        let show = self.show.key();
        if !show.is_empty() {
            parts.push(format!("show={show}"));
        }
        if let Some(device_id) = &self.device_id {
            parts.push(format!("node={}", query_value(device_id)));
        }
        if let Some((entity, channel)) = self.channel {
            parts.push(format!("channel={}", channel_key(entity, channel)));
        }
        parts.join("&")
    }

    fn matches(&self, entry: &TrafficEntry) -> bool {
        let kind = entry.kind();
        let shown = match self.show {
            Show::AllButHeartbeats => kind != Some(MessageKind::Heartbeat),
            Show::All => true,
            Show::Kind(chosen) => kind == Some(chosen),
            Show::Dropped => kind.is_none(),
        };
        // A broadcast reaches every node.
        let node = match (&self.device_id, &entry.device_id) {
            (None, _) => true,
            (Some(chosen), Some(device_id)) => chosen == device_id,
            (Some(_), None) => entry.direction == Direction::Down,
        };
        let channel = self
            .channel
            .is_none_or(|(entity, channel)| entry.touches(entity, channel));
        shown && node && channel
    }
}

fn channel_key(entity: u8, channel: u8) -> String {
    format!("{entity}.{channel}")
}

fn parse_channel(key: &str) -> Option<(u8, u8)> {
    let (entity, channel) = key.split_once('.')?;
    Some((entity.parse().ok()?, channel.parse().ok()?))
}

struct Row {
    time: String,
    direction: Direction,
    title: String,
    link: Option<String>,
    /// The message kind, or "Dropped".
    kind: &'static str,
    dropped: Option<String>,
    /// For an `Ack` or `Error`: the command it answers and how long it took.
    answer: Option<String>,
    size: String,
    /// Close to the limit of the node or the broker.
    large: bool,
    topic: String,
    /// On one line, as it came.
    text: String,
    /// Indented, when it is JSON.
    pretty: String,
}

/// Boards have a small MQTT buffer; a broadcast reaches them too.
fn limit_for(registry: &Registry, device_id: Option<&str>) -> usize {
    let board = match device_id {
        Some(device_id) => registry
            .node(device_id)
            .is_some_and(|node| node.build.name == "firmware"),
        None => true,
    };
    if board { BOARD_LIMIT } else { BROKER_LIMIT }
}

/// `180 ms`, `1.2 s`.
fn latency_label(latency: Duration) -> String {
    if latency < Duration::from_secs(1) {
        format!("{} ms", latency.as_millis())
    } else {
        format!("{:.1} s", latency.as_secs_f64())
    }
}

fn rows(hub: &Hub, entries: &[Arc<TrafficEntry>]) -> Vec<Row> {
    let time_zone = hub.time_zone().clone();
    let clock = |at: Timestamp| {
        at.to_zoned(time_zone.clone())
            .strftime("%H:%M:%S%.3f")
            .to_string()
    };
    // Looked up before the snapshot, which `answered` does not need.
    let commands: Vec<Option<Arc<TrafficEntry>>> = entries
        .iter()
        .map(|entry| match (&entry.device_id, &entry.reading) {
            (
                Some(device_id),
                Reading::Read {
                    kind: MessageKind::Ack | MessageKind::Error,
                    reply_to: Some(reply_to),
                    ..
                },
            ) => hub.traffic().answered(device_id, *reply_to),
            _ => None,
        })
        .collect();
    let snapshot = hub.snapshot();
    entries
        .iter()
        .zip(commands)
        .map(|(entry, command)| {
            // An answer is about the channel of its command.
            let about = command
                .as_ref()
                .map_or(&entry.reading, |command| &command.reading);
            let title = match (&entry.device_id, about) {
                (
                    Some(device_id),
                    Reading::Read {
                        entity: Some(entity),
                        channels,
                        ..
                    },
                ) => {
                    let channel = match channels.as_slice() {
                        [channel] => *channel,
                        _ => 0,
                    };
                    let description = channel_description(
                        &snapshot.nodes,
                        &snapshot.registry,
                        device_id,
                        *entity,
                        channel,
                        None,
                    );
                    if channels.len() > 1 {
                        format!("{description} · {} values", channels.len())
                    } else {
                        description
                    }
                }
                (Some(device_id), _) => overview::name(
                    &snapshot.nodes,
                    &snapshot.registry,
                    &NameTarget::Node(device_id.clone()),
                )
                .unwrap_or_else(|| device_id.clone()),
                (None, _) if entry.topic == "oxihome/broadcast/down" => "Every node".into(),
                (None, _) => entry.topic.clone(),
            };
            let (kind, dropped) = match &entry.reading {
                Reading::Read { kind, .. } => (kind.name(), None),
                Reading::Dropped(reason) => ("Dropped", Some(reason.clone())),
            };
            let pretty = serde_json::from_str::<serde_json::Value>(&entry.text)
                .ok()
                .and_then(|json| serde_json::to_string_pretty(&json).ok())
                .unwrap_or_else(|| entry.text.clone());
            let size = entry.text.len();
            let limit = limit_for(&snapshot.registry, entry.device_id.as_deref());
            let large = is_large(size, limit);
            Row {
                time: clock(entry.at),
                direction: entry.direction,
                title,
                link: entry
                    .device_id
                    .as_ref()
                    .map(|device_id| format!("/nodes/{device_id}")),
                kind,
                dropped,
                answer: command.map(|command| {
                    format!(
                        "Answer to the command at {}, {} later",
                        clock(command.at),
                        latency_label(
                            Duration::try_from(entry.at.duration_since(command.at))
                                .unwrap_or_default()
                        )
                    )
                }),
                size: if large {
                    format!("{} of {}", size_label(size), size_label(limit))
                } else {
                    size_label(size)
                },
                large,
                topic: entry.topic.clone(),
                text: entry.text.clone(),
                pretty,
            }
        })
        .collect()
}

fn size_label(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

fn parameters(cx: &Cx) -> HashMap<String, String> {
    parse_query_params::<HashMap<String, String>>(cx).unwrap_or_default()
}

#[page("/traffic")]
pub async fn traffic_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let filter = Filter::from_query(&parameters(cx));
    let entries: Vec<Arc<TrafficEntry>> = hub
        .traffic()
        .recent()
        .into_iter()
        .rev()
        .filter(|entry| filter.matches(entry))
        .take(SHOWN)
        .collect();
    let rows = rows(hub, &entries);
    let nodes = overview::nodes(hub);
    let channels: Vec<(String, String)> = filter
        .device_id
        .as_ref()
        .and_then(|device_id| nodes.iter().find(|node| &node.device_id == device_id))
        .map(|node| {
            let snapshot = hub.snapshot();
            node.channels
                .iter()
                .map(|channel| {
                    (
                        channel_key(channel.entity, channel.channel),
                        channel_description(
                            &snapshot.nodes,
                            &snapshot.registry,
                            &node.device_id,
                            channel.entity,
                            channel.channel,
                            None,
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let nodes: Vec<(String, String)> = nodes
        .into_iter()
        .map(|node| {
            let name = node.name.unwrap_or_else(|| node.device_id.clone());
            (node.device_id, name)
        })
        .collect();
    let chosen_show = filter.show;
    let chosen_node = filter.device_id.clone().unwrap_or_default();
    let chosen_channel = filter
        .channel
        .map(|(entity, channel)| channel_key(entity, channel))
        .unwrap_or_default();
    let stream = match filter.query() {
        query if query.is_empty() => "/traffic/events".to_string(),
        query => format!("/traffic/events?{query}"),
    };
    Ok(view! {
        layout(
            title: "Oxihome traffic",
            <div class="space-y-4">
                <a href="/tools" class="text-sm text-muted-foreground hover:underline">"← Tools"</a>
                <div>
                    <h1 class="text-2xl font-semibold">"Traffic"</h1>
                    <p class="text-sm text-muted-foreground">
                        "MQTT messages the hub receives and sends. Kept in memory and gone on restart."
                    </p>
                </div>
                map::live_map(filter: filter.clone())
                <div>
                    <h2 class="text-lg font-semibold">"Messages"</h2>
                    <p class="text-sm text-muted-foreground">"Newest first, from the last 500."</p>
                </div>
                <form method="get" action="/traffic" class="flex flex-wrap items-center gap-2">
                    <select name="show" class=(SELECT) onchange="this.form.submit()" aria-label="What to show">
                        for show in Show::options() {
                            <option value=(show.key()) if show == chosen_show { selected="" }>(show.label())</option>
                        }
                    </select>
                    <select name="node" class=(SELECT) onchange="this.form.channel && (this.form.channel.value = ''); this.form.submit()" aria-label="Node">
                        <option value="" if chosen_node.is_empty() { selected="" }>"All nodes"</option>
                        for (device_id, name) in nodes {
                            <option value=(device_id.clone()) if device_id == chosen_node { selected="" }>(name)</option>
                        }
                    </select>
                    if !channels.is_empty() {
                        <select name="channel" class=(SELECT) onchange="this.form.submit()" aria-label="Channel">
                            <option value="" if chosen_channel.is_empty() { selected="" }>"All channels"</option>
                            for (key, label) in channels {
                                <option value=(key.clone()) if key == chosen_channel { selected="" }>(label)</option>
                            }
                        </select>
                    }
                    <button
                        type="button"
                        id="traffic-pause"
                        aria-pressed="false"
                        class="inline-flex h-9 items-center rounded-lg border border-border px-4 text-sm font-medium hover:bg-foreground/5 aria-pressed:bg-foreground/10"
                    >"Pause"</button>
                </form>
                <div sse-connect=(stream)>
                    <ul
                        id="traffic-list"
                        sse-swap="traffic"
                        hx-swap="afterbegin"
                        data-shown=(SHOWN.to_string())
                        class="peer divide-y divide-border rounded-lg border border-border bg-card text-sm empty:hidden"
                    >
                        for row in rows {
                            traffic_row(row: row)
                        }
                    </ul>
                    <p class="hidden text-muted-foreground peer-empty:block">"No messages yet."</p>
                </div>
            </div>
            <script src="/static/traffic.js"></script>
        )
    })
}

const SELECT: &str = "h-9 rounded-lg border border-border bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring";

/// One `traffic` event per new message the filter lets through, carrying its row.
#[route(GET "/traffic/events")]
pub async fn traffic_events(cx: &Cx) -> Result<Sse<impl Stream<Item = Result<Event>> + use<>>> {
    let hub: Arc<Hub> = app_context::<Arc<Hub>>(cx).clone();
    let filter = Filter::from_query(&parameters(cx));
    let cx: Cx = cx.clone();
    // Ends when the hub shuts down, so the web server does not wait for open pages.
    let shutdown = Box::pin(hub.shutdown());
    let receiver = hub.traffic().subscribe();
    let rows = stream::unfold((receiver, shutdown), move |(mut receiver, mut shutdown)| {
        let hub = hub.clone();
        let filter = filter.clone();
        let cx = cx.clone();
        async move {
            loop {
                let received = tokio::select! {
                    received = receiver.recv() => received,
                    () = &mut shutdown => return None,
                };
                match received {
                    Ok(entry) if filter.matches(&entry) => {
                        let event = rendered_row(&hub, &cx, entry)
                            .await
                            .map(|html| Event::new().event("traffic").data(html));
                        return Some((event, (receiver, shutdown)));
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                }
            }
        }
    });
    Ok(Sse::new(rows).keep_alive(KeepAlive::new()))
}

/// Kept out of the route so the stream does not borrow the request.
async fn rendered_row(hub: &Hub, cx: &Cx, entry: Arc<TrafficEntry>) -> Result<String> {
    let row = rows(hub, &[entry]).pop().expect("one row per entry");
    Ok(view! { cx => traffic_row(row: row) }
        .first()
        .await?
        .render(cx))
}

#[component]
async fn traffic_row(row: Row) -> Result<impl View> {
    let (arrow, direction_label): (IconData, &str) = match row.direction {
        Direction::Up => (icons::UP, "Received"),
        Direction::Down => (icons::DOWN, "Sent"),
    };
    let variant = if row.dropped.is_some() {
        BadgeVariant::Destructive
    } else {
        BadgeVariant::Secondary
    };
    Ok(view! {
        <li>
            <details class="group">
                <summary class="flex cursor-pointer list-none items-start gap-3 px-3 py-2 hover:bg-foreground/5">
                    <span class="w-24 shrink-0 pt-px text-xs tabular-nums text-muted-foreground">(row.time)</span>
                    <span class="mt-0.5 shrink-0 text-muted-foreground" title=(direction_label)>icon(data: arrow, size: 16)</span>
                    <div class="min-w-0 flex-1">
                        <div class="flex flex-wrap items-center gap-2">
                            <span class="font-medium">(row.title)</span>
                            badge(variant: variant, (row.kind))
                        </div>
                        if let Some(answer) = row.answer {
                            <p class="text-xs text-muted-foreground">(answer)</p>
                        }
                        match row.dropped {
                            Some(reason) => <p class="break-words text-destructive">(reason)</p>,
                            None => <p class="truncate font-mono text-xs text-muted-foreground group-open:hidden">(row.text)</p>,
                        }
                    </div>
                    if row.large {
                        badge(variant: BadgeVariant::Destructive, (row.size))
                    } else {
                        <span class="shrink-0 text-xs tabular-nums text-muted-foreground">(row.size)</span>
                    }
                </summary>
                <div class="space-y-2 border-t border-border bg-background/50 px-3 py-2">
                    <div class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                        <span class="font-mono">(row.topic)</span>
                        if let Some(link) = row.link {
                            <a href=(link) class="font-medium text-foreground hover:underline">"Open node"</a>
                        }
                    </div>
                    <pre class="overflow-x-auto font-mono text-xs">(row.pretty)</pre>
                </div>
            </details>
        </li>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traffic::Traffic;

    fn entry(topic: &str, direction: Direction, reading: Reading) -> Arc<TrafficEntry> {
        let traffic = Traffic::default();
        traffic.record(direction, topic, "{}", reading);
        traffic.recent().pop().unwrap()
    }

    fn read(kind: MessageKind, entity: Option<u8>, channels: Vec<u8>) -> Reading {
        Reading::Read {
            kind,
            entity,
            channels,
            message_id: 0,
            reply_to: None,
        }
    }

    fn filter(query: &[(&str, &str)]) -> Filter {
        Filter::from_query(
            &query
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        )
    }

    #[test]
    fn heartbeats_are_hidden_by_default() {
        let heartbeat = entry(
            "oxihome/a/up",
            Direction::Up,
            read(MessageKind::Heartbeat, None, Vec::new()),
        );
        assert!(!filter(&[]).matches(&heartbeat));
        assert!(filter(&[("show", "all")]).matches(&heartbeat));
        assert!(filter(&[("show", "heartbeat")]).matches(&heartbeat));
    }

    #[test]
    fn a_node_sees_its_messages_and_broadcasts() {
        let chosen = filter(&[("node", "a")]);
        let query = read(MessageKind::Query, None, Vec::new());
        assert!(chosen.matches(&entry("oxihome/a/up", Direction::Up, query.clone())));
        assert!(!chosen.matches(&entry("oxihome/b/up", Direction::Up, query.clone())));
        assert!(chosen.matches(&entry("oxihome/broadcast/down", Direction::Down, query)));
    }

    #[test]
    fn channel_needs_a_node() {
        let state = entry(
            "oxihome/a/up",
            Direction::Up,
            read(MessageKind::State, Some(1), vec![0, 2]),
        );
        assert!(filter(&[("node", "a"), ("channel", "1.2")]).matches(&state));
        assert!(!filter(&[("node", "a"), ("channel", "1.1")]).matches(&state));
        assert!(filter(&[("channel", "1.1")]).channel.is_none());
    }

    #[test]
    fn latency_switches_to_seconds() {
        assert_eq!(latency_label(Duration::from_millis(180)), "180 ms");
        assert_eq!(latency_label(Duration::from_millis(1250)), "1.2 s");
    }

    #[test]
    fn query_round_trips() {
        let original = filter(&[("show", "dropped"), ("node", "a b"), ("channel", "3.0")]);
        assert_eq!(original.query(), "show=dropped&node=a%20b&channel=3.0");
        assert!(filter(&[]).query().is_empty());
    }
}
