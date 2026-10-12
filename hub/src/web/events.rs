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
    // Ends when the hub shuts down, so the web server does not wait for open pages.
    let shutdown = Box::pin(hub.shutdown());
    let changes = stream::unfold(
        (hub.subscribe(), shutdown),
        |(mut receiver, mut shutdown)| async move {
            loop {
                let received = tokio::select! {
                    received = receiver.recv() => received,
                    () = &mut shutdown => return None,
                };
                match received {
                    // Browsers drop events with empty data.
                    Ok(change) => {
                        let name = change.event_name();
                        return Some((
                            Ok(Event::new().event(name).data(name)),
                            (receiver, shutdown),
                        ));
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => return None,
                }
            }
        },
    );
    Ok(Sse::new(changes).keep_alive(KeepAlive::new()))
}
