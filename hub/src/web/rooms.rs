use std::sync::Arc;

use oxihome_protocol::{Value, management};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, path_param},
    view::{View, component, view},
};

use super::dashboard::{RuleButton, channels_block, rule_button_view};
use super::format::{ago_label, query_value, value_label};
use super::layout::layout;
use super::nodes::presence_badge;
use super::overview::{
    self, DashboardChannel, NodeOverview, Presence, RoomOverview, SceneOverview,
};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::config::dashboard::ChannelReference;
use crate::hub::{Hub, node_event};

path_param!(name);

pub(super) fn room_path(room: &str) -> String {
    format!("/rooms/{}", query_value(room))
}

/// What makes a room refetch: its nodes' values, presence, config changes and scene runs.
pub(super) fn room_trigger(room: &RoomOverview) -> String {
    let mut triggers: Vec<String> = Vec::new();
    for node in &room.nodes {
        let trigger = node_event(&node.device_id)
            .map_or_else(|| "sse:nodes".to_string(), |name| format!("sse:{name}"));
        if !triggers.contains(&trigger) {
            triggers.push(trigger);
        }
    }
    triggers.extend(
        [
            "sse:presence",
            "sse:config",
            "sse:timeline",
            "every 60s",
            "refresh",
        ]
        .map(String::from),
    );
    triggers.join(", ")
}

/// Every room with its nodes and scenes.
#[page("/rooms")]
pub async fn rooms_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! {
        layout(
            title: "Oxihome rooms",
            <div id="rooms" hx-get="/rooms/fragment" hx-trigger="sse:nodes, sse:presence, sse:config, sse:timeline, every 60s, refresh">
                room_list(rooms: overview::rooms(hub))
            </div>
        )
    })
}

#[page("/rooms/fragment")]
pub async fn rooms_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { room_list(rooms: overview::rooms(hub)) })
}

#[component]
async fn room_list(rooms: Vec<RoomOverview>) -> Result<impl View> {
    Ok(view! {
        if rooms.is_empty() {
            <p class="text-muted-foreground">
                "No rooms yet: add them to nodes.toml in "
                <a href="/settings/nodes.toml" class="font-medium hover:underline">"Settings"</a>
                "."
            </p>
        }
        <div class="grid gap-4 lg:grid-cols-2">
            for room in rooms {
                room_card(room: room)
            }
        </div>
    })
}

#[component]
async fn room_card(room: RoomOverview) -> Result<impl View> {
    let name = room.name.clone();
    let path = room_path(&name);
    Ok(view! {
        card(
            card_header(card_title(
                <a href=(path) class="hover:underline">(name)</a>
            ))
            card_content(room_details(room: room))
        )
    })
}

/// One room: its scenes, then each node's channels with values and controls.
#[page("/rooms/{name}")]
pub async fn room_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let name = path_param::<Name>(cx).to_string();
    let title = format!("{name} · Oxihome");
    let room = overview::room(hub, &name);
    let trigger = room.as_ref().map(room_trigger).unwrap_or_default();
    let fragment = format!("{}/fragment", room_path(&name));
    let nodes = room
        .as_ref()
        .map(|room| room_channels(hub, room))
        .unwrap_or_default();
    Ok(view! {
        layout(
            title: &title,
            <div class="mb-4">
                <a href="/rooms" class="text-sm text-muted-foreground hover:underline">"← Rooms"</a>
                <h1 class="text-xl font-semibold">(name.clone())</h1>
            </div>
            <div id="room" hx-get=(fragment) hx-trigger=(trigger)>
                room_content(room: room, nodes: nodes, name: name)
            </div>
        )
    })
}

#[page("/rooms/{name}/fragment")]
pub async fn room_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let name = path_param::<Name>(cx).to_string();
    let room = overview::room(hub, &name);
    let nodes = room
        .as_ref()
        .map(|room| room_channels(hub, room))
        .unwrap_or_default();
    Ok(view! { room_content(room: room, nodes: nodes, name: name) })
}

/// A node of the room and its channels, without the management entity.
struct RoomNode {
    node: NodeOverview,
    channels: Vec<DashboardChannel>,
}

fn room_channels(hub: &Hub, room: &RoomOverview) -> Vec<RoomNode> {
    room.nodes
        .iter()
        .map(|node| {
            let references: Vec<ChannelReference> = node
                .channels
                .iter()
                .filter(|channel| channel.entity != management::ENTITY)
                .map(|channel| ChannelReference {
                    device: node.device_id.clone(),
                    entity: channel.entity,
                    channel: channel.channel,
                })
                .collect();
            RoomNode {
                node: node.clone(),
                channels: overview::dashboard_channels(hub, &references),
            }
        })
        .collect()
}

