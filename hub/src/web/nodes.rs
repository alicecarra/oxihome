use std::sync::Arc;
use std::time::Duration;

use oxihome_protocol::{PROTOCOL_VERSION, Value, management};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::page,
    view::{Child, View, attributes, component, view},
};

use super::format::{ago_label, bool_label, css_color, duration_label, entity_label, value_label};
use super::icons;
use super::layout::layout;
use super::overview::{self, ChannelOverview, Invocation, NodeOverview, Presence};
use crate::components::badge::{BadgeVariant, badge};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::components::switch::switch;
use crate::config::nodes::ConfiguredEntity;
use crate::hub::Hub;

#[page("/nodes")]
pub async fn nodes_page(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);

    Ok(view! {
        layout(
            title: "Oxihome nodes",
            <div
                id="node-filter"
                data-filter-items="#nodes"
                data-filter-attribute="presence"
                data-filter-empty="nodes-filtered-out"
                class="mb-4 flex flex-wrap gap-2"
            >
                for (presence, label) in [("online", "Online"), ("offline", "Offline"), ("not-seen", "Not seen")] {
                    button(
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        attrs: attributes! {
                            type="button"
                            data-filter=(presence)
                            aria-pressed="true"
                            class="aria-pressed:border-transparent aria-pressed:bg-primary aria-pressed:text-primary-foreground aria-pressed:hover:bg-primary/90"
                        },
                        (label)
                        <span data-count="" class="tabular-nums opacity-70"></span>
                    )
                }
            </div>
            <div id="nodes" hx-get="/nodes/fragment" hx-trigger="sse:nodes, every 30s, refresh">
                node_list(nodes: overview::nodes(hub))
            </div>
            <p id="nodes-filtered-out" class="text-muted-foreground" hidden="">"No nodes match the filter."</p>
            <script src="/static/filter.js"></script>
            <script src="/static/node-sort.js"></script>
        )
    })
}

#[page("/nodes/fragment")]
pub async fn nodes_fragment(cx: &Cx) -> Result<impl View> {
    let hub: &Arc<Hub> = app_context(cx);
    Ok(view! { node_list(nodes: overview::nodes(hub)) })
}

/// One row per node; values and controls live on each node's page and on the dashboard.
#[component]
pub async fn node_list(nodes: Vec<NodeOverview>) -> Result<impl View> {
    Ok(view! {
        if nodes.is_empty() {
            <p class="text-muted-foreground">"No nodes yet."</p>
        } else {
            // A card's look without its vertical padding, so rows reach the edges.
            <div class="overflow-hidden rounded-xl border border-border bg-card text-card-foreground shadow-sm">
                <table class="w-full text-sm">
                    <thead class="text-left text-muted-foreground">
                        <tr>
                            sortable_header(column: "node", class: "", "Node")
                            sortable_header(column: "status", class: "", "Status")
                            sortable_header(column: "channels", class: "hidden sm:table-cell", "Channels")
                            sortable_header(column: "software", class: "hidden sm:table-cell", "Software")
                            sortable_header(column: "uptime", class: "", "Uptime")
                            sortable_header(column: "signal", class: "hidden sm:table-cell", "Signal")
                        </tr>
                    </thead>
                    <tbody>
                        for node in nodes {
                            node_row(node: node)
                        }
                    </tbody>
                </table>
            </div>
        }
    })
}

/// A column header that `node-sort.js` sorts by when clicked.
#[component]
async fn sortable_header(
    column: &'static str,
    class: &'static str,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <th class=(format!("group px-4 py-1.5 font-medium {class}")) data-sort-column=(column) aria-sort="none">
            <button type="button" class="-mx-2 inline-flex items-center gap-1 rounded-md px-2 py-1.5 hover:bg-foreground/5 hover:text-foreground">
                (child)
                <span class="group-aria-[sort=ascending]:hidden group-aria-[sort=descending]:hidden opacity-50">icon(data: icons::SORT, size: 14)</span>
                <span class="hidden group-aria-[sort=ascending]:inline">icon(data: icons::SORT_ASCENDING, size: 14)</span>
                <span class="hidden group-aria-[sort=descending]:inline">icon(data: icons::SORT_DESCENDING, size: 14)</span>
            </button>
        </th>
    })
}

