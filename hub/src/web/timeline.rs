use std::collections::HashMap;
use std::sync::Arc;

use jiff::Timestamp;
use jiff::civil::Date;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::{IconData, icon},
    router::{page, parse_query_params},
    view::{View, component, view},
};

use super::format::{query_value, rule_anchor, value_label};
use super::icons;
use super::layout::layout;
use super::overview::{self, channel_description, event_data_label};
use crate::config::nodes::NameTarget;
use crate::hub::Hub;
use crate::node::events::EventRecord;
use crate::storage::{
    Operation, StoredCommand, TimelineEntry, TimelineKind, TimelineQuery, UpdateOutcome,
};

const PAGE_SIZE: usize = 100;

/// The kind, node and rule picked on the page.
#[derive(Clone, Default)]
struct Filter {
    kind: Option<TimelineKind>,
    device_id: Option<String>,
    rule: Option<String>,
}

impl Filter {
    fn from_query(parameters: &HashMap<String, String>) -> Self {
        Self {
            kind: parameters
                .get("show")
                .and_then(|key| TimelineKind::from_key(key)),
            device_id: parameters
                .get("node")
                .filter(|device_id| !device_id.is_empty())
                .cloned(),
            rule: parameters
                .get("rule")
                .filter(|rule| !rule.is_empty())
                .cloned(),
        }
    }

    fn query(&self) -> String {
        let mut parts = Vec::new();
        if let Some(kind) = self.kind {
            parts.push(format!("show={}", kind.key()));
        }
        if let Some(device_id) = &self.device_id {
            parts.push(format!("node={}", query_value(device_id)));
        }
        if let Some(rule) = &self.rule {
            parts.push(format!("rule={}", query_value(rule)));
        }
        parts.join("&")
    }
}

struct Line {
    time: String,
    icon: IconData,
    title: String,
    link: Option<String>,
    detail: String,
    /// Identical events in a row are shown once.
    count: usize,
    failed: bool,
}

struct Day {
    label: String,
    /// Continues the last day shown above, so it has no heading.
    continued: bool,
    lines: Vec<Line>,
}

struct TimelinePage {
    days: Vec<Day>,
    /// The URL of the next page, when there may be one.
    more: Option<String>,
}

fn kind_label(kind: TimelineKind) -> &'static str {
    match kind {
        TimelineKind::Events => "Events",
        TimelineKind::Rules => "Rule runs",
        TimelineKind::Commands => "Commands",
        TimelineKind::Notifications => "Notifications",
        TimelineKind::Updates => "Updates",
    }
}

fn parameters(cx: &Cx) -> HashMap<String, String> {
    parse_query_params::<HashMap<String, String>>(cx).unwrap_or_default()
}

/// The page after `before` (or the first one); `shown_day` is the last day already on the page.
async fn timeline_page(
    hub: &Hub,
    filter: &Filter,
    before: Option<Timestamp>,
    shown_day: Option<Date>,
) -> TimelinePage {
    let entries = hub
        .timeline(&TimelineQuery {
            kind: filter.kind,
            device_id: filter.device_id.clone(),
            rule: filter.rule.clone(),
            before,
            limit: PAGE_SIZE,
        })
        .await
        .unwrap_or_else(|error| {
            println!("timeline: {error:#}");
            Vec::new()
        });
    let more = (entries.len() == PAGE_SIZE)
        .then(|| entries.last().map(TimelineEntry::at))
        .flatten()
        .map(|oldest| {
            let day = oldest.to_zoned(hub.time_zone().clone()).date();
            let filter_query = filter.query();
            let separator = if filter_query.is_empty() { "" } else { "&" };
            format!(
                "/timeline/more?{filter_query}{separator}before={}&day={day}",
                oldest.as_millisecond()
            )
        });
    TimelinePage {
        days: days(hub, entries, shown_day),
        more,
    }
}

