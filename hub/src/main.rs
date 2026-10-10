mod hub;
mod mqtt;
mod registry;
mod state;
mod web;

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use oxihome_protocol::{Payload, QueryKind};
use serde::Deserialize;
use tokio::net::TcpListener;

use hub::Hub;
use mqtt::{Incoming, Mqtt};

const PRESENCE_CHECK_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    http: HttpConfig,
}

#[derive(Deserialize)]
struct MqttConfig {
    host: String,
    port: u16,
}

#[derive(Deserialize)]
struct HttpConfig {
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

    let (mqtt, mut incoming) = Mqtt::connect(&config.mqtt.host, config.mqtt.port);
    let hub = Arc::new(Hub::new(mqtt));

    let listener = TcpListener::bind((config.http.host.as_str(), config.http.port)).await?;
    println!(
        "web interface on http://{}:{}",
        config.http.host, config.http.port
    );
    let mut web_server = tokio::spawn(topcoat::serve(listener, web::router(hub.clone())));

    let mut presence_check = tokio::time::interval(PRESENCE_CHECK_INTERVAL);
    loop {
        tokio::select! {
            Some(event) = incoming.recv() => match event {
                Incoming::Connected => {
                    println!("connected to {}:{}", config.mqtt.host, config.mqtt.port);
                    hub.mqtt().broadcast(Payload::Query { what: QueryKind::Hello }).await?;
                    hub.mqtt().broadcast(Payload::Query { what: QueryKind::State { entity: None } }).await?;
                }
                Incoming::Message(message) => {
                    let (node_event, updates) = hub.handle(&message, Instant::now());
                    if let Some(node_event) = node_event {
                        println!("{node_event}");
                    }
                    for update in updates {
                        println!("{update}");
                    }
                }
            },
            _ = presence_check.tick() => {
                for node_event in hub.check_presence(Instant::now()) {
                    println!("{node_event}");
                }
            }
            result = &mut web_server => {
                return Ok(result??);
            }
        }
    }
}