#[component]
async fn node_row(node: NodeOverview) -> Result<impl View> {
    let title = node.name.clone().unwrap_or_else(|| node.device_id.clone());
    let (channels_count, channels_label, channels_title) = if node.presence == Presence::NotSeen {
        let configured: usize = node
            .configured_entities
            .iter()
            .map(|entity| entity.channels.len())
            .sum();
        let names: Vec<String> = node
            .configured_entities
            .iter()
            .map(configured_entity_label)
            .collect();
        (
            configured,
            format!("{configured} configured"),
            names.join("; "),
        )
    } else {
        // The management entity holds node parameters, not things in the home.
        let declared = node
            .channels
            .iter()
            .filter(|channel| channel.entity != management::ENTITY)
            .count();
        (declared, declared.to_string(), String::new())
    };
    // The last uptime of an offline node is stale; when it went quiet says more.
    let uptime = match (node.presence, node.last_seen) {
        (Presence::Offline, Some(ago)) => Some(format!("last seen {}", ago_label(ago))),
        (Presence::Online, _) => node
            .uptime_seconds
            .map(|uptime| duration_label(Duration::from_secs(uptime.into()))),
        _ => None,
    };
    let signal = node.rssi.map(|rssi| format!("{rssi} dBm"));
    // Online nodes by uptime, then offline ones by how recently they were seen.
    let uptime_order = match (node.presence, node.last_seen) {
        (Presence::Offline, Some(ago)) => Some(-(ago.as_secs() as i64)),
        (Presence::Online, _) => node.uptime_seconds.map(i64::from),
        _ => None,
    };
    Ok(view! {
        <tr
            class="border-t border-border"
            data-presence=(presence_key(node.presence))
            data-sort-node=(title.to_lowercase())
            data-sort-status=((node.presence as u8).to_string())
            data-sort-channels=(channels_count.to_string())
            data-sort-software=(node.software.clone().unwrap_or_default())
            data-sort-uptime=(uptime_order.map(|order| order.to_string()).unwrap_or_default())
            data-sort-signal=(node.rssi.map(|rssi| rssi.to_string()).unwrap_or_default())
        >
            <td class="px-4 py-3">
                <a href=(format!("/nodes/{}", node.device_id)) class="font-medium hover:underline">(title)</a>
                <span class="ml-2">protocol_badge(version: node.protocol_version)</span>
                if node.name.is_some() {
                    <p class="text-xs text-muted-foreground">(node.device_id.clone())</p>
                }
            </td>
            <td class="px-4 py-3">presence_badge(presence: node.presence)</td>
            <td class="hidden px-4 py-3 tabular-nums sm:table-cell" title=(channels_title)>(channels_label)</td>
            <td class="hidden px-4 py-3 sm:table-cell">(node.software.unwrap_or_else(|| "—".into()))</td>
            <td class="px-4 py-3 tabular-nums">(uptime.unwrap_or_else(|| "—".into()))</td>
            <td class="hidden px-4 py-3 tabular-nums sm:table-cell">(signal.unwrap_or_else(|| "—".into()))</td>
        </tr>
    })
}

/// For a node on an older protocol than the hub's, which the hub still reads (see
/// `oxihome_protocol::legacy`): it should be updated.
#[component]
pub(super) async fn protocol_badge(version: Option<u8>) -> Result<impl View> {
    Ok(view! {
        if let Some(version) = version.filter(|version| *version < PROTOCOL_VERSION) {
            badge(
                variant: BadgeVariant::Outline,
                attrs: attributes! { title=(format!("Speaks protocol {version}, the hub {PROTOCOL_VERSION}: update it")) },
                (format!("protocol {version}"))
            )
        }
    })
}

#[component]
pub(super) async fn presence_badge(presence: Presence) -> Result<impl View> {
    Ok(view! {
        match presence {
            Presence::Online => badge(variant: BadgeVariant::Success, <span class="size-1.5 rounded-full bg-current"></span> "online"),
            Presence::Offline => badge(variant: BadgeVariant::Muted, <span class="size-1.5 rounded-full bg-current"></span> "offline"),
            Presence::NotSeen => badge(variant: BadgeVariant::Outline, "not seen"),
        }
    })
}

/// What the node filter on `/nodes` matches, in `data-presence`.
pub(super) fn presence_key(presence: Presence) -> &'static str {
    match presence {
        Presence::Online => "online",
        Presence::Offline => "offline",
        Presence::NotSeen => "not-seen",
    }
}

