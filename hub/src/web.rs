use std::sync::Arc;

use oxihome_protocol::Value;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Router,
        content::Form,
        error::{SeeOther, see_other},
        page, route,
    },
    view::{View, view},
};

use crate::hub::Hub;

pub fn router(hub: Arc<Hub>) -> Router {
    Router::builder()
        .page(home)
        .route(set)
        .app_context(hub)
        .build()
}

#[page("/")]
async fn home(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let nodes = hub.overview();
    let error = hub.take_last_error();

    Ok(view! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">
                <title>"Oxihome"</title>
            </head>
            <body>
                <h1>"Oxihome Hub"</h1>
                if let Some(error) = error {
                    <p><strong>"Error: "</strong>(error)</p>
                }
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
                                    <form method="post" action="/set">
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
            </body>
        </html>
    })
}

#[derive(Deserialize)]
struct SetForm {
    device_id: String,
    entity: u8,
    channel: u8,
    value: bool,
}

#[route(POST "/set")]
async fn set(cx: &Cx, Form(form): Form<SetForm>) -> Result<SeeOther> {
    let hub: &Arc<Hub> = app_context(cx);
    if let Err(error) = hub
        .set(
            &form.device_id,
            form.entity,
            form.channel,
            Value::Bool(form.value),
        )
        .await
    {
        hub.set_last_error(error.to_string());
    }
    Ok(see_other("/"))
}
