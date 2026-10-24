//! The cards and fields of the form.

use std::sync::Arc;

use topcoat::{
    Result,
    view::{View, attributes, component, view},
};

use super::catalog::{Catalog, Choice, ChoiceGroup, Control, Purpose, pair_groups, ungrouped};
use super::draft::{
    ALERTS, ActionDraft, ActionKind, ConditionDraft, ConditionKind, DAYS, EVENTS, Kind,
    TriggerDraft, TriggerKind, alert_texts, event_key, event_label,
};
use super::{Editor, SchedulePreview};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::checkbox::checkbox;
use crate::components::input::input;
use crate::components::label::label;
use crate::components::select::select;
use crate::components::switch::switch;

#[component]
pub(super) async fn rule_form(editor: Editor) -> Result<impl View> {
    let Editor {
        draft,
        catalog,
        text,
        error,
        placeholders,
        schedule,
        warnings,
    } = editor;
    let placeholders_hint = (!placeholders.is_empty()).then(|| {
        let names: Vec<String> = placeholders
            .iter()
            .map(|name| format!("{{{name}}}"))
            .collect();
        format!("Texts can use {}.", names.join(", "))
    });
    let takes_cooldown = draft.trigger.kind != TriggerKind::Button;
    let conditions: Vec<(usize, ConditionDraft)> =
        draft.conditions.iter().cloned().enumerate().collect();
    let actions: Vec<(usize, ActionDraft, Option<String>)> = draft
        .actions
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, action)| (index, action, warnings.get(index).cloned().flatten()))
        .collect();
    let invalid = error.is_some();
    Ok(view! {
        <form
            id="rule-editor"
            class="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,28rem)]"
            hx-post="/rules/editor"
            hx-trigger="change, submit"
            hx-target="this"
            hx-swap="outerHTML"
        >
            <input type="hidden" name="previous" value=(draft.previous.clone().unwrap_or_default())>
            <div class="min-w-0 space-y-4">
                card(
                    card_header(card_title("Rule"))
                    card_content(
                        <div class="flex items-end gap-4">
                            form_field(title: "Name", name: "name".to_string(),
                                input(attrs: attributes! { id="name" name="name" value=(draft.name.clone()) autocomplete="off" })
                            )
                            <div class="flex h-9 items-center gap-2">
                                switch(attrs: attributes! {
                                    id="enabled"
                                    name="enabled"
                                    value="true"
                                    if draft.enabled {
                                        checked=""
                                    }
                                })
                                label(attrs: attributes! { for="enabled" }, "Enabled")
                            </div>
                        </div>
                        if takes_cooldown {
                            <div class="mt-4 space-y-2">
                                form_field(title: "Cooldown (optional)", name: "cooldown".to_string(),
                                    text_input(name: "cooldown".to_string(), text: draft.cooldown.clone(), placeholder: Some("e.g. 1h"))
                                )
                                <p class="text-sm text-muted-foreground">"After the rule starts, triggers about the same node and channel are ignored this long. At most 7 days."</p>
                            </div>
                        }
                    )
                )
                card(
                    card_header(
                        card_title("When")
                        card_description("What runs the rule.")
                    )
                    card_content(
                        trigger_fields(trigger: draft.trigger.clone(), schedule: schedule, catalog: catalog.clone())
                    )
                )
                card(
                    card_header(
                        <div class="flex items-center justify-between gap-2">
                            card_title("If")
                            add_button(command: "add-condition", text: "Add condition")
                        </div>
                        card_description("Checked when the rule runs; every condition must hold. Optional.")
                    )
                    card_content(
                        if conditions.is_empty() {
                            <p class="text-sm text-muted-foreground">"No conditions: the actions always run."</p>
                        }
                        <div class="space-y-3">
                            for (index, condition) in conditions {
                                condition_fields(index: index, condition: condition, catalog: catalog.clone())
                            }
                        </div>
                    )
                )
                card(
                    card_header(
                        <div class="flex items-center justify-between gap-2">
                            card_title("Then")
                            add_button(command: "add-action", text: "Add action")
                        </div>
                        card_description("Run in order; a failed action does not stop the next ones.")
                        if let Some(hint) = placeholders_hint {
                            card_description((hint))
                        }
                    )
                    card_content(
                        if actions.is_empty() {
                            <p class="text-sm text-muted-foreground">"No actions yet."</p>
                        }
                        <div class="space-y-3">
                            for (index, action, warning) in actions {
                                action_fields(index: index, action: action, warning: warning, catalog: catalog.clone())
                            }
                        </div>
                    )
                )
            </div>
            <div class="min-w-0 space-y-4 lg:sticky lg:top-4">
                card(
                    card_header(
                        card_title("Preview")
                        card_description("As it goes in rules.toml.")
                    )
                    card_content(
                        if let Some(text) = text {
                            <pre class="overflow-x-auto rounded-lg bg-muted p-3 font-mono text-xs">(text)</pre>
                        }
                        if let Some(error) = error {
                            <pre class="mt-3 text-sm whitespace-pre-wrap text-destructive">(error)</pre>
                        }
                    )
                )
                <div class="flex justify-end gap-2">
                    <a
                        href="/rules"
                        class="inline-flex h-8 items-center rounded-lg border border-border px-3 text-sm font-medium hover:bg-foreground/5"
                    >"Cancel"</a>
                    button(
                        size: ButtonSize::Sm,
                        attrs: attributes! {
                            type="button"
                            hx-post="/rules/save"
                            hx-target="#rule-editor"
                            hx-swap="outerHTML"
                            if invalid {
                                disabled=""
                            }
                        },
                        "Save"
                    )
                </div>
            </div>
        </form>
    })
}

