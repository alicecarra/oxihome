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
                            (format!("Nodes (names and parameters), rules, remotes and panels live in TOML files in {directory}. Edit them here, or in an editor and then reload. Each node's page also edits its names and parameters."))
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
        <form id=(editor.file.clone()) class="space-y-2" hx-post="/config/save" hx-target="this" hx-swap="outerHTML">
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
