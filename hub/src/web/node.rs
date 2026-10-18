use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, parse_query_params, path_param},
    view::{View, attributes, component, view},
};

use super::chart::{CHART_BUCKETS, Chart, ChartRange, chart, series};
use super::format::{duration_label, unit_symbol};
use super::layout::layout;
use super::nodes::{presence_badge, protocol_badge};
use super::overview::{self, NodeOverview, ParameterOverview};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::config::nodes::parameter_key;
use crate::config::rules::RuleUse;
use crate::hub::{Hub, UpdateStatus};
use crate::storage::{StoredNotification, StoredUpdate};

mod download;
mod fields;
mod forms;
mod hardware;
mod history;

use fields::{fields_section, rules_section};
use hardware::{hardware_rows, hardware_section};
use history::{history, history_section};

pub use download::entities_download;
pub use forms::{edit, forget, start_update};
pub use hardware::hardware_fragment;
pub use history::history_fragment;

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
    let forget_question = format!(
        "Forget {title}? Its values and update history go away; it comes back if it speaks again."
    );
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
                        <div class="mt-4">
                            <a
                                href=(format!("/nodes/{device_id}/entities.json"))
                                download=""
                                class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm))
                            >
                                "Download entities"
                            </a>
                        </div>
                        if node.presence == overview::Presence::Offline {
                            <div class="mt-4">
                                button(
                                    variant: ButtonVariant::Destructive,
                                    size: ButtonSize::Sm,
                                    attrs: attributes! {
                                        type="button"
                                        hx-post="/nodes/forget"
                                        hx-target="#status"
                                        hx-confirm=(forget_question)
                                    },
                                    "Forget"
                                )
                            </div>
                        }
                    }
                )
            )
        </form>
    })
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
        "wifi_power_save" => "Wi-Fi power save",
        other => other,
    };
    let unit = match row.parameter.unit {
        Some(unit) => format!(" {}", unit_symbol(unit)),
        None => String::new(),
    };
    let range = match key {
        "wifi_power_save" => "0 off, 1 light, 2 maximum".to_string(),
        _ => format!("{}–{}{unit}", row.parameter.min, row.parameter.max),
    };
    let pending = row.desired.is_some() && row.desired != row.current;
    let shown = row
        .desired
        .or(row.current)
        .map_or_else(|| "—".to_string(), |value| format!("{value}{unit}"));
    let note = if pending {
        row.current.map_or_else(
            || "node: unknown".to_string(),
            |value| format!("node: {value}{unit}"),
        )
    } else if row.desired.is_none() {
        "node's value".to_string()
    } else {
        String::new()
    };
    Ok(view! {
        <div class="min-w-0">
            <dt class="text-xs text-muted-foreground" title=(key)>(format!("{label} ({range})"))</dt>
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
