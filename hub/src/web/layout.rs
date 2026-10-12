use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::{IconData, icon},
    router::request::uri,
    view::{Child, View, component, view},
};

use super::icons;
use super::toasts::toast_poller;
use crate::hub::Hub;

pub(super) const MENU: [(&str, &str, IconData); 6] = [
    ("/", "Dashboard", icons::DASHBOARD),
    ("/nodes", "Nodes", icons::NODES),
    ("/panel", "Panels", icons::PANELS),
    ("/rules", "Rules", icons::RULES),
    ("/remotes", "Remotes", icons::REMOTES),
    ("/settings", "Settings", icons::SETTINGS),
];

/// The document around every page.
#[component]
pub async fn layout(cx: &Cx, title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let since = hub.last_notification_id();
    let path = uri(cx).path().to_string();
    // A section's own pages, like `/nodes/<id>`, light up its item too.
    let current =
        |href: &str| path == href || (href != "/" && path.starts_with(&format!("{href}/")));
    let menu: Vec<(&str, &str, IconData, bool)> = MENU
        .into_iter()
        .map(|(href, label, data)| (href, label, data, current(href)))
        .collect();
    Ok(view! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title)</title>
                <link rel="icon" type="image/svg+xml" href=(icons::FAVICON)>
                <link rel="stylesheet" href="/static/app.css">
                <script src="/static/htmx.min.js"></script>
                <script src="/static/htmx-ext-sse.min.js"></script>
            </head>
            <body hx-ext="sse" sse-connect="/events">
                <header class="sticky top-0 z-40 border-b border-border bg-card/95 backdrop-blur">
                    <div class="mx-auto flex max-w-6xl flex-col gap-2 px-4 py-3 sm:flex-row sm:items-center sm:justify-between sm:px-6">
                        <a href="/" class="flex items-center gap-2 text-lg font-semibold">
                            <span class="text-primary">icon(data: icons::LOGO, size: 28)</span>
                            "Oxihome"
                        </a>
                        <nav class="-mx-1 flex gap-2 overflow-x-auto text-xs">
                            for (href, label, data, active) in menu {
                                <a
                                    href=(href)
                                    class=(if active {
                                        "flex shrink-0 flex-col items-center gap-1 border-b-2 border-foreground px-2 py-1 font-medium text-foreground"
                                    } else {
                                        "flex shrink-0 flex-col items-center gap-1 border-b-2 border-transparent px-2 py-1 text-muted-foreground hover:border-border hover:text-foreground"
                                    })
                                >
                                    icon(data: data, size: 18)
                                    (label)
                                </a>
                            }
                        </nav>
                    </div>
                </header>
                <main class="mx-auto max-w-6xl p-4 sm:p-6">
                    <div id="status" class="mb-4"></div>
                    (child)
                </main>
                <div id="toasts" class="fixed right-4 bottom-4 z-50 flex w-80 flex-col gap-2"></div>
                toast_poller(since: since)
            </body>
        </html>
    })
}