#[component]
async fn form_field(
    title: &'static str,
    name: String,
    #[default] child: topcoat::view::Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <div class="flex min-w-0 flex-1 flex-col gap-2">
            label(attrs: attributes! { for=(name) }, (title))
            (child)
        </div>
    })
}

#[component]
async fn add_button(command: &'static str, text: &'static str) -> Result<impl View> {
    Ok(view! {
        button(
            variant: ButtonVariant::Outline,
            size: ButtonSize::Sm,
            attrs: attributes! {
                type="button"
                name="command"
                value=(command)
                hx-post="/rules/editor"
                hx-target="#rule-editor"
                hx-swap="outerHTML"
            },
            (text)
        )
    })
}

#[component]
async fn remove_button(command: String) -> Result<impl View> {
    Ok(view! {
        button(
            variant: ButtonVariant::Ghost,
            size: ButtonSize::Sm,
            attrs: attributes! {
                type="button"
                name="command"
                value=(command)
                hx-post="/rules/editor"
                hx-target="#rule-editor"
                hx-swap="outerHTML"
            },
            "Remove"
        )
    })
}

#[component]
async fn choice_select(
    name: String,
    chosen: String,
    groups: Vec<ChoiceGroup>,
    #[default] placeholder: Option<&'static str>,
) -> Result<impl View> {
    Ok(view! {
        select(
            attrs: attributes! { id=(name.clone()) name=(name) },
            if let Some(placeholder) = placeholder {
                <option value="" if chosen.is_empty() { selected="" }>(placeholder)</option>
            }
            for group in groups {
                if group.label.is_empty() {
                    for choice in group.choices {
                        <option value=(choice.value.clone()) if choice.value == chosen { selected="" }>(choice.text)</option>
                    }
                } else {
                    <optgroup label=(group.label.clone())>
                        <legend>(group.label)</legend>
                        for choice in group.choices {
                            <option value=(choice.value.clone()) if choice.value == chosen { selected="" }>(choice.text)</option>
                        }
                    </optgroup>
                }
            }
        )
    })
}

#[component]
async fn text_input(
    name: String,
    text: String,
    #[default] kind: Option<&'static str>,
    #[default] placeholder: Option<&'static str>,
) -> Result<impl View> {
    Ok(view! {
        input(attrs: attributes! {
            id=(name.clone())
            name=(name)
            value=(text)
            type=(kind.unwrap_or("text"))
            autocomplete="off"
            placeholder=(placeholder.unwrap_or(""))
        })
    })
}

