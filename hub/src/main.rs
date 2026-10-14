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
mod ota;
mod schedule;
mod storage;
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
use ota::{Ota, OtaConfig};
use schedule::Schedule;
use storage::Storage;

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
    storage: StorageConfig,
    /// Over-the-air updates; off when missing.
    ota: Option<OtaConfig>,
}

#[derive(Deserialize)]
struct StorageConfig {
    /// SQLite database with the nodes' last state and the history; created when missing.
    path: PathBuf,
    /// Days of readings and events kept.
    #[serde(default = "default_retention_days")]
    retention_days: u32,
}

fn default_retention_days() -> u32 {
    7
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
    /// A TOML file out of git with the broker password; anonymous when missing.
    secrets: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Secrets {
    mqtt: MqttSecrets,
}

#[derive(Deserialize)]
struct MqttSecrets {
    password: String,
}

#[derive(Deserialize)]
struct HttpConfig {
    host: String,
    port: u16,
    /// Other names the interface answers to besides `host` and localhost, e.g. when `host` is 0.0.0.0.
    #[serde(default)]
    allowed_hosts: Vec<String>,
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
    ensure!(
        config.storage.retention_days >= 1,
        "{path}: storage.retention_days must be at least 1"
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
    let password = match &config.mqtt.secrets {
        Some(secrets_path) => {
            let text = std::fs::read_to_string(secrets_path)
                .with_context(|| format!("reading {}", secrets_path.display()))?;
            let secrets: Secrets = toml::from_str(&text)
                .with_context(|| format!("parsing {}", secrets_path.display()))?;
            Some(secrets.mqtt.password)
        }
        None => None,
    };
    let retention = Duration::from_secs(u64::from(config.storage.retention_days) * 86400);
    let (storage, restored) = Storage::open(&config.storage.path, retention).await?;
    println!(
        "{} nodes restored from {}",
        restored.nodes.len(),
        config.storage.path.display()
    );
    let ota = match &config.ota {
        Some(ota_config) => {
            let ota = Arc::new(Ota::new(ota_config));
            let listener = TcpListener::bind(ota_config.listen.as_str())
                .await
                .with_context(|| format!("{path}: ota.listen"))?;
            println!("firmware images on {}", ota_config.listen);
            tokio::spawn(ota::serve(listener, ota.clone()));
            Some(ota)
        }
        None => None,
    };
    let (mqtt, mut incoming) = Mqtt::connect(&config.mqtt.host, config.mqtt.port, password);
    let hub = Arc::new(Hub::new(
        mqtt,
        config.presence.missed_heartbeats,
        config.config.dir.clone(),
        home_config,
        time_zone,
        storage,
        restored,
        ota,
    ));

    let listener = TcpListener::bind((config.http.host.as_str(), config.http.port)).await?;
    println!(
        "web interface on http://{}:{}",
        config.http.host, config.http.port
    );
    let service = RouterService::new(web::router(hub.clone(), &config.http.host, &config.http.allowed_hosts))
        .shutdown_timeout(SHUTDOWN_TIMEOUT);
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
                Incoming::OtherProtocol { device_id, version } => {
                    hub.notify_other_protocol(&device_id, version);
                }
            },
            _ = presence_check.tick() => dispatch::on_presence_check(&hub),
            _ = clock.tick() => dispatch::on_clock_tick(&hub, &mut schedule),
            result = &mut web_server => {
                hub.flush_storage().await;
                return Ok(result??);
            }
            () = &mut signal => {
                println!("shutting down");
                hub.shut_down();
                let result = web_server.await;
                hub.flush_storage().await;
                return Ok(result??);
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
