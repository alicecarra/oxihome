use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use oxihome_protocol::hardware::{Driver, HardwareEntity};
use oxihome_protocol::management;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, parse_query_params, path_param, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::chart::{CHART_BUCKETS, Chart, ChartRange, chart, history_chart, series};
use super::format::{
    ago_label, channel_label, duration_label, elapsed, entity_label, rule_anchor, value_label,
};
use super::layout::layout;
use super::nodes::{channel_controls, channel_value, presence_badge, protocol_badge};
use super::overview::{self, ChannelOverview, NodeOverview, ParameterOverview};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::config::nodes::{EntityEdit, NameTarget, NodeEdits, parameter_key};
use crate::config::rules::{RuleRole, RuleUse};
use crate::hub::{Hub, UpdateStatus};
use crate::node::hardware::{HardwareState, HardwareStatus};
use crate::node::parameters;
use crate::storage::{StoredNotification, StoredUpdate, UpdateOutcome};

/// Rows of the node's history card.
const HISTORY_ROWS: usize = 10;

path_param!(device_id);

/// What happened to the node: its over-the-air updates (kept for good) and the notifications about
/// it (kept as long as the rest of the history).
#[derive(Clone, Default)]
struct History {
    updates: Vec<StoredUpdate>,
    notifications: Vec<StoredNotification>,
}

struct NodeView {
    node: NodeOverview,
    parameters: Vec<ParameterOverview>,
    update: Option<UpdateStatus>,
    history: History,
    rules: Vec<RuleUse>,
    /// By entity and channel; only the fields section draws them.
    charts: HashMap<(u8, u8), Chart>,
    range: ChartRange,
}

#[derive(Deserialize)]
struct RangeQuery {
    range: Option<String>,
}

fn chart_range(cx: &Cx) -> ChartRange {
    let query = parse_query_params::<RangeQuery>(cx).ok();
    ChartRange::from_key(query.as_ref().and_then(|query| query.range.as_deref()))
}

async fn node_view(
    hub: &Hub,
    device_id: &str,
    section: Option<Section>,
    range: ChartRange,
) -> Option<NodeView> {
    let node = overview::node(hub, device_id)?;
    let parameters = overview::parameters(hub)
        .into_iter()
        .find(|node| node.device_id == device_id)
        .map(|node| node.parameters)
        .unwrap_or_default();
    let rules = hub.rules_involving(device_id);
    let charts = if section.is_none_or(|section| section == Section::Fields) {
        charts(hub, &node, range).await
    } else {
        HashMap::new()
    };
    let history = if section.is_none() {
        history(hub, device_id).await
    } else {
        History::default()
    };
    Some(NodeView {
        update: hub.update_status(device_id),
        history,
        node,
        parameters,
        rules,
        charts,
        range,
    })
}

async fn charts(hub: &Hub, node: &NodeOverview, range: ChartRange) -> HashMap<(u8, u8), Chart> {
    let since = Timestamp::now() - jiff::SignedDuration::try_from(range.span()).unwrap_or_default();
    let mut charts = HashMap::new();
    for channel in node.channels.iter().filter(|channel| channel.chartable) {
        match hub
            .history(
                &node.device_id,
                channel.entity,
                channel.channel,
                since,
                range.bucket_length(),
            )
            .await
        {
            Ok(history) => {
                if let Some(drawn) = chart(&series(&history, CHART_BUCKETS)) {
                    charts.insert((channel.entity, channel.channel), drawn);
                }
            }
            Err(error) => println!(
                "history of {} {}/{}: {error:#}",
                node.device_id, channel.entity, channel.channel
            ),
        }
    }
    charts
}

#[page("/nodes/{device_id}")]
pub async fn node_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let device_id = path_param::<DeviceId>(cx).to_string();
    let node = node_view(hub, &device_id, None, chart_range(cx)).await;
    Ok(view! {
        layout(
            title: "Oxihome node",
            <div class="space-y-4">
                <a href="/nodes" class="text-sm text-muted-foreground hover:underline">"← Nodes"</a>
                match node {
                    None => <p class="text-muted-foreground">(format!("Unknown node {device_id}: it has never sent a Hello."))</p>,
                    Some(node) => {
                        overview_section(node: node.node.clone(), parameters: node.parameters, update: node.update, editing: false)
                        fields_section(node: node.node.clone(), rules: node.rules.clone(), charts: node.charts, range: node.range, editing: false)
                        hardware_section(device_id: device_id.clone(), rows: hardware_rows(hub, &device_id), status: hub.hardware_status(&device_id))
                        history_section(device_id: device_id.clone(), history: node.history)
                        rules_section(channels: node.node.channels, rules: node.rules)
                    },
                }
            </div>
        )
    })
}

