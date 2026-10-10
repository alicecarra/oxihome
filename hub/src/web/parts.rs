use std::time::Duration;

use oxihome_protocol::Value;
use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::hub::NodeOverview;

#[component]
pub async fn node_list(nodes: Vec<NodeOverview>) -> Result<impl View> {
    Ok(view! {
        if nodes.is_empty() {
            <p>"No nodes yet."</p>
        }
        for node in nodes {
            <h2>
                match &node.name {
                    Some(name) => {
                        (name.clone()) " " <small>(node.device_id.clone())</small>
                    },
                    None => (node.device_id.clone()),
                }
            </h2>
            <p>
                if node.online { "online" } else { "offline" }
                ", firmware " (node.firmware_version.clone())
                if let Some(uptime) = node.uptime_seconds {
                    ", up " (duration_label(Duration::from_secs(uptime.into())))
                }
                if let Some(rssi) = node.rssi {
                    ", RSSI " (rssi.to_string()) " dBm"
                }
            </p>
            <ul>
                for channel in node.channels {
                    <li>
                        (entity_label(channel.entity, &channel.entity_name)) ", "
                        (channel_label(channel.channel, &channel.channel_name)) ": "
                        match &channel.value {
                            _ if !channel.has_value => match channel.last_event {
                                Some((kind, ago)) => {
                                    "last event: " (format!("{kind:?}")) ", " (ago_label(ago))
                                },
                                None => "no events yet",
                            },
                            Some(value) => (format!("{value:?}")),
                            None => "unknown",
                        }
                        if channel.switchable {
                            let on = channel.value == Some(Value::Bool(true));
                            <form
                                hx-post="/set"
                                hx-target="#status"
                                ("hx-on--after-request")="htmx.trigger('#nodes', 'refresh')"
                            >
                                <input type="hidden" name="device_id" value=(node.device_id.clone())>
                                <input type="hidden" name="entity" value=(channel.entity.to_string())>
                                <input type="hidden" name="channel" value=(channel.channel.to_string())>
                                <input type="hidden" name="value" value=((!on).to_string())>
                                <button>if on { "Turn off" } else { "Turn on" }</button>
                            </form>
                        }
                    </li>
                }
            </ul>
        }
    })
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
