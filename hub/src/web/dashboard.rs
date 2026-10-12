use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    view::{View, component, view},
};

use super::layout::layout;
use super::nodes::{channel_controls, channel_value, presence_badge};
use super::overview::{self, DashboardChannel, NodeOverview, Presence};
use super::panel::{PanelButton, panel_button_view, panel_buttons};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::config::dashboard::{Block, BlockKind};
use crate::config::panels::Panel;
use crate::hub::Hub;

/// A block with what it shows, read before rendering.
struct BlockView {
    title: String,
    width: u8,
    content: BlockContent,
}

enum BlockContent {
    Nodes(Vec<NodeOverview>),
    Channels {
        channels: Vec<DashboardChannel>,
        /// Whether rows name their node: only when the block mixes nodes.
        several_nodes: bool,
    },
    Panel(Vec<PanelButton>),
}

#[page("/")]
pub async fn dashboard_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! {
        layout(
            title: "Oxihome",
            <div id="dashboard" hx-get="/dashboard/fragment" hx-trigger="sse:nodes, every 30s, refresh">
                dashboard_blocks(blocks: block_views(hub))
            </div>
        )
    })
}

#[page("/dashboard/fragment")]
pub async fn dashboard_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { dashboard_blocks(blocks: block_views(hub)) })
}

fn block_views(hub: &Hub) -> Vec<BlockView> {
    let Some(dashboard) = hub.dashboard() else {
        return Vec::new();
    };
    let panels = hub.panels();
    dashboard
        .blocks
        .iter()
        .map(|block| block_view(hub, block, &panels))
        .collect()
}

fn block_view(hub: &Hub, block: &Block, panels: &[Panel]) -> BlockView {
    let (default_title, content) = match block.kind {
        BlockKind::Nodes => ("Nodes".to_string(), BlockContent::Nodes(overview::nodes(hub))),
        BlockKind::Channels => {
            let channels = overview::dashboard_channels(hub, &block.channels);
            let several_nodes = channels
                .iter()
                .any(|channel| channel.device_id != channels[0].device_id);
            let title = if several_nodes {
                "Channels".to_string()
            } else {
                channels[0].node_label.clone()
            };
            (
                title,
                BlockContent::Channels {
                    channels,
                    several_nodes,
                },
            )
        }
        BlockKind::Panel => {
            // Validation makes sure the panel exists.
            let name = block.panel.clone().unwrap_or_default();
            let buttons = panels
                .iter()
                .find(|panel| panel.name == name)
                .map(|panel| panel_buttons(hub, panel))
                .unwrap_or_default();
            (name, BlockContent::Panel(buttons))
        }
    };
    BlockView {
        title: block.title.clone().unwrap_or(default_title),
        width: block.width,
        content,
    }
}

#[component]
async fn dashboard_blocks(blocks: Vec<BlockView>) -> Result<impl View> {
    Ok(view! {
        if blocks.is_empty() {
            <p class="text-muted-foreground">
                "No dashboard yet: add blocks to dashboard.toml in "
                <a href="/settings#dashboard.toml" class="font-medium hover:underline">"Settings"</a>
                ", or see all nodes in "
                <a href="/nodes" class="font-medium hover:underline">"Nodes"</a>
                "."
            </p>
        }
        <div class="grid items-start gap-4 lg:grid-cols-3">
            for block in blocks {
                <div class=(width_class(block.width))>
                    card(
                        card_header(card_title((block.title)))
                        card_content(
                            match block.content {
                                BlockContent::Nodes(nodes) => node_status(nodes: nodes),
                                BlockContent::Channels { channels, several_nodes } => {
                                    <ul class="divide-y divide-border">
                                        for channel in channels {
                                            dashboard_channel_row(channel: channel, several_nodes: several_nodes)
                                        }
                                    </ul>
                                },
                                BlockContent::Panel(buttons) => {
                                    // In one row while they fit, sharing the width.
                                    <div class="flex flex-wrap gap-3">
                                        for panel_button in buttons {
                                            <div class="min-w-32 flex-1">
                                                panel_button_view(panel_button: panel_button)
                                            </div>
                                        }
                                    </div>
                                },
                            }
                        )
                    )
                </div>
            }
        </div>
    })
}

fn width_class(width: u8) -> &'static str {
    match width {
        2 => "lg:col-span-2",
        3 => "lg:col-span-3",
        _ => "",
    }
}

/// The nodes grouped by presence; empty groups are left out.
#[component]
async fn node_status(nodes: Vec<NodeOverview>) -> Result<impl View> {
    let groups: Vec<(Presence, Vec<NodeOverview>)> =
        [Presence::Online, Presence::Offline, Presence::NotSeen]
            .into_iter()
            .map(|presence| {
                let members = nodes
                    .iter()
                    .filter(|node| node.presence == presence)
                    .cloned()
                    .collect();
                (presence, members)
            })
            .filter(|(_, members): &(Presence, Vec<NodeOverview>)| !members.is_empty())
            .collect();
    Ok(view! {
        if groups.is_empty() {
            <p class="text-sm text-muted-foreground">"No nodes yet."</p>
        }
        <div class="space-y-4">
            for (presence, members) in groups {
                <div class="space-y-2">
                    presence_badge(presence: presence)
                    <ul class="space-y-1">
                        for node in members {
                            <li>
                                <a href=(format!("/nodes/{}", node.device_id)) class="text-sm font-medium hover:underline">
                                    (node.name.clone().unwrap_or_else(|| node.device_id.clone()))
                                </a>
                                if node.name.is_some() {
                                    <span class="ml-2 text-xs text-muted-foreground">(node.device_id)</span>
                                }
                            </li>
                        }
                    </ul>
                </div>
            }
        </div>
    })
}

#[component]
async fn dashboard_channel_row(channel: DashboardChannel, several_nodes: bool) -> Result<impl View> {
    let label = if several_nodes {
        format!("{} · {}", channel.node_label, channel.label)
    } else {
        channel.label.clone()
    };
    Ok(view! {
        <li class="flex items-center justify-between gap-4 py-3">
            <div>
                <p class="text-sm font-medium">(label.clone())</p>
                match channel.channel.clone() {
                    Ok(overview) => channel_value(channel: overview),
                    Err(reason) => <p class="text-sm text-muted-foreground">(reason)</p>,
                }
            </div>
            if let Ok(overview) = channel.channel {
                channel_controls(device_id: channel.device_id, channel: overview, label: label)
            }
        </li>
    })
}
