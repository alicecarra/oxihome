use std::time::Duration;

use oxihome_protocol::{Value, management};
use topcoat::{
    Result,
    view::{Child, View, attributes, component, view},
};

use super::format::{ago_label, channel_label, duration_label, entity_label, value_label};
use super::overview::{ChannelOverview, NodeOverview};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::switch::switch;

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
            card_header(
                <div class="flex items-center justify-between gap-2">
                    card_title((title))
                    presence_badge(online: node.online)
                </div>
                card_description((description))
            )
            card_content(
                <ul class="divide-y divide-border">
                    for channel in channels {
                        channel_row(device_id: node.device_id.clone(), channel: channel)
                    }
                </ul>
            )
        )
    })
}

#[component]
async fn presence_badge(online: bool) -> Result<impl View> {
    Ok(view! {
        if online {
            badge(variant: BadgeVariant::Success, <span class="size-1.5 rounded-full bg-current"></span> "online")
        } else {
            badge(variant: BadgeVariant::Muted, <span class="size-1.5 rounded-full bg-current"></span> "offline")
        }
    })
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
    let on = channel.value == Some(Value::Bool(true));
    let label = format!(
        "{} · {}",
        entity_label(channel.entity, &channel.entity_name),
        channel_label(channel.channel, &channel.channel_name, channel.quantity)
    );
    Ok(view! {
        <li class="flex items-center justify-between gap-4 py-3">
            <div>
                <p class="text-sm font-medium">(label.clone())</p>
                match &channel.value {
                    Some(Value::Bool(active)) => status_dot(active: *active, if *active { "on" } else { "off" }),
                    _ if channel.invocations.is_some() => {}
                    _ => <p class="text-sm text-muted-foreground">(channel_state(&channel))</p>,
                }
            </div>
            if let Some(invocations) = channel.invocations.clone() {
                <form
                    class="flex flex-wrap justify-end gap-2"
                    hx-post="/invoke"
                    hx-target="#status"
                >
                    <input type="hidden" name="device_id" value=(device_id.clone())>
                    <input type="hidden" name="entity" value=(channel.entity.to_string())>
                    <input type="hidden" name="channel" value=(channel.channel.to_string())>
                    for invocation in invocations {
                        match invocation {
                            Some(option) => button(
                                variant: ButtonVariant::Outline,
                                size: ButtonSize::Sm,
                                attrs: attributes! { type="submit" name="value" value=(option.clone()) },
                                (option)
                            ),
                            None => button(
                                variant: ButtonVariant::Outline,
                                size: ButtonSize::Sm,
                                attrs: attributes! { type="submit" },
                                "Run"
                            ),
                        }
                    }
                </form>
            }
            if channel.switchable {
                <form
                    hx-post="/set"
                    hx-trigger="change"
                    hx-target="#status"
                    ("hx-on--after-request")="htmx.trigger('#nodes', 'refresh')"
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
        </li>
    })
}

fn node_description(node: &NodeOverview) -> String {
    let mut parts = Vec::new();
    if node.name.is_some() {
        parts.push(node.device_id.clone());
    }
    parts.push(format!("firmware {}", node.firmware_version));
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
