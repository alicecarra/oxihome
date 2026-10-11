use std::sync::Arc;

use futures_util::stream::{self, Stream};
use tokio::sync::broadcast::error::RecvError;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::sse::{Event, KeepAlive, Sse},
        route,
    },
};

use crate::hub::Hub;

/// One event per change, named after what changed; the pages fetch the new HTML themselves.
#[route(GET "/events")]
pub async fn events(cx: &Cx) -> Result<Sse<impl Stream<Item = Result<Event>> + use<>>> {
    let hub: &Arc<Hub> = app_context(cx);
    let changes = stream::unfold(hub.subscribe(), |mut receiver| async move {
        loop {
            match receiver.recv().await {
                // Browsers drop events with empty data.
                Ok(change) => {
                    let name = change.event_name();
                    return Some((Ok(Event::new().event(name).data(name)), receiver));
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(changes).keep_alive(KeepAlive::new()))
}
