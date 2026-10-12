use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::format::ago_label;
use super::layout::layout;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::config::nodes::NameTarget;
use crate::config::remotes::RemoteKey;
use crate::hub::Hub;

#[page("/remotes")]
pub async fn remotes_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! {
        layout(
            title: "Oxihome remotes",
            <div class="max-w-3xl space-y-4">
                <div class="flex items-center justify-between gap-4">
                    <p class="text-sm text-muted-foreground">"Infrared remotes and the codes of their keys. Rules can use a key by name. They come from remotes.toml."</p>
                    <a href="/settings/remotes.toml" class="text-sm font-medium whitespace-nowrap hover:underline">"Edit in Settings"</a>
                </div>
                <div id="remotes" hx-get="/remotes/fragment" hx-trigger="sse:nodes, refresh">
                    remotes_content(learning: learning(hub), remotes: remote_list(hub))
                </div>
            </div>
        )
    })
}

#[page("/remotes/fragment")]
pub async fn remotes_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(
        view! { remotes_content(learning: learning(hub), remotes: remote_list(hub)) },
    )
}

struct LearningCode {
    code: String,
    receiver: String,
    ago: String,
    known: Option<RemoteKey>,
    /// The remote with keys of the same protocol and address.
    suggested_remote: Option<String>,
}

struct RemoteOverview {
    name: String,
    /// Key name and code.
    keys: Vec<(String, String)>,
}

fn learning(hub: &Hub) -> Vec<LearningCode> {
    let recent = hub.recent_codes();
    let snapshot = hub.snapshot();
    let now = Instant::now();
    recent
        .into_iter()
        .map(|received| LearningCode {
            receiver: snapshot
                .nodes
                .name(&NameTarget::Node(received.device_id.clone()))
                .map_or(received.device_id.clone(), String::from),
            ago: ago_label(now - received.at),
            known: snapshot.remotes.key_for(&received.code),
            suggested_remote: snapshot.remotes.remote_for(&received.code).map(String::from),
            code: received.code,
        })
        .collect()
}

fn remote_list(hub: &Hub) -> Vec<RemoteOverview> {
    hub.snapshot()
        .remotes
        .all()
        .iter()
        .map(|(name, remote)| RemoteOverview {
            name: name.clone(),
            keys: remote
                .keys
                .iter()
                .map(|(key, code)| (key.clone(), code.clone()))
                .collect(),
        })
        .collect()
}

#[component]
async fn remotes_content(
    learning: Vec<LearningCode>,
    remotes: Vec<RemoteOverview>,
) -> Result<impl View> {
    let remote_names: Vec<String> = remotes.iter().map(|remote| remote.name.clone()).collect();
    Ok(view! {
        card(
            card_header(
                card_title("Learn")
                card_description("Press a key on a remote: its code shows here and you can name it. Held keys and undecoded signals are left out.")
            )
            card_content(
                if learning.is_empty() {
                    <p class="text-sm text-muted-foreground">"No codes received yet."</p>
                }
                <ul class="divide-y divide-border">
                    for received in learning {
                        learning_row(received: received)
                    }
                </ul>
                <datalist id="remote-names">
                    for name in remote_names.clone() {
                        <option value=(name)></option>
                    }
                </datalist>
            )
        )
        for remote in remotes {
            card(
                card_header(card_title((remote.name)))
                card_content(
                    <dl class="grid grid-cols-[8rem_1fr] gap-x-3 gap-y-2 text-sm">
                        for (key, code) in remote.keys {
                            <dt class="font-medium">(key)</dt>
                            <dd><code class="font-mono text-muted-foreground">(code)</code></dd>
                        }
                    </dl>
                )
            )
        }
    })
}

#[component]
async fn learning_row(received: LearningCode) -> Result<impl View> {
    // Inputs keep what was typed when the list refreshes on new codes. The suggestion is a
    // placeholder, not a value, and is part of the id: a new suggestion gives a fresh input.
    let code_id: String = received
        .code
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let suggestion = received.suggested_remote.clone().unwrap_or_default();
    let remote_id = format!(
        "learn-remote-{code_id}-{}",
        suggestion
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
    );
    let key_id = format!("learn-key-{code_id}");
    Ok(view! {
        <li class="flex flex-wrap items-center justify-between gap-3 py-3">
            <div>
                <code class="font-mono text-sm font-medium">(received.code.clone())</code>
                <p class="text-sm text-muted-foreground">(format!("{} · {}", received.receiver, received.ago))</p>
            </div>
            match received.known {
                Some(known) => badge(variant: BadgeVariant::Success, (format!("{} · {}", known.remote, known.key))),
                None => <form
                    class="flex flex-wrap items-center gap-2"
                    hx-post="/remotes/keys"
                    hx-target="#status"
                    ("hx-on--after-request")="htmx.trigger('#remotes', 'refresh')"
                >
                    <input type="hidden" name="code" value=(received.code.clone())>
                    <input type="hidden" name="suggested_remote" value=(suggestion.clone())>
                    input(attrs: attributes! {
                        id=(remote_id)
                        hx-preserve="true"
                        name="remote"
                        list="remote-names"
                        if suggestion.is_empty() {
                            required=""
                        }
                        placeholder=(if suggestion.is_empty() { "Remote".to_string() } else { suggestion.clone() })
                        aria-label="Remote"
                        class="h-8 w-36"
                    })
                    input(attrs: attributes! {
                        id=(key_id)
                        hx-preserve="true"
                        name="key"
                        required=""
                        placeholder="Key"
                        aria-label="Key"
                        class="h-8 w-24"
                    })
                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save")
                </form>,
            }
        </li>
    })
}

#[derive(Deserialize)]
struct KeyForm {
    remote: String,
    /// Used when `remote` is left empty.
    #[serde(default)]
    suggested_remote: String,
    key: String,
    code: String,
}

/// Answers with the content of `#status`: empty on success, the error otherwise.
#[route(POST "/remotes/keys")]
pub async fn learn_key(cx: &Cx, Form(form): Form<KeyForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let remote = match form.remote.trim() {
        "" => form.suggested_remote.trim(),
        typed => typed,
    };
    let key = form.key.trim();
    let result = if remote.is_empty() || key.is_empty() {
        Err(anyhow::anyhow!("the remote and the key need a name"))
    } else {
        hub.learn_key(remote, key, &form.code)
    };

    view! { cx =>
        if let Err(error) = result {
            <pre class="text-sm whitespace-pre-wrap text-destructive">(format!("{error:#}"))</pre>
        }
    }
    .single()
    .await
}
