//! The tab row: one tab per dashboard, with a dot on the others when they need a look.

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    view::{View, component, view},
};

use super::attention::{Attention, dashboard_attention};
use super::{Link, Picked, Place, node_trigger};
use crate::config::dashboard::BlockKind;
use crate::hub::Hub;

/// The tab row alone, refreshed when a tab may need attention.
#[page("/dashboard/tabs")]
pub async fn dashboard_tabs_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let picked = Picked::from_query(cx);
    let tabs = Place::of(hub, picked.dashboard).and_then(|place| tab_row(hub, &place));
    Ok(view! {
        match tabs {
            Some(tabs) => dashboard_tabs(tabs: tabs),
            None => <div id="dashboard-tabs" hidden=""></div>,
        }
    })
}

/// One tab per dashboard.
pub(super) struct TabRow {
    pub(super) tabs: Vec<Tab>,
    /// Where the row fetches itself again.
    pub(super) refresh: String,
    pub(super) trigger: String,
}

pub(super) struct Tab {
    pub(super) name: String,
    pub(super) link: Link,
    pub(super) current: bool,
    /// Only on the other tabs: the current one shows it on its blocks.
    pub(super) attention: Attention,
}

/// `None` with a single dashboard: no tabs then.
pub(super) fn tab_row(hub: &Hub, current: &Place) -> Option<TabRow> {
    let dashboards = hub.dashboards();
    if dashboards.len() < 2 {
        return None;
    }
    let mut job_nodes: Vec<String> = Vec::new();
    let tabs = dashboards
        .iter()
        .enumerate()
        .map(|(index, dashboard)| {
            let place = Place {
                name: dashboard.name.clone(),
                first: index == 0,
            };
            let is_current = dashboard.name == current.name;
            let attention = if is_current {
                Attention::None
            } else {
                for block in &dashboard.blocks {
                    if block.kind == BlockKind::Job
                        && block.attention
                        && let Some(device) = &block.device
                    {
                        let trigger = node_trigger(device);
                        if !job_nodes.contains(&trigger) {
                            job_nodes.push(trigger);
                        }
                    }
                }
                dashboard_attention(hub, dashboard)
            };
            Tab {
                name: dashboard.name.clone(),
                link: place.link(""),
                current: is_current,
                attention,
            }
        })
        .collect();
    Some(TabRow {
        tabs,
        refresh: current.tabs(),
        trigger: tabs_trigger(&job_nodes),
    })
}

/// Nodes going offline and rules failing can mark any tab; a job's state only its own node.
pub(super) fn tabs_trigger(job_nodes: &[String]) -> String {
    let mut triggers = vec!["sse:presence".to_string(), "sse:timeline".to_string()];
    triggers.extend(job_nodes.iter().cloned());
    triggers.push("every 60s".into());
    triggers.join(", ")
}

/// Stays under the header while scrolling (`--header-height`, from `sticky-header.js`); a tab
/// swaps the dashboard in place and pushes its address, so back works.
#[component]
pub(super) async fn dashboard_tabs(tabs: TabRow) -> Result<impl View> {
    Ok(view! {
        <nav
            id="dashboard-tabs"
            aria-label="Dashboards"
            hx-get=(tabs.refresh)
            hx-trigger=(tabs.trigger)
            hx-swap="outerHTML"
            class="sticky z-30 -mx-4 mb-4 overflow-x-auto border-b border-border bg-background/95 px-4 py-2 backdrop-blur sm:-mx-6 sm:px-6"
            style="top: var(--header-height, 0px)"
        >
            <div class="inline-flex gap-0.5 rounded-lg border border-border p-0.5">
                for tab in tabs.tabs {
                    <a
                        href=(tab.link.page.clone())
                        hx-get=(tab.link.fragment)
                        hx-target="#dashboard"
                        hx-swap="outerHTML"
                        hx-push-url=(tab.link.page)
                        aria-current=(if tab.current { "page" } else { "false" })
                        class="relative inline-flex h-8 shrink-0 items-center rounded-md px-3 text-sm whitespace-nowrap text-muted-foreground hover:bg-foreground/5 hover:text-foreground aria-[current=page]:bg-foreground/10 aria-[current=page]:font-medium aria-[current=page]:text-foreground"
                    >
                        (tab.name)
                        if tab.attention == Attention::Warning {
                            <span class="absolute top-1 right-1 size-2 rounded-full bg-amber-500" title="A node it shows is offline" aria-label="needs attention"></span>
                        }
                        if tab.attention == Attention::Alert {
                            <span class="absolute top-1 right-1 size-2 rounded-full bg-destructive" title="Something failed" aria-label="something failed"></span>
                        }
                    </a>
                }
            </div>
        </nav>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_listen_to_presence_rules_and_jobs() {
        assert_eq!(
            tabs_trigger(&["sse:node-printer".to_string()]),
            "sse:presence, sse:timeline, sse:node-printer, every 60s"
        );
    }
}
