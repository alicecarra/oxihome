//! The form that creates and edits a rule. Every change posts the whole form and the server
//! draws it again, so the fields follow the trigger, the channel and its type.

use std::collections::HashMap;
use std::sync::Arc;

use jiff::Zoned;
use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        page, parse_query_params,
        response::{IntoResponse, Response},
        route,
    },
    view::{View, ViewExt, ViewHandle, component, view},
};

use crate::config::rules::{Rule, Rules, Subject, Trigger};
use crate::hub::Hub;
use crate::web::format::rule_anchor;
use crate::web::layout::layout;
use crate::web::overview::{
    action_warning, cron_description, next_run_description, schedule_description,
};

type Built<T> = std::result::Result<T, String>;

mod catalog;
mod draft;
mod form;
mod table;

use catalog::Catalog;
use draft::Draft;
use form::rule_form;
use table::{rule_table, rule_text};

/// Everything the form shows.
struct Editor {
    draft: Draft,
    catalog: Arc<Catalog>,
    /// The rule as it would go in `rules.toml`.
    text: Option<String>,
    /// Why the rule cannot be saved as it is.
    error: Option<String>,
    /// What the trigger fills in the texts of the actions, like `{node}`.
    placeholders: &'static [&'static str],
    schedule: Option<SchedulePreview>,
    /// By action, the card's warning on `/rules`, like an invoke value the node does not take.
    warnings: Vec<Option<String>>,
}

/// What a time trigger means, and when it runs next.
#[derive(Clone)]
struct SchedulePreview {
    description: String,
    runs: Vec<String>,
}

const PREVIEW_RUNS: usize = 5;

impl SchedulePreview {
    fn new(trigger: &Trigger, now: &Zoned) -> Option<Self> {
        let description = match trigger {
            Trigger::Cron(trigger) => cron_description(trigger),
            Trigger::Schedule(trigger) => schedule_description(trigger),
            _ => return None,
        };
        let mut runs = Vec::new();
        let mut after = now.clone();
        while runs.len() < PREVIEW_RUNS
            && let Some(run) = trigger.next_run(&after)
        {
            let within_a_week = now
                .date()
                .until(run.date())
                .is_ok_and(|span| span.get_days() < 7);
            runs.push(if within_a_week {
                next_run_description(now, &run)
            } else {
                run.strftime("%A %Y-%m-%d %H:%M").to_string()
            });
            after = run;
        }
        Some(Self { description, runs })
    }
}

impl Editor {
    /// Checks the rule the way saving does, against the whole configuration.
    fn new(hub: &Hub, catalog: Arc<Catalog>, mut draft: Draft, save_error: Option<String>) -> Self {
        draft.settle(&catalog);
        let (text, error, rule) = match rule_table(&draft, &catalog) {
            Ok(table) => {
                let text = rule_text(&table);
                let rule = toml::from_str::<Rules>(&text)
                    .ok()
                    .and_then(|rules| rules.all().first().cloned());
                let error = hub
                    .check_rule(draft.previous.as_deref(), table)
                    .err()
                    .map(|error| format!("{error:#}"));
                (Some(text), error, rule)
            }
            Err(error) => (None, Some(error), None),
        };
        let now = Zoned::now().with_time_zone(hub.time_zone().clone());
        let schedule = rule
            .as_ref()
            .and_then(|rule| SchedulePreview::new(&rule.when, &now));
        let warnings = rule.as_ref().map_or_else(Vec::new, |rule| {
            // Before the snapshot: the stand-ins ask the hub for names.
            let subject = Subject::by_hand(&rule.when, hub);
            let snapshot = hub.snapshot();
            rule.then
                .iter()
                .map(|action| action_warning(&snapshot.registry, action, &subject))
                .collect()
        });
        Self {
            draft,
            catalog,
            text,
            error: save_error.or(error),
            placeholders: rule.map_or(&[][..], |rule| rule.when.placeholders()),
            schedule,
            warnings,
        }
    }
}

#[derive(Deserialize)]
struct RuleQuery {
    name: Option<String>,
    copy: Option<String>,
}

fn find_rule(hub: &Hub, name: Option<String>) -> Option<Rule> {
    let name = name?;
    hub.rules().into_iter().find(|rule| rule.name == name)
}

#[page("/rules/new")]
pub async fn new_rule(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let copy = parse_query_params::<RuleQuery>(cx)
        .ok()
        .and_then(|query| find_rule(hub, query.copy));
    let draft = match copy {
        Some(rule) => Draft {
            previous: None,
            name: format!("{} (copy)", rule.name),
            ..Draft::from_rule(&rule)
        },
        None => Draft::new(),
    };
    let editor = Editor::new(hub, Arc::new(Catalog::new(hub)), draft, None);
    Ok(view! {
        layout(
            title: "Oxihome new rule",
            editor_page(editor: Some(editor), missing: String::new())
        )
    })
}