// Each section swaps itself between showing and editing. Shown, it refreshes on every change;
// being edited, it does not, so typing is kept.

#[page("/nodes/{device_id}/overview")]
pub async fn overview_fragment(cx: &Cx) -> Result<impl View> {
    let node = fragment_node(cx, Section::Overview).await;
    Ok(view! { section_body(node: node, section: Section::Overview, editing: false) })
}

#[page("/nodes/{device_id}/overview/edit")]
pub async fn overview_edit_fragment(cx: &Cx) -> Result<impl View> {
    let node = fragment_node(cx, Section::Overview).await;
    Ok(view! { section_body(node: node, section: Section::Overview, editing: true) })
}

#[page("/nodes/{device_id}/fields")]
pub async fn fields_fragment(cx: &Cx) -> Result<impl View> {
    let node = fragment_node(cx, Section::Fields).await;
    Ok(view! { section_body(node: node, section: Section::Fields, editing: false) })
}

#[page("/nodes/{device_id}/fields/edit")]
pub async fn fields_edit_fragment(cx: &Cx) -> Result<impl View> {
    let node = fragment_node(cx, Section::Fields).await;
    Ok(view! { section_body(node: node, section: Section::Fields, editing: true) })
}

async fn fragment_node(cx: &Cx, section: Section) -> Option<NodeView> {
    let hub: &Arc<Hub> = app_context(cx);
    node_view(
        hub,
        path_param::<DeviceId>(cx),
        Some(section),
        chart_range(cx),
    )
    .await
}

#[derive(Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Section {
    Overview,
    Fields,
}

/// `out_of_band` marks it to replace the section wherever it is (after a save).
#[component]
async fn section_body(
    node: Option<NodeView>,
    section: Section,
    editing: bool,
    #[default] out_of_band: bool,
) -> Result<impl View> {
    Ok(view! {
        if let Some(node) = node {
            match section {
                Section::Overview => overview_section(node: node.node, parameters: node.parameters, update: node.update, editing: editing, out_of_band: out_of_band),
                Section::Fields => fields_section(node: node.node, rules: node.rules, charts: node.charts, range: node.range, editing: editing, out_of_band: out_of_band),
            }
        }
    })
}

#[component]
async fn overview_section(
    node: NodeOverview,
    parameters: Vec<ParameterOverview>,
    update: Option<UpdateStatus>,
    editing: bool,
    #[default] out_of_band: bool,
) -> Result<impl View> {
    let title = node.name.clone().unwrap_or_else(|| node.device_id.clone());
    let uptime = node.uptime_seconds.map_or_else(
        || "—".to_string(),
        |seconds| duration_label(Duration::from_secs(seconds.into())),
    );
    let signal = node
        .rssi
        .map_or_else(|| "—".to_string(), |rssi| format!("{rssi} dBm"));
    let device_id = node.device_id.clone();
    // Shown, it refreshes on every change; edited, it is the form and does not refresh.
    let attrs = attributes! {
        id="node-overview"
        if out_of_band {
            hx-swap-oob="outerHTML"
        }
        if editing {
            hx-post="/nodes/edit"
            hx-target="#status"
        } else {
            hx-get=(format!("/nodes/{device_id}/overview"))
            hx-trigger="sse:nodes, refresh"
            hx-swap="outerHTML"
        }
    };
    Ok(view! {
        <form (attrs)>
            <input type="hidden" name="device_id" value=(device_id.clone())>
            <input type="hidden" name="section" value="overview">
            card(
                card_header(
                    <div class="flex flex-wrap items-center justify-between gap-3">
                        <div class="flex items-center gap-3">
                            if editing {
                                input(attrs: attributes! { name="name" value=(node.name.clone().unwrap_or_default()) placeholder=(device_id.clone()) aria-label="Node name" class="h-9 w-64 text-base font-semibold" })
                            } else {
                                card_title((title))
                            }
                            presence_badge(presence: node.presence)
                            protocol_badge(version: node.protocol_version)
                        </div>
                        edit_buttons(device_id: device_id.clone(), path: "overview", target: "#node-overview", editing: editing, label: "Edit")
                    </div>
                    card_description("Node overview")
                )
                card_content(
                    <dl class="grid grid-cols-2 gap-x-6 gap-y-4 text-sm sm:grid-cols-3 lg:grid-cols-6">
                        stat(label: "Device ID", value: device_id.clone())
                        stat(label: "Software", value: node.software.clone().unwrap_or_default())
                        stat(label: "Uptime", value: uptime)
                        stat(label: "Signal", value: signal)
                        for row in parameters {
                            parameter_stat(row: row, editing: editing)
                        }
                    </dl>
                    if !editing {
                        match update {
                            Some(UpdateStatus::Available(version)) => {
                                <div class="mt-4">
                                    button(
                                        variant: ButtonVariant::Outline,
                                        size: ButtonSize::Sm,
                                        attrs: attributes! { type="button" hx-post="/nodes/update" hx-target="#status" },
                                        (format!("Update to {version}"))
                                    )
                                </div>
                            },
                            Some(UpdateStatus::InProgress(version)) => {
                                <p class="mt-4 text-sm text-muted-foreground">(format!("Updating to {version}…"))</p>
                            },
                            None => {},
                        }
                    }
                )
            )
        </form>
    })
}

