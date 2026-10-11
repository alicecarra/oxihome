// Copied from the Topcoat UI registry; not every helper is used.
#[allow(dead_code)]
mod components;
mod config;
mod events;
mod hub;
mod mqtt;
mod names;
mod notifications;
mod parameters;
mod registry;
mod rules;
mod state;
mod web;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use oxihome_protocol::management;
use oxihome_protocol::{Payload, QueryKind};
use serde::Deserialize;
use tokio::net::TcpListener;

use config::HomeConfig;
use hub::Hub;
use mqtt::{Incoming, Mqtt};
use registry::{DEFAULT_MISSED_HEARTBEATS, NodeEvent};

const PRESENCE_CHECK_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    config: ConfigDirectory,
    http: HttpConfig,
    #[serde(default)]
    presence: PresenceConfig,
}

#[derive(Deserialize)]
struct PresenceConfig {
    /// A node is offline after this many heartbeat intervals without messages.
    missed_heartbeats: u32,
}

impl Default for PresenceConfig {
    fn default() -> Self {
        Self {
            missed_heartbeats: DEFAULT_MISSED_HEARTBEATS,
        }
    }
}

/// The home's configuration files (names, rules), reloadable from the web interface.
#[derive(Deserialize)]
struct ConfigDirectory {
    dir: PathBuf,
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
    ensure!(
        config.presence.missed_heartbeats >= 1,
        "{path}: presence.missed_heartbeats must be at least 1"
    );

    let home_config = HomeConfig::load(&config.config.dir)?;
    println!(
        "{} rules loaded from {}",
        home_config.rules.len(),
        config.config.dir.display()
    );
    let (mqtt, mut incoming) = Mqtt::connect(&config.mqtt.host, config.mqtt.port);
    let hub = Arc::new(Hub::new(
        mqtt,
        config.presence.missed_heartbeats,
        config.config.dir.clone(),
        home_config,
    ));

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
                    let (node_events, updates, event) = hub.handle(&message, Instant::now());
                    let reports_parameters = matches!(
                        message.payload,
                        Payload::State { entity: management::ENTITY, .. }
                    );
                    let came_back = node_events
                        .iter()
                        .any(|node_event| matches!(node_event, NodeEvent::Online(_)));
                    if reports_parameters || came_back {
                        tokio::spawn(parameters::apply(hub.clone(), message.src.clone()));
                    }
                    for node_event in node_events {
                        println!("{node_event}");
                        if let NodeEvent::Unknown(device_id) = node_event {
                            let mqtt = hub.mqtt();
                            mqtt.publish(
                                mqtt.next_message_id(),
                                Some(&device_id),
                                Payload::Query { what: QueryKind::Hello },
                            )
                            .await?;
                        }
                    }
                    for update in updates {
                        println!("{update}");
                    }
                    if let Some(event) = event {
                        println!("{event}");
                        for rule in hub.matching_rules(&event) {
                            tokio::spawn(rules::run(hub.clone(), rule));
                        }
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
