use std::sync::Arc;

use oxihome_protocol::Value;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Router,
        content::{Form, Js},
        page, route,
    },
    view::{View, ViewExt, ViewHandle, component, view},
};

use crate::hub::{Hub, NodeOverview};
use crate::names::NameTarget;

const HTMX: &str = include_str!("../static/htmx.min.js");

pub fn router(hub: Arc<Hub>) -> Router {
    Router::builder()
        .page(home)
        .page(nodes_fragment)
        .page(settings)
        .route(set)
        .route(rename)
        .route(htmx_script)
        .app_context(hub)
        .build()
}

#[page("/")]
async fn home(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);

    Ok(view! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">
                <title>"Oxihome"</title>
                <script src="/static/htmx.min.js"></script>
            </head>
            <body>
                <h1>"Oxihome Hub"</h1>
                <p><a href="/settings">"Settings"</a></p>
                <div id="status"></div>
                <div id="nodes" hx-get="/nodes" hx-trigger="every 2s, refresh">
                    node_list(nodes: hub.overview())
                </div>
            </body>
        </html>
    })
}

#[page("/nodes")]
async fn nodes_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { node_list(nodes: hub.overview()) })
}

#[component]
async fn node_list(nodes: Vec<NodeOverview>) -> Result<impl View> {
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
            </p>
            <ul>
                for channel in node.channels {
                    <li>
                        (entity_label(channel.entity, &channel.entity_name)) ", "
                        (channel_label(channel.channel, &channel.channel_name)) ": "
                        match &channel.value {
                            _ if !channel.has_value => "no value",
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

fn entity_label(entity: u8, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| format!("entity {entity}"))
}

fn channel_label(channel: u8, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| format!("channel {channel}"))
}

#[page("/settings")]
async fn settings(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let nodes = hub.overview();
    let forms: Vec<(String, Vec<NameRow>)> = nodes.into_iter().map(name_rows).collect();

    Ok(view! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">
                <title>"Oxihome settings"</title>
                <script src="/static/htmx.min.js"></script>
            </head>
            <body>
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
            </body>
        </html>
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
async fn rename(cx: &Cx, Form(form): Form<RenameForm>) -> Result<ViewHandle> {
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

#[derive(Deserialize)]
struct SetForm {
    device_id: String,
    entity: u8,
    channel: u8,
    value: bool,
}

/// Answers with the content of `#status`: empty on success, the error otherwise.
#[route(POST "/set")]
async fn set(cx: &Cx, Form(form): Form<SetForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub
        .set(
            &form.device_id,
            form.entity,
            form.channel,
            Value::Bool(form.value),
        )
        .await;

    view! { cx =>
        if let Err(error) = result {
            <p><strong>"Error: "</strong>(error.to_string())</p>
        }
    }
    .single()
    .await
}

#[route(GET "/static/htmx.min.js")]
async fn htmx_script() -> Result<Js<&'static str>> {
    Ok(Js(HTMX))
}
