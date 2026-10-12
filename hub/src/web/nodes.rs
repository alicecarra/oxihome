use std::sync::Arc;
use std::time::Duration;

use oxihome_protocol::{Value, management};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    view::{Child, View, attributes, component, view},
};

use super::format::{ago_label, channel_label, duration_label, entity_label, value_label};
use super::layout::layout;
use super::overview::{self, ChannelOverview, Invocation, NodeOverview, Presence};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::components::switch::switch;
use crate::config::nodes::ConfiguredEntity;
use crate::hub::Hub;

#[page("/nodes")]
pub async fn nodes_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);

    Ok(view! {
        layout(
            title: "Oxihome nodes",
            <div id="node-filter" class="mb-4 flex flex-wrap gap-2">
                for (presence, label) in [("online", "Online"), ("offline", "Offline"), ("not-seen", "Not seen")] {
                    button(
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        attrs: attributes! {
                            type="button"
                            data-filter=(presence)
                            aria-pressed="true"
                            class="aria-pressed:border-transparent aria-pressed:bg-primary aria-pressed:text-primary-foreground aria-pressed:hover:bg-primary/90"
                        },
                        (label)
                        <span data-count="" class="tabular-nums opacity-70"></span>
                    )
                }
            </div>
            <div id="nodes" hx-get="/nodes/fragment" hx-trigger="sse:nodes, every 30s, refresh">
                node_list(nodes: overview::nodes(hub))
            </div>
            <p id="nodes-filtered-out" class="text-muted-foreground" hidden="">"No nodes match the filter."</p>
            <script src="/static/node-filter.js"></script>
        )
    })
}

#[page("/nodes/fragment")]
pub async fn nodes_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { node_list(nodes: overview::nodes(hub)) })
}

#[component]
pub async fn node_list(nodes: Vec<NodeOverview>) -> Result<impl View> {
    Ok(view! {
        if nodes.is_empty() {
            <p class="text-muted-foreground">"No nodes yet."</p>
        }
        <div class="grid items-start gap-4 sm:grid-cols-2 xl:grid-cols-3">
            for node in nodes {
                node_card(node: node)
            }
        </div>
    })
}

#[component]
async fn node_card(node: NodeOverview) -> Result<impl View> {
    let title = node.name.clone().unwrap_or_else(|| node.device_id.clone());
    let description = node_description(&node);
    // The management entity holds node parameters, not things in the home.
    let channels: Vec<ChannelOverview> = node
        .channels
        .into_iter()
        .filter(|channel| channel.entity != management::ENTITY)
        .collect();
    Ok(view! {
        card(
            attrs: attributes! { data-presence=(presence_key(node.presence)) },
            card_header(
                <div class="flex items-center justify-between gap-2">
                    <a href=(format!("/nodes/{}", node.device_id)) class="hover:underline">card_title((title))</a>
                    presence_badge(presence: node.presence)
                </div>
                card_description((description))
            )
            if !channels.is_empty() {
                card_content(
                    <ul class="divide-y divide-border">
                        for channel in channels {
                            channel_row(device_id: node.device_id.clone(), channel: channel)
                        }
                    </ul>
                )
            }
            if !node.configured_entities.is_empty() {
                card_content(
                    <ul class="space-y-1 text-sm text-muted-foreground">
                        for entity in node.configured_entities {
                            <li>(configured_entity_label(&entity))</li>
                        }
                    </ul>
                )
            }
        )
    })
}

#[component]
pub(super) async fn presence_badge(presence: Presence) -> Result<impl View> {
    Ok(view! {
        match presence {
            Presence::Online => badge(variant: BadgeVariant::Success, <span class="size-1.5 rounded-full bg-current"></span> "online"),
            Presence::Offline => badge(variant: BadgeVariant::Muted, <span class="size-1.5 rounded-full bg-current"></span> "offline"),
            Presence::NotSeen => badge(variant: BadgeVariant::Outline, "not seen"),
        }
    })
}

/// What the node filter on the home page matches, in `data-presence`.
pub(super) fn presence_key(presence: Presence) -> &'static str {
    match presence {
        Presence::Online => "online",
        Presence::Offline => "offline",
        Presence::NotSeen => "not-seen",
    }
}

/// A coloured dot before a short state text: green when active, grey otherwise.
#[component]
async fn status_dot(active: bool, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <p class="inline-flex items-center gap-1.5 text-sm text-muted-foreground">
            <span class=(if active { "size-2 rounded-full bg-success" } else { "size-2 rounded-full bg-muted-foreground/40" })></span>
            (child)
        </p>
    })
}

