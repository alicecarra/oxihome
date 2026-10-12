use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    view::{Child, View, component, view},
};

use super::toasts::toast_poller;
use crate::hub::Hub;

/// The document around every page.
#[component]
pub async fn layout(cx: &Cx, title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let since = hub.last_notification_id();
    Ok(view! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title)</title>
                <link rel="stylesheet" href="/static/app.css">
                <script src="/static/htmx.min.js"></script>
                <script src="/static/htmx-ext-sse.min.js"></script>
            </head>
            <body class="mx-auto max-w-6xl p-4 sm:p-6" hx-ext="sse" sse-connect="/events">
                <header class="mb-6 flex flex-wrap items-center justify-between gap-x-6 gap-y-2">
                    <a href="/" class="text-2xl font-bold">"Oxihome Hub"</a>
                    <nav class="flex flex-wrap gap-x-4 gap-y-1 text-sm text-muted-foreground">
                        <a href="/nodes" class="hover:text-foreground hover:underline">"Nodes"</a>
                        <a href="/panel" class="hover:text-foreground hover:underline">"Panel"</a>
                        <a href="/rules" class="hover:text-foreground hover:underline">"Rules"</a>
                        <a href="/remotes" class="hover:text-foreground hover:underline">"Remotes"</a>
                        <a href="/settings" class="hover:text-foreground hover:underline">"Settings"</a>
                    </nav>
                </header>
                <div id="status" class="mb-4"></div>
                (child)
                <div id="toasts" class="fixed right-4 bottom-4 z-50 flex w-80 flex-col gap-2"></div>
                toast_poller(since: since)
            </body>
        </html>
    })
}
