use std::sync::Arc;

use oxihome_protocol::Value;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, view},
};

use super::layout::layout;
use super::parts::node_list;
use crate::hub::Hub;

#[page("/")]
pub async fn home(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);

    Ok(view! {
        layout(
            title: "Oxihome",
            <div id="nodes" hx-get="/nodes" hx-trigger="every 2s, refresh">
                node_list(nodes: hub.overview())
            </div>
        )
    })
}

#[page("/nodes")]
pub async fn nodes_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { node_list(nodes: hub.overview()) })
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
pub async fn set(cx: &Cx, Form(form): Form<SetForm>) -> Result<ViewHandle> {
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
            <p class="text-sm text-destructive">(error.to_string())</p>
        }
    }
    .single()
    .await
}
