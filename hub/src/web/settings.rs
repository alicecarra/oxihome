use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, component, view},
};

use super::layout::layout;
use crate::hub::{Hub, NodeOverview};
use crate::names::NameTarget;

#[page("/settings")]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let nodes = hub.overview();
    let forms: Vec<(String, Vec<NameRow>)> = nodes.into_iter().map(name_rows).collect();

    Ok(view! {
        layout(
            title: "Oxihome settings",
            <h1>"Settings"</h1>
                <p><a href="/">"Back"</a></p>
                <div id="status"></div>
                <p>"Leave a name empty to remove it."</p>
                if forms.is_empty() {
                    <p>"No nodes yet."</p>
                }
                for (device_id, rows) in forms {
                    <h2>(device_id)</h2>
                    for row in rows {
                        name_form(row: row)
                    }
                }
        )
    })
}

struct NameRow {
    device_id: String,
    entity: Option<u8>,
    channel: Option<u8>,
    label: String,
    name: Option<String>,
}

/// One form for the node, one per entity and one per channel.
fn name_rows(node: NodeOverview) -> (String, Vec<NameRow>) {
    let mut rows = vec![NameRow {
        device_id: node.device_id.clone(),
        entity: None,
        channel: None,
        label: "Node".into(),
        name: node.name,
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
            label: format!("Entity {} channel {}", channel.entity, channel.channel),
            name: channel.channel_name,
        });
    }
    (node.device_id, rows)
}

#[component]
async fn name_form(row: NameRow) -> Result<impl View> {
    Ok(view! {
        <form hx-post="/rename" hx-target="#status">
            <label>(row.label) " "
                <input type="text" name="name" value=(row.name.unwrap_or_default())>
            </label>
            <input type="hidden" name="device_id" value=(row.device_id)>
            if let Some(entity) = row.entity {
                <input type="hidden" name="entity" value=(entity.to_string())>
            }
            if let Some(channel) = row.channel {
                <input type="hidden" name="channel" value=(channel.to_string())>
            }
            " " <button>"Save"</button>
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
            Ok(()) => <p>"Saved."</p>,
            Err(error) => <p><strong>"Error: "</strong>(error.to_string())</p>,
        }
    }
    .single()
    .await
}
