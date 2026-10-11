use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, route},
    view::{View, ViewExt, ViewHandle, view},
};

use super::layout::layout;
use crate::components::button::{ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::hub::Hub;

#[page("/settings")]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let directory = hub.config_directory().display().to_string();

    Ok(view! {
        layout(
            title: "Oxihome settings",
            <div class="max-w-3xl">
                card(
                    card_header(
                        card_title("Configuration")
                        card_description(
                            "Names and rules live in TOML files. Edit them, then reload."
                        )
                    )
                    card_content(
                        <ul class="mb-4 space-y-1 text-sm">
                            <li><code class="font-mono">(format!("{directory}/names.toml"))</code></li>
                            <li><code class="font-mono">(format!("{directory}/rules.toml"))</code></li>
                        </ul>
                        <form hx-post="/reload" hx-target="#status">
                            button(variant: ButtonVariant::Primary, "Reload config")
                        </form>
                    )
                )
            </div>
        )
    })
}

/// Answers with the content of `#status`; a success also shows a toast.
#[route(POST "/reload")]
pub async fn reload(cx: &Cx) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.reload_config();
    if let Ok(rule_count) = &result {
        hub.notify(
            "Config reloaded".into(),
            format!("{rule_count} rules"),
            "settings".into(),
        );
    }

    view! { cx =>
        if let Err(error) = result {
            <pre class="text-sm whitespace-pre-wrap text-destructive">(format!("{error:#}"))</pre>
        }
    }
    .single()
    .await
}