/// One entity of the hardware card: as `nodes.toml` lists it and as the board mounts it.
#[derive(Clone)]
struct HardwareRow {
    id: u8,
    name: Option<String>,
    desired: Option<HardwareEntity>,
    reported: Option<HardwareEntity>,
}

fn hardware_rows(hub: &Hub, device_id: &str) -> Vec<HardwareRow> {
    let status = hub.hardware_status(device_id);
    let entities = |hardware: &Option<oxihome_protocol::hardware::Hardware>| {
        hardware
            .as_ref()
            .map(|hardware| hardware.entities.clone())
            .unwrap_or_default()
    };
    let (desired, reported) = (entities(&status.desired), entities(&status.reported));
    let mut ids: Vec<u8> = desired
        .iter()
        .chain(&reported)
        .map(|entity| entity.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let snapshot = hub.snapshot();
    let find =
        |list: &[HardwareEntity], id: u8| list.iter().find(|entity| entity.id == id).cloned();
    ids.into_iter()
        .map(|id| HardwareRow {
            id,
            name: snapshot
                .nodes
                .name(&NameTarget::Entity(device_id.to_string(), id))
                .map(String::from),
            desired: find(&desired, id),
            reported: find(&reported, id),
        })
        .collect()
}

#[page("/nodes/{device_id}/hardware")]
pub async fn hardware_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let device_id = path_param::<DeviceId>(cx).to_string();
    let rows = hardware_rows(hub, &device_id);
    let status = hub.hardware_status(&device_id);
    Ok(view! { hardware_section(device_id: device_id, rows: rows, status: status) })
}

/// Read only: the hardware comes from `nodes.toml`. Red where the board mounts something else.
#[component]
async fn hardware_section(
    device_id: String,
    rows: Vec<HardwareRow>,
    status: HardwareStatus,
) -> Result<impl View> {
    let configured = status.desired.is_some();
    let (note, failed) = match &status.state {
        HardwareState::NotConfigured if status.reported.is_some() => (
            "Not set in nodes.toml: this is what the board mounts.".to_string(),
            false,
        ),
        HardwareState::NotConfigured => ("Not set in nodes.toml.".to_string(), false),
        HardwareState::Offline => ("The node is offline.".to_string(), false),
        HardwareState::Unsupported => (
            "This node's software does not take a hardware configuration.".to_string(),
            false,
        ),
        HardwareState::Invalid(problems) => (format!("Not sent: {}", problems.join("; ")), true),
        HardwareState::Matches => ("The board mounts this hardware.".to_string(), false),
        HardwareState::Sending => (
            "Sending; the board restarts to mount it…".to_string(),
            false,
        ),
        HardwareState::Refused(reason) => (format!("Not applied: {reason}"), true),
        HardwareState::Differs => (
            "The board mounts something else; it is sent at the next chance.".to_string(),
            true,
        ),
    };
    Ok(view! {
        <div
            id="node-hardware"
            hx-get=(format!("/nodes/{device_id}/hardware"))
            hx-trigger="sse:nodes, refresh"
            hx-swap="outerHTML"
        >
            card(
                card_header(
                    card_title("Hardware")
                    card_description("What is wired to each pin, from the node's hardware in nodes.toml.")
                )
                card_content(
                    <p class=(if failed { "mb-3 text-sm text-destructive" } else { "mb-3 text-sm text-muted-foreground" })>(note)</p>
                    if !status.reports.is_empty() {
                        <div class="mb-3 text-sm text-destructive">
                            <p class="font-medium">"The board reports:"</p>
                            <ul class="list-disc pl-5">
                                for report in status.reports {
                                    <li>(report)</li>
                                }
                            </ul>
                        </div>
                    }
                    if !rows.is_empty() {
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="text-left text-xs text-muted-foreground">
                                    <th class="py-1 pr-3 font-normal">"Entity"</th>
                                    <th class="py-1 pr-3 font-normal">"Driver"</th>
                                    <th class="py-1 font-normal">"Pin"</th>
                                </tr>
                            </thead>
                            <tbody class="divide-y divide-border">
                                for row in rows {
                                    hardware_row(row: row, configured: configured)
                                }
                            </tbody>
                        </table>
                    }
                )
            )
        </div>
    })
}

