mod commands;
// Copied from the Topcoat UI registry; not every helper is used.
#[allow(dead_code)]
mod components;
mod config;
mod dispatch;
mod hub;
mod mqtt;
mod node;
mod notifications;
mod schedule;
mod web;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Deserialize;
use tokio::net::TcpListener;
use topcoat::router::RouterService;

use config::HomeConfig;
use hub::Hub;
use mqtt::{Incoming, Mqtt};
use node::registry::DEFAULT_MISSED_HEARTBEATS;
use schedule::Schedule;

const PRESENCE_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const CLOCK_INTERVAL: Duration = Duration::from_secs(1);
/// How long requests get to finish after Ctrl+C or `SIGTERM`.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    config: ConfigDirectory,
    http: HttpConfig,
    #[serde(default)]
    presence: PresenceConfig,
    #[serde(default)]
    time: TimeConfig,
}

#[derive(Default, Deserialize)]
struct TimeConfig {
    /// IANA name, e.g. "America/Sao_Paulo"; the system's zone when missing.
    zone: Option<String>,
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

    let time_zone = match &config.time.zone {
        Some(name) => TimeZone::get(name).with_context(|| format!("{path}: time.zone"))?,
        None => TimeZone::system(),
    };

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
        time_zone,
    ));

    let listener = TcpListener::bind((config.http.host.as_str(), config.http.port)).await?;
    println!(
        "web interface on http://{}:{}",
        config.http.host, config.http.port
    );
    let service = RouterService::new(web::router(hub.clone())).shutdown_timeout(SHUTDOWN_TIMEOUT);
    let mut web_server = tokio::spawn(topcoat::serve_until(listener, service, hub.shutdown()));
    let mut signal = std::pin::pin!(shutdown_signal());

    let mut presence_check = tokio::time::interval(PRESENCE_CHECK_INTERVAL);
    let mut clock = tokio::time::interval(CLOCK_INTERVAL);
    let mut schedule = Schedule::new(hub.time_zone(), Timestamp::now());
    loop {
        tokio::select! {
            Some(event) = incoming.recv() => match event {
                Incoming::Connected => {
                    println!("connected to {}:{}", config.mqtt.host, config.mqtt.port);
                    dispatch::on_connected(&hub).await?;
                }
                Incoming::Message(message) => dispatch::on_message(&hub, &message).await?,
            },
            _ = presence_check.tick() => dispatch::on_presence_check(&hub),
            _ = clock.tick() => dispatch::on_clock_tick(&hub, &mut schedule),
            result = &mut web_server => {
                return Ok(result??);
            }
            () = &mut signal => {
                println!("shutting down");
                hub.shut_down();
                return Ok(web_server.await??);
            }
        }
    }
}

/// Ctrl+C, or `SIGTERM` (systemd, `kill`).
async fn shutdown_signal() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("installing the SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
