use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::layout::layout;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::components::label::label;
use crate::hub::{Hub, NodeOverview};
use crate::names::NameTarget;

#[page("/settings")]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let nodes: Vec<NodeNames> = hub.overview().into_iter().map(node_names).collect();

    Ok(view! {
        layout(
            title: "Oxihome settings",
            <div class="mb-4">
                <h1 class="text-xl font-semibold">"Names"</h1>
                <p class="text-sm text-muted-foreground">
                    "Friendly names shown on the nodes page. Leave a name empty to remove it."
                </p>
            </div>
            if nodes.is_empty() {
                <p class="text-muted-foreground">"No nodes yet."</p>
            }
            <div class="space-y-4">
                for node in nodes {
                    card(
                        card_header(
                            card_title((node.title))
                            card_description((node.device_id))
                        )
                        card_content(
                            <div class="divide-y divide-border">
                                for row in node.rows {
                                    name_form(row: row)
                                }
                            </div>
                        )
                    )
                }
            </div>
        )
    })
}

struct NodeNames {
    device_id: String,
    title: String,
    rows: Vec<NameRow>,
}

struct NameRow {
    device_id: String,
    entity: Option<u8>,
    channel: Option<u8>,
    label: String,
    name: Option<String>,
}

/// One form for the node, one per entity and one per channel.
fn node_names(node: NodeOverview) -> NodeNames {
    let mut rows = vec![NameRow {
        device_id: node.device_id.clone(),
        entity: None,
        channel: None,
        label: "Node".into(),
        name: node.name.clone(),
    }];
    let mut previous_entity = None;
    for channel in node.channels {
        if previous_entity != Some(channel.entity) {
            previous_entity = Some(channel.entity);
            rows.push(NameRow {
                device_id: node.device_id.clone(),
                entity: Some(channel.entity),
                channel: None,
                label: format!("Entity {}", channel.entity),
                name: channel.entity_name,
            });
        }
        rows.push(NameRow {
            device_id: node.device_id.clone(),
            entity: Some(channel.entity),
            channel: Some(channel.channel),
            label: format!("Channel {}", channel.channel),
            name: channel.channel_name,
        });
    }
    NodeNames {
        title: node.name.unwrap_or_else(|| node.device_id.clone()),
        device_id: node.device_id,
        rows,
    }
}

#[component]
async fn name_form(row: NameRow) -> Result<impl View> {
    let id = format!(
        "name-{}-{}-{}",
        row.device_id,
        row.entity
            .map(|entity| entity.to_string())
            .unwrap_or_default(),
        row.channel
            .map(|channel| channel.to_string())
            .unwrap_or_default(),
    );
    let label_class = if row.channel.is_some() {
        "pl-4 text-muted-foreground"
    } else {
        ""
    };
    Ok(view! {
        <form
            class="grid grid-cols-[8rem_1fr_auto] items-center gap-3 py-3"
            hx-post="/rename"
            hx-target="#status"
        >
            label(attrs: attributes! { for=(id.clone()) class=(label_class) }, (row.label))
            input(attrs: attributes! {
                id=(id)
                type="text"
                name="name"
                value=(row.name.unwrap_or_default())
            })
            <input type="hidden" name="device_id" value=(row.device_id)>
            if let Some(entity) = row.entity {
                <input type="hidden" name="entity" value=(entity.to_string())>
            }
            if let Some(channel) = row.channel {
                <input type="hidden" name="channel" value=(channel.to_string())>
            }
            button(variant: ButtonVariant::Secondary, size: ButtonSize::Sm, "Save")
        </form>
    })
}

#[derive(Deserialize)]
struct RenameForm {
    device_id: String,
    entity: Option<u8>,
    channel: Option<u8>,
    name: String,
}

/// Answers with the content of `#status`.
#[route(POST "/rename")]
pub async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let target = match (form.entity, form.channel) {
        (None, None) => Some(NameTarget::Node(form.device_id)),
        (Some(entity), None) => Some(NameTarget::Entity(form.device_id, entity)),
        (Some(entity), Some(channel)) => Some(NameTarget::Channel(form.device_id, entity, channel)),
        (None, Some(_)) => None,
    };
    let result = match target {
        Some(target) => hub.rename(target, &form.name),
        None => Err(anyhow::anyhow!("a channel needs an entity")),
    };

    view! { cx =>
        match result {
            Ok(()) => <p class="text-sm text-muted-foreground">"Saved."</p>,
            Err(error) => <p class="text-sm text-destructive">(error.to_string())</p>,
        }
    }
    .single()
    .await
}
