//! A software node that links the house to a Telegram chat: rules send messages to it, and the
//! commands typed there become events.

mod bot;
mod node;

use std::collections::HashSet;
use std::time::Duration;

use anyhow::bail;
use oxihome_node::config::{BrokerConfig, read_config};
use oxihome_node::mqtt;
use serde::Deserialize;
use teloxide_core::Bot;
use teloxide_core::types::{ChatId, UserId};
use tokio::sync::mpsc;

use node::TelegramNode;

/// Longer than the long polling timeout.
const HTTP_TIMEOUT: Duration = Duration::from_secs(70);
const OUTGOING_QUEUE: usize = 64;
const DEFAULT_MAX_AGE_S: u64 = 60;

#[derive(Deserialize)]
struct Config {
    mqtt: BrokerConfig,
    node: NodeConfig,
    telegram: TelegramConfig,
    commands: Vec<bot::Command>,
}

#[derive(Deserialize)]
struct NodeConfig {
    id: String,
}

#[derive(Deserialize)]
struct TelegramConfig {
    token: String,
    users: Vec<u64>,
    chat: i64,
    /// Seconds; older commands are ignored.
    max_age: Option<u64>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (path, config): (String, Config) = read_config("oxihome-telegram <telegram.toml>")?;
    if config.commands.is_empty() {
        bail!("{path}: declare at least one [[commands]]");
    }
    let mut names = HashSet::new();
    for command in &config.commands {
        command
            .check()
            .map_err(|error| anyhow::anyhow!("{path}: {error}"))?;
        if !names.insert(&command.name) {
            bail!("{path}: command \"{}\" declared twice", command.name);
        }
    }
    if config.telegram.users.is_empty() {
        bail!("{path}: [telegram] users is empty, nobody could send commands");
    }

    let (outgoing_sender, outgoing) = mpsc::channel(OUTGOING_QUEUE);
    let command_names = config
        .commands
        .iter()
        .map(|command| command.name.clone())
        .collect();
    let handle = mqtt::spawn(
        TelegramNode::new(config.node.id, command_names, outgoing_sender),
        config.mqtt.broker(),
    );
    let client = teloxide_core::net::default_reqwest_settings()
        .timeout(HTTP_TIMEOUT)
        .build()?;
    let bot = Bot::with_client(config.telegram.token, client);
    tokio::spawn(bot::send(
        bot.clone(),
        ChatId(config.telegram.chat),
        outgoing,
        handle.clone(),
    ));
    let settings = bot::Settings {
        users: config.telegram.users.into_iter().map(UserId).collect(),
        max_age: Duration::from_secs(config.telegram.max_age.unwrap_or(DEFAULT_MAX_AGE_S)),
        commands: config.commands,
    };
    tokio::select! {
        _ = bot::poll(bot, settings, handle) => {}
        () = oxihome_node::shutdown_signal() => {}
    }
    Ok(())
}
