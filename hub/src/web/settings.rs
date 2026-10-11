use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, route},
    view::{View, ViewExt, ViewHandle, component, view},
};

use super::layout::layout;
use crate::components::button::{ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::hub::{Hub, NodeParametersOverview, ParameterOverview};
use crate::parameters;

#[page("/settings")]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let directory = hub.config_directory().display().to_string();
    let nodes = hub.parameters_overview();

    Ok(view! {
        layout(
            title: "Oxihome settings",
            <div class="max-w-3xl">
                card(
                    card_header(
                        card_title("Configuration")
                        card_description(
                            "Names, rules and node parameters live in TOML files. Edit them, then reload."
                        )
                    )
                    card_content(
                        <ul class="mb-4 space-y-1 text-sm">
                            <li><code class="font-mono">(format!("{directory}/names.toml"))</code></li>
                            <li><code class="font-mono">(format!("{directory}/rules.toml"))</code></li>
                            <li><code class="font-mono">(format!("{directory}/nodes.toml"))</code></li>
                        </ul>
                        <form hx-post="/reload" hx-target="#status">
                            button(variant: ButtonVariant::Primary, "Reload config")
                        </form>
                    )
                )
                for node in nodes {
                    parameters_card(node: node)
                }
            </div>
        )
    })
}

/// Read-only: the desired values come from `nodes.toml`.
#[component]
async fn parameters_card(node: NodeParametersOverview) -> Result<impl View> {
    let title = node.name.clone().unwrap_or_else(|| node.device_id.clone());
    Ok(view! {
        <div class="mt-6">
            card(
                card_header(
                    card_title((title))
                    card_description((format!("{} · parameters", node.device_id)))
                )
                card_content(
                    <table class="w-full text-sm">
                        <thead class="text-left text-muted-foreground">
                            <tr>
                                <th class="pb-2 font-medium">"Parameter"</th>
                                <th class="pb-2 font-medium">"Node"</th>
                                <th class="pb-2 font-medium">"nodes.toml"</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-border">
                            for row in node.parameters {
                                parameter_row(row: row)
                            }
                        </tbody>
                    </table>
                )
            )
        </div>
    })
}

#[component]
async fn parameter_row(row: ParameterOverview) -> Result<impl View> {
    let pending = row.desired.is_some() && row.desired != row.current;
    let current_class = if pending {
        "py-2 text-destructive"
    } else {
        "py-2"
    };
    Ok(view! {
        <tr>
            <td class="py-2"><code class="font-mono">(parameters::key(row.parameter))</code></td>
            <td class=(current_class)>(seconds_label(row.current))</td>
            <td class="py-2 text-muted-foreground">(seconds_label(row.desired))</td>
        </tr>
    })
}

fn seconds_label(value: Option<i32>) -> String {
    value.map_or_else(|| "—".into(), |seconds| format!("{seconds} s"))
}

/// Answers with the content of `#status`; a success also shows a toast.
#[route(POST "/reload")]
pub async fn reload(cx: &Cx) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.reload_config();
    if let Ok(rule_count) = &result {
        for device_id in hub.device_ids() {
            tokio::spawn(parameters::apply(hub.clone(), device_id));
        }
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
