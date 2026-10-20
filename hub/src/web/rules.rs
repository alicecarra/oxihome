use std::sync::Arc;

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, page, route},
    view::{View, ViewExt, ViewHandle, attributes, component, view},
};

use super::format::{ago_label, query_value, rule_anchor};
use super::layout::layout;
use super::overview::{self, RuleOverview};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::components::switch::switch;
use crate::config::rules::{Action, Start, Subject, start};
use crate::hub::Hub;

mod editor;

pub use editor::{edit_rule, new_rule, redraw, save};

#[page("/rules")]
pub async fn rules(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let rules = overview::rules(hub);
    Ok(view! {
        layout(
            title: "Oxihome rules",
            <div class="space-y-4">
                <div class="flex items-center justify-between gap-4">
                    <p class="text-sm text-muted-foreground">"Rules run when their event happens or at their time, if their conditions hold. They come from rules.toml."</p>
                    <div class="flex items-center gap-4">
                        <a href="/settings/rules.toml" class="text-sm font-medium whitespace-nowrap hover:underline">"Edit in Settings"</a>
                        <a
                            href="/rules/new"
                            class="inline-flex h-8 items-center rounded-lg bg-primary px-3 text-sm font-medium whitespace-nowrap text-primary-foreground hover:bg-primary/90"
                        >"New rule"</a>
                    </div>
                </div>
                if rules.is_empty() {
                    <p class="text-muted-foreground">"No rules yet."</p>
                } else {
                    <div
                        id="rule-filter"
                        data-filter-items="#rules"
                        data-filter-attribute="status"
                        data-filter-empty="rules-filtered-out"
                        class="flex flex-wrap gap-2"
                    >
                        for (status, label) in [("ok", "OK"), ("failed", "Failed"), ("never-ran", "Never ran"), ("disabled", "Disabled")] {
                            button(
                                variant: ButtonVariant::Outline,
                                size: ButtonSize::Sm,
                                attrs: attributes! {
                                    type="button"
                                    data-filter=(status)
                                    aria-pressed="true"
                                    class="aria-pressed:border-transparent aria-pressed:bg-primary aria-pressed:text-primary-foreground aria-pressed:hover:bg-primary/90"
                                },
                                (label)
                                <span data-count="" class="tabular-nums opacity-70"></span>
                            )
                        }
                    </div>
                    <div id="rules" class="grid items-start gap-4 md:grid-cols-2">
                        for rule in rules {
                            rule_card(rule: rule)
                        }
                    </div>
                    <p id="rules-filtered-out" class="text-muted-foreground" hidden="">"No rules match the filter."</p>
                    <script src="/static/filter.js"></script>
                }
            </div>
        )
    })
}

