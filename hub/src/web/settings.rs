use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::layout::layout;
use serde::Deserialize;

use super::overview::{self, NodeParametersOverview, ParameterOverview};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::textarea::textarea;
use crate::config::FILES;
use crate::hub::Hub;
use crate::node::parameters;

#[page("/settings")]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let directory = hub.config_directory().display().to_string();
    let nodes = overview::parameters(hub);
    let editors: Vec<Editor> = FILES
        .iter()
        .map(|file| match hub.config_file(file) {
            Ok(content) => Editor {
                file: file.to_string(),
                text: content.text,
                modified: modified_field(content.modified),
                error: None,
            },
            Err(error) => Editor {
                file: file.to_string(),
                text: String::new(),
                modified: String::new(),
                error: Some(format!("{error:#}")),
            },
        })
        .collect();

    Ok(view! {
        layout(
            title: "Oxihome settings",
            <div class="max-w-3xl">
                card(
                    card_header(
                        card_title("Configuration")
                        card_description(
                            (format!("Names, rules and node parameters live in TOML files in {directory}. Edit them here, or in an editor and then reload."))
                        )
                    )
                    card_content(
                        <div class="space-y-6">
                            for editor in editors {
                                config_editor(editor: editor)
                            }
                        </div>
                        <form class="mt-6" hx-post="/reload" hx-target="#status">
                            button(variant: ButtonVariant::Outline, "Reload config from disk")
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

struct Editor {
    file: String,
    text: String,
    /// `modified_field` of the file when the text was read.
    modified: String,
    error: Option<String>,
}

/// One config file in a text box; saving replaces this form with a fresh one.
#[component]
async fn config_editor(editor: Editor) -> Result<impl View> {
    let invalid = editor.error.is_some();
    Ok(view! {
        <form class="space-y-2" hx-post="/config/save" hx-target="this" hx-swap="outerHTML">
            <div class="flex items-center justify-between">
                <code class="font-mono text-sm font-medium">(editor.file.clone())</code>
                button(variant: ButtonVariant::Secondary, size: ButtonSize::Sm, "Save")
            </div>
            textarea(
                attrs: attributes! {
                    name="text"
                    class="min-h-48 font-mono text-sm"
                    spellcheck="false"
                    if invalid {
                        aria-invalid="true"
                    }
                },
                (editor.text)
            )
            <input type="hidden" name="file" value=(editor.file)>
            <input type="hidden" name="modified" value=(editor.modified)>
            if let Some(error) = editor.error {
                <pre class="text-sm whitespace-pre-wrap text-destructive">(error)</pre>
            }
        </form>
    })
}

#[derive(Deserialize)]
struct SaveForm {
    file: String,
    text: String,
    modified: String,
}

/// Answers with the editor for the file: refreshed on success, with the error otherwise.
#[route(POST "/config/save")]
pub async fn save(cx: &Cx, Form(form): Form<SaveForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    // Browsers send text areas with CRLF line endings.
    let text = form.text.replace("\r\n", "\n");
    let result = hub.save_config_file(&form.file, &text, parse_modified(&form.modified));

    let editor = match result {
        Ok(_) => {
            for device_id in hub.device_ids() {
                tokio::spawn(parameters::apply(hub.clone(), device_id));
            }
            hub.notify(
                format!("{} saved", form.file),
                "Configuration reloaded".into(),
                "settings".into(),
            );
            let modified = hub
                .config_file(&form.file)
                .map(|content| modified_field(content.modified))
                .unwrap_or_default();
            Editor {
                file: form.file,
                text,
                modified,
                error: None,
            }
        }
        Err(error) => Editor {
            file: form.file,
            text,
            modified: form.modified,
            error: Some(format!("{error:#}")),
        },
    };

    view! { cx => config_editor(editor: editor) }.single().await
}

/// Nanoseconds since the Unix epoch, empty for a missing file.
fn modified_field(modified: Option<SystemTime>) -> String {
    modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_nanos().to_string())
        .unwrap_or_default()
}

fn parse_modified(field: &str) -> Option<SystemTime> {
    let nanos: u64 = field.parse().ok()?;
    Some(UNIX_EPOCH + Duration::from_nanos(nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_survives_the_form_field() {
        let time = UNIX_EPOCH + Duration::from_nanos(1_760_000_000_123_456_789);
        assert_eq!(parse_modified(&modified_field(Some(time))), Some(time));
        assert_eq!(parse_modified(&modified_field(None)), None);
    }
}
