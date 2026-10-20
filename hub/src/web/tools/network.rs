//! `/network`: how each node is connected, to find the board with bad Wi-Fi.

use std::sync::Arc;
use std::time::Instant;

use jiff::Timestamp;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    view::{View, component, view},
};

use crate::components::badge::{BadgeVariant, badge};
use crate::config::nodes::NameTarget;
use crate::hub::Hub;
use crate::node::network::Connection;
use crate::web::format::{ago_label, duration_label, elapsed};
use crate::web::layout::layout;
use crate::web::nodes::presence_badge;
use crate::web::overview::{self, Presence};

/// Usual Wi-Fi rules of thumb: from -67 dBm the link is solid, below -75 dBm it drops.
const GOOD_SIGNAL: i8 = -67;
const FAIR_SIGNAL: i8 = -75;

const SPARKLINE_WIDTH: usize = 180;
const SPARKLINE_HEIGHT: usize = 24;
/// The range the sparkline draws, in dBm.
const SPARKLINE_FLOOR: f64 = -95.0;
const SPARKLINE_CEILING: f64 = -35.0;

#[derive(Clone)]
struct NetworkRow {
    title: String,
    link: String,
    presence: Presence,
    rssi: Option<i8>,
    weakest: Option<i8>,
    sparkline: String,
    ip: Option<String>,
    connected: String,
    reconnections: usize,
    reboots: usize,
    last_message: String,
    dropped: u32,
}

#[page("/network")]
pub async fn network_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! {
        layout(
            title: "Oxihome network",
            <div class="space-y-4">
                <a href="/tools" class="text-sm text-muted-foreground hover:underline">"← Tools"</a>
                <div>
                    <h1 class="text-2xl font-semibold">"Network"</h1>
                    <p class="text-sm text-muted-foreground">"Weakest signal first. Drops and the signal chart cover the last 24 hours, or since the hub started if that is more recent."</p>
                </div>
                <div id="network" hx-get="/network/fragment" hx-trigger="sse:nodes, every 30s">
                    network_table(rows: rows(hub))
                </div>
            </div>
        )
    })
}

#[page("/network/fragment")]
pub async fn network_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { network_table(rows: rows(hub)) })
}

fn rows(hub: &Hub) -> Vec<NetworkRow> {
    let now = Instant::now();
    let timestamp = Timestamp::now();
    let traffic = hub.traffic().summary();
    let snapshot = hub.snapshot();
    let mut rows: Vec<NetworkRow> = snapshot
        .registry
        .nodes()
        .into_iter()
        .map(|(device_id, node)| {
            let signal = node.network.signal(now);
            let title = overview::name(
                &snapshot.nodes,
                &snapshot.registry,
                &NameTarget::Node(device_id.clone()),
            )
            .unwrap_or_else(|| device_id.clone());
            let presence = if node.online {
                Presence::Online
            } else {
                Presence::Offline
            };
            let connected = match node.network.connection {
                Some(connection) => connection_label(connection, now),
                None => format!(
                    "last seen {}",
                    ago_label(elapsed(node.last_seen_at, timestamp))
                ),
            };
            let summary = traffic
                .iter()
                .find(|summary| summary.device_id.as_ref() == Some(device_id));
            NetworkRow {
                title,
                link: format!("/nodes/{device_id}"),
                presence,
                rssi: node.rssi,
                weakest: signal.iter().copied().min(),
                sparkline: sparkline_path(&signal),
                ip: node.ip.clone(),
                connected,
                reconnections: node.network.reconnections(now),
                reboots: node.network.reboots(now),
                last_message: ago_label(elapsed(node.last_seen_at, timestamp)),
                dropped: summary.map_or(0, |summary| summary.dropped),
            }
        })
        .collect();
    // Nodes without Wi-Fi, like software nodes, last.
    rows.sort_by_key(|row| (row.rssi.is_none(), row.rssi, row.title.to_lowercase()));
    rows
}

fn connection_label(connection: Connection, now: Instant) -> String {
    let duration = duration_label(now.saturating_duration_since(connection.since));
    if connection.exact {
        duration
    } else {
        format!("over {duration}")
    }
}

/// One point per bucket of samples, averaged, so a day of heartbeats stays a short path.
fn sparkline_path(signal: &[i8]) -> String {
    if signal.len() < 2 {
        return String::new();
    }
    let buckets = signal.len().min(SPARKLINE_WIDTH / 2);
    let points: Vec<f64> = (0..buckets)
        .map(|bucket| {
            let start = bucket * signal.len() / buckets;
            let end = ((bucket + 1) * signal.len() / buckets).max(start + 1);
            let samples = &signal[start..end];
            samples.iter().map(|rssi| f64::from(*rssi)).sum::<f64>() / samples.len() as f64
        })
        .collect();
    let step = SPARKLINE_WIDTH as f64 / (points.len() - 1) as f64;
    points
        .iter()
        .enumerate()
        .map(|(index, rssi)| {
            let level =
                ((rssi - SPARKLINE_FLOOR) / (SPARKLINE_CEILING - SPARKLINE_FLOOR)).clamp(0.0, 1.0);
            let x = index as f64 * step;
            let y = (1.0 - level) * SPARKLINE_HEIGHT as f64;
            let command = if index == 0 { 'M' } else { 'L' };
            format!("{command}{x:.1} {y:.1}")
        })
        .collect()
}

