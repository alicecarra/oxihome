use std::sync::Arc;

use jiff::Timestamp;
use oxihome_protocol::hardware::Driver;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::format::{ago_label, elapsed, entity_label};
use super::icons;
use super::layout::layout;
use super::overview;
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::components::switch::switch;
use crate::config::nodes::NameTarget;
use crate::config::remotes::RemoteKey;
use crate::hub::{CommandOrigin, Hub};

#[page("/remotes")]
pub async fn remotes_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let transmitters = transmitters(hub);
    let can_send = !transmitters.is_empty();
    Ok(view! {
        layout(
            title: "Oxihome remotes",
            // `remote-codes.js` sets `data-codes` from the switch, which the code cells follow.
            <div id="remotes-page" class="group space-y-4" data-codes="hidden">
                <div class="flex flex-wrap items-center justify-between gap-4">
                    <p class="text-sm text-muted-foreground">"Infrared remotes and the codes of their keys. Rules can use a key by name. They come from remotes.toml."</p>
                    <div class="flex flex-wrap items-center gap-4">
                        <label class="flex items-center gap-2 text-sm">
                            switch(attrs: attributes! { id="show-codes" })
                            "Codes"
                        </label>
                        if transmitters.is_empty() {
                            <p class="text-sm text-muted-foreground">"No IR transmitter to send with"</p>
                        } else {
                            // Outside #remotes, so the choice survives its refreshes.
                            <select id="transmitter" name="transmitter" class=(SELECT) aria-label="IR transmitter">
                                for transmitter in transmitters {
                                    <option value=(transmitter.value)>(transmitter.label)</option>
                                }
                            </select>
                        }
                        <a href="/settings/remotes.toml" class="text-sm font-medium whitespace-nowrap hover:underline">"Edit in Settings"</a>
                    </div>
                </div>
                <div id="remotes" hx-get="/remotes/fragment" hx-trigger="sse:nodes, refresh">
                    remotes_content(learning: learning(hub), remotes: remote_list(hub), can_send: can_send)
                </div>
            </div>
            <script src="/static/remote-codes.js"></script>
        )
    })
}

#[page("/remotes/fragment")]
pub async fn remotes_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let can_send = !transmitters(hub).is_empty();
    Ok(
        view! { remotes_content(learning: learning(hub), remotes: remote_list(hub), can_send: can_send) },
    )
}

const SELECT: &str = "h-9 rounded-lg border border-border bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring";

struct Transmitter {
    /// `<device id>/<entity>`.
    value: String,
    label: String,
}

/// The IR transmitters in the hardware of `nodes.toml`.
fn transmitters(hub: &Hub) -> Vec<Transmitter> {
    let snapshot = hub.snapshot();
    let mut transmitters = Vec::new();
    for device_id in snapshot.nodes.devices_id() {
        let Some(hardware) = snapshot.nodes.hardware(device_id) else {
            continue;
        };
        let node = snapshot
            .nodes
            .name(&NameTarget::Node(device_id.to_string()))
            .unwrap_or(device_id);
        let online = snapshot
            .registry
            .node(device_id)
            .is_some_and(|node| node.online);
        for entity in hardware
            .entities
            .iter()
            .filter(|entity| entity.driver == Driver::IrTransmitter)
        {
            let entity_name = entity_label(
                entity.id,
                &overview::name(
                    &snapshot.nodes,
                    &snapshot.registry,
                    &NameTarget::Entity(device_id.to_string(), entity.id),
                ),
            );
            transmitters.push(Transmitter {
                value: format!("{device_id}/{}", entity.id),
                label: format!(
                    "{node} · {entity_name}{}",
                    if online { "" } else { " (offline)" }
                ),
            });
        }
    }
    transmitters
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
    let now = Timestamp::now();
    recent
        .into_iter()
        .map(|received| LearningCode {
            receiver: snapshot
                .nodes
                .name(&NameTarget::Node(received.device_id.clone()))
                .map_or(received.device_id.clone(), String::from),
            ago: ago_label(elapsed(received.at, now)),
            known: snapshot.remotes.key_for(&received.code),
            suggested_remote: snapshot
                .remotes
                .remote_for(&received.code)
                .map(String::from),
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
    can_send: bool,
) -> Result<impl View> {
    let remote_names: Vec<String> = remotes.iter().map(|remote| remote.name.clone()).collect();
    Ok(view! {
        <div class="grid gap-4 md:grid-cols-3">
            for remote in remotes {
                card(
                    attrs: attributes! { class="h-full" },
                    card_header(card_title((remote.name.clone())))
                    card_content(
                        <dl class="grid grid-cols-[5rem_1fr_auto_auto] items-center gap-x-3 gap-y-2 text-sm group-data-[codes=hidden]:grid-cols-[1fr_auto_auto]">
                            for (key, code) in remote.keys {
                                <dt class="font-medium">(key.clone())</dt>
                                <dd class="group-data-[codes=hidden]:hidden"><code class="font-mono text-muted-foreground">(code.clone())</code></dd>
                                <dd>send_button(code: code, can_send: can_send)</dd>
                                <dd>remove_button(remote: remote.name.clone(), key: key)</dd>
                            }
                        </dl>
                    )
                )
            }
        </div>
        <div class="mt-4">
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
                            learning_row(received: received, can_send: can_send)
                        }
                    </ul>
                    <datalist id="remote-names">
                        for name in remote_names.clone() {
                            <option value=(name)></option>
                        }
                    </datalist>
                )
            )
        </div>
    })
}

