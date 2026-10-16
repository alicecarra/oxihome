//! A software node that follows RSS, Atom and JSON feeds: an event per new item and an unread count
//! per feed, kept across restarts.

mod fetch;
mod node;

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, bail};
use oxihome_node::mqtt::{self, Broker, Handle};
use oxihome_protocol::NAME_MAX_LENGTH;
use regex::RegexBuilder;
use serde::Deserialize;

use fetch::{Fetched, Fetcher};
use node::{ALL_FEEDS_ENTITY, Feed, RssNode};

const REPORT_INTERVAL: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_INTERVAL_MINUTES: u64 = 30;
const MINIMUM_INTERVAL_MINUTES: u64 = 15;
/// Between the first fetches of consecutive feeds, so they never all go out together.
const START_SPACING: Duration = Duration::from_secs(10);
/// Next to the configuration file.
const STATE_FILE: &str = "rss-state.json";

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    node: NodeConfig,
    feeds: Vec<FeedConfig>,
}

#[derive(Deserialize)]
struct MqttConfig {
    host: String,
    port: u16,
    /// The node's password on the broker; anonymous when missing.
    password: Option<String>,
    /// Outgoing messages waiting for the connection; `mqtt::DEFAULT_QUEUE_SIZE` when missing.
    queue_size: Option<usize>,
}

#[derive(Deserialize)]
struct NodeConfig {
    id: String,
}

#[derive(Deserialize)]
struct FeedConfig {
    entity: u8,
    name: String,
    url: String,
    /// Minutes.
    interval: Option<u64>,
    /// A regular expression on the title, ignoring case.
    filter: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: oxihome-rss <rss.toml>")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let config: Config = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;

    let mut entities = HashSet::new();
    let mut feeds = Vec::new();
    let mut schedules = Vec::new();
    for feed in config.feeds {
        if feed.entity <= ALL_FEEDS_ENTITY {
            bail!(
                "{path}: feed \"{}\": entity must be above {ALL_FEEDS_ENTITY}",
                feed.name
            );
        }
        if feed.name.len() > NAME_MAX_LENGTH {
            bail!(
                "{path}: feed \"{}\": name longer than {NAME_MAX_LENGTH} bytes",
                feed.name
            );
        }
        if !entities.insert(feed.entity) {
            bail!("{path}: entity {} used twice", feed.entity);
        }
        let interval = feed.interval.unwrap_or(DEFAULT_INTERVAL_MINUTES);
        if interval < MINIMUM_INTERVAL_MINUTES {
            bail!(
                "{path}: feed \"{}\": interval must be at least {MINIMUM_INTERVAL_MINUTES} minutes",
                feed.name
            );
        }
        let filter = feed
            .filter
            .map(|filter| RegexBuilder::new(&filter).case_insensitive(true).build())
            .transpose()
            .with_context(|| format!("{path}: feed \"{}\": filter", feed.name))?;
        schedules.push((
            feed.entity,
            feed.name.clone(),
            feed.url.clone(),
            Duration::from_secs(interval * 60),
        ));
        feeds.push(Feed {
            entity: feed.entity,
            name: feed.name,
            url: feed.url,
            filter,
        });
    }

    let state_path = Path::new(&path).with_file_name(STATE_FILE);
    let saved = node::read_state(&state_path);
    let handle = mqtt::spawn(
        RssNode::new(config.node.id, feeds, saved, Some(state_path)),
        Broker {
            host: config.mqtt.host,
            port: config.mqtt.port,
            password: config.mqtt.password,
            report_interval: REPORT_INTERVAL,
            queue_size: config.mqtt.queue_size.unwrap_or(mqtt::DEFAULT_QUEUE_SIZE),
        },
    );
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("oxihome-rss/", env!("CARGO_PKG_VERSION")))
        .build()?;
    for (index, (entity, name, url, interval)) in schedules.into_iter().enumerate() {
        let start = START_SPACING * index as u32;
        tokio::spawn(follow(
            entity,
            name,
            Fetcher::new(url),
            start,
            interval,
            client.clone(),
            handle.clone(),
        ));
    }
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn follow(
    entity: u8,
    name: String,
    mut fetcher: Fetcher,
    start: Duration,
    interval: Duration,
    client: reqwest::Client,
    handle: Handle<RssNode>,
) {
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + start, interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        let acted = match fetcher.fetch(&client).await {
            Ok(Fetched::Items(items)) => handle.act(move |node| node.fetched(entity, items)).await,
            Ok(Fetched::NotModified) => handle.act(move |node| node.not_modified(entity)).await,
            Err(error) => {
                println!("{name}: fetching failed: {error:#}");
                handle.act(move |node| node.failed(entity)).await
            }
        };
        if !acted {
            return;
        }
    }
}