#[component]
async fn hardware_row(row: HardwareRow, configured: bool) -> Result<impl View> {
    let shown = if configured {
        &row.desired
    } else {
        &row.reported
    };
    let differs = configured && row.desired != row.reported;
    let board = match (&row.reported, differs) {
        (_, false) => None,
        (None, true) => Some("not on the board".to_string()),
        (Some(reported), true) => Some(format!(
            "the board mounts {} on GPIO {}",
            driver_label(&reported.driver),
            reported.pin
        )),
    };
    let missing = shown.is_none();
    let entity = format!("{} · {}", row.id, entity_label(row.id, &row.name));
    let driver = shown.as_ref().map_or_else(
        || "not in nodes.toml".to_string(),
        |entity| driver_label(&entity.driver),
    );
    let pin = shown
        .as_ref()
        .map_or_else(String::new, |entity| format!("GPIO {}", entity.pin));
    Ok(view! {
        <tr class=(if missing { "align-top text-muted-foreground" } else { "align-top" })>
            <td class="py-2 pr-3">(entity)</td>
            <td class="py-2 pr-3">
                (driver)
                if let Some(board) = board {
                    <p class="text-xs text-destructive">(board)</p>
                }
            </td>
            <td class="py-2">(pin)</td>
        </tr>
    })
}

fn driver_label(driver: &Driver) -> String {
    match driver {
        Driver::Output { inverted: false } => "output".into(),
        Driver::Output { inverted: true } => "output, inverted".into(),
        Driver::Button { pull_up, toggles } => {
            let mut label = "button".to_string();
            if !pull_up {
                label.push_str(", no pull-up");
            }
            if let Some(output) = toggles {
                label.push_str(&format!(", toggles entity {output}"));
            }
            label
        }
        Driver::Dht11 {
            read_interval_seconds,
        } => format!("DHT11, read every {read_interval_seconds} s"),
        Driver::Buzzer => "buzzer".into(),
        Driver::IrReceiver => "IR receiver".into(),
        Driver::Potentiometer { minimum_change } => {
            format!("potentiometer, changes of {minimum_change}% or more")
        }
    }
}

async fn history(hub: &Hub, device_id: &str) -> History {
    History {
        updates: hub.updates(device_id, HISTORY_ROWS).await,
        notifications: hub.notifications_about(device_id, HISTORY_ROWS).await,
    }
}

#[page("/nodes/{device_id}/history")]
pub async fn history_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let device_id = path_param::<DeviceId>(cx).to_string();
    let history = history(hub, &device_id).await;
    Ok(view! { history_section(device_id: device_id, history: history) })
}

/// Between the fields and the rules; refreshes on node changes and new notifications.
#[component]
async fn history_section(device_id: String, history: History) -> Result<impl View> {
    let now = Timestamp::now();
    Ok(view! {
        <div
            id="node-history"
            hx-get=(format!("/nodes/{device_id}/history"))
            hx-trigger="sse:nodes, sse:notifications, refresh"
            hx-swap="outerHTML"
        >
            card(
                card_header(card_title("History"))
                card_content(
                    <div class="grid gap-6 md:grid-cols-2">
                        <div>
                            <h3 class="mb-2 text-sm font-medium">"Firmware updates"</h3>
                            if history.updates.is_empty() {
                                <p class="text-sm text-muted-foreground">"None yet."</p>
                            }
                            <ul class="divide-y divide-border text-sm">
                                for update in history.updates {
                                    <li class="flex justify-between gap-3 py-2 first:pt-0 last:pb-0">
                                        <div class="min-w-0">
                                            <p class="font-medium">(format!("{} → {}", update.from_version, update.to_version))</p>
                                            <p class="truncate text-muted-foreground" title=(update_outcome_label(&update))>(update_outcome_label(&update))</p>
                                        </div>
                                        <span class="shrink-0 text-xs text-muted-foreground">(ago_label(elapsed(update.finished_at, now)))</span>
                                    </li>
                                }
                            </ul>
                        </div>
                        <div>
                            <h3 class="mb-2 text-sm font-medium">"Activity"</h3>
                            if history.notifications.is_empty() {
                                <p class="text-sm text-muted-foreground">"Nothing in the last days."</p>
                            }
                            <ul class="divide-y divide-border text-sm">
                                for notification in history.notifications {
                                    <li class="flex justify-between gap-3 py-2 first:pt-0 last:pb-0">
                                        <div class="min-w-0">
                                            <p class="font-medium">(notification.title)</p>
                                            <p class="truncate text-muted-foreground" title=(notification.message.clone())>(notification.message)</p>
                                        </div>
                                        <span class="shrink-0 text-xs text-muted-foreground">(ago_label(elapsed(notification.at, now)))</span>
                                    </li>
                                }
                            </ul>
                        </div>
                    </div>
                )
            )
        </div>
    })
}