fn days(hub: &Hub, entries: Vec<TimelineEntry>, shown_day: Option<Date>) -> Vec<Day> {
    let time_zone = hub.time_zone().clone();
    let today = Timestamp::now().to_zoned(time_zone.clone()).date();
    let rule_names: Vec<String> = hub.rules().into_iter().map(|rule| rule.name).collect();
    // Held until the end: calling `hub` methods that lock again would deadlock.
    let snapshot = hub.snapshot();
    let mut days: Vec<(Date, Day)> = Vec::new();
    let mut previous_event: Option<EventRecord> = None;
    for entry in entries {
        let zoned = entry.at().to_zoned(time_zone.clone());
        let date = zoned.date();
        if days.last().is_none_or(|(last, _)| *last != date) {
            days.push((
                date,
                Day {
                    label: day_label(date, today),
                    continued: shown_day == Some(date) && days.is_empty(),
                    lines: Vec::new(),
                },
            ));
            previous_event = None;
        }
        let lines = &mut days.last_mut().unwrap().1.lines;
        if let TimelineEntry::Event(event, _) = &entry {
            if previous_event.as_ref() == Some(event)
                && let Some(last) = lines.last_mut()
            {
                last.count += 1;
                continue;
            }
            previous_event = Some(event.clone());
        } else {
            previous_event = None;
        }
        let time = zoned.strftime("%H:%M:%S").to_string();
        let node_link = |device_id: &str| Some(format!("/nodes/{device_id}"));
        let line = match entry {
            TimelineEntry::Event(event, _) => Line {
                time,
                icon: icons::EVENT,
                title: channel_description(
                    &snapshot.nodes,
                    &snapshot.registry,
                    &event.device_id,
                    event.entity,
                    event.channel,
                    None,
                ),
                link: node_link(&event.device_id),
                detail: match &event.data {
                    Some(data) => format!(
                        "{:?} {}",
                        event.kind,
                        event_data_label(data, &snapshot.remotes)
                    ),
                    None => format!("{:?}", event.kind),
                },
                count: 1,
                failed: false,
            },
            TimelineEntry::RuleRun(name, run) => Line {
                time,
                icon: icons::RULES,
                link: Some(format!("/rules#{}", rule_anchor(&name))),
                title: format!("Rule {name}"),
                failed: !run.errors.is_empty(),
                detail: if run.errors.is_empty() {
                    "Ran".into()
                } else {
                    format!("Failed: {}", run.errors.join("; "))
                },
                count: 1,
            },
            TimelineEntry::Command(command) => Line {
                time,
                icon: icons::COMMAND,
                title: channel_description(
                    &snapshot.nodes,
                    &snapshot.registry,
                    &command.device_id,
                    command.entity,
                    command.channel,
                    None,
                ),
                link: node_link(&command.device_id),
                failed: command.error.is_some(),
                detail: command_detail(&command),
                count: 1,
            },
            TimelineEntry::Notification(notification) => Line {
                time,
                icon: icons::NOTIFICATION,
                link: if rule_names.contains(&notification.source) {
                    Some(format!("/rules#{}", rule_anchor(&notification.source)))
                } else if snapshot.registry.node(&notification.source).is_some()
                    || snapshot
                        .nodes
                        .devices_id()
                        .any(|id| id == notification.source)
                {
                    node_link(&notification.source)
                } else {
                    None
                },
                title: notification.title,
                detail: notification.message,
                count: 1,
                failed: false,
            },
            TimelineEntry::Update(update) => {
                let outcome = match update.outcome {
                    UpdateOutcome::Updated => "updated",
                    UpdateOutcome::RolledBack => "rolled back",
                    UpdateOutcome::Failed => "failed",
                    UpdateOutcome::NoAnswer => "no answer",
                };
                let reason = update
                    .detail
                    .as_deref()
                    .map(|detail| format!(" · {detail}"))
                    .unwrap_or_default();
                Line {
                    time,
                    icon: icons::UPDATE,
                    title: format!(
                        "{} update",
                        snapshot
                            .nodes
                            .name(&NameTarget::Node(update.device_id.clone()))
                            .unwrap_or(&update.device_id)
                    ),
                    link: node_link(&update.device_id),
                    failed: update.outcome != UpdateOutcome::Updated,
                    detail: format!(
                        "{} → {}: {outcome}{reason}",
                        update.from_version, update.to_version
                    ),
                    count: 1,
                }
            }
        };
        lines.push(line);
    }
    days.into_iter().map(|(_, day)| day).collect()
}

