//! For Docker's healthcheck and outside monitors: no login, and nothing about the home.

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, response::IntoResponse, route},
};

use crate::hub::Hub;

/// 200 while the hub is connected to the broker, 503 otherwise.
#[route(GET "/health")]
pub async fn health(cx: &Cx) -> Result<impl IntoResponse> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(if hub.mqtt().is_connected() {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "broker disconnected")
    })
}