/// A coloured dot before a short state text: green when active, grey otherwise.
#[component]
async fn status_dot(active: bool, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <p class="inline-flex items-center gap-1.5 text-sm text-muted-foreground">
            <span class=(if active { "size-2 rounded-full bg-success" } else { "size-2 rounded-full bg-muted-foreground/40" })></span>
            (child)
        </p>
    })
}

/// A dot filled with the color, before its text.
#[component]
pub(super) async fn color_dot(color: String, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <span class="inline-flex items-center gap-1.5">
            <span class="size-3 shrink-0 rounded-full border border-border" style=(format!("background-color: {color}"))></span>
            (child)
        </span>
    })
}

/// The value, the on/off dot or the last event; nothing for actions, whose controls say it all.
#[component]
pub(super) async fn channel_value(channel: ChannelOverview) -> Result<impl View> {
    Ok(view! {
        match &channel.value {
            Some(Value::Bool(active)) => status_dot(active: *active, (bool_label(*active, channel.switchable))),
            Some(value) if css_color(value).is_some() => <p class="text-sm text-muted-foreground">
                color_dot(color: css_color(value).unwrap_or_default(), (value_label(value, None)))
            </p>,
            _ if channel.invocation.is_some() => {}
            _ => <p class="text-sm text-muted-foreground">(channel_state(&channel))</p>,
        }
    })
}

/// The switch of a writable `Bool`, or the buttons or text box of a `Trigger`.
#[component]
pub(super) async fn channel_controls(
    device_id: String,
    channel: ChannelOverview,
    label: String,
) -> Result<impl View> {
    let on = channel.value == Some(Value::Bool(true));
    Ok(view! {
        if let Some(invocation) = channel.invocation.clone() {
            <form
                class="flex flex-wrap justify-end gap-2"
                hx-post="/invoke"
                hx-target="#status"
            >
                <input type="hidden" name="device_id" value=(device_id.clone())>
                <input type="hidden" name="entity" value=(channel.entity.to_string())>
                <input type="hidden" name="channel" value=(channel.channel.to_string())>
                match invocation {
                    Invocation::Options(options) => for option in options {
                        button(
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" name="value" value=(option.clone()) },
                            (option)
                        )
                    },
                    Invocation::Text { max_length } => {
                        // Kept while the node list refreshes, so typing is not lost.
                        input(attrs: attributes! {
                            id=(format!("text-{device_id}-{}-{}", channel.entity, channel.channel))
                            hx-preserve="true"
                            name="value"
                            required=""
                            maxlength=(max_length.to_string())
                            aria-label=(label.clone())
                            class="h-8 w-48"
                        })
                        button(
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" },
                            "Send"
                        )
                    },
                    Invocation::Plain => button(
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        attrs: attributes! { type="submit" },
                        "Run"
                    ),
                }
            </form>
        }
        if channel.switchable {
            <form
                hx-post="/set"
                hx-trigger="change"
                hx-target="#status"
                // Refreshes the list or page this switch is in, sooner than its next update.
                ("hx-on--after-request")="htmx.trigger(this.closest('[hx-get]'), 'refresh')"
            >
                <input type="hidden" name="device_id" value=(device_id.clone())>
                <input type="hidden" name="entity" value=(channel.entity.to_string())>
                <input type="hidden" name="channel" value=(channel.channel.to_string())>
                switch(attrs: attributes! {
                    name="value"
                    value="true"
                    aria-label=(label.clone())
                    if on {
                        checked=""
                    }
                })
            </form>
        }
    })
}

/// `LED: Power`, or the entity alone when no channel is named.
fn configured_entity_label(entity: &ConfiguredEntity) -> String {
    let label = entity_label(entity.id, &entity.name);
    if entity.channels.is_empty() {
        return label;
    }
    let channels: Vec<&str> = entity
        .channels
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    format!("{label}: {}", channels.join(", "))
}

fn channel_state(channel: &ChannelOverview) -> String {
    if !channel.has_value {
        return match &channel.last_event {
            Some((kind, Some(data), ago)) => {
                format!("last event: {kind:?} {data}, {}", ago_label(*ago))
            }
            Some((kind, None, ago)) => format!("last event: {kind:?}, {}", ago_label(*ago)),
            None => "no events yet".into(),
        };
    }
    match &channel.value {
        Some(value) => value_label(value, channel.unit),
        None => "unknown".into(),
    }
}
