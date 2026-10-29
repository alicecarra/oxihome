mod commands;
// Copied from the Topcoat UI registry; not every helper is used.
#[allow(dead_code)]
mod components;
mod config;
mod dispatch;
mod health;
mod hub;
mod mqtt;
mod node;
mod notifications;
mod ota;
mod schedule;
mod storage;
mod traffic;
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
use hub::{Hub, HubSetup};
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

#[derive(Deserialize)]
struct TimeConfig {
    /// IANA name, e.g. "America/Sao_Paulo"; the database built into the hub covers machines without
    /// tzdata.
    zone: String,
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
    /// A TOML file out of git with the broker password and the web interface's password hash;
    /// anonymous and without login when missing.
    secrets: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Secrets {
    mqtt: MqttSecrets,
    http: Option<HttpSecrets>,
}

#[derive(Deserialize)]
struct MqttSecrets {
    password: String,
}

#[derive(Deserialize)]
struct HttpSecrets {
    /// From `oxihome-hub hash-password`; required unless the hub listens on localhost only.
    password_hash: Option<String>,
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
        .context(
            "usage: oxihome-hub <config.toml> | oxihome-hub hash-password | oxihome-hub health <config.toml>",
        )?;
    if path == "hash-password" {
        return print_password_hash();
    }
    if path == "health" {
        let path = std::env::args()
            .nth(2)
            .context("usage: oxihome-hub health <config.toml>")?;
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
        let config: Config = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;
        return health::check(&config.http.host, config.http.port).await;
    }
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

    let time_zone =
        TimeZone::get(&config.time.zone).with_context(|| format!("{path}: time.zone"))?;
    web::format::set_time_zone(time_zone.clone());

    let home_config = HomeConfig::load(&config.config.dir)?;
    println!(
        "{} rules loaded from {}",
        home_config.rules.len(),
        config.config.dir.display()
    );
    let secrets = match &config.mqtt.secrets {
        Some(secrets_path) => {
            let text = std::fs::read_to_string(secrets_path)
                .with_context(|| format!("reading {}", secrets_path.display()))?;
            let secrets: Secrets = toml::from_str(&text)
                .with_context(|| format!("parsing {}", secrets_path.display()))?;
            Some(secrets)
        }
        None => None,
    };
    let (password, password_hash) = match secrets {
        Some(secrets) => (
            Some(secrets.mqtt.password),
            secrets.http.and_then(|http| http.password_hash),
        ),
        None => (None, None),
    };
    ensure!(
        password_hash.is_some() || is_loopback(&config.http.host),
        "{path}: the web interface listens on {} without a password; set [http] password_hash \
         in the secrets file (see `oxihome-hub hash-password`)",
        config.http.host
    );
    let retention = Duration::from_secs(u64::from(config.storage.retention_days) * 86400);
    let (storage, mut restored) = Storage::open(&config.storage.path, retention).await?;
    let waits = std::mem::take(&mut restored.waits);
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
    let auth = web::Auth::new(password_hash, storage.sessions())
        .context("[http] password_hash in the secrets file")?;
    let (mqtt, mut incoming) = Mqtt::connect(&config.mqtt.host, config.mqtt.port, password);
    let hub = Arc::new(Hub::new(HubSetup {
        mqtt,
        missed_heartbeats: config.presence.missed_heartbeats,
        config_directory: config.config.dir.clone(),
        config: home_config,
        time_zone,
        storage,
        restored,
        ota,
    }));
    config::rules::resume_waits(hub.clone(), waits);

    let listener = TcpListener::bind((config.http.host.as_str(), config.http.port)).await?;
    println!(
        "web interface on http://{}:{}",
        config.http.host, config.http.port
    );
    let service = RouterService::new(web::router(
        hub.clone(),
        auth,
        &config.http.host,
        &config.http.allowed_hosts,
    ))
    .shutdown_timeout(SHUTDOWN_TIMEOUT);
    let mut web_server = tokio::spawn(topcoat::serve_until(listener, service, hub.shutdown()));
    let mut signal = std::pin::pin!(shutdown_signal());

    let (_config_watcher, mut config_changes) =
        match config::watch::watch(&config.config.dir, config::watch::QUIET) {
            Ok((watcher, changes)) => (Some(watcher), changes),
            Err(error) => {
                println!("config files not watched, reload them by hand: {error:#}");
                (None, tokio::sync::mpsc::channel(1).1)
            }
        };
    let mut presence_check = tokio::time::interval(PRESENCE_CHECK_INTERVAL);
    let mut clock = tokio::time::interval(CLOCK_INTERVAL);
    let mut schedule = Schedule::new(hub.time_zone(), Timestamp::now());
    loop {
        tokio::select! {
            Some(event) = incoming.recv() => match event {
                Incoming::Connected => {
                    println!("connected to {}:{}", config.mqtt.host, config.mqtt.port);
                    dispatch::on_connected(&hub);
                }
                Incoming::Message(message) => dispatch::on_message(&hub, &message),
                Incoming::OtherProtocol { device_id, version } => {
                    hub.notify_other_protocol(&device_id, version);
                }
            },
            Some(()) = config_changes.recv() => dispatch::on_config_changed(&hub),
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

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Reads the password from the first line of standard input.
fn print_password_hash() -> anyhow::Result<()> {
    eprintln!("Password for the web interface:");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']);
    ensure!(!password.is_empty(), "empty password");
    println!("{}", web::hash_password(password)?);
    Ok(())
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
