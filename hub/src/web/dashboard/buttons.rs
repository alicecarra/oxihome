use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::config::rules;
use crate::hub::Hub;

/// A button and the rule it runs.
pub(super) struct RuleButton {
    pub button: String,
    pub rule: String,
    pub enabled: bool,
}

/// Validation makes sure each button has a rule.
pub(super) fn rule_buttons(hub: &Hub, buttons: &[String]) -> Vec<RuleButton> {
    buttons
        .iter()
        .filter_map(|name| {
            let rule = hub.button_rule(name)?;
            Some(RuleButton {
                button: name.clone(),
                rule: rule.name,
                enabled: rule.enabled,
            })
        })
        .collect()
}

#[component]
pub(super) async fn buttons_block(buttons: Vec<RuleButton>) -> Result<impl View> {
    Ok(view! {
        // In one row while they fit, sharing the width.
        <div class="flex flex-wrap gap-3">
            for rule_button in buttons {
                <div class="min-w-32 flex-1">
                    rule_button_view(rule_button: rule_button)
                </div>
            }
        </div>
    })
}

#[component]
async fn rule_button_view(rule_button: RuleButton) -> Result<impl View> {
    let tooltip = if rule_button.enabled {
        rule_button.rule
    } else {
        format!("{} (disabled)", rule_button.rule)
    };
    Ok(view! {
        <form hx-post="/buttons/press" hx-target="#status">
            <input type="hidden" name="button" value=(rule_button.button.clone())>
            button(
                variant: ButtonVariant::Secondary,
                size: ButtonSize::Lg,
                attrs: attributes! {
                    type="submit"
                    class="h-16 w-full text-base"
                    title=(tooltip)
                    if !rule_button.enabled {
                        disabled=""
                    }
                },
                (rule_button.button)
            )
        </form>
    })
}

#[derive(Deserialize)]
struct PressForm {
    button: String,
}

/// Starts the button's rule; answers with the content of `#status`. Failed actions show as
/// toasts, like any rule.
#[route(POST "/buttons/press")]
pub async fn press(cx: &Cx, Form(form): Form<PressForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let rule = hub.button_rule(&form.button).filter(|rule| rule.enabled);
    let none = rule.is_none();
    if let Some(rule) = rule {
        println!("button {}: rule {}", form.button, rule.name);
        rules::start(hub.clone(), rule, rules::Subject::default());
    }

    view! { cx =>
        if none {
            <p class="text-sm text-destructive">(format!("No enabled rule has the button \"{}\"", form.button))</p>
        }
    }
    .single()
    .await
}
