use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jiff::Timestamp;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{content::Form, page, path_param, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::auth::Auth;
use super::icons;
use super::layout::layout;
use serde::Deserialize;

use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::textarea::textarea;
use crate::config::FILES;
use crate::hub::Hub;
use crate::node::{hardware, parameters};

path_param!(file);

#[page("/settings")]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let directory = hub.config_directory().display().to_string();
    let login_enabled = app_context::<Auth>(cx).enabled();
    let files: Vec<(&str, String)> = FILES
        .iter()
        .map(|file| {
            let modified = match hub.config_file(file) {
                Ok(content) => content.modified.map_or_else(
                    || "missing, counts as empty".to_string(),
                    |modified| format!("modified {}", time_label(hub, modified)),
                ),
                Err(error) => format!("{error:#}"),
            };
            (*file, modified)
        })
        .collect();

    Ok(view! {
        layout(
            title: "Oxihome settings",
            <div class="max-w-3xl space-y-4">
                card(
                    card_header(
                        <div class="flex flex-wrap items-center justify-between gap-3">
                            card_title("Configuration")
                            <form hx-post="/reload" hx-target="#status">
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, "Reload all")
                            </form>
                        </div>
                        card_description(
                            (format!("Nodes (names, parameters and hardware), rules, remotes and the dashboard live in TOML files in {directory}. Edit them here, or in an editor and then reload. Each node's page also edits its names and parameters."))
                        )
                    )
                    card_content(
                        <ul class="divide-y divide-border">
                            for (file, modified) in files {
                                <li>
                                    <a href=(format!("/settings/{file}")) class="flex items-center justify-between gap-4 py-3 hover:bg-foreground/5">
                                        <div>
                                            <p class="font-mono text-sm font-medium">(file)</p>
                                            <p class="text-sm text-muted-foreground">(file_description(file))</p>
                                        </div>
                                        <p class="text-right text-xs text-muted-foreground">(modified)</p>
                                    </a>
                                </li>
                            }
                        </ul>
                    )
                )
                card(
                    card_header(
                        card_title("Tools")
                        card_description("Pages left out of the menu.")
                    )
                    card_content(
                        <ul class="divide-y divide-border">
                            for (href, name, description) in TOOLS {
                                <li>
                                    <a href=(href) class="block py-3 hover:bg-foreground/5">
                                        <p class="text-sm font-medium">(name)</p>
                                        <p class="text-sm text-muted-foreground">(description)</p>
                                    </a>
                                </li>
                            }
                        </ul>
                    )
                )
                if login_enabled {
                    card(
                        card_header(
                            <div class="flex flex-wrap items-center justify-between gap-3">
                                <div class="flex flex-col gap-1.5">
                                    card_title("Session")
                                    card_description("Signed in to this browser with the hub password.")
                                </div>
                                <form method="post" action="/logout">
                                    button(
                                        variant: ButtonVariant::Outline,
                                        size: ButtonSize::Sm,
                                        attrs: attributes! { type="submit" },
                                        icon(data: icons::SIGN_OUT, size: 16)
                                        "Sign out"
                                    )
                                </form>
                            </div>
                        )
                    )
                }
            </div>
        )
    })
}

const TOOLS: [(&str, &str, &str); 2] = [
    (
        "/traffic",
        "Traffic",
        "MQTT messages the hub receives and sends, live.",
    ),
    ("/lab", "Lab", "Components and styles being tried out."),
];

#[page("/settings/{file}")]
pub async fn settings_file(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let file = path_param::<File>(cx).to_string();
    let editor = FILES
        .contains(&file.as_str())
        .then(|| match hub.config_file(&file) {
            Ok(content) => Editor {
                file: file.clone(),
                text: content.text,
                modified: modified_field(content.modified),
                error: None,
                unsaved: false,
            },
            Err(error) => Editor {
                file: file.clone(),
                text: String::new(),
                modified: String::new(),
                error: Some(format!("{error:#}")),
                unsaved: false,
            },
        });

    Ok(view! {
        layout(
            title: "Oxihome settings",
            <div class="max-w-3xl space-y-4">
                <a href="/settings" class="text-sm text-muted-foreground hover:underline">"← Settings"</a>
                match editor {
                    None => <p class="text-muted-foreground">(format!("Unknown config file {file}."))</p>,
                    Some(editor) => card(
                        card_header(
                            card_title(<span class="font-mono">(file.clone())</span>)
                            card_description((file_description(&file)))
                        )
                        card_content(config_editor(editor: editor))
                    ),
                }
            </div>
            <script src="/static/config-editor.js"></script>
        )
    })
}

fn file_description(file: &str) -> &'static str {
    match file {
        "nodes.toml" => {
            "Names of nodes, entities and channels, and the operation parameters and hardware the hub sets."
        }
        "rules.toml" => "What happens on events, values, schedules, remote keys and buttons.",
        "remotes.toml" => "Infrared remotes and the names of their keys.",
        "dashboard.toml" => "The blocks of the home page.",
        _ => "",
    }
}

/// Like `2026-10-12 14:30`, in the hub's time zone.
fn time_label(hub: &Hub, time: SystemTime) -> String {
    Timestamp::try_from(time).map_or_else(
        |_| "unknown".to_string(),
        |timestamp| {
            timestamp
                .to_zoned(hub.time_zone().clone())
                .strftime("%Y-%m-%d %H:%M")
                .to_string()
        },
    )
}

/// Answers with the content of `#status`; a success also shows a toast.
#[route(POST "/reload")]
pub async fn reload(cx: &Cx) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.reload_config();
    if let Ok(rule_count) = &result {
        for device_id in hub.device_ids() {
            tokio::spawn(parameters::apply(hub.clone(), device_id.clone()));
            tokio::spawn(hardware::apply(hub.clone(), device_id));
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
    /// The text differs from the file because saving it failed.
    unsaved: bool,
}

/// One config file in a text box; saving replaces this form with a fresh one.
/// `config-editor.js` adds the keyboard shortcuts and the warning about unsaved changes.
#[component]
async fn config_editor(editor: Editor) -> Result<impl View> {
    let invalid = editor.error.is_some();
    Ok(view! {
        <form
            id="config-editor"
            class="space-y-2"
            hx-post="/config/save"
            hx-target="this"
            hx-swap="outerHTML"
            data-unsaved=(if editor.unsaved { "true" } else { "false" })
        >
            <div class="flex items-center justify-end gap-3">
                <p class="hidden text-xs text-muted-foreground sm:block">"Ctrl+S saves · Tab indents · Esc then Tab leaves the box"</p>
                button(variant: ButtonVariant::Secondary, size: ButtonSize::Sm, "Save")
            </div>
            textarea(
                attrs: attributes! {
                    id="config-text"
                    name="text"
                    class="min-h-96 font-mono text-sm"
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
        Ok(rule_count) => {
            for device_id in hub.device_ids() {
                tokio::spawn(parameters::apply(hub.clone(), device_id.clone()));
                tokio::spawn(hardware::apply(hub.clone(), device_id));
            }
            hub.notify(
                format!("{} saved", form.file),
                format!("All config reloaded ({rule_count} rules)"),
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
                unsaved: false,
            }
        }
        Err(error) => Editor {
            file: form.file,
            text,
            modified: form.modified,
            error: Some(format!("{error:#}")),
            unsaved: true,
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