#[component]
async fn network_table(rows: Vec<NetworkRow>) -> Result<impl View> {
    Ok(view! {
        if rows.is_empty() {
            <p class="text-muted-foreground">"No nodes yet."</p>
        } else {
            <div class="overflow-x-auto rounded-xl border border-border bg-card text-card-foreground shadow-sm">
                <table class="w-full text-sm">
                    <thead class="text-left text-muted-foreground">
                        <tr>
                            <th class="px-4 py-3 font-medium">"Node"</th>
                            <th class="px-4 py-3 font-medium">"Signal"</th>
                            <th class="hidden px-4 py-3 font-medium md:table-cell">"Last 24 h"</th>
                            <th class="hidden px-4 py-3 font-medium sm:table-cell">"Address"</th>
                            <th class="px-4 py-3 font-medium">"Connected"</th>
                            <th class="hidden px-4 py-3 font-medium sm:table-cell">"Drops"</th>
                            <th class="hidden px-4 py-3 font-medium lg:table-cell">"Last message"</th>
                        </tr>
                    </thead>
                    <tbody>
                        for row in rows {
                            network_line(row: row)
                        }
                    </tbody>
                </table>
            </div>
        }
    })
}

#[component]
async fn network_line(row: NetworkRow) -> Result<impl View> {
    let weakest = row
        .weakest
        .map(|weakest| format!("weakest {weakest} dBm"))
        .unwrap_or_default();
    let drops = match (row.reconnections, row.reboots) {
        (0, 0) => "—".to_string(),
        (reconnections, 0) => format!("{reconnections} reconnections"),
        (0, reboots) => format!("{reboots} reboots"),
        (reconnections, reboots) => format!("{reconnections} reconnections, {reboots} reboots"),
    };
    Ok(view! {
        <tr class="border-t border-border">
            <td class="px-4 py-3">
                <div class="flex flex-wrap items-center gap-2">
                    <a href=(row.link) class="font-medium hover:underline">(row.title)</a>
                    presence_badge(presence: row.presence)
                    if row.dropped > 0 {
                        badge(variant: BadgeVariant::Destructive, (format!("{} dropped", row.dropped)))
                    }
                </div>
            </td>
            <td class="px-4 py-3">
                if let Some(rssi) = row.rssi {
                    <div class="flex flex-wrap items-center gap-2">
                        <span class="tabular-nums">(format!("{rssi} dBm"))</span>
                        signal_badge(rssi: rssi)
                    </div>
                    <p class="text-xs text-muted-foreground tabular-nums">(weakest)</p>
                } else {
                    <span class="text-muted-foreground">"—"</span>
                }
            </td>
            <td class="hidden px-4 py-3 md:table-cell">
                <svg
                    viewBox=(format!("0 0 {SPARKLINE_WIDTH} {SPARKLINE_HEIGHT}"))
                    class="h-6 w-[180px] overflow-visible text-foreground/60"
                    aria-hidden="true"
                >
                    <path d=(row.sparkline) fill="none" stroke="currentColor" stroke-width="1.5" vector-effect="non-scaling-stroke"></path>
                </svg>
            </td>
            <td class="hidden px-4 py-3 font-mono text-xs sm:table-cell">(row.ip.unwrap_or_else(|| "—".into()))</td>
            <td class="px-4 py-3 tabular-nums">(row.connected)</td>
            <td class="hidden px-4 py-3 tabular-nums sm:table-cell">(drops)</td>
            <td class="hidden px-4 py-3 tabular-nums lg:table-cell">(row.last_message)</td>
        </tr>
    })
}

#[component]
async fn signal_badge(rssi: i8) -> Result<impl View> {
    Ok(view! {
        if rssi >= GOOD_SIGNAL {
            badge(variant: BadgeVariant::Success, "good")
        } else if rssi >= FAIR_SIGNAL {
            badge(variant: BadgeVariant::Secondary, "fair")
        } else {
            badge(variant: BadgeVariant::Destructive, "poor")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_needs_two_samples() {
        assert_eq!(sparkline_path(&[]), "");
        assert_eq!(sparkline_path(&[-60]), "");
    }

    #[test]
    fn sparkline_spans_the_width_and_the_range() {
        assert_eq!(sparkline_path(&[-95, -35]), "M0.0 24.0L180.0 0.0");
        assert_eq!(sparkline_path(&[-100, -20]), "M0.0 24.0L180.0 0.0");
    }

    #[test]
    fn long_signal_is_averaged_into_buckets() {
        let signal = vec![-65; 2880];
        let path = sparkline_path(&signal);
        assert_eq!(path.matches('L').count(), SPARKLINE_WIDTH / 2 - 1);
        assert!(path.starts_with("M0.0 12.0"));
    }
}
