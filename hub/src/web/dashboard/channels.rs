use topcoat::{
    Result,
    view::{View, component, view},
};

use super::value::{percent, percent_bar};
use crate::web::nodes::{channel_controls, channel_value};
use crate::web::overview::DashboardChannel;

#[component]
pub(super) async fn channels_block(
    channels: Vec<DashboardChannel>,
    several_nodes: bool,
) -> Result<impl View> {
    Ok(view! {
        <ul class="divide-y divide-border">
            for channel in channels {
                dashboard_channel_row(channel: channel, several_nodes: several_nodes)
            }
        </ul>
    })
}

#[component]
async fn dashboard_channel_row(
    channel: DashboardChannel,
    several_nodes: bool,
) -> Result<impl View> {
    let label = if several_nodes {
        format!("{} · {}", channel.node_label, channel.label)
    } else {
        channel.label.clone()
    };
    let percent = percent(&channel);
    Ok(view! {
        <li class="flex items-center justify-between gap-4 py-3">
            <div class="min-w-0 flex-1 space-y-1">
                <p class="text-sm font-medium">(label.clone())</p>
                match channel.channel.clone() {
                    Ok(overview) => channel_value(channel: overview),
                    Err(reason) => <p class="text-sm text-muted-foreground">(reason)</p>,
                }
                if let Some(percent) = percent {
                    percent_bar(percent: percent)
                }
            </div>
            if let Ok(overview) = channel.channel {
                channel_controls(device_id: channel.device_id, channel: overview, label: label)
            }
        </li>
    })
}