#[component]
async fn rule_card(rule: RuleOverview) -> Result<impl View> {
    // The key is what the filter matches, in `data-status`.
    let (mut variant, mut status, key) = if !rule.enabled {
        (BadgeVariant::Muted, "disabled".to_string(), "disabled")
    } else {
        match &rule.last_run {
            None => (BadgeVariant::Muted, "never ran".to_string(), "never-ran"),
            Some((ago, errors)) if errors.is_empty() => (
                BadgeVariant::Success,
                format!("ok · {}", ago_label(*ago)),
                "ok",
            ),
            Some((ago, _)) => (
                BadgeVariant::Destructive,
                format!("failed · {}", ago_label(*ago)),
                "failed",
            ),
        }
    };
    // The filter still goes by the last run.
    if rule.running {
        (variant, status) = (BadgeVariant::Primary, "running".to_string());
    }
    let errors = rule.last_run.map(|(_, errors)| errors).unwrap_or_default();
    Ok(view! {
        card(
            attrs: attributes! {
                id=(rule_anchor(&rule.name))
                data-status=(key)
                // Highlighted when a link from a node's page points at it.
                class=(if rule.enabled { "scroll-mt-4 target:ring-2 target:ring-ring h-full" } else { "scroll-mt-4 opacity-60 target:ring-2 target:ring-ring h-full" })
            },
            card_header(
                <div class="flex items-center justify-between gap-2">
                    card_title((rule.name.clone()))
                    <div class="flex items-center gap-3">
                        badge(variant: variant, (status))
                        // Answered with this card, replaced in place.
                        <form
                            class="flex"
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
                    <dd class="flex items-center justify-between gap-2">
                        (rule.trigger)
                        if rule.enabled {
                            <form
                                hx-post="/rules/run"
                                hx-target=(format!("#{}", rule_anchor(&rule.name)))
                                hx-swap="outerHTML"
                                hx-disabled-elt="find button"
                            >
                                <input type="hidden" name="name" value=(rule.name.clone())>
                                button(
                                    variant: ButtonVariant::Secondary,
                                    size: ButtonSize::Sm,
                                    attrs: attributes! { type="submit" class="whitespace-nowrap" },
                                    "Run now"
                                )
                            </form>
                        }
                    </dd>
                    if !rule.conditions.is_empty() {
                        <dt class="text-muted-foreground">"If"</dt>
                        <dd>
                            <ul class="space-y-1">
                                for condition in rule.conditions {
                                    <li>(condition)</li>
                                }
                            </ul>
                        </dd>
                    }
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
                <div class="mt-3 flex gap-4 text-sm font-medium">
                    <a href=(format!("/rules/edit?name={}", query_value(&rule.name))) class="hover:underline">"Edit"</a>
                    <a href=(format!("/rules/new?copy={}", query_value(&rule.name))) class="hover:underline">"Duplicate"</a>
                    <a
                        href=(format!("/timeline?rule={}", query_value(&rule.name)))
                        class="ml-auto text-muted-foreground hover:text-foreground hover:underline"
                    >
                        "History"
                    </a>
                </div>
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
        rules_indicator(failing: overview::failing_rules(hub), oob: true)
        <div id="status" class="mb-4" hx-swap-oob="true">
            if let Err(error) = result {
                <pre class="text-sm whitespace-pre-wrap text-destructive">(format!("{error:#}"))</pre>
            }
        </div>
    }
    .single()
    .await
}

/// The dot on the menu's Rules item while enabled rules failed their last run. Out of band, it
/// replaces the one in the menu from another answer.
#[component]
pub(super) async fn rules_indicator(failing: usize, #[default] oob: bool) -> Result<impl View> {
    let title = match failing {
        1 => "1 rule failed its last run".to_string(),
        count => format!("{count} rules failed their last run"),
    };
    Ok(view! {
        <span
            id="rules-indicator"
            hx-get="/rules/indicator"
            hx-trigger="sse:timeline"
            hx-swap="outerHTML"
            title=(title.clone())
            aria-label=(title)
            class="absolute -top-0.5 -right-1 size-2 rounded-full bg-destructive"
            if failing == 0 {
                hidden=""
            }
            if oob {
                hx-swap-oob="true"
            }
        ></span>
    })
}

#[page("/rules/indicator")]
pub async fn indicator_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { rules_indicator(failing: overview::failing_rules(hub)) })
}

#[derive(Deserialize)]
struct RunForm {
    name: String,
}

/// Runs the rule as its trigger would, conditions included, and answers with its card once the
/// actions are done, or right away for a rule that waits; a skip shows in `#status` out of band.
#[route(POST "/rules/run")]
pub async fn run_now(cx: &Cx, Form(form): Form<RunForm>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let rule = hub
        .rules()
        .into_iter()
        .find(|rule| rule.name == form.name && rule.enabled);
    let (class, message) = match rule {
        None => (
            "text-destructive",
            Some(format!("No enabled rule is named \"{}\"", form.name)),
        ),
        Some(rule) => {
            let waits = rule
                .then
                .iter()
                .any(|action| matches!(action, Action::Wait(_)));
            let subject = Subject::by_hand(&rule.when, hub.as_ref());
            let message = match start(hub.clone(), rule, subject) {
                Start::Started(actions) if !waits => {
                    let _ = actions.await;
                    None
                }
                Start::Started(_) => Some(format!(
                    "{}: running; reload to see how it ended",
                    form.name
                )),
                Start::Skipped(condition) => Some(format!(
                    "{}: skipped, \"{}\" does not hold",
                    form.name,
                    overview::condition(hub, &condition)
                )),
                Start::Busy => Some(format!("{}: still running", form.name)),
            };
            ("text-muted-foreground", message)
        }
    };
    let rule = overview::rules(hub)
        .into_iter()
        .find(|rule| rule.name == form.name);

    view! { cx =>
        if let Some(rule) = rule {
            rule_card(rule: rule)
        }
        <div id="status" class="mb-4" hx-swap-oob="true">
            if let Some(message) = message {
                <p class=(format!("text-sm {class}"))>(message)</p>
            }
        </div>
    }
    .single()
    .await
}
