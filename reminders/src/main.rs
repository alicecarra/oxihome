//! A software node with reminders: each one falls due at fixed times or some time after it was
//! last done, is sent again while pending, and is marked done from the hub or a chat.

mod config;
mod node;
mod schedule;

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use oxihome_node::config::read_config;
use oxihome_node::mqtt;

use config::Config;
use node::RemindersNode;

const TICK: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (path, config): (String, Config) = read_config("oxihome-reminders <reminders.toml>")?;
    let settings = config
        .node
        .settings(config.reminders)
        .map_err(|error| anyhow::anyhow!("{path}: {error}"))?;

    let state_path = state_path(Path::new(&path));
    let saved = node::read_state(&state_path);
    let handle = mqtt::spawn(
        RemindersNode::new(
            config.node.id,
            settings.reminders,
            settings.zone,
            settings.quiet,
            saved,
            Some(state_path),
            Timestamp::now(),
        ),
        config.mqtt.broker(),
    );
    let mut ticks = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = oxihome_node::shutdown_signal() => return Ok(()),
        }
        // What a tick sends while disconnected would be lost, and it is sent only once.
        if handle.is_connected() && !handle.act(|node| node.tick(Timestamp::now())).await {
            return Ok(());
        }
    }
}

/// Next to the configuration and named after it, like the rss node's.
fn state_path(config: &Path) -> PathBuf {
    let stem = config
        .file_stem()
        .map_or("reminders".into(), |stem| stem.to_string_lossy());
    config.with_file_name(format!("{stem}-state.json"))
}
