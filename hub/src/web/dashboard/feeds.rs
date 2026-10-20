//! The dashboard's `feeds` block: the items of a node laid out like the rss node
//! (`rss/src/node.rs`), every feed in one list, newest first.

use std::collections::BTreeMap;
use std::time::Duration;

use jiff::Timestamp;
use oxihome_protocol::Value;
use topcoat::{
    Result,
    view::{View, attributes, component, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::hub::Hub;
use crate::web::format::{ago_label, elapsed};
use crate::web::overview;

// The rss node's entities and channels; the protocol has no lists, so the block knows them.
const ALL_FEEDS_ENTITY: u8 = 1;
const MARK_ALL_READ_CHANNEL: u8 = 0;
const TOTAL_UNREAD_CHANNEL: u8 = 1;
const TITLE_CHANNEL: u8 = 0;
const LINK_CHANNEL: u8 = 1;
const UNREAD_CHANNEL: u8 = 2;
const NEW_ITEM_CHANNEL: u8 = 5;
const SEPARATOR: &str = " — ";

/// Enough for the newest items of every feed and their unread marks.
const EVENTS_READ: usize = 500;

pub struct FeedList {
    /// The node's name, or its id.
    pub node_label: String,
    pub total_unread: i32,
    /// Newest first; items without a time go last.
    pub items: Vec<FeedItem>,
}

#[derive(Clone)]
pub struct FeedItem {
    pub feed: String,
    pub title: String,
    pub link: Option<String>,
    /// `None` for the newest item known only from the feed's state, before any event.
    pub ago: Option<Duration>,
    /// A feed's newest `unread` items count as unread: the node keeps a count, not which ones.
    pub unread: bool,
}

/// `None` when the node never sent a `Hello`.
pub async fn feed_list(hub: &Hub, device_id: &str) -> Option<FeedList> {
    let node = overview::node(hub, device_id)?;
    let value = |entity: u8, channel: u8| {
        node.channels
            .iter()
            .find(|overview| overview.entity == entity && overview.channel == channel)
            .and_then(|overview| overview.value.clone())
    };
    let int = |entity, channel| match value(entity, channel) {
        Some(Value::Int(number)) => number,
        _ => 0,
    };
    let text = |entity, channel| match value(entity, channel) {
        Some(Value::Text(text)) if !text.is_empty() => Some(text),
        _ => None,
    };

    let mut names: BTreeMap<u8, String> = BTreeMap::new();
    for channel in &node.channels {
        if channel.entity != ALL_FEEDS_ENTITY {
            names.entry(channel.entity).or_insert_with(|| {
                channel
                    .entity_name
                    .clone()
                    .unwrap_or_else(|| format!("Feed {}", channel.entity))
            });
        }
    }

    let events = hub
        .events_of(device_id, EVENTS_READ)
        .await
        .unwrap_or_else(|error| {
            println!("dashboard: events of {device_id}: {error:#}");
            Vec::new()
        });
    let now = Timestamp::now();
    let mut by_feed: BTreeMap<u8, Vec<FeedItem>> = BTreeMap::new();
    for (event, at) in events {
        let (Some(name), Some(Value::Text(data))) = (names.get(&event.entity), &event.data) else {
            continue;
        };
        if event.channel != NEW_ITEM_CHANNEL {
            continue;
        }
        let (title, link) = match data.rsplit_once(SEPARATOR) {
            Some((title, link)) if link.starts_with("http") => (title, Some(link.to_string())),
            _ => (data.as_str(), None),
        };
        by_feed.entry(event.entity).or_default().push(FeedItem {
            feed: name.clone(),
            title: title.to_string(),
            link,
            ago: Some(elapsed(at, now)),
            unread: false,
        });
    }

    let mut items = Vec::new();
    for (entity, name) in names {
        let mut feed_items = by_feed.remove(&entity).unwrap_or_default();
        if feed_items.is_empty()
            && let Some(title) = text(entity, TITLE_CHANNEL)
        {
            feed_items.push(FeedItem {
                feed: name,
                title,
                link: text(entity, LINK_CHANNEL),
                ago: None,
                unread: false,
            });
        }
        let unread = usize::try_from(int(entity, UNREAD_CHANNEL)).unwrap_or(0);
        for item in feed_items.iter_mut().take(unread) {
            item.unread = true;
        }
        items.extend(feed_items);
    }
    items.sort_by_key(|item| item.ago.unwrap_or(Duration::MAX));
    Some(FeedList {
        node_label: node.name.unwrap_or_else(|| device_id.to_string()),
        total_unread: int(ALL_FEEDS_ENTITY, TOTAL_UNREAD_CHANNEL),
        items,
    })
}

/// Unread items only, unless `show_read`; `toggle_query` is the dashboard query that flips it.
#[component]
pub async fn feeds_block(
    list: Option<FeedList>,
    device_id: String,
    limit: usize,
    show_read: bool,
    toggle_query: String,
) -> Result<impl View> {
    let shown = list.map(|list| {
        let read_count = list.items.iter().filter(|item| !item.unread).count();
        let items: Vec<FeedItem> = list
            .items
            .into_iter()
            .filter(|item| show_read || item.unread)
            .take(limit)
            .collect();
        (list.total_unread, read_count, items)
    });
    Ok(view! {
        match shown {
            None => <p class="text-sm text-muted-foreground">(format!("{device_id} has not said hello yet."))</p>,
            Some((total_unread, read_count, items)) => feeds_content(
                device_id: device_id,
                total_unread: total_unread,
                read_count: read_count,
                items: items,
                show_read: show_read,
                toggle_query: toggle_query
            ),
        }
    })
}

#[component]
async fn feeds_content(
    device_id: String,
    total_unread: i32,
    read_count: usize,
    items: Vec<FeedItem>,
    show_read: bool,
    toggle_query: String,
) -> Result<impl View> {
    Ok(view! {
        <div class="space-y-3">
            <div class="flex flex-wrap items-center justify-between gap-2">
                button(
                    variant: ButtonVariant::Ghost,
                    size: ButtonSize::Sm,
                    attrs: attributes! {
                        type="button"
                        hx-get=(format!("/dashboard/fragment?{toggle_query}"))
                        hx-target="#dashboard"
                        hx-swap="outerHTML"
                        hx-push-url=(format!("/?{toggle_query}"))
                    },
                    (if show_read { "Hide read".to_string() } else { format!("Show read ({read_count})") })
                )
                <form hx-post="/invoke" hx-target="#status">
                    <input type="hidden" name="device_id" value=(device_id)>
                    <input type="hidden" name="entity" value=(ALL_FEEDS_ENTITY.to_string())>
                    <input type="hidden" name="channel" value=(MARK_ALL_READ_CHANNEL.to_string())>
                    if total_unread == 0 {
                        button(
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" disabled="" },
                            "Mark all read"
                        )
                    } else {
                        button(
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" },
                            (format!("Mark all read ({total_unread})"))
                        )
                    }
                </form>
            </div>
            if items.is_empty() {
                <p class="text-sm text-muted-foreground">
                    (if show_read { "Nothing yet: items appear here as the feeds publish." } else { "All caught up." })
                </p>
            }
            <ul class="divide-y divide-border">
                for item in items {
                    <li class="flex items-start gap-3 py-2.5 first:pt-0 last:pb-0">
                        <span
                            class=(if item.unread { "mt-1.5 size-2 shrink-0 rounded-full bg-primary" } else { "mt-1.5 size-2 shrink-0" })
                            title=(if item.unread { "unread" } else { "" })
                        ></span>
                        <div class="min-w-0 flex-1">
                            item_title(item: item.clone())
                            <p class="text-xs text-muted-foreground">(item.feed.clone())</p>
                        </div>
                        if let Some(ago) = item.ago {
                            <span class="shrink-0 text-xs text-muted-foreground tabular-nums">(ago_label(ago))</span>
                        }
                    </li>
                }
            </ul>
        </div>
    })
}

/// Opens in a new tab; long titles wrap up to two lines.
#[component]
async fn item_title(item: FeedItem) -> Result<impl View> {
    let class = if item.unread {
        "line-clamp-2 text-sm font-medium"
    } else {
        "line-clamp-2 text-sm text-muted-foreground"
    };
    Ok(view! {
        match item.link {
            Some(link) => {
                <a
                    href=(link.clone())
                    target="_blank"
                    rel="noopener noreferrer"
                    title=(link)
                    class=(format!("{class} hover:underline"))
                >
                    (item.title)
                </a>
            },
            None => <p class=(class)>(item.title)</p>,
        }
    })
}
