//! `/tools`: pages for looking into the hub and the network, one menu item for all of them.

use topcoat::{
    Result,
    router::page,
    view::{View, view},
};

use super::layout::layout;
use crate::components::card::{card, card_content};

pub(super) mod lab;
mod network;
mod traffic;

pub use network::{network_fragment, network_page};
pub use traffic::{traffic_events, traffic_map_fragment, traffic_page};

pub(super) const TOOLS: [(&str, &str, &str); 3] = [
    (
        "/network",
        "Network",
        "How each node is connected: Wi-Fi signal, address, time on the broker, reconnections.",
    ),
    (
        "/traffic",
        "Traffic",
        "MQTT messages the hub receives and sends, live.",
    ),
    ("/lab", "Lab", "Components and styles being tried out."),
];

#[page("/tools")]
pub async fn tools() -> Result<impl View> {
    Ok(view! {
        layout(
            title: "Oxihome tools",
            <div class="max-w-3xl">
                card(
                    card_content(
                        <ul class="divide-y divide-border">
                            for (href, name, description) in TOOLS {
                                <li>
                                    <a href=(href) class="block py-3 hover:bg-foreground/5">
                                        <p class="text-sm font-medium">(name)</p>
                                        <p class="text-sm text-muted-foreground">(description)</p>
                                    </a>
                                </li>
                            }
                        </ul>
                    )
                )
            </div>
        )
    })
}