#[page("/rules/edit")]
pub async fn edit_rule(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let name = parse_query_params::<RuleQuery>(cx)
        .ok()
        .and_then(|query| query.name);
    let editor = find_rule(hub, name.clone()).map(|rule| {
        Editor::new(
            hub,
            Arc::new(Catalog::new(hub)),
            Draft::from_rule(&rule),
            None,
        )
    });
    Ok(view! {
        layout(
            title: "Oxihome edit rule",
            editor_page(editor: editor, missing: name.unwrap_or_default())
        )
    })
}

#[component]
async fn editor_page(editor: Option<Editor>, missing: String) -> Result<impl View> {
    Ok(view! {
        <div class="space-y-4">
            <div class="flex items-center justify-between gap-4">
                <p class="text-sm text-muted-foreground">"The rule is saved in rules.toml; the other rules and the comments stay as they are."</p>
                <a href="/rules" class="text-sm font-medium whitespace-nowrap hover:underline">"Back to rules"</a>
            </div>
            if let Some(editor) = editor {
                rule_form(editor: editor)
            } else {
                <p class="text-muted-foreground">(format!("No rule named \"{missing}\"."))</p>
            }
        </div>
    })
}

/// Answers with the form drawn again from what it sent.
#[route(POST "/rules/editor")]
pub async fn redraw(cx: &Cx, Form(fields): Form<HashMap<String, String>>) -> Result<ViewHandle> {
    let hub: &Arc<Hub> = app_context(cx);
    let catalog = Arc::new(Catalog::new(hub));
    let editor = Editor::new(hub, catalog, Draft::from_fields(&fields), None);
    view! { cx => rule_form(editor: editor) }.single().await
}

