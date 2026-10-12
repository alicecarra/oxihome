use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::layout::layout;
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::config::panels::Panel;
use crate::config::rules;
use crate::hub::Hub;

/// A button and the rules it runs, with whether each is enabled.
pub(super) struct PanelButton {
    panel: String,
    button: String,
    rules: Vec<(String, bool)>,
}

pub(super) fn panel_buttons(hub: &Hub, configured: &Panel) -> Vec<PanelButton> {
    configured
        .buttons
        .iter()
        .map(|name| PanelButton {
            panel: configured.name.clone(),
            button: name.clone(),
            rules: hub.rules_for_button(&configured.name, name),
        })
        .collect()
}

#[page("/panel")]
pub async fn panel(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let panels: Vec<(String, Vec<PanelButton>)> = hub
        .panels()
        .into_iter()
        .map(|panel| {
            let buttons = panel_buttons(hub, &panel);
            (panel.name, buttons)
        })
        .collect();
    Ok(view! {
        layout(
            title: "Oxihome panel",
            <div class="space-y-4">
                <div class="flex items-center justify-between gap-4">
                    <p class="text-sm text-muted-foreground">"Panels and buttons come from panels.toml; rules say what a button does with when = { panel = \"...\", button = \"...\" }."</p>
                    <a href="/settings/panels.toml" class="text-sm font-medium whitespace-nowrap hover:underline">"Edit in Settings"</a>
                </div>
                if panels.is_empty() {
                    <p class="text-muted-foreground">"No panels yet."</p>
                }
                for (name, buttons) in panels {
                    card(
                        card_header(card_title((name)))
                        card_content(
                            <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
                                for panel_button in buttons {
                                    panel_button_view(panel_button: panel_button)
                                }
                            </div>
                        )
                    )
                }
            </div>
        )
    })
}

#[component]
pub(super) async fn panel_button_view(panel_button: PanelButton) -> Result<impl View> {
    let rules: Vec<String> = panel_button
        .rules
        .iter()
        .map(|(name, enabled)| {
            if *enabled {
                name.clone()
            } else {
                format!("{name} (disabled)")
            }
        })
        .collect();
    let any_enabled = panel_button.rules.iter().any(|(_, enabled)| *enabled);
    let caption = if rules.is_empty() {
        "no rule yet".to_string()
    } else {
        rules.join(", ")
    };
    Ok(view! {
        <form class="flex flex-col gap-1" hx-post="/panel/press" hx-target="#status">
            <input type="hidden" name="panel" value=(panel_button.panel)>
            <input type="hidden" name="button" value=(panel_button.button.clone())>
            button(
                variant: ButtonVariant::Secondary,
                size: ButtonSize::Lg,
                attrs: attributes! {
                    type="submit"
                    class="h-16 w-full text-base"
                    if !any_enabled {
                        disabled=""
                    }
                },
                (panel_button.button)
            )
            <p class="truncate text-xs text-muted-foreground" title=(caption.clone())>(caption)</p>
        </form>
    })
}

#[derive(Deserialize)]
struct PressForm {
    panel: String,
    button: String,
}

/// Starts the button's rules; answers with the content of `#status`. Failed actions show as
/// toasts, like any rule.
#[route(POST "/panel/press")]
pub async fn press(cx: &Cx, Form(form): Form<PressForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let pressed = hub.pressed_rules(&form.panel, &form.button);
    let none = pressed.is_empty();
    println!(
        "panel {} button {}: {} rules",
        form.panel,
        form.button,
        pressed.len()
    );
    for rule in pressed {
        tokio::spawn(rules::run(hub.clone(), rule));
    }

    view! { cx =>
        if none {
            <p class="text-sm text-destructive">(format!("No enabled rule uses the button \"{}\"", form.button))</p>
        }
    }
    .single()
    .await
}