#[component]
async fn room_content(
    room: Option<RoomOverview>,
    nodes: Vec<RoomNode>,
    name: String,
) -> Result<impl View> {
    Ok(view! {
        match room {
            None => <p class="text-muted-foreground">(format!("No room named \"{name}\"."))</p>,
            Some(room) => <div class="grid gap-4 lg:grid-cols-2">
                if !room.scenes.is_empty() {
                    card(
                        attrs: topcoat::view::attributes! { class="lg:col-span-2" },
                        card_header(card_title("Scenes"))
                        card_content(scene_list(scenes: room.scenes))
                    )
                }
                if nodes.is_empty() {
                    <p class="text-muted-foreground">"No nodes in this room."</p>
                }
                for RoomNode { node, channels } in nodes {
                    card(
                        card_header(
                            <div class="flex items-center justify-between gap-2">
                                card_title(
                                    <a href=(format!("/nodes/{}", node.device_id)) class="hover:underline">
                                        (node.name.clone().unwrap_or_else(|| node.device_id.clone()))
                                    </a>
                                )
                                presence_badge(presence: node.presence)
                            </div>
                        )
                        card_content(
                            if channels.is_empty() {
                                <p class="text-sm text-muted-foreground">"No channels known yet."</p>
                            } else {
                                channels_block(channels: channels, several_nodes: false)
                            }
                        )
                    )
                }
            </div>,
        }
    })
}

/// The card of a room in the list and the dashboard's `room` block.
#[component]
async fn room_details(room: RoomOverview) -> Result<impl View> {
    let online = room
        .nodes
        .iter()
        .filter(|node| node.presence == Presence::Online)
        .count();
    let total = room.nodes.len();
    let not_online: Vec<NodeOverview> = room
        .nodes
        .iter()
        .filter(|node| node.presence != Presence::Online)
        .cloned()
        .collect();
    let measures: Vec<(String, String)> = room.nodes.iter().flat_map(measures).collect();
    Ok(view! {
        <div class="space-y-3">
            <p class="text-sm text-muted-foreground">(format!("{online} of {total} nodes online"))</p>
            if !not_online.is_empty() {
                <ul class="space-y-1">
                    for node in not_online {
                        <li class="flex items-center gap-2 text-sm">
                            <a href=(format!("/nodes/{}", node.device_id)) class="font-medium hover:underline">
                                (node.name.clone().unwrap_or_else(|| node.device_id.clone()))
                            </a>
                            presence_badge(presence: node.presence)
                        </li>
                    }
                </ul>
            }
            if !measures.is_empty() {
                <dl class="grid grid-cols-[1fr_auto] gap-x-4 gap-y-1 text-sm">
                    for (label, value) in measures {
                        <dt class="text-muted-foreground">(label)</dt>
                        <dd class="text-right tabular-nums">(value)</dd>
                    }
                </dl>
            }
            scene_list(scenes: room.scenes)
        </div>
    })
}

/// What an online node measures, as label and value.
fn measures(node: &NodeOverview) -> Vec<(String, String)> {
    if node.presence != Presence::Online {
        return Vec::new();
    }
    node.channels
        .iter()
        .filter(|channel| channel.chartable)
        .filter_map(|channel| {
            let value: &Value = channel.value.as_ref()?;
            let label = channel
                .channel_name
                .clone()
                .or_else(|| channel.quantity.map(|quantity| format!("{quantity:?}")))
                .or_else(|| channel.entity_name.clone())?;
            Some((label, value_label(value, channel.unit)))
        })
        .collect()
}

#[component]
pub(super) async fn room_summary(room: Option<RoomOverview>) -> Result<impl View> {
    let path = room
        .as_ref()
        .map(|room| room_path(&room.name))
        .unwrap_or_default();
    Ok(view! {
        match room {
            None => <p class="text-sm text-muted-foreground">"This room is no longer in nodes.toml."</p>,
            Some(room) => <div class="space-y-3">
                <a href=(path) class="text-sm font-medium hover:underline">"Open room"</a>
                room_details(room: room)
            </div>,
        }
    })
}

/// A button for each scene, with how its last run went.
#[component]
async fn scene_list(scenes: Vec<SceneOverview>) -> Result<impl View> {
    Ok(view! {
        if !scenes.is_empty() {
            <div class="flex flex-wrap gap-3">
                for scene in scenes {
                    <div class="min-w-32 flex-1 space-y-1">
                        rule_button_view(rule_button: RuleButton {
                            button: scene.button.clone(),
                            rule: scene.rule.clone(),
                            enabled: scene.enabled,
                        })
                        scene_state(scene: scene)
                    </div>
                }
            </div>
        }
    })
}

#[component]
async fn scene_state(scene: SceneOverview) -> Result<impl View> {
    let text = match &scene.last_run {
        _ if scene.running => "running".to_string(),
        Some((ago, errors)) if !errors.is_empty() => format!("failed {}", ago_label(*ago)),
        Some((ago, _)) => format!("ran {}", ago_label(*ago)),
        None => "not run yet".to_string(),
    };
    let failed = !scene.running && scene.last_run.is_some_and(|(_, errors)| !errors.is_empty());
    Ok(view! {
        <p class=(if failed { "text-xs text-destructive" } else { "text-xs text-muted-foreground" })>(text)</p>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_paths_are_encoded() {
        assert_eq!(room_path("Sala"), "/rooms/Sala");
        assert_eq!(room_path("Escritório"), "/rooms/Escrit%C3%B3rio");
    }

    #[test]
    fn rooms_listen_to_their_nodes() {
        let room = RoomOverview {
            name: "Sala".into(),
            nodes: Vec::new(),
            scenes: Vec::new(),
        };
        assert_eq!(
            room_trigger(&room),
            "sse:presence, sse:config, sse:timeline, every 60s, refresh"
        );
    }
}
