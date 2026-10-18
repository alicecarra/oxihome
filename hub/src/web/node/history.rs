//! The history card: over-the-air updates and the notifications about the node.

use std::sync::Arc;

use jiff::Timestamp;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, path_param},
    view::{View, component, view},
};

use super::{DeviceId, HISTORY_ROWS, History};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::hub::Hub;
use crate::storage::{StoredUpdate, UpdateOutcome};
use crate::web::format::{ago_label, duration_label, elapsed, query_value};

pub(super) async fn history(hub: &Hub, device_id: &str) -> History {
    History {
        updates: hub.updates(device_id, HISTORY_ROWS).await,
        notifications: hub.notifications_about(device_id, HISTORY_ROWS).await,
    }
}

#[page("/nodes/{device_id}/history")]
pub async fn history_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let device_id = path_param::<DeviceId>(cx).to_string();
    let history = history(hub, &device_id).await;
    Ok(view! { history_section(device_id: device_id, history: history) })
}

/// Between the fields and the rules; refreshes on node changes and new notifications.
#[component]
pub(super) async fn history_section(device_id: String, history: History) -> Result<impl View> {
    let now = Timestamp::now();
    Ok(view! {
        <div
            id="node-history"
            hx-get=(format!("/nodes/{device_id}/history"))
            hx-trigger="sse:nodes, sse:notifications, refresh"
            hx-swap="outerHTML"
        >
            card(
                card_header(
                    <div class="flex items-center justify-between gap-3">
                        card_title("History")
                        <a
                            href=(format!("/timeline?node={}", query_value(&device_id)))
                            class="text-sm font-medium text-muted-foreground hover:text-foreground hover:underline"
                        >
                            "Open in timeline"
                        </a>
                    </div>
                )
                card_content(
                    <div class="grid gap-6 md:grid-cols-2">
                        <div>
                            <h3 class="mb-2 text-sm font-medium">"Firmware updates"</h3>
                            if history.updates.is_empty() {
                                <p class="text-sm text-muted-foreground">"None yet."</p>
                            }
                            <ul class="divide-y divide-border text-sm">
                                for update in history.updates {
                                    <li class="flex justify-between gap-3 py-2 first:pt-0 last:pb-0">
                                        <div class="min-w-0">
                                            <p class="font-medium">(format!("{} → {}", update.from_version, update.to_version))</p>
                                            <p class="truncate text-muted-foreground" title=(update_outcome_label(&update))>(update_outcome_label(&update))</p>
                                        </div>
                                        <span class="shrink-0 text-xs text-muted-foreground">(ago_label(elapsed(update.finished_at, now)))</span>
                                    </li>
                                }
                            </ul>
                        </div>
                        <div>
                            <h3 class="mb-2 text-sm font-medium">"Activity"</h3>
                            if history.notifications.is_empty() {
                                <p class="text-sm text-muted-foreground">"Nothing in the last days."</p>
                            }
                            <ul class="divide-y divide-border text-sm">
                                for notification in history.notifications {
                                    <li class="flex justify-between gap-3 py-2 first:pt-0 last:pb-0">
                                        <div class="min-w-0">
                                            <p class="font-medium">(notification.title)</p>
                                            <p class="truncate text-muted-foreground" title=(notification.message.clone())>(notification.message)</p>
                                        </div>
                                        <span class="shrink-0 text-xs text-muted-foreground">(ago_label(elapsed(notification.at, now)))</span>
                                    </li>
                                }
                            </ul>
                        </div>
                    </div>
                )
            )
        </div>
    })
}

fn update_outcome_label(update: &StoredUpdate) -> String {
    match update.outcome {
        UpdateOutcome::Updated => {
            let took = elapsed(update.started_at, update.finished_at);
            format!("Updated in {}", duration_label(took))
        }
        UpdateOutcome::RolledBack => "Rolled back: the new version did not reach the broker".into(),
        UpdateOutcome::Failed => format!("Failed: {}", update.detail.as_deref().unwrap_or("error")),
        UpdateOutcome::NoAnswer => "No answer after the update".into(),
    }
}
