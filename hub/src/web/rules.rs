use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::format::{ago_label, rule_anchor};
use super::layout::layout;
use super::overview::{self, RuleOverview};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::components::switch::switch;
use crate::hub::Hub;

#[page("/rules")]
pub async fn rules(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let rules = overview::rules(hub);
    Ok(view! {
        layout(
            title: "Oxihome rules",
            <div class="max-w-3xl space-y-4">
                <div class="flex items-center justify-between gap-4">
                    <p class="text-sm text-muted-foreground">"Rules run when their event happens or at their time. They come from rules.toml."</p>
                    <a href="/settings/rules.toml" class="text-sm font-medium hover:underline">"Edit in Settings"</a>
                </div>
                if rules.is_empty() {
                    <p class="text-muted-foreground">"No rules yet."</p>
                }
                for rule in rules {
                    rule_card(rule: rule)
                }
            </div>
        )
    })
}

#[component]
async fn rule_card(rule: RuleOverview) -> Result<impl View> {
    let (variant, status) = if !rule.enabled {
        (BadgeVariant::Muted, "disabled".to_string())
    } else {
        match &rule.last_run {
            None => (BadgeVariant::Muted, "never ran".to_string()),
            Some((ago, errors)) if errors.is_empty() => {
                (BadgeVariant::Success, format!("ok · {}", ago_label(*ago)))
            }
            Some((ago, _)) => (
                BadgeVariant::Destructive,
                format!("failed · {}", ago_label(*ago)),
            ),
        }
    };
    let errors = rule.last_run.map(|(_, errors)| errors).unwrap_or_default();
    Ok(view! {
        card(
            attrs: attributes! {
                id=(rule_anchor(&rule.name))
                // Highlighted when a link from a node's page points at it.
                class=(if rule.enabled { "scroll-mt-4 target:ring-2 target:ring-ring" } else { "scroll-mt-4 opacity-60 target:ring-2 target:ring-ring" })
            },
            card_header(
                <div class="flex items-center justify-between gap-2">
                    card_title((rule.name.clone()))
                    <div class="flex items-center gap-3">
                        badge(variant: variant, (status))
                        // Answered with this card, replaced in place.
                        <form
                            hx-post="/rules/enabled"
                            hx-trigger="change"
                            hx-target=(format!("#{}", rule_anchor(&rule.name)))
                            hx-swap="outerHTML"
                        >
                            <input type="hidden" name="name" value=(rule.name.clone())>
                            switch(attrs: attributes! {
                                name="enabled"
                                value="true"
                                aria-label=(format!("{} enabled", rule.name))
                                if rule.enabled {
                                    checked=""
                                }
                            })
                        </form>
                    </div>
                </div>
            )
            card_content(
                <dl class="grid grid-cols-[3rem_1fr] gap-x-3 gap-y-2 text-sm">
                    <dt class="text-muted-foreground">"When"</dt>
                    <dd>(rule.trigger)</dd>
                    if let Some(next_run) = rule.next_run {
                        <dt class="text-muted-foreground">"Next"</dt>
                        <dd>(next_run)</dd>
                    }
                    <dt class="text-muted-foreground">"Then"</dt>
                    <dd>
                        <ul class="space-y-1">
                            for action in rule.actions {
                                <li>
                                    (action.text)
                                    if let Some(warning) = action.warning {
                                        <p class="text-destructive">(warning)</p>
                                    }
                                </li>
                            }
                        </ul>
                    </dd>
                </dl>
                for error in errors {
                    <p class="mt-3 text-sm text-destructive">(error)</p>
                }
            )
        )
    })
}

#[derive(Deserialize)]
struct EnabledForm {
    name: String,
    /// An unchecked switch sends no value.
    #[serde(default)]
    enabled: bool,
}

/// Answers with the rule's card, and with an error in `#status` out of band when the change failed.
#[route(POST "/rules/enabled")]
pub async fn set_enabled(cx: &Cx, Form(form): Form<EnabledForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let result = hub.set_rule_enabled(&form.name, form.enabled);
    let rule = overview::rules(hub)
        .into_iter()
        .find(|rule| rule.name == form.name);

    view! { cx =>
        if let Some(rule) = rule {
            rule_card(rule: rule)
        }
        <div id="status" class="mb-4" hx-swap-oob="true">
            if let Err(error) = result {
                <pre class="text-sm whitespace-pre-wrap text-destructive">(format!("{error:#}"))</pre>
            }
        </div>
    }
    .single()
    .await
}
