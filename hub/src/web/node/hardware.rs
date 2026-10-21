//! The hardware card: what `nodes.toml` lists for the board against what it mounts.

use std::sync::Arc;

use oxihome_protocol::hardware::{Driver, HardwareEntity};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{page, path_param},
    view::{View, component, view},
};

use super::DeviceId;
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::config::nodes::NameTarget;
use crate::hub::Hub;
use crate::node::hardware::{HardwareState, HardwareStatus};
use crate::web::format::entity_label;
use crate::web::overview::{self};

/// One entity of the hardware card: as `nodes.toml` lists it and as the board mounts it.
#[derive(Clone)]
pub(super) struct HardwareRow {
    id: u8,
    name: Option<String>,
    desired: Option<HardwareEntity>,
    reported: Option<HardwareEntity>,
}

pub(super) fn hardware_rows(hub: &Hub, device_id: &str) -> Vec<HardwareRow> {
    let status = hub.hardware_status(device_id);
    let entities = |hardware: &Option<oxihome_protocol::hardware::Hardware>| {
        hardware
            .as_ref()
            .map(|hardware| hardware.entities.clone())
            .unwrap_or_default()
    };
    let (desired, reported) = (entities(&status.desired), entities(&status.reported));
    let mut ids: Vec<u8> = desired
        .iter()
        .chain(&reported)
        .map(|entity| entity.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let snapshot = hub.snapshot();
    let find =
        |list: &[HardwareEntity], id: u8| list.iter().find(|entity| entity.id == id).cloned();
    ids.into_iter()
        .map(|id| HardwareRow {
            id,
            name: overview::name(
                &snapshot.nodes,
                &snapshot.registry,
                &NameTarget::Entity(device_id.to_string(), id),
            ),
            desired: find(&desired, id),
            reported: find(&reported, id),
        })
        .collect()
}

#[page("/nodes/{device_id}/hardware")]
pub async fn hardware_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    let device_id = path_param::<DeviceId>(cx).to_string();
    let rows = hardware_rows(hub, &device_id);
    let status = hub.hardware_status(&device_id);
    Ok(view! { hardware_section(device_id: device_id, rows: rows, status: status) })
}

/// Read only: the hardware comes from `nodes.toml`. Red where the board mounts something else.
#[component]
pub(super) async fn hardware_section(
    device_id: String,
    rows: Vec<HardwareRow>,
    status: HardwareStatus,
) -> Result<impl View> {
    let configured = status.desired.is_some();
    let (note, failed) = match &status.state {
        HardwareState::NotConfigured if status.reported.is_some() => (
            "Not set in nodes.toml: this is what the board mounts.".to_string(),
            false,
        ),
        HardwareState::NotConfigured => ("Not set in nodes.toml.".to_string(), false),
        HardwareState::Offline => ("The node is offline.".to_string(), false),
        HardwareState::Unsupported => (
            "This node's software does not take a hardware configuration.".to_string(),
            false,
        ),
        HardwareState::Invalid(problems) => (format!("Not sent: {}", problems.join("; ")), true),
        HardwareState::Matches => ("The board mounts this hardware.".to_string(), false),
        HardwareState::Sending => (
            "Sending; the board restarts to mount it…".to_string(),
            false,
        ),
        HardwareState::Refused(reason) => (format!("Not applied: {reason}"), true),
        HardwareState::Differs => (
            "The board mounts something else; it is sent at the next chance.".to_string(),
            true,
        ),
    };
    Ok(view! {
        <div
            id="node-hardware"
            hx-get=(format!("/nodes/{device_id}/hardware"))
            hx-trigger="sse:nodes, refresh"
            hx-swap="outerHTML"
        >
            card(
                card_header(
                    card_title("Hardware")
                    card_description("What is wired to each pin, from the node's hardware in nodes.toml.")
                )
                card_content(
                    <p class=(if failed { "mb-3 text-sm text-destructive" } else { "mb-3 text-sm text-muted-foreground" })>(note)</p>
                    if !status.reports.is_empty() {
                        <div class="mb-3 text-sm text-destructive">
                            <p class="font-medium">"The board reports:"</p>
                            <ul class="list-disc pl-5">
                                for report in status.reports {
                                    <li>(report)</li>
                                }
                            </ul>
                        </div>
                    }
                    if !rows.is_empty() {
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="text-left text-xs text-muted-foreground">
                                    <th class="py-1 pr-3 font-normal">"Entity"</th>
                                    <th class="py-1 pr-3 font-normal">"Driver"</th>
                                    <th class="py-1 font-normal">"Pin"</th>
                                </tr>
                            </thead>
                            <tbody class="divide-y divide-border">
                                for row in rows {
                                    hardware_row(row: row, configured: configured)
                                }
                            </tbody>
                        </table>
                    }
                )
            )
        </div>
    })
}

#[component]
async fn hardware_row(row: HardwareRow, configured: bool) -> Result<impl View> {
    let shown = if configured {
        &row.desired
    } else {
        &row.reported
    };
    let differs = configured && row.desired != row.reported;
    let board = match (&row.reported, differs) {
        (_, false) => None,
        (None, true) => Some("not on the board".to_string()),
        (Some(reported), true) => Some(format!(
            "the board mounts {} on GPIO {}",
            driver_label(&reported.driver),
            reported.pin
        )),
    };
    let missing = shown.is_none();
    let entity = format!("{} · {}", row.id, entity_label(row.id, &row.name));
    let driver = shown.as_ref().map_or_else(
        || "not in nodes.toml".to_string(),
        |entity| driver_label(&entity.driver),
    );
    let pin = shown
        .as_ref()
        .map_or_else(String::new, |entity| format!("GPIO {}", entity.pin));
    Ok(view! {
        <tr class=(if missing { "align-top text-muted-foreground" } else { "align-top" })>
            <td class="py-2 pr-3">(entity)</td>
            <td class="py-2 pr-3">
                (driver)
                if let Some(board) = board {
                    <p class="text-xs text-destructive">(board)</p>
                }
            </td>
            <td class="py-2">(pin)</td>
        </tr>
    })
}

fn driver_label(driver: &Driver) -> String {
    match driver {
        Driver::Output { inverted: false } => "output".into(),
        Driver::Output { inverted: true } => "output, inverted".into(),
        Driver::Button { pull_up, toggles } => {
            let mut label = "button".to_string();
            if !pull_up {
                label.push_str(", no pull-up");
            }
            if let Some(output) = toggles {
                label.push_str(&format!(", toggles entity {output}"));
            }
            label
        }
        Driver::Dht11 {
            read_interval_seconds,
        } => format!("DHT11, read every {read_interval_seconds} s"),
        Driver::Buzzer => "buzzer".into(),
        Driver::IrReceiver => "IR receiver".into(),
        Driver::IrTransmitter => "IR transmitter".into(),
        Driver::Potentiometer { minimum_change } => {
            format!("potentiometer, changes of {minimum_change}% or more")
        }
        Driver::Aht20Bmp280 {
            clock_pin,
            read_interval_seconds,
        } => format!(
            "AHT20 + BMP280, SDA on this pin, SCL on {clock_pin}, read every {read_interval_seconds} s"
        ),
    }
}
