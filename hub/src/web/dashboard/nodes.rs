use topcoat::{
    Result,
    view::{View, component, view},
};

use crate::web::nodes::{presence_badge, protocol_badge};
use crate::web::overview::{NodeOverview, Presence};

/// The nodes grouped by presence; empty groups are left out.
#[component]
pub(super) async fn node_status(nodes: Vec<NodeOverview>) -> Result<impl View> {
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
                                    <span class="ml-2 text-xs text-muted-foreground">(node.device_id.clone())</span>
                                }
                                <span class="ml-2">protocol_badge(version: node.protocol_version)</span>
                            </li>
                        }
                    </ul>
                </div>
            }
        </div>
    })
}
