use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    view::{View, attributes, component, view},
};

use super::format::ago_label;
use super::layout::layout;
use super::overview::{self, RuleOverview};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::card::{card, card_content, card_header, card_title};
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
                    <a href="/settings#rules.toml" class="text-sm font-medium hover:underline">"Edit in Settings"</a>
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
                if !rule.enabled {
                    class="opacity-60"
                }
            },
            card_header(
                <div class="flex items-center justify-between gap-2">
                    card_title((rule.name))
                    badge(variant: variant, (status))
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
