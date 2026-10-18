//! The fields card (one card per entity, with charts) and the rules that use the node.

use std::collections::HashMap;

use oxihome_protocol::management;
use topcoat::{
    Result,
    view::{View, attributes, component, view},
};

use super::edit_buttons;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::config::rules::{RuleRole, RuleUse};
use crate::web::chart::{Chart, ChartRange, history_chart};
use crate::web::format::{channel_label, entity_label, rule_anchor, value_label};
use crate::web::nodes::{channel_controls, channel_value};
use crate::web::overview::{ChannelOverview, NodeOverview};

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
pub(super) async fn rules_section(
    channels: Vec<ChannelOverview>,
    rules: Vec<RuleUse>,
) -> Result<impl View> {
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

/// One card per entity, in a grid. The management entity is left out: its parameters are in
/// the overview.
#[component]
pub(super) async fn fields_section(
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
    // Shown, it refreshes on every change; edited, it is the form and does not refresh. Only a
    // div when shown: the channel controls are forms, and a browser drops a form inside a form.
    let attrs = attributes! {
        id="node-fields"
        class="space-y-3"
        if out_of_band {
            hx-swap-oob="outerHTML"
        }
        if editing {
            hx-post="/nodes/edit"
            hx-target="#status"
        } else {
            hx-get=(format!("/nodes/{device_id}/fields{query}"))
            hx-disinherit="*"
            hx-trigger="sse:nodes, refresh"
            hx-swap="outerHTML"
        }
    };
    Ok(view! {
        if editing {
            <form (attrs)>
                <input type="hidden" name="device_id" value=(device_id.clone())>
                <input type="hidden" name="section" value="fields">
                <input type="hidden" name="range" value=(range.key())>
                fields_content(device_id: device_id.clone(), entities: entities.clone(), rules: rules.clone(), charts: charts.clone(), range: range, has_charts: has_charts, editing: true)
            </form>
        } else {
            <div (attrs)>
                fields_content(device_id: device_id.clone(), entities: entities.clone(), rules: rules.clone(), charts: charts.clone(), range: range, has_charts: has_charts, editing: false)
            </div>
        }
    })
}

#[component]
async fn fields_content(
    device_id: String,
    entities: Vec<(u8, Option<String>, Vec<ChannelOverview>)>,
    rules: Vec<RuleUse>,
    charts: HashMap<(u8, u8), Chart>,
    range: ChartRange,
    has_charts: bool,
    editing: bool,
) -> Result<impl View> {
    let query = format!("?range={}", range.key());
    Ok(view! {
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
    let declared_name = channels
        .first()
        .and_then(|channel| channel.entity_declared_name.clone());
    // The field holds only the hub's own name; emptied, the node's name shows again.
    let own_name = name
        .clone()
        .filter(|name| Some(name) != declared_name.as_ref());
    Ok(view! {
        card(
            attrs: attributes! { class="h-full" },
            card_header(
                <div class="flex items-center justify-between gap-2">
                    if editing {
                        input(attrs: attributes! {
                            name=(format!("entity.{entity}"))
                            value=(own_name.clone().unwrap_or_default())
                            placeholder=(entity_label(entity, &declared_name))
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
    let own_name = channel
        .channel_name
        .clone()
        .filter(|name| Some(name) != channel.channel_declared_name.as_ref());
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
                        value=(own_name.clone().unwrap_or_default())
                        placeholder=(channel_label(channel.channel, &channel.channel_declared_name, channel.quantity))
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