/// Sends the browser to the rule's card on success; answers with the form and the error otherwise.
#[route(POST "/rules/save")]
pub async fn save(cx: &Cx, Form(fields): Form<HashMap<String, String>>) -> Result<Response> {
    let hub: &Arc<Hub> = app_context(cx);
    let catalog = Arc::new(Catalog::new(hub));
    let mut draft = Draft::from_fields(&fields);
    draft.settle(&catalog);
    let result = rule_table(&draft, &catalog)
        .map_err(anyhow::Error::msg)
        .and_then(|table| hub.save_rule(draft.previous.as_deref(), table));
    match result {
        Ok(()) => {
            let name = draft.name.trim().to_string();
            let location = format!("/rules#{}", rule_anchor(&name));
            hub.notify("Rule saved".into(), name, "rules".into());
            ([("HX-Redirect", location)], ()).into_response(cx)
        }
        Err(error) => {
            let editor = Editor::new(hub, catalog, draft, Some(format!("{error:#}")));
            view! { cx => rule_form(editor: editor) }
                .single()
                .await?
                .into_response(cx)
        }
    }
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::{Access, Channel};

    use super::*;

    const EVERY_KIND: &str = r#"
        [[rules]]
        name = "Schedule with conditions"
        enabled = false
        when = { at = "07:30", days = ["monday", "friday"] }
        cooldown = "1h"
        if = [
            { device = "board", entity = 3, channel = 0, above = 28.5 },
            { device = "board", entity = 1, channel = 0, equals = true },
            { device = "board", entity = 2, channel = 0, equals = "auto" },
            { from = "22:00", to = "06:00" },
            { days = ["saturday", "sunday"] },
            { online = "board" },
            { offline = "phone" },
        ]
        then = [
            { set = { device = "board", entity = 1, channel = 0, value = true } },
            { toggle = { device = "board", entity = 1, channel = 0 } },
            { invoke = { device = "board", entity = 4, channel = 0, value = "beep" } },
            { invoke = { device = "board", entity = 4, channel = 1 } },
            { wait = "1m 30s" },
            { notify = { title = "Hi", message = "" } },
        ]

        [[rules]]
        name = "Cron"
        when = { cron = "*/5 * * * *" }
        then = []

        [[rules]]
        name = "Event"
        when = { device = "board", entity = 2, channel = 0, event = "Received", data = "NEC 00 45" }
        then = [{ notify = { title = "{node}", message = "{channel}: {value}" } }]

        [[rules]]
        name = "Threshold"
        when = { device = "board", entity = 3, channel = 0, below = 10, for = "5m" }
        then = []

        [[rules]]
        name = "Any offline"
        when = { offline = "*" }
        then = []

        [[rules]]
        name = "Board offline"
        when = { offline = "board" }
        then = []

        [[rules]]
        name = "Board offline a while"
        when = { offline = "board", for = "30m" }
        then = []

        [[rules]]
        name = "Door open"
        when = { device = "board", entity = 7, channel = 0, equals = true, for = "10m" }
        then = []

        [[rules]]
        name = "Mode auto"
        when = { device = "board", entity = 2, channel = 0, equals = "auto" }
        then = []

        [[rules]]
        name = "Stopped"
        when = { stopped = "*" }
        then = []

        [[rules]]
        name = "Unanswered"
        when = { unanswered = "board" }
        then = []

        [[rules]]
        name = "Remote key"
        when = { remote = "TV", key = "Power" }
        then = []

        [[rules]]
        name = "Button"
        when = { button = "Good night" }
        then = []
    "#;

    /// A field the editor does not know fails here instead of being dropped when the rule is
    /// saved from the form.
    #[test]
    fn rules_come_back_the_same_from_the_form() {
        let catalog = Catalog {
            channels: Vec::new(),
            nodes: Vec::new(),
            remote_keys: Vec::new(),
        };
        for text in [EVERY_KIND, include_str!("../../../config/rules.toml")] {
            let rules: Rules = toml::from_str(text).unwrap();
            for rule in rules.all() {
                let mut draft = Draft::from_rule(rule);
                draft.settle(&catalog);
                let table = rule_table(&draft, &catalog).unwrap();
                let saved: Rules = toml::from_str(&rule_text(&table)).unwrap();
                let saved = &saved.all()[0];
                assert_eq!(saved.name, rule.name);
                assert_eq!(saved.enabled, rule.enabled, "{}", rule.name);
                assert_eq!(saved.when, rule.when, "{}", rule.name);
                assert_eq!(saved.cooldown, rule.cooldown, "{}", rule.name);
                assert_eq!(saved.conditions, rule.conditions, "{}", rule.name);
                assert_eq!(saved.then, rule.then, "{}", rule.name);
            }
        }
    }

    #[test]
    fn time_triggers_preview_their_next_runs() {
        let rules: Rules = toml::from_str(
            r#"
            [[rules]]
            name = "Weekly"
            when = { cron = "0 7 * * 1" }
            then = []

            [[rules]]
            name = "Daily"
            when = { at = "07:30" }
            then = []
            "#,
        )
        .unwrap();
        let now: Zoned = "2026-10-19T08:00:00-03:00[-03:00]".parse().unwrap();
        let weekly = SchedulePreview::new(&rules.all()[0].when, &now).unwrap();
        assert!(weekly.description.contains("0 7 * * 1"));
        assert_eq!(weekly.runs.len(), 5);
        assert_eq!(weekly.runs[0], "Monday 2026-10-26 07:00");
        let daily = SchedulePreview::new(&rules.all()[1].when, &now).unwrap();
        assert_eq!(daily.runs[..2], ["tomorrow 07:30", "Wednesday 07:30"]);
    }

    #[tokio::test]
    async fn channels_show_the_names_the_node_declares() {
        let hub = Hub::for_tests("rule-editor-names", "").await;
        let event = |id: u8| Channel {
            id,
            name: None,
            access: Access::Event,
            value: None,
            quantity: None,
            unit: None,
        };
        let hello = oxihome_protocol::Message::new(
            "phone",
            0,
            oxihome_protocol::Payload::Hello {
                build: oxihome_protocol::Build::new("node", "0.1.0", "linux"),
                ip: None,
                entities: vec![oxihome_protocol::EntityDescriptor {
                    id: 1,
                    name: Some("Phone".into()),
                    channels: vec![event(0), event(1).named("Presence")],
                }],
            },
        );
        crate::dispatch::on_message(&hub, &hello);
        let names: Vec<String> = Catalog::new(&hub)
            .channels
            .into_iter()
            .map(|choice| choice.name)
            .collect();
        assert_eq!(names, ["Phone", "Phone · Presence"]);
    }

    #[tokio::test]
    async fn the_editor_does_not_lock_itself_and_checks_names() {
        let hub = Hub::for_tests(
            "rule-editor",
            r#"
            [[rules]]
            name = "Hot"
            when = { device = "board", entity = 3, channel = 0, above = 28 }
            then = [{ notify = { title = "{node}", message = "{channel} at {value}" } }]
            "#,
        )
        .await;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rule = hub.rules().remove(0);
            let edited = Editor::new(
                &hub,
                Arc::new(Catalog::new(&hub)),
                Draft::from_rule(&rule),
                None,
            );
            let copy = Draft {
                previous: None,
                ..Draft::from_rule(&rule)
            };
            let copied = Editor::new(&hub, Arc::new(Catalog::new(&hub)), copy, None);
            sender
                .send((edited.error, edited.placeholders, copied.error))
                .unwrap();
        });
        let (edited, placeholders, copied) = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(edited, None);
        assert_eq!(placeholders, ["node", "channel", "value"]);
        assert!(copied.unwrap().contains("already a rule named \"Hot\""));
    }
}