fn update_outcome_label(update: &StoredUpdate) -> String {
    match update.outcome {
        UpdateOutcome::Updated => {
            let took = elapsed(update.started_at, update.finished_at);
            format!("Updated in {}", duration_label(took))
        }
        UpdateOutcome::RolledBack => "Rolled back: the new version did not reach the broker".into(),
        UpdateOutcome::Failed => format!("Failed: {}", update.detail.as_deref().unwrap_or("error")),
        UpdateOutcome::NoAnswer => "No answer after the update".into(),
    }
}

/// Edit swaps the section for its form; Cancel and Save swap it back.
#[component]
async fn edit_buttons(
    device_id: String,
    path: &'static str,
    target: &'static str,
    editing: bool,
    label: &'static str,
    /// Kept by the section across edits, e.g. `?range=6h`.
    #[default]
    query: String,
) -> Result<impl View> {
    Ok(view! {
        if editing {
            <div class="flex gap-2">
                button(
                    variant: ButtonVariant::Outline,
                    size: ButtonSize::Sm,
                    attrs: attributes! { type="button" hx-get=(format!("/nodes/{device_id}/{path}{query}")) hx-params="none" hx-target=(target) hx-swap="outerHTML" },
                    "Cancel"
                )
                button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save")
            </div>
        } else {
            button(
                variant: ButtonVariant::Outline,
                size: ButtonSize::Sm,
                attrs: attributes! { type="button" hx-get=(format!("/nodes/{device_id}/{path}/edit{query}")) hx-params="none" hx-target=(target) hx-swap="outerHTML" },
                (label)
            )
        }
    })
}

struct RuleRow {
    rule: String,
    enabled: bool,
    runs_on: Vec<String>,
    checks: Vec<String>,
    acts_on: Vec<String>,
}

/// The rules that run on something from this node, check it or act on it, linked to their cards on
/// `/rules`.
#[component]
async fn rules_section(channels: Vec<ChannelOverview>, rules: Vec<RuleUse>) -> Result<impl View> {
    let target = |role: RuleRole, channel: Option<(u8, u8)>| match channel {
        None if role == RuleRole::Checks => "whether the node is online".to_string(),
        None => "the node going offline".to_string(),
        Some((entity, number)) => match channels
            .iter()
            .find(|known| known.entity == entity && known.channel == number)
        {
            Some(known) => format!(
                "{} · {}",
                entity_label(entity, &known.entity_name),
                channel_label(number, &known.channel_name, known.quantity)
            ),
            None => format!("entity {entity} · channel {number}"),
        },
    };
    // One line per rule: what it runs on, checks and acts on, in file order.
    let mut rows: Vec<RuleRow> = Vec::new();
    for use_ in rules {
        let index = match rows.iter().position(|row| row.rule == use_.rule) {
            Some(index) => index,
            None => {
                rows.push(RuleRow {
                    rule: use_.rule.clone(),
                    enabled: use_.enabled,
                    runs_on: Vec::new(),
                    checks: Vec::new(),
                    acts_on: Vec::new(),
                });
                rows.len() - 1
            }
        };
        let row = &mut rows[index];
        let text = target(use_.role, use_.channel);
        match use_.role {
            RuleRole::RunsOn => row.runs_on.push(text),
            RuleRole::Checks => row.checks.push(text),
            RuleRole::ActsOn => row.acts_on.push(text),
        }
    }
    Ok(view! {
        card(
            attrs: attributes! { id="node-rules" },
            card_header(
                card_title("Rules")
                card_description("Rules that run on something from this node, check it or act on it.")
            )
            card_content(
                if rows.is_empty() {
                    <p class="text-sm text-muted-foreground">"No rule involves this node."</p>
                }
                <ul class="divide-y divide-border text-sm">
                    for RuleRow { rule, enabled, runs_on, checks, acts_on } in rows {
                        <li class=(if enabled { "py-2 first:pt-0 last:pb-0" } else { "py-2 opacity-60 first:pt-0 last:pb-0" })>
                            <a href=(format!("/rules#{}", rule_anchor(&rule))) class="font-medium hover:underline">(rule)</a>
                            if !enabled {
                                <span class="ml-2 text-xs text-muted-foreground">"(disabled)"</span>
                            }
                            if !runs_on.is_empty() {
                                <p class="text-muted-foreground">"runs on " <span class="text-foreground">(runs_on.join(", "))</span></p>
                            }
                            if !checks.is_empty() {
                                <p class="text-muted-foreground">"checks " <span class="text-foreground">(checks.join(", "))</span></p>
                            }
                            if !acts_on.is_empty() {
                                <p class="text-muted-foreground">"acts on " <span class="text-foreground">(acts_on.join(", "))</span></p>
                            }
                        </li>
                    }
                </ul>
            )
        )
    })
}

