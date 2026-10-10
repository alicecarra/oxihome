use std::time::Duration;

use oxihome_protocol::Value;
use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::hub::{ChannelOverview, NodeOverview};

#[component]
pub async fn node_list(nodes: Vec<NodeOverview>) -> Result<impl View> {
    Ok(view! {
        if nodes.is_empty() {
            <p class="text-muted-foreground">"No nodes yet."</p>
        }
        <div class="space-y-4">
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
                <ul class="divide-y">
                    for channel in node.channels {
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
            badge(variant: BadgeVariant::Primary, "online")
        } else {
            badge(variant: BadgeVariant::Outline, "offline")
        }
    })
}

#[component]
async fn channel_row(device_id: String, channel: ChannelOverview) -> Result<impl View> {
    let on = channel.value == Some(Value::Bool(true));
    Ok(view! {
        <li class="flex items-center justify-between gap-4 py-3">
            <div>
                <p class="text-sm font-medium">
                    (entity_label(channel.entity, &channel.entity_name)) " · "
                    (channel_label(channel.channel, &channel.channel_name))
                </p>
                <p class="text-sm text-muted-foreground">(channel_state(&channel))</p>
            </div>
            if channel.switchable {
                <form
                    hx-post="/set"
                    hx-target="#status"
                    ("hx-on--after-request")="htmx.trigger('#nodes', 'refresh')"
                >
                    <input type="hidden" name="device_id" value=(device_id)>
                    <input type="hidden" name="entity" value=(channel.entity.to_string())>
                    <input type="hidden" name="channel" value=(channel.channel.to_string())>
                    <input type="hidden" name="value" value=((!on).to_string())>
                    button(
                        variant: if on { ButtonVariant::Outline } else { ButtonVariant::Primary },
                        size: ButtonSize::Sm,
                        if on { "Turn off" } else { "Turn on" }
                    )
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
        return match channel.last_event {
            Some((kind, ago)) => format!("last event: {kind:?}, {}", ago_label(ago)),
            None => "no events yet".into(),
        };
    }
    match &channel.value {
        Some(Value::Bool(true)) => "on".into(),
        Some(Value::Bool(false)) => "off".into(),
        Some(value) => format!("{value:?}"),
        None => "unknown".into(),
    }
}

pub fn ago_label(ago: Duration) -> String {
    format!("{} ago", duration_label(ago))
}

pub fn duration_label(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        3600..86400 => format!("{} h", seconds / 3600),
        _ => format!("{} d", seconds / 86400),
    }
}

pub fn entity_label(entity: u8, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| format!("entity {entity}"))
}

pub fn channel_label(channel: u8, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| format!("channel {channel}"))
}
