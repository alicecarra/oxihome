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
                <title>(title)</title>
                <link rel="stylesheet" href="/static/app.css">
                <script src="/static/htmx.min.js"></script>
            </head>
            <body class="mx-auto max-w-6xl p-6">
                <header class="mb-6 flex items-center justify-between">
                    <a href="/" class="text-2xl font-bold">"Oxihome Hub"</a>
                    <nav class="flex gap-4 text-sm text-muted-foreground">
                        <a href="/" class="hover:text-foreground hover:underline">"Nodes"</a>
                        <a href="/rules" class="hover:text-foreground hover:underline">"Rules"</a>
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