#[component]
async fn stat(label: &'static str, value: String) -> Result<impl View> {
    Ok(view! {
        <div class="min-w-0">
            <dt class="text-xs text-muted-foreground">(label)</dt>
            <dd class="truncate font-medium" title=(value.clone())>(value)</dd>
        </div>
    })
}

/// The desired value (or the node's when none is set); red when the node reports another one.
#[component]
async fn parameter_stat(row: ParameterOverview, editing: bool) -> Result<impl View> {
    let key = parameter_key(row.parameter);
    let label = match key {
        "heartbeat_interval" => "Heartbeat",
        "wifi_check_interval" => "Wi-Fi check",
        other => other,
    };
    let pending = row.desired.is_some() && row.desired != row.current;
    let shown = row
        .desired
        .or(row.current)
        .map_or_else(|| "—".to_string(), |value| format!("{value} s"));
    let note = if pending {
        row.current.map_or_else(
            || "node: unknown".to_string(),
            |value| format!("node: {value} s"),
        )
    } else if row.desired.is_none() {
        "node's value".to_string()
    } else {
        String::new()
    };
    Ok(view! {
        <div class="min-w-0">
            <dt class="text-xs text-muted-foreground" title=(key)>(format!("{label} ({}–{} s)", row.parameter.min, row.parameter.max))</dt>
            <dd class="font-medium">
                if editing {
                    input(attrs: attributes! {
                        type="number"
                        name=(format!("parameter.{key}"))
                        min=(row.parameter.min.to_string())
                        max=(row.parameter.max.to_string())
                        value=(row.desired.map(|value| value.to_string()).unwrap_or_default())
                        placeholder=(row.current.map(|value| value.to_string()).unwrap_or_default())
                        aria-label=(label)
                        class="h-8 w-24"
                    })
                } else {
                    (shown)
                }
            </dd>
            if !note.is_empty() {
                <p class=(if pending { "text-xs text-destructive" } else { "text-xs text-muted-foreground" })>(note)</p>
            }
        </div>
    })
}

/// One card per entity, in a grid. The management entity is left out: its parameters are in
/// the overview.
#[component]
async fn fields_section(
    node: NodeOverview,
    rules: Vec<RuleUse>,
    charts: HashMap<(u8, u8), Chart>,
    range: ChartRange,
    editing: bool,
    #[default] out_of_band: bool,
) -> Result<impl View> {
    let device_id = node.device_id.clone();
    let query = format!("?range={}", range.key());
    let has_charts = node.channels.iter().any(|channel| channel.chartable);
    let mut entities: Vec<(u8, Option<String>, Vec<ChannelOverview>)> = Vec::new();
    for channel in node
        .channels
        .into_iter()
        .filter(|channel| channel.entity != management::ENTITY)
    {
        match entities.iter_mut().find(|(id, _, _)| *id == channel.entity) {
            Some((_, _, list)) => list.push(channel),
            None => entities.push((channel.entity, channel.entity_name.clone(), vec![channel])),
        }
    }
    // Shown, it refreshes on every change; edited, it is the form and does not refresh.
    let attrs = attributes! {
        id="node-fields"
        if out_of_band {
            hx-swap-oob="outerHTML"
        }
        if editing {
            hx-post="/nodes/edit"
            hx-target="#status"
        } else {
            hx-get=(format!("/nodes/{device_id}/fields{query}"))
            // The form's fields would repeat `range` in the query.
            hx-params="none"
            hx-disinherit="*"
            hx-trigger="sse:nodes, refresh"
            hx-swap="outerHTML"
        }
    };
    Ok(view! {
        <form class="space-y-3" (attrs)>
            <input type="hidden" name="device_id" value=(device_id.clone())>
            <input type="hidden" name="section" value="fields">
            <input type="hidden" name="range" value=(range.key())>
            <div class="flex flex-wrap items-start justify-between gap-3">
                <div>
                    <h2 class="text-base font-semibold">"Node fields"</h2>
                    <p class="text-sm text-muted-foreground">"What the node declares: each entity's channels, what they are, their values and controls."</p>
                </div>
                <div class="flex flex-wrap items-center gap-2">
                    if has_charts && !editing {
                        range_picker(device_id: device_id.clone(), range: range)
                    }
                    edit_buttons(device_id: device_id.clone(), path: "fields", target: "#node-fields", editing: editing, label: "Edit names", query: query.clone())
                </div>
            </div>
            // Cards of a row share its height, so the grid stays aligned.
            <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
                for (entity, name, channels) in entities {
                    entity_card(device_id: device_id.clone(), entity: entity, name: name, channels: channels, rules: rules.clone(), charts: charts.clone(), range: range, editing: editing)
                }
            </div>
        </form>
    })
}

