use std::sync::Arc;

use oxihome_protocol::Value;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::redirect, page, route},
    view::{ViewExt, ViewHandle, view},
};

use crate::hub::Hub;

/// The dashboard will live here; until then, the node list.
#[page("/")]
pub async fn home() -> Result<()> {
    Err(redirect("/nodes").into())
}

#[derive(Deserialize)]
struct SetForm {
    device_id: String,
    entity: u8,
    channel: u8,
    /// An unchecked switch sends no value.
    #[serde(default)]
    value: bool,
}

#[derive(Deserialize)]
struct InvokeForm {
    device_id: String,
    entity: u8,
    channel: u8,
    /// The clicked button's option; missing for a `Trigger` without a value.
    value: Option<String>,
}

/// Answers with the content of `#status`: empty on success, the error otherwise.
#[route(POST "/invoke")]
pub async fn invoke(cx: &Cx, Form(form): Form<InvokeForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub
        .invoke(&form.device_id, form.entity, form.channel, form.value)
        .await;

    view! { cx =>
        if let Err(error) = result {
            <p class="text-sm text-destructive">(error.to_string())</p>
        }
    }
    .single()
    .await
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
