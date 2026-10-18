//! A software node that follows RSS, Atom and JSON feeds: an event per new item and an unread count
//! per feed, kept across restarts.

mod fetch;
mod node;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use oxihome_node::config::{BrokerConfig, read_config};
use oxihome_node::mqtt::{self, Handle};
use oxihome_protocol::NAME_MAX_LENGTH;
use regex::RegexBuilder;
use serde::Deserialize;

use fetch::{Fetched, Fetcher};
use node::{ALL_FEEDS_ENTITY, Feed, RssNode};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_INTERVAL_MINUTES: u64 = 30;
const MINIMUM_INTERVAL_MINUTES: u64 = 15;
/// Between the first fetches of consecutive feeds, so they never all go out together.
const START_SPACING: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Config {
    mqtt: BrokerConfig,
    node: NodeConfig,
    feeds: Vec<FeedConfig>,
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
    /// A regular expression on the title and the link, ignoring case: matching items are skipped.
    exclude: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (path, config): (String, Config) = read_config("oxihome-rss <rss.toml>")?;

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
        let pattern = |pattern: Option<String>, field: &str| {
            pattern
                .map(|pattern| RegexBuilder::new(&pattern).case_insensitive(true).build())
                .transpose()
                .with_context(|| format!("{path}: feed \"{}\": {field}", feed.name))
        };
        let filter = pattern(feed.filter, "filter")?;
        let exclude = pattern(feed.exclude, "exclude")?;
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
            exclude,
        });
    }

    let state_path = state_path(Path::new(&path));
    let saved = node::read_state(&state_path);
    let handle = mqtt::spawn(
        RssNode::new(config.node.id, feeds, saved, Some(state_path)),
        config.mqtt.broker(),
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
    oxihome_node::shutdown_signal().await;
    Ok(())
}

/// Next to the configuration and named after it, so two instances can share a directory:
/// `rss.toml` keeps `rss-state.json`, `youtube.toml` keeps `youtube-state.json`.
fn state_path(config: &Path) -> PathBuf {
    let stem = config
        .file_stem()
        .map_or("rss".into(), |stem| stem.to_string_lossy());
    config.with_file_name(format!("{stem}-state.json"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_configuration_keeps_its_own_state() {
        assert_eq!(
            state_path(Path::new("rss/rss.toml")),
            Path::new("rss/rss-state.json")
        );
        assert_eq!(
            state_path(Path::new("youtube.toml")),
            Path::new("youtube-state.json")
        );
    }
}