/// How far back the charts go; the address keeps the choice for reloads.
#[component]
async fn range_picker(device_id: String, range: ChartRange) -> Result<impl View> {
    Ok(view! {
        <div class="inline-flex gap-0.5 rounded-lg border border-border p-0.5" role="group" aria-label="Chart range">
            for option in ChartRange::ALL {
                button(
                    variant: if option == range { ButtonVariant::Secondary } else { ButtonVariant::Ghost },
                    size: ButtonSize::Sm,
                    attrs: attributes! {
                        type="button"
                        aria-pressed=(if option == range { "true" } else { "false" })
                        hx-get=(format!("/nodes/{device_id}/fields?range={}", option.key()))
                        hx-target="#node-fields"
                        hx-swap="outerHTML"
                        hx-push-url=(format!("/nodes/{device_id}?range={}", option.key()))
                    },
                    (option.label())
                )
            }
        </div>
    })
}

#[component]
async fn entity_card(
    device_id: String,
    entity: u8,
    name: Option<String>,
    channels: Vec<ChannelOverview>,
    rules: Vec<RuleUse>,
    charts: HashMap<(u8, u8), Chart>,
    range: ChartRange,
    editing: bool,
) -> Result<impl View> {
    Ok(view! {
        card(
            attrs: attributes! { class="h-full" },
            card_header(
                <div class="flex items-center justify-between gap-2">
                    if editing {
                        input(attrs: attributes! {
                            name=(format!("entity.{entity}"))
                            value=(name.clone().unwrap_or_default())
                            placeholder=(format!("entity {entity}"))
                            aria-label=(format!("Entity {entity} name"))
                            class="h-9 font-semibold"
                        })
                    } else {
                        card_title((entity_label(entity, &name)))
                    }
                    <span class="shrink-0 text-xs text-muted-foreground">(format!("entity {entity}"))</span>
                </div>
            )
            card_content(
                <ul class="divide-y divide-border">
                    for channel in channels {
                        field_row(
                            chart: charts.get(&(channel.entity, channel.channel)).cloned(),
                            range: range,
                            rules: rules
                                .iter()
                                .filter(|use_| use_.channel == Some((channel.entity, channel.channel)))
                                .map(|use_| use_.rule.clone())
                                .collect(),
                            device_id: device_id.clone(),
                            channel: channel,
                            editing: editing,
                        )
                    }
                </ul>
            )
        )
    })
}

/// `rules` names the rules that use this channel.
#[component]
async fn field_row(
    device_id: String,
    channel: ChannelOverview,
    rules: Vec<String>,
    chart: Option<Chart>,
    range: ChartRange,
    editing: bool,
) -> Result<impl View> {
    let channel_name = channel_label(channel.channel, &channel.channel_name, channel.quantity);
    let label = format!(
        "{} · {channel_name}",
        entity_label(channel.entity, &channel.entity_name)
    );
    // With a chart, the value stands out on the right, above the chart's end.
    let highlighted = chart.as_ref().map(|_| {
        channel
            .value
            .as_ref()
            .map_or_else(|| "—".to_string(), |value| value_label(value, channel.unit))
    });
    Ok(view! {
        <li class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 py-3 first:pt-0 last:pb-0">
            <div class="min-w-0 space-y-0.5">
                if editing {
                    input(attrs: attributes! {
                        name=(format!("channel.{}.{}", channel.entity, channel.channel))
                        value=(channel.channel_name.clone().unwrap_or_default())
                        placeholder=(channel_label(channel.channel, &None, channel.quantity))
                        aria-label=(format!("Channel {} name", channel.channel))
                        class="h-8"
                    })
                } else {
                    <p class="text-sm font-medium">(channel_name)</p>
                }
                <p class="text-xs text-muted-foreground">
                    (channel.kind.clone())
                    if !rules.is_empty() {
                        <a href="#node-rules" class="ml-2 rounded-full bg-foreground/5 px-1.5 py-0.5 hover:bg-foreground/10" title=(rules.join("\n"))>
                            (if rules.len() == 1 { "1 rule".to_string() } else { format!("{} rules", rules.len()) })
                        </a>
                    }
                </p>
                if !editing && highlighted.is_none() {
                    channel_value(channel: channel.clone())
                }
            </div>
            if !editing {
                if let Some(value) = highlighted {
                    <p class="text-2xl font-semibold tracking-tight tabular-nums">(value)</p>
                }
                channel_controls(device_id: device_id, channel: channel.clone(), label: label)
                if let Some(chart) = chart {
                    history_chart(chart: chart, unit: channel.unit, range: range)
                }
            }
        </li>
    })
}