#[component]
async fn learning_row(received: LearningCode, can_send: bool) -> Result<impl View> {
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
            <div class="flex flex-wrap items-center gap-2">
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
            send_button(code: received.code.clone(), can_send: can_send)
            </div>
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

#[component]
async fn remove_button(remote: String, key: String) -> Result<impl View> {
    Ok(view! {
        <form
            hx-post="/remotes/keys/remove"
            hx-target="#status"
            hx-confirm=(format!("Remove {key} from {remote}?"))
            ("hx-on--after-request")="htmx.trigger('#remotes', 'refresh')"
        >
            <input type="hidden" name="remote" value=(remote.clone())>
            <input type="hidden" name="key" value=(key.clone())>
            button(
                variant: ButtonVariant::Ghost,
                size: ButtonSize::Icon,
                attrs: attributes! {
                    type="submit"
                    title="Remove"
                    aria-label=(format!("Remove {key}"))
                    class="size-8 text-muted-foreground hover:text-destructive"
                },
                icon(data: icons::REMOVE, size: 16)
            )
        </form>
    })
}

#[derive(Deserialize)]
struct RemoveKeyForm {
    remote: String,
    key: String,
}

/// Answers with the content of `#status`: empty on success, the error otherwise.
#[route(POST "/remotes/keys/remove")]
pub async fn forget_key(cx: &Cx, Form(form): Form<RemoveKeyForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.forget_key(&form.remote, &form.key);

    view! { cx =>
        if let Err(error) = result {
            <pre class="text-sm whitespace-pre-wrap text-destructive">(format!("{error:#}"))</pre>
        }
    }
    .single()
    .await
}

/// Sends `code` with the transmitter picked at the top of the page.
#[component]
async fn send_button(code: String, can_send: bool) -> Result<impl View> {
    Ok(view! {
        <form hx-post="/remotes/send" hx-target="#status" hx-include="#transmitter">
            <input type="hidden" name="code" value=(code)>
            button(
                variant: ButtonVariant::Outline,
                size: ButtonSize::Sm,
                attrs: attributes! {
                    type="submit"
                    if !can_send {
                        disabled=""
                    }
                },
                "Send"
            )
        </form>
    })
}

#[derive(Deserialize)]
struct SendForm {
    /// `<device id>/<entity>`.
    transmitter: String,
    code: String,
}

/// Answers with the content of `#status`: empty on success, the error otherwise.
#[route(POST "/remotes/send")]
pub async fn send_code(cx: &Cx, Form(form): Form<SendForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = match form
        .transmitter
        .rsplit_once('/')
        .and_then(|(device_id, entity)| Some((device_id, entity.parse::<u8>().ok()?)))
    {
        Some((device_id, entity)) => {
            hub.invoke(device_id, entity, 0, Some(form.code), CommandOrigin::Web)
                .await
        }
        None => Err(anyhow::anyhow!("pick an IR transmitter")),
    };

    view! { cx =>
        if let Err(error) = result {
            <p class="text-sm text-destructive">(format!("{error:#}"))</p>
        }
    }
    .single()
    .await
}
