use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, route},
    view::{View, ViewExt, ViewHandle, component, view},
};

use crate::hub::Hub;
use crate::notifications::Notification;

/// Polls for notifications newer than `since`; each answer replaces it with a newer `since`.
#[component]
pub async fn toast_poller(since: u64) -> Result<impl View> {
    Ok(view! {
        <div
            id="toast-poller"
            hx-get=(format!("/toasts?since={since}"))
            hx-trigger="every 2s"
            hx-swap="outerHTML"
        ></div>
    })
}

#[component]
async fn toast(notification: Notification) -> Result<impl View> {
    Ok(view! {
        <div
            class="flex items-start gap-3 rounded-lg border border-border bg-card p-3 text-card-foreground shadow-sm"
            ("hx-on--load")="setTimeout(() => this.remove(), 15000)"
        >
            <div class="flex-1">
                <p class="text-sm font-semibold">(notification.title)</p>
                <p class="text-sm text-muted-foreground">(notification.message)</p>
            </div>
            <button
                type="button"
                class="text-muted-foreground hover:text-foreground"
                aria-label="Close"
                onclick="this.parentElement.remove()"
            >
                "×"
            </button>
        </div>
    })
}

#[derive(Deserialize)]
struct ToastsQuery {
    since: u64,
}

/// A new poller, plus the new toasts appended out of band to `#toasts`.
#[route(GET "/toasts")]
pub async fn toasts(cx: &Cx, Form(query): Form<ToastsQuery>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let notifications = hub.notifications_since(query.since);
    let since = notifications
        .last()
        .map_or(query.since, |notification| notification.id);

    view! { cx =>
        toast_poller(since: since)
        if !notifications.is_empty() {
            <div hx-swap-oob="beforeend:#toasts">
                for notification in notifications {
                    toast(notification: notification)
                }
            </div>
        }
    }
    .single()
    .await
}
