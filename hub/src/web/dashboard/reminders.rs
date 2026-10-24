//! The dashboard's `reminders` block: the reminders of a node laid out like the reminders node
//! (`reminders/src/node.rs`), pending first, each with Done and, while pending, Snooze.

use std::collections::BTreeMap;

use jiff::Timestamp;
use oxihome_protocol::Value;
use topcoat::{
    Result,
    view::{View, attributes, component, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::hub::Hub;
use crate::web::format::{ago_label, elapsed, moment_label};
use crate::web::overview;

// The reminders node's entities and channels; the protocol has no lists, so the block knows them.
const ALL_REMINDERS_ENTITY: u8 = 1;
const PENDING_CHANNEL: u8 = 0;
const DONE_CHANNEL: u8 = 1;
const LAST_DONE_CHANNEL: u8 = 2;
const NEXT_DUE_CHANNEL: u8 = 3;
const SNOOZE_CHANNEL: u8 = 5;
/// The node's snooze options and their labels.
const SNOOZE_CHOICES: [(&str, &str); 4] = [
    ("15m", "15 min"),
    ("1h", "1 h"),
    ("3h", "3 h"),
    ("tomorrow", "Tomorrow"),
];

pub struct ReminderList {
    /// The node's name, or its id.
    pub node_label: String,
    /// Pending first, then by next due time.
    pub items: Vec<ReminderItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReminderItem {
    pub entity: u8,
    pub name: String,
    pub pending: bool,
    /// Unix time: when it fell due while pending, when it comes back while snoozed.
    pub next_due: Option<i64>,
    pub last_done: Option<i64>,
    /// Whether the node declares the snooze channel.
    pub snoozable: bool,
}

impl ReminderItem {
    /// Pending with its next due time ahead.
    fn snoozed(&self, now: Timestamp) -> bool {
        self.pending && self.next_due.is_some_and(|due| due > now.as_second())
    }

    fn needs_attention(&self, now: Timestamp) -> bool {
        self.pending && !self.snoozed(now)
    }
}

/// `None` when the node never sent a `Hello`.
pub fn reminder_list(hub: &Hub, device_id: &str) -> Option<ReminderList> {
    let node = overview::node(hub, device_id)?;
    let mut items: BTreeMap<u8, ReminderItem> = BTreeMap::new();
    for channel in &node.channels {
        if channel.entity == ALL_REMINDERS_ENTITY {
            continue;
        }
        let item = items.entry(channel.entity).or_insert_with(|| ReminderItem {
            entity: channel.entity,
            name: channel
                .entity_name
                .clone()
                .unwrap_or_else(|| format!("Reminder {}", channel.entity)),
            pending: false,
            next_due: None,
            last_done: None,
            snoozable: false,
        });
        match (channel.channel, &channel.value) {
            (PENDING_CHANNEL, Some(Value::Bool(pending))) => item.pending = *pending,
            (LAST_DONE_CHANNEL, Some(Value::Int(at))) => item.last_done = Some(i64::from(*at)),
            (NEXT_DUE_CHANNEL, Some(Value::Int(at))) => item.next_due = Some(i64::from(*at)),
            (SNOOZE_CHANNEL, _) => item.snoozable = true,
            _ => {}
        }
    }
    let mut items: Vec<ReminderItem> = items.into_values().collect();
    sort(&mut items);
    Some(ReminderList {
        node_label: node.name.unwrap_or_else(|| device_id.to_string()),
        items,
    })
}

fn sort(items: &mut [ReminderItem]) {
    items.sort_by_key(|item| (!item.pending, item.next_due.unwrap_or(i64::MAX)));
}

/// A reminder is pending and not snoozed.
pub fn needs_attention(hub: &Hub, device_id: &str) -> bool {
    let now = Timestamp::now();
    reminder_list(hub, device_id)
        .is_some_and(|list| list.items.iter().any(|item| item.needs_attention(now)))
}

#[component]
pub async fn reminders_block(list: Option<ReminderList>, device_id: String) -> Result<impl View> {
    Ok(view! {
        match list {
            None => <p class="text-sm text-muted-foreground">(format!("{device_id} has not said hello yet."))</p>,
            Some(list) => reminder_rows(items: list.items, device_id: device_id),
        }
    })
}

#[component]
async fn reminder_rows(items: Vec<ReminderItem>, device_id: String) -> Result<impl View> {
    let now = Timestamp::now();
    Ok(view! {
        if items.is_empty() {
            <p class="text-sm text-muted-foreground">"No reminders in its configuration."</p>
        }
        <ul class="divide-y divide-border">
            for item in items {
                reminder_row(item: item, device_id: device_id.clone(), now: now)
            }
        </ul>
    })
}

#[component]
async fn reminder_row(item: ReminderItem, device_id: String, now: Timestamp) -> Result<impl View> {
    let snoozed = item.snoozed(now);
    let attention = item.needs_attention(now);
    let status = status_label(&item, now);
    let status_class = if attention {
        "text-xs font-medium text-destructive"
    } else {
        "text-xs text-muted-foreground"
    };
    let last_done = item
        .last_done
        .and_then(|at| Timestamp::from_second(at).ok())
        .map(|at| format!("Done {}", ago_label(elapsed(at, now))));
    let snooze = item.pending && item.snoozable && !snoozed;
    let entity = item.entity.to_string();
    Ok(view! {
        <li class="space-y-2 py-2.5 first:pt-0 last:pb-0">
            <div class="flex items-start gap-3">
                <span
                    class=(if attention { "mt-1.5 size-2 shrink-0 rounded-full bg-destructive" } else { "mt-1.5 size-2 shrink-0" })
                    title=(if attention { "pending" } else { "" })
                ></span>
                <div class="min-w-0 flex-1">
                    <p class="text-sm font-medium">(item.name.clone())</p>
                    <p class=(status_class)>(status)</p>
                    if let Some(last_done) = last_done {
                        <p class="text-xs text-muted-foreground">(last_done)</p>
                    }
                </div>
                <form hx-post="/invoke" hx-target="#status">
                    <input type="hidden" name="device_id" value=(device_id.clone())>
                    <input type="hidden" name="entity" value=(entity.clone())>
                    <input type="hidden" name="channel" value=(DONE_CHANNEL.to_string())>
                    button(
                        variant: if item.pending { ButtonVariant::Primary } else { ButtonVariant::Outline },
                        size: ButtonSize::Sm,
                        attrs: attributes! { type="submit" },
                        "Done"
                    )
                </form>
            </div>
            if snooze {
                <form hx-post="/invoke" hx-target="#status" class="flex flex-wrap items-center gap-1 pl-5">
                    <input type="hidden" name="device_id" value=(device_id)>
                    <input type="hidden" name="entity" value=(entity)>
                    <input type="hidden" name="channel" value=(SNOOZE_CHANNEL.to_string())>
                    <span class="text-xs text-muted-foreground">"Snooze"</span>
                    for (option, label) in SNOOZE_CHOICES {
                        button(
                            variant: ButtonVariant::Ghost,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" name="value" value=(option) },
                            (label)
                        )
                    }
                </form>
            }
        </li>
    })
}

/// `Pending since 08:30`, `Snoozed until 09:45`, `Due Sat 08:30`.
fn status_label(item: &ReminderItem, now: Timestamp) -> String {
    let Some(due) = item.next_due else {
        return if item.pending {
            "Pending".into()
        } else {
            "Not scheduled".into()
        };
    };
    let moment = moment_label(due, now);
    if item.snoozed(now) {
        format!("Snoozed until {moment}")
    } else if item.pending {
        format!("Pending since {moment}")
    } else {
        format!("Due {moment}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(entity: u8, pending: bool, next_due: Option<i64>) -> ReminderItem {
        ReminderItem {
            entity,
            name: format!("Reminder {entity}"),
            pending,
            next_due,
            last_done: None,
            snoozable: true,
        }
    }

    #[test]
    fn pending_come_first_then_the_next_due() {
        let mut items = vec![
            item(2, false, Some(300)),
            item(3, true, Some(200)),
            item(4, false, None),
            item(5, false, Some(100)),
            item(6, true, Some(50)),
        ];
        sort(&mut items);
        let order: Vec<u8> = items.iter().map(|item| item.entity).collect();
        assert_eq!(order, [6, 3, 5, 2, 4]);
    }

    #[test]
    fn a_snoozed_reminder_waits_without_attention() {
        let now = Timestamp::from_second(1_000).unwrap();
        let due = item(2, true, Some(900));
        assert!(due.needs_attention(now));
        assert!(status_label(&due, now).starts_with("Pending since"));
        let snoozed = item(2, true, Some(1_600));
        assert!(!snoozed.needs_attention(now));
        assert!(status_label(&snoozed, now).starts_with("Snoozed until"));
        let later = item(2, false, Some(1_600));
        assert!(!later.needs_attention(now));
        assert!(status_label(&later, now).starts_with("Due"));
    }
}
