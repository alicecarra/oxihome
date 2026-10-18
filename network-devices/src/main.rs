//! A software node with the devices on the home network (phones, TV...), by MAC address.

mod node;
mod scan;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use oxihome_node::mqtt::{self, Broker};
use oxihome_protocol::NAME_MAX_LENGTH;
use serde::Deserialize;

use node::{NETWORK_ENTITY, NetworkDevicesNode, Scan, WatchedDevice};
use scan::{Mac, Subnet};

const REPORT_INTERVAL: Duration = Duration::from_secs(30);
const SCAN_INTERVAL: Duration = Duration::from_secs(30);
/// Longer than the kernel takes to fail a silent address: 5 s of delay plus 3 probes of 1 s.
const SETTLE_TIME: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    node: NodeConfig,
    network: NetworkConfig,
    devices: Vec<DeviceConfig>,
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
struct NetworkConfig {
    subnet: Subnet,
    /// Seconds.
    offline_after: u64,
}

#[derive(Deserialize)]
struct DeviceConfig {
    entity: u8,
    name: String,
    mac: Mac,
    /// Seconds; the `[network]` one when missing.
    offline_after: Option<u64>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: oxihome-network-devices <network-devices.toml>")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let config: Config = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;
    let watched = watched_devices(&config)?;
    let subnet = config.network.subnet;
    println!(
        "node {}, watching {} devices on {subnet}",
        config.node.id,
        watched.len()
    );

    let handle = mqtt::spawn(
        NetworkDevicesNode::new(config.node.id, watched, Instant::now()),
        Broker {
            host: config.mqtt.host,
            port: config.mqtt.port,
            password: config.mqtt.password,
            report_interval: REPORT_INTERVAL,
            queue_size: config.mqtt.queue_size.unwrap_or(mqtt::DEFAULT_QUEUE_SIZE),
        },
    );
    let mut scan = tokio::time::interval(SCAN_INTERVAL);
    loop {
        tokio::select! {
            _ = scan.tick() => {}
            () = oxihome_node::shutdown_signal() => return Ok(()),
        }

        if let Err(error) = tokio::task::spawn_blocking(move || scan::probe(subnet)).await? {
            println!("probing {subnet} failed: {error}");
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(SETTLE_TIME) => {}
            () = oxihome_node::shutdown_signal() => return Ok(()),
        }
        let answered = match scan::answered(subnet) {
            Ok(answered) => answered,
            Err(error) => {
                println!("reading the ARP table failed: {error}");
                continue;
            }
        };
        if answered.is_empty() {
            println!("nothing answered on {subnet}; is this machine on that network?");
        }
        let now = Instant::now();
        let clock = jiff::Zoned::now().strftime("%Y-%m-%d %H:%M").to_string();
        let running = handle
            .act(move |node| {
                let (messages, changes) = node.update(&Scan {
                    answered: &answered,
                    now,
                    clock: &clock,
                });
                for change in changes {
                    let state = if change.online { "online" } else { "offline" };
                    println!("{} {state}", change.name);
                }
                messages
            })
            .await;
        if !running {
            return Ok(());
        }
    }
}

fn watched_devices(config: &Config) -> anyhow::Result<Vec<WatchedDevice>> {
    ensure!(!config.devices.is_empty(), "no [[devices]] to watch");
    let mut entities = HashSet::new();
    let mut macs = HashSet::new();
    let minimum = 2 * SCAN_INTERVAL.as_secs();
    config
        .devices
        .iter()
        .map(|device| {
            ensure!(
                device.entity != 0 && device.entity != NETWORK_ENTITY,
                "{}: entities 0 and {NETWORK_ENTITY} are reserved",
                device.name
            );
            ensure!(
                device.name.len() <= NAME_MAX_LENGTH,
                "{}: name longer than {NAME_MAX_LENGTH} bytes",
                device.name
            );
            ensure!(
                entities.insert(device.entity),
                "{}: entity {} is repeated",
                device.name,
                device.entity
            );
            ensure!(
                macs.insert(device.mac),
                "{}: MAC {} is repeated",
                device.name,
                device.mac
            );
            let offline_after = device.offline_after.unwrap_or(config.network.offline_after);
            ensure!(
                offline_after >= minimum,
                "{}: offline_after must be at least {minimum} s (two scans)",
                device.name
            );
            Ok(WatchedDevice {
                entity: device.entity,
                name: device.name.clone(),
                mac: device.mac,
                offline_after: Duration::from_secs(offline_after),
            })
        })
        .collect()
}
