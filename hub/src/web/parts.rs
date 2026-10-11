use std::time::Duration;

use oxihome_protocol::{Quantity, Unit, Value, management};
use topcoat::{
    Result,
    view::{Child, View, attributes, component, view},
};

use super::overview::{ChannelOverview, NodeOverview};
use crate::components::badge::{BadgeVariant, badge};
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
                    _ => <p class="text-sm text-muted-foreground">(channel_state(&channel))</p>,
                }
            </div>
            if channel.switchable {
                <form
                    hx-post="/set"
                    hx-trigger="change"
                    hx-target="#status"
                    ("hx-on--after-request")="htmx.trigger('#nodes', 'refresh')"
                >
                    <input type="hidden" name="device_id" value=(device_id)>
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
        return match channel.last_event {
            Some((kind, ago)) => format!("last event: {kind:?}, {}", ago_label(ago)),
            None => "no events yet".into(),
        };
    }
    match &channel.value {
        Some(value) => value_label(value, channel.unit),
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

pub fn channel_label(channel: u8, name: &Option<String>, quantity: Option<Quantity>) -> String {
    name.clone()
        .unwrap_or_else(|| default_channel_name(channel, quantity))
}

/// What a channel is called until it gets a name: its quantity, or its id.
pub fn default_channel_name(channel: u8, quantity: Option<Quantity>) -> String {
    match quantity {
        Some(quantity) => format!("{quantity:?}"),
        None => format!("channel {channel}"),
    }
}

/// `22.3 °C`, `30 s`, `on`.
pub fn value_label(value: &Value, unit: Option<Unit>) -> String {
    let suffix = unit
        .map(|unit| format!(" {}", unit_symbol(unit)))
        .unwrap_or_default();
    match value {
        Value::Bool(on) => if *on { "on" } else { "off" }.into(),
        Value::Int(number) => format!("{number}{suffix}"),
        Value::Float(number) => format!("{number:.1}{suffix}"),
        Value::Enum(option) => option.clone(),
        Value::Color(color) => format!("{color:?}"),
        Value::Bytes(bytes) => format!("{} bytes", bytes.len()),
    }
}

fn unit_symbol(unit: Unit) -> &'static str {
    match unit {
        Unit::Celsius => "°C",
        Unit::Percent => "%",
        Unit::Lux => "lx",
        Unit::Volt => "V",
        Unit::Ampere => "A",
        Unit::Watt => "W",
        Unit::Second => "s",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_has_one_decimal_and_unit() {
        assert_eq!(
            value_label(&Value::Float(22.34), Some(Unit::Celsius)),
            "22.3 °C"
        );
        assert_eq!(
            value_label(&Value::Float(54.0), Some(Unit::Percent)),
            "54.0 %"
        );
    }

    #[test]
    fn value_without_unit_is_just_the_number() {
        assert_eq!(value_label(&Value::Float(1.25), None), "1.2");
        assert_eq!(value_label(&Value::Int(30), Some(Unit::Second)), "30 s");
        assert_eq!(value_label(&Value::Bool(true), None), "on");
    }

    #[test]
    fn default_channel_name_prefers_the_quantity() {
        assert_eq!(
            default_channel_name(0, Some(Quantity::Temperature)),
            "Temperature"
        );
        assert_eq!(default_channel_name(1, None), "channel 1");
        assert_eq!(
            channel_label(0, &Some("power".into()), Some(Quantity::Power)),
            "power"
        );
    }
}