/// `Set on · rule Night`, `Invoke Alarm · web · failed: no answer`.
fn command_detail(command: &StoredCommand) -> String {
    let operation = match command.operation {
        Operation::Set => "Set",
        Operation::Invoke => "Invoke",
    };
    let mut detail = match &command.value {
        Some(value) => format!("{operation} {}", value_label(value, None)),
        None => operation.to_string(),
    };
    match &command.rule {
        Some(rule) => detail.push_str(&format!(" · rule {rule}")),
        None => detail.push_str(" · web"),
    }
    if let Some(error) = &command.error {
        detail.push_str(&format!(" · failed: {error}"));
    }
    detail
}

fn day_label(date: Date, today: Date) -> String {
    if date == today {
        "Today".into()
    } else if today.yesterday().is_ok_and(|yesterday| yesterday == date) {
        "Yesterday".into()
    } else {
        date.strftime("%A, %-d %B").to_string()
    }
}

#[page("/timeline")]
pub async fn timeline(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let filter = Filter::from_query(&parameters(cx));
    let page = timeline_page(hub, &filter, None, None).await;
    let nodes: Vec<(String, String)> = overview::nodes(hub)
        .into_iter()
        .map(|node| {
            let name = node.name.unwrap_or_else(|| node.device_id.clone());
            (node.device_id, name)
        })
        .collect();
    let chosen_kind = filter.kind.map(TimelineKind::key).unwrap_or_default();
    let chosen_node = filter.device_id.clone().unwrap_or_default();
    let chosen_rule = filter.rule.clone().unwrap_or_default();
    let mut rule_names: Vec<String> = hub.rules().into_iter().map(|rule| rule.name).collect();
    // A rule taken out of rules.toml keeps its history.
    if !chosen_rule.is_empty() && !rule_names.contains(&chosen_rule) {
        rule_names.push(chosen_rule.clone());
    }
    Ok(view! {
        layout(
            title: "Oxihome timeline",
            <div class="space-y-4">
                <form method="get" action="/timeline" class="flex flex-wrap items-center gap-2">
                    <select name="show" class=(SELECT) onchange="this.form.submit()" aria-label="What to show">
                        <option value="" if chosen_kind.is_empty() { selected="" }>"Everything"</option>
                        for kind in TimelineKind::ALL {
                            <option value=(kind.key()) if kind.key() == chosen_kind { selected="" }>(kind_label(kind))</option>
                        }
                    </select>
                    <select name="node" class=(SELECT) onchange="this.form.submit()" aria-label="Node">
                        <option value="" if chosen_node.is_empty() { selected="" }>"All nodes"</option>
                        for (device_id, name) in nodes {
                            <option value=(device_id.clone()) if device_id == chosen_node { selected="" }>(name)</option>
                        }
                    </select>
                    <select name="rule" class=(SELECT) onchange="this.form.submit()" aria-label="Rule">
                        <option value="" if chosen_rule.is_empty() { selected="" }>"All rules"</option>
                        for name in rule_names {
                            <option value=(name.clone()) if name == chosen_rule { selected="" }>(name)</option>
                        }
                    </select>
                    if filter.device_id.is_some() && filter.kind.is_none_or(|kind| kind == TimelineKind::Rules) {
                        <p class="text-sm text-muted-foreground">"Rule runs are not about one node, so they are hidden."</p>
                    }
                    if filter.rule.is_some() && filter.kind.is_none_or(|kind| matches!(kind, TimelineKind::Events | TimelineKind::Updates)) {
                        <p class="text-sm text-muted-foreground">"Events and updates are not about a rule, so they are hidden."</p>
                    }
                </form>
                live_updates(query: filter.query())
                <div id="timeline-list">
                    timeline_items(page: page)
                </div>
            </div>
        )
    })
}

