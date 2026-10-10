use topcoat::{
    Result,
    view::{Child, View, component, view},
};

/// The document around every page.
#[component]
pub async fn layout(title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">
                <title>(title)</title>
                <link rel="stylesheet" href="/static/app.css">
                <script src="/static/htmx.min.js"></script>
            </head>
            <body class="mx-auto max-w-3xl p-6">
                <header class="mb-6 flex items-center justify-between">
                    <a href="/" class="text-2xl font-bold">"Oxihome Hub"</a>
                    <nav class="flex gap-4 text-sm text-muted-foreground">
                        <a href="/" class="hover:text-foreground hover:underline">"Nodes"</a>
                        <a href="/settings" class="hover:text-foreground hover:underline">"Settings"</a>
                    </nav>
                </header>
                <div id="status" class="mb-4"></div>
                (child)
            </body>
        </html>
    })
}
