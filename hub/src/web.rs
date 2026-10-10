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

const HTMX: &str = include_str!("../static/htmx.min.js");

pub fn router(hub: Arc<Hub>) -> Router {
    Router::builder()
        .page(home)
        .page(nodes_fragment)
        .route(set)
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
            <h2>(node.device_id.clone())</h2>
            <p>
                if node.online { "online" } else { "offline" }
                ", firmware " (node.firmware_version.clone())
            </p>
            <ul>
                for channel in node.channels {
                    <li>
                        "entity " (channel.entity.to_string())
                        ", channel " (channel.channel.to_string()) ": "
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