#[component]
async fn day_checkboxes(prefix: String, days: Vec<String>) -> Result<impl View> {
    let boxes: Vec<(String, &'static str, bool)> = DAYS
        .iter()
        .map(|(day, short)| {
            (
                format!("{prefix}.day.{day}"),
                *short,
                days.iter().any(|chosen| chosen == day),
            )
        })
        .collect();
    Ok(view! {
        <div class="flex flex-wrap gap-x-4 gap-y-2">
            for (name, short, checked) in boxes {
                <div class="flex items-center gap-2">
                    checkbox(attrs: attributes! {
                        id=(name.clone())
                        name=(name.clone())
                        value="true"
                        if checked {
                            checked=""
                        }
                    })
                    label(attrs: attributes! { for=(name) }, (short))
                </div>
            }
        </div>
    })
}

/// The select for a value of the channel's type, or a text box.
#[component]
async fn value_control(name: String, text: String, control: Control) -> Result<impl View> {
    let choices = control.choices();
    Ok(view! {
        match control {
            Control::OnOff | Control::Options(_) => {
                choice_select(name: name, chosen: text, groups: choices)
            },
            Control::Number => {
                text_input(name: name, text: text, kind: Some("number"), placeholder: Some("e.g. 28"))
            },
            Control::Text => {
                text_input(name: name, text: text)
            },
            Control::Nothing => {
                <p class="flex h-9 items-center text-sm text-muted-foreground">"Takes no value."</p>
            },
        }
    })
}

#[component]
async fn trigger_fields(
    trigger: TriggerDraft,
    schedule: Option<SchedulePreview>,
    catalog: Arc<Catalog>,
) -> Result<impl View> {
    let now = catalog.now(&trigger.channel);
    let purpose = match trigger.kind {
        TriggerKind::Threshold => Purpose::Condition,
        _ => Purpose::Event,
    };
    let control = Control::for_condition(catalog.declared(&trigger.channel));
    let mut tests = vec![Choice::new("equals", "is")];
    if control.allows_limits() {
        tests.push(Choice::new("above", "rises above"));
        tests.push(Choice::new("below", "falls below"));
    }
    let shown_control = if trigger.direction == "equals" {
        control
    } else {
        Control::Number
    };
    let channels = catalog.channel_groups(purpose, &trigger.channel);
    let events = ungrouped(
        EVENTS
            .iter()
            .map(|event| Choice::new(&event_key(*event), event_label(*event)))
            .collect(),
    );
    let nodes = catalog.node_groups(&trigger.device, true);
    let alerts = ungrouped(
        ALERTS
            .iter()
            .map(|kind| {
                let (key, text) = alert_texts(*kind);
                Choice::new(key, text)
            })
            .collect(),
    );
    let remote_keys = pair_groups(&catalog.remote_keys, &trigger.remote_key);
    Ok(view! {
        <div class="space-y-4">
            form_field(title: "Trigger", name: "trigger.kind".to_string(),
                choice_select(name: "trigger.kind".to_string(), chosen: trigger.kind.key().to_string(), groups: TriggerKind::choices())
            )
            match trigger.kind {
                TriggerKind::Event => {
                    form_field(title: "Channel", name: "trigger.channel".to_string(),
                        choice_select(name: "trigger.channel".to_string(), chosen: trigger.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                    <div class="flex gap-4">
                        form_field(title: "Event", name: "trigger.event".to_string(),
                            choice_select(name: "trigger.event".to_string(), chosen: trigger.event, groups: events)
                        )
                        form_field(title: "Data (optional)", name: "trigger.data".to_string(),
                            text_input(name: "trigger.data".to_string(), text: trigger.data, placeholder: Some("exact text, e.g. NEC 00 45"))
                        )
                    </div>
                },
                TriggerKind::Threshold => {
                    form_field(title: "Channel", name: "trigger.channel".to_string(),
                        choice_select(name: "trigger.channel".to_string(), chosen: trigger.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                    <div class="flex gap-4">
                        form_field(title: "When the value", name: "trigger.direction".to_string(),
                            choice_select(name: "trigger.direction".to_string(), chosen: trigger.direction, groups: ungrouped(tests))
                        )
                        form_field(title: "Value", name: "trigger.limit".to_string(),
                            value_control(name: "trigger.limit".to_string(), text: trigger.limit, control: shown_control)
                        )
                    </div>
                    if let Some(now) = now {
                        <p class="text-sm text-muted-foreground">(format!("Now: {now}"))</p>
                    }
                    form_field(title: "For (optional)", name: "trigger.lasting".to_string(),
                        text_input(name: "trigger.lasting".to_string(), text: trigger.lasting, placeholder: Some("e.g. 5m"))
                    )
                    <p class="text-sm text-muted-foreground">"Runs once when the value starts passing, not again while it keeps passing. With For, only if it keeps passing that long (at most 24 hours), even across a restart of the hub."</p>
                },
                TriggerKind::Schedule => {
                    form_field(title: "Time", name: "trigger.at".to_string(),
                        text_input(name: "trigger.at".to_string(), text: trigger.at, kind: Some("time"))
                    )
                    <div class="space-y-2">
                        <p class="text-sm font-medium">"Days"</p>
                        day_checkboxes(prefix: "trigger".to_string(), days: trigger.days)
                        <p class="text-sm text-muted-foreground">"None checked means every day."</p>
                    </div>
                    schedule_preview(schedule: schedule)
                },
                TriggerKind::Cron => {
                    form_field(title: "Expression", name: "trigger.cron".to_string(),
                        text_input(name: "trigger.cron".to_string(), text: trigger.cron, placeholder: Some("*/5 * * * *"))
                    )
                    <p class="text-sm text-muted-foreground">"Minute, hour, day of month, month and day of week, in local time."</p>
                    schedule_preview(schedule: schedule)
                },
                TriggerKind::Offline => {
                    form_field(title: "Node", name: "trigger.device".to_string(),
                        choice_select(name: "trigger.device".to_string(), chosen: trigger.device, groups: nodes, placeholder: Some("Choose a node…"))
                    )
                    form_field(title: "For (optional)", name: "trigger.lasting".to_string(),
                        text_input(name: "trigger.lasting".to_string(), text: trigger.lasting, placeholder: Some("e.g. 30m"))
                    )
                    <p class="text-sm text-muted-foreground">"With For, runs only if the node stays offline that long (at most 24 hours)."</p>
                },
                TriggerKind::Notice => {
                    form_field(title: "Node", name: "trigger.device".to_string(),
                        choice_select(name: "trigger.device".to_string(), chosen: trigger.device, groups: nodes, placeholder: Some("Choose a node…"))
                    )
                    <p class="text-sm text-muted-foreground">"Messages can use {title} and {message} of the notice."</p>
                },
                TriggerKind::Traffic => {
                    <div class="flex gap-4">
                        form_field(title: "Alert", name: "trigger.alert".to_string(),
                            choice_select(name: "trigger.alert".to_string(), chosen: trigger.alert, groups: alerts)
                        )
                        form_field(title: "Node", name: "trigger.device".to_string(),
                            choice_select(name: "trigger.device".to_string(), chosen: trigger.device, groups: nodes)
                        )
                    </div>
                    <p class="text-sm text-muted-foreground">"Alerts come from the Traffic page."</p>
                },
                TriggerKind::RemoteKey => {
                    form_field(title: "Key", name: "trigger.remote_key".to_string(),
                        choice_select(name: "trigger.remote_key".to_string(), chosen: trigger.remote_key, groups: remote_keys, placeholder: Some("Choose a key…"))
                    )
                    <p class="text-sm text-muted-foreground">"Keys come from the Remotes page."</p>
                },
                TriggerKind::Button => {
                    form_field(title: "Button", name: "trigger.button".to_string(),
                        text_input(name: "trigger.button".to_string(), text: trigger.button, placeholder: Some("e.g. Good night"))
                    )
                    <p class="text-sm text-muted-foreground">"The button shows on the Rules page; dashboard.toml can show it too. Its name is unique among rules."</p>
                },
            }
        </div>
    })
}

/// Nothing while the trigger does not parse; the error shows in the preview.
#[component]
async fn schedule_preview(schedule: Option<SchedulePreview>) -> Result<impl View> {
    Ok(view! {
        if let Some(schedule) = schedule {
            <div class="space-y-1 rounded-lg bg-muted p-3 text-sm">
                <p>(schedule.description)</p>
                if schedule.runs.is_empty() {
                    <p class="text-muted-foreground">"Never runs."</p>
                } else {
                    <p class="text-muted-foreground">"Next runs:"</p>
                    <ul class="list-inside list-disc text-muted-foreground">
                        for run in schedule.runs {
                            <li>(run)</li>
                        }
                    </ul>
                }
            </div>
        }
    })
}

#[component]
async fn condition_fields(
    index: usize,
    condition: ConditionDraft,
    catalog: Arc<Catalog>,
) -> Result<impl View> {
    let prefix = format!("condition.{index}");
    let name = {
        let prefix = prefix.clone();
        move |part: &str| format!("{prefix}.{part}")
    };
    let control = Control::for_condition(catalog.declared(&condition.channel));
    let now = catalog.now(&condition.channel);
    let mut tests = vec![Choice::new("equals", "is")];
    if control.allows_limits() {
        tests.push(Choice::new("above", "is above"));
        tests.push(Choice::new("below", "is below"));
    }
    let shown_control = if condition.test == "equals" {
        control
    } else {
        Control::Number
    };
    let channels = catalog.channel_groups(Purpose::Condition, &condition.channel);
    let nodes = catalog.node_groups(&condition.device, false);
    Ok(view! {
        <div class="space-y-3 rounded-lg border border-border p-3">
            <div class="flex items-end gap-2">
                form_field(title: "Condition", name: name("kind"),
                    choice_select(name: name("kind"), chosen: condition.kind.key().to_string(), groups: ConditionKind::choices())
                )
                remove_button(command: format!("remove-{prefix}"))
            </div>
            match condition.kind {
                ConditionKind::Channel => {
                    form_field(title: "Channel", name: name("channel"),
                        choice_select(name: name("channel"), chosen: condition.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                    <div class="flex gap-4">
                        form_field(title: "Test", name: name("test"),
                            choice_select(name: name("test"), chosen: condition.test, groups: ungrouped(tests))
                        )
                        form_field(title: "Value", name: name("value"),
                            value_control(name: name("value"), text: condition.value, control: shown_control)
                        )
                    </div>
                    if let Some(now) = now {
                        <p class="text-sm text-muted-foreground">(format!("Now: {now}"))</p>
                    }
                    <p class="text-sm text-muted-foreground">"Does not hold while the node is offline or its value is unknown."</p>
                },
                ConditionKind::Time => {
                    <div class="flex gap-4">
                        form_field(title: "From", name: name("from"),
                            text_input(name: name("from"), text: condition.from, kind: Some("time"))
                        )
                        form_field(title: "To", name: name("to"),
                            text_input(name: name("to"), text: condition.to, kind: Some("time"))
                        )
                    </div>
                    <p class="text-sm text-muted-foreground">"From included, to excluded; may cross midnight."</p>
                },
                ConditionKind::Days => {
                    day_checkboxes(prefix: prefix.clone(), days: condition.days)
                },
                ConditionKind::Online | ConditionKind::Offline => {
                    form_field(title: "Node", name: name("device"),
                        choice_select(name: name("device"), chosen: condition.device, groups: nodes, placeholder: Some("Choose a node…"))
                    )
                },
            }
        </div>
    })
}

#[component]
async fn action_fields(
    index: usize,
    action: ActionDraft,
    warning: Option<String>,
    catalog: Arc<Catalog>,
) -> Result<impl View> {
    let prefix = format!("action.{index}");
    let name = {
        let prefix = prefix.clone();
        move |part: &str| format!("{prefix}.{part}")
    };
    let purpose = match action.kind {
        ActionKind::Invoke => Purpose::Invoke,
        _ => Purpose::Switch,
    };
    let channels = catalog.channel_groups(purpose, &action.channel);
    let invoke_control = Control::for_invoke(catalog.declared(&action.channel));
    Ok(view! {
        <div class="space-y-3 rounded-lg border border-border p-3">
            <div class="flex items-end gap-2">
                form_field(title: "Action", name: name("kind"),
                    choice_select(name: name("kind"), chosen: action.kind.key().to_string(), groups: ActionKind::choices())
                )
                remove_button(command: format!("remove-{prefix}"))
            </div>
            match action.kind {
                ActionKind::Set => {
                    <div class="flex gap-4">
                        form_field(title: "Channel", name: name("channel"),
                            choice_select(name: name("channel"), chosen: action.channel, groups: channels, placeholder: Some("Choose a channel…"))
                        )
                        <div class="w-28">
                            form_field(title: "To", name: name("value"),
                                value_control(name: name("value"), text: action.value, control: Control::OnOff)
                            )
                        </div>
                    </div>
                },
                ActionKind::Toggle => {
                    form_field(title: "Channel", name: name("channel"),
                        choice_select(name: name("channel"), chosen: action.channel, groups: channels, placeholder: Some("Choose a channel…"))
                    )
                },
                ActionKind::Invoke => {
                    <div class="flex gap-4">
                        form_field(title: "Channel", name: name("channel"),
                            choice_select(name: name("channel"), chosen: action.channel, groups: channels, placeholder: Some("Choose a channel…"))
                        )
                        form_field(title: "Value", name: name("value"),
                            value_control(name: name("value"), text: action.value, control: invoke_control)
                        )
                    </div>
                },
                ActionKind::Notify => {
                    form_field(title: "Title", name: name("title"),
                        text_input(name: name("title"), text: action.title)
                    )
                    form_field(title: "Message", name: name("message"),
                        text_input(name: name("message"), text: action.message)
                    )
                },
                ActionKind::Wait => {
                    form_field(title: "How long", name: name("wait"),
                        text_input(name: name("wait"), text: action.wait, placeholder: Some("e.g. 2s"))
                    )
                    <p class="text-sm text-muted-foreground">"Pauses before the next action; at most 30 minutes."</p>
                },
            }
            if let Some(warning) = warning {
                <p class="text-sm text-destructive">(warning)</p>
            }
        </div>
    })
}