const SELECT: &str = "h-9 rounded-lg border border-border bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring";

/// The first page again, when something happens.
#[page("/timeline/fragment")]
pub async fn timeline_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let filter = Filter::from_query(&parameters(cx));
    let page = timeline_page(hub, &filter, None, None).await;
    Ok(view! { timeline_items(page: page) })
}

/// The next page, in place of the "Load more" button. Live updates stop, since they would
/// replace what was loaded.
#[page("/timeline/more")]
pub async fn timeline_more(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let parameters = parameters(cx);
    let filter = Filter::from_query(&parameters);
    let before = parameters
        .get("before")
        .and_then(|before| before.parse().ok())
        .and_then(|before| Timestamp::from_millisecond(before).ok());
    let shown_day = parameters.get("day").and_then(|day| day.parse().ok());
    let page = timeline_page(hub, &filter, before, shown_day).await;
    let latest = match filter.query() {
        query if query.is_empty() => "/timeline".to_string(),
        query => format!("/timeline?{query}"),
    };
    Ok(view! {
        timeline_items(page: page)
        <p id="timeline-live" hx-swap-oob="true" class="text-sm text-muted-foreground">
            "Live updates are paused while older entries are shown. "
            <a href=(latest) class="font-medium text-foreground hover:underline">"Back to the latest"</a>
        </p>
    })
}

#[component]
async fn live_updates(query: String) -> Result<impl View> {
    Ok(view! {
        <div
            id="timeline-live"
            hx-get=(format!("/timeline/fragment?{query}"))
            hx-trigger="sse:timeline, sse:notifications"
            hx-target="#timeline-list"
        ></div>
    })
}

#[component]
async fn timeline_items(page: TimelinePage) -> Result<impl View> {
    let empty = page.days.is_empty() && page.more.is_none();
    Ok(view! {
        if empty {
            <p class="text-muted-foreground">"Nothing happened yet."</p>
        }
        for day in page.days {
            <section class="space-y-2">
                if !day.continued {
                    <h2 class="pt-2 text-sm font-semibold text-muted-foreground">(day.label)</h2>
                }
                <ul class="divide-y divide-border rounded-lg border border-border bg-card text-sm">
                    for line in day.lines {
                        timeline_line(line: line)
                    }
                </ul>
            </section>
        }
        if let Some(more) = page.more {
            <div class="flex justify-center pt-2">
                <button
                    type="button"
                    hx-get=(more)
                    hx-target="closest div"
                    hx-swap="outerHTML"
                    class="inline-flex h-9 items-center rounded-lg border border-border px-4 text-sm font-medium hover:bg-muted"
                >"Load more"</button>
            </div>
        }
    })
}

#[component]
async fn timeline_line(line: Line) -> Result<impl View> {
    let icon_class = if line.failed {
        "mt-0.5 shrink-0 text-destructive"
    } else {
        "mt-0.5 shrink-0 text-muted-foreground"
    };
    Ok(view! {
        <li class="flex items-start gap-3 px-3 py-2">
            <span class="w-16 shrink-0 pt-px text-xs tabular-nums text-muted-foreground">(line.time)</span>
            <span class=(icon_class)>icon(data: line.icon, size: 16)</span>
            <div class="min-w-0 flex-1">
                match line.link {
                    Some(link) => <a href=(link) class="font-medium hover:underline">(line.title)</a>,
                    None => <p class="font-medium">(line.title)</p>,
                }
                <p class=(if line.failed { "break-words text-destructive" } else { "break-words text-muted-foreground" })>(line.detail)</p>
            </div>
            if line.count > 1 {
                <span class="shrink-0 text-xs tabular-nums text-muted-foreground">(format!("×{}", line.count))</span>
            }
        </li>
    })
}
