//! What the node page posts: edits of names and parameters, updates and forgetting a node.

use std::collections::HashMap;
use std::sync::Arc;

use oxihome_protocol::management;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        header::{HeaderName, HeaderValue},
        response::response_headers,
        route,
    },
    view::{ViewExt, ViewHandle, view},
};

use super::{Section, node_view, section_body};
use crate::config::nodes::{EntityEdit, NodeEdits, parameter_key};
use crate::hub::Hub;
use crate::web::chart::ChartRange;

#[derive(Deserialize)]
struct EditForm {
    device_id: String,
    section: Section,
    #[serde(flatten)]
    fields: HashMap<String, String>,
}

/// Saves one section's form.
#[route(POST "/nodes/edit")]
pub async fn edit(cx: &Cx, Form(form): Form<EditForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = edits(&form.fields).and_then(|edits| hub.edit_node(&form.device_id, &edits));

    // On success the section comes back shown, out of band; `#status` gets the error otherwise.
    let section = form.section;
    let node = match result {
        Ok(()) => {
            let range = ChartRange::from_key(form.fields.get("range").map(String::as_str));
            node_view(hub, &form.device_id, Some(section), range).await
        }
        Err(_) => None,
    };
    view! { cx =>
        if let Err(error) = &result {
            <pre class="text-sm whitespace-pre-wrap text-destructive">(format!("{error:#}"))</pre>
        }
        section_body(node: node, section: section, editing: false, out_of_band: true)
    }
    .single()
    .await
}

#[derive(Deserialize)]
struct UpdateForm {
    device_id: String,
}

/// Answers with the content of `#status`: empty on success, the error otherwise. Progress and the
/// outcome come as notifications.
#[route(POST "/nodes/update")]
pub async fn start_update(cx: &Cx, Form(form): Form<UpdateForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.start_update(&form.device_id).await;
    view! { cx =>
        if let Err(error) = result {
            <p class="text-sm text-destructive">(format!("{error:#}"))</p>
        }
    }
    .single()
    .await
}

/// On success the browser goes to the node list; the error goes to `#status` otherwise.
#[route(POST "/nodes/forget")]
pub async fn forget(cx: &Cx, Form(form): Form<UpdateForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.forget_node(&form.device_id);
    if result.is_ok() {
        response_headers(cx).append(
            HeaderName::from_static("hx-redirect"),
            HeaderValue::from_static("/nodes"),
        );
    }
    view! { cx =>
        if let Err(error) = result {
            <p class="text-sm text-destructive">(format!("{error:#}"))</p>
        }
    }
    .single()
    .await
}

/// `name`, `entity.<id>`, `channel.<entity>.<channel>` and `parameter.<key>` fields.
fn edits(fields: &HashMap<String, String>) -> anyhow::Result<NodeEdits> {
    let text = |value: &str| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    };
    let mut edits = NodeEdits {
        name: fields.get("name").map(|value| text(value)),
        ..NodeEdits::default()
    };
    for (field, value) in fields {
        let parts: Vec<&str> = field.split('.').collect();
        match parts.as_slice() {
            ["entity", id] => {
                entity_edit(&mut edits.entities, id.parse()?).name = Some(text(value));
            }
            ["channel", id, channel] => entity_edit(&mut edits.entities, id.parse()?)
                .channels
                .push((channel.parse()?, text(value))),
            ["parameter", key] => {
                let key = management::PARAMETERS
                    .into_iter()
                    .map(parameter_key)
                    .find(|known| known == key)
                    .ok_or_else(|| anyhow::anyhow!("unknown parameter {key}"))?;
                let value = match text(value) {
                    Some(value) => Some(
                        value
                            .parse::<i32>()
                            .map_err(|_| anyhow::anyhow!("{key} must be a whole number"))?,
                    ),
                    None => None,
                };
                edits.parameters.push((key, value));
            }
            _ => {}
        }
    }
    edits.entities.sort_by_key(|entity| entity.id);
    for entity in &mut edits.entities {
        entity.channels.sort_by_key(|(channel, _)| *channel);
    }
    edits.parameters.sort();
    Ok(edits)
}

fn entity_edit(entities: &mut Vec<EntityEdit>, id: u8) -> &mut EntityEdit {
    let index = match entities.iter().position(|entity| entity.id == id) {
        Some(index) => index,
        None => {
            entities.push(EntityEdit {
                id,
                ..EntityEdit::default()
            });
            entities.len() - 1
        }
    };
    &mut entities[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_fields_become_edits() {
        let fields: HashMap<String, String> = [
            ("name", " Test board "),
            ("entity.4", "External LED"),
            ("channel.4.0", ""),
            ("channel.1.0", "Power"),
            ("parameter.heartbeat_interval", "60"),
            ("parameter.wifi_check_interval", ""),
        ]
        .into_iter()
        .map(|(field, value)| (field.to_string(), value.to_string()))
        .collect();
        let edits = edits(&fields).unwrap();
        assert_eq!(edits.name, Some(Some("Test board".to_string())));
        assert_eq!(
            edits.entities,
            vec![
                EntityEdit {
                    id: 1,
                    name: None,
                    channels: vec![(0, Some("Power".to_string()))],
                },
                EntityEdit {
                    id: 4,
                    name: Some(Some("External LED".to_string())),
                    channels: vec![(0, None)],
                },
            ]
        );
        assert_eq!(
            edits.parameters,
            vec![
                ("heartbeat_interval", Some(60)),
                ("wifi_check_interval", None)
            ]
        );
    }

    #[test]
    fn bad_numbers_are_refused() {
        let fields: HashMap<String, String> = [(
            "parameter.heartbeat_interval".to_string(),
            "soon".to_string(),
        )]
        .into();
        assert!(
            edits(&fields)
                .unwrap_err()
                .to_string()
                .contains("whole number")
        );
    }
}
