mod mqtt;
mod registry;

use std::time::{Duration, Instant};

use anyhow::Context;
use oxihome_protocol::{Payload, QueryKind};
use serde::Deserialize;

use mqtt::{Incoming, Mqtt};
use registry::Registry;

const PRESENCE_CHECK_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
}

#[derive(Deserialize)]
struct MqttConfig {
    host: String,
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: oxihome-hub <config.toml>")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let config: Config = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;

    let (mut mqtt, mut incoming) = Mqtt::connect(&config.mqtt.host, config.mqtt.port);
    let mut registry = Registry::default();
    let mut presence_check = tokio::time::interval(PRESENCE_CHECK_INTERVAL);

    loop {
        tokio::select! {
            Some(event) = incoming.recv() => match event {
                Incoming::Connected => {
                    println!("connected to {}:{}", config.mqtt.host, config.mqtt.port);
                    mqtt.broadcast(Payload::Query { what: QueryKind::Hello }).await?;
                }
                Incoming::Message(message) => {
                    if let Some(node_event) = registry.handle(&message, Instant::now()) {
                        println!("{node_event}");
                    }
                }
            },
            _ = presence_check.tick() => {
                for node_event in registry.check_presence(Instant::now()) {
                    println!("{node_event}");
                }
            }
        }
    }
}