#[component]
async fn channel_row(device_id: String, channel: ChannelOverview) -> Result<impl View> {
    let label = format!(
        "{} · {}",
        entity_label(channel.entity, &channel.entity_name),
        channel_label(channel.channel, &channel.channel_name, channel.quantity)
    );
    Ok(view! {
        <li class="flex items-center justify-between gap-4 py-3">
            <div>
                <p class="text-sm font-medium">(label.clone())</p>
                channel_value(channel: channel.clone())
            </div>
            channel_controls(device_id: device_id, channel: channel, label: label)
        </li>
    })
}

/// The value, the on/off dot or the last event; nothing for actions, whose controls say it all.
#[component]
pub(super) async fn channel_value(channel: ChannelOverview) -> Result<impl View> {
    Ok(view! {
        match &channel.value {
            Some(Value::Bool(active)) => status_dot(active: *active, if *active { "on" } else { "off" }),
            _ if channel.invocation.is_some() => {}
            _ => <p class="text-sm text-muted-foreground">(channel_state(&channel))</p>,
        }
    })
}

/// The switch of a writable `Bool`, or the buttons or text box of a `Trigger`.
#[component]
pub(super) async fn channel_controls(
    device_id: String,
    channel: ChannelOverview,
    label: String,
) -> Result<impl View> {
    let on = channel.value == Some(Value::Bool(true));
    Ok(view! {
        if let Some(invocation) = channel.invocation.clone() {
            <form
                class="flex flex-wrap justify-end gap-2"
                hx-post="/invoke"
                hx-target="#status"
            >
                <input type="hidden" name="device_id" value=(device_id.clone())>
                <input type="hidden" name="entity" value=(channel.entity.to_string())>
                <input type="hidden" name="channel" value=(channel.channel.to_string())>
                match invocation {
                    Invocation::Options(options) => for option in options {
                        button(
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" name="value" value=(option.clone()) },
                            (option)
                        )
                    },
                    Invocation::Text { max_length } => {
                        // Kept while the node list refreshes, so typing is not lost.
                        input(attrs: attributes! {
                            id=(format!("text-{device_id}-{}-{}", channel.entity, channel.channel))
                            hx-preserve="true"
                            name="value"
                            required=""
                            maxlength=(max_length.to_string())
                            aria-label=(label.clone())
                            class="h-8 w-48"
                        })
                        button(
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" },
                            "Send"
                        )
                    },
                    Invocation::Plain => button(
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        attrs: attributes! { type="submit" },
                        "Run"
                    ),
                }
            </form>
        }
        if channel.switchable {
            <form
                hx-post="/set"
                hx-trigger="change"
                hx-target="#status"
                // Refreshes the list or page this switch is in, sooner than its next update.
                ("hx-on--after-request")="htmx.trigger(this.closest('[hx-get]'), 'refresh')"
            >
                <input type="hidden" name="device_id" value=(device_id.clone())>
                <input type="hidden" name="entity" value=(channel.entity.to_string())>
                <input type="hidden" name="channel" value=(channel.channel.to_string())>
                switch(attrs: attributes! {
                    name="value"
                    value="true"
                    aria-label=(label.clone())
                    if on {
                        checked=""
                    }
                })
            </form>
        }
    })
}

pub(super) fn node_description(node: &NodeOverview) -> String {
    let mut parts = Vec::new();
    if node.name.is_some() {
        parts.push(node.device_id.clone());
    }
    if node.presence == Presence::NotSeen {
        parts.push("not seen since the hub started".into());
    }
    if let Some(firmware_version) = &node.firmware_version {
        parts.push(format!("firmware {firmware_version}"));
    }
    if let Some(uptime) = node.uptime_seconds {
        parts.push(format!(
            "up {}",
            duration_label(Duration::from_secs(uptime.into()))
        ));
    }
    if let Some(rssi) = node.rssi {
        parts.push(format!("RSSI {rssi} dBm"));
    }
    parts.join(" · ")
}

/// `LED: Power`, or the entity alone when no channel is named.
fn configured_entity_label(entity: &ConfiguredEntity) -> String {
    let label = entity_label(entity.id, &entity.name);
    if entity.channels.is_empty() {
        return label;
    }
    let channels: Vec<&str> = entity
        .channels
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    format!("{label}: {}", channels.join(", "))
}

fn channel_state(channel: &ChannelOverview) -> String {
    if !channel.has_value {
        return match &channel.last_event {
            Some((kind, Some(data), ago)) => {
                format!("last event: {kind:?} {data}, {}", ago_label(*ago))
            }
            Some((kind, None, ago)) => format!("last event: {kind:?}, {}", ago_label(*ago)),
            None => "no events yet".into(),
        };
    }
    match &channel.value {
        Some(value) => value_label(value, channel.unit),
        None => "unknown".into(),
    }
}
