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
                (child)
            </body>
        </html>
    })
}