#[derive(Deserialize)]
struct EditForm {
    device_id: String,
    section: Section,
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// Saves one section's form.
#[route(POST "/nodes/edit")]
pub async fn edit(cx: &Cx, Form(form): Form<EditForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = edits(&form.fields).and_then(|edits| hub.edit_node(&form.device_id, &edits));
    if result.is_ok() {
        tokio::spawn(parameters::apply(hub.clone(), form.device_id.clone()));
    }

    // On success the section comes back shown, out of band; `#status` gets the error otherwise.
    let section = form.section;
    let node = match result {
        Ok(()) => {
            let range = ChartRange::from_key(form.fields.get("range").map(String::as_str));
            node_view(hub, &form.device_id, Some(section), range).await
        }
        Err(_) => None,
    };
    view! { cx =>
        if let Err(error) = &result {
            <pre class="text-sm whitespace-pre-wrap text-destructive">(format!("{error:#}"))</pre>
        }
        section_body(node: node, section: section, editing: false, out_of_band: true)
    }
    .single()
    .await
}

#[derive(Deserialize)]
struct UpdateForm {
    device_id: String,
}

/// Answers with the content of `#status`: empty on success, the error otherwise. Progress and the
/// outcome come as notifications.
#[route(POST "/nodes/update")]
pub async fn start_update(cx: &Cx, Form(form): Form<UpdateForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.start_update(&form.device_id).await;
    view! { cx =>
        if let Err(error) = result {
            <p class="text-sm text-destructive">(format!("{error:#}"))</p>
        }
    }
    .single()
    .await
}

/// `name`, `entity.<id>`, `channel.<entity>.<channel>` and `parameter.<key>` fields.
fn edits(fields: &HashMap<String, String>) -> anyhow::Result<NodeEdits> {
    let text = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    let mut edits = NodeEdits {
        name: fields.get("name").map(|value| text(value)),
        ..NodeEdits::default()
    };
    for (field, value) in fields {
        let parts: Vec<&str> = field.split('.').collect();
        match parts.as_slice() {
            ["entity", id] => {
                entity_edit(&mut edits.entities, id.parse()?).name = Some(text(value));
            }
            ["channel", id, channel] => entity_edit(&mut edits.entities, id.parse()?)
                .channels
                .push((channel.parse()?, text(value))),
            ["parameter", key] => {
                let key = management::PARAMETERS
                    .into_iter()
                    .map(parameter_key)
                    .find(|known| known == key)
                    .ok_or_else(|| anyhow::anyhow!("unknown parameter {key}"))?;
                let value = match text(value) {
                    Some(value) => Some(
                        value
                            .parse::<i32>()
                            .map_err(|_| anyhow::anyhow!("{key} must be a whole number"))?,
                    ),
                    None => None,
                };
                edits.parameters.push((key, value));
            }
            _ => {}
        }
    }
    edits.entities.sort_by_key(|entity| entity.id);
    for entity in &mut edits.entities {
        entity.channels.sort_by_key(|(channel, _)| *channel);
    }
    edits.parameters.sort();
    Ok(edits)
}

fn entity_edit(entities: &mut Vec<EntityEdit>, id: u8) -> &mut EntityEdit {
    let index = match entities.iter().position(|entity| entity.id == id) {
        Some(index) => index,
        None => {
            entities.push(EntityEdit {
                id,
                ..EntityEdit::default()
            });
            entities.len() - 1
        }
    };
    &mut entities[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_fields_become_edits() {
        let fields: HashMap<String, String> = [
            ("name", " Test board "),
            ("entity.4", "External LED"),
            ("channel.4.0", ""),
            ("channel.1.0", "Power"),
            ("parameter.heartbeat_interval", "60"),
            ("parameter.wifi_check_interval", ""),
        ]
        .into_iter()
        .map(|(field, value)| (field.to_string(), value.to_string()))
        .collect();
        let edits = edits(&fields).unwrap();
        assert_eq!(edits.name, Some(Some("Test board".to_string())));
        assert_eq!(
            edits.entities,
            vec![
                EntityEdit {
                    id: 1,
                    name: None,
                    channels: vec![(0, Some("Power".to_string()))],
                },
                EntityEdit {
                    id: 4,
                    name: Some(Some("External LED".to_string())),
                    channels: vec![(0, None)],
                },
            ]
        );
        assert_eq!(
            edits.parameters,
            vec![
                ("heartbeat_interval", Some(60)),
                ("wifi_check_interval", None)
            ]
        );
    }

    #[test]
    fn bad_numbers_are_refused() {
        let fields: HashMap<String, String> = [(
            "parameter.heartbeat_interval".to_string(),
            "soon".to_string(),
        )]
        .into();
        assert!(
            edits(&fields)
                .unwrap_err()
                .to_string()
                .contains("whole number")
        );
    }
}
