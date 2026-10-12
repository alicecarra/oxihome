//! A software node with the state of the Linux machine it runs on.

mod node;
mod sample;

use std::time::Duration;

use anyhow::{Context, ensure};
use oxihome_node::mqtt::{self, Broker};
use serde::Deserialize;

use node::{MAXIMUM_PER_RANGE, MonitorNode};
use sample::Sampler;

/// Matches the default `HeartbeatInterval`, so the hub's offline timeout fits.
const REPORT_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    #[serde(default)]
    node: NodeConfig,
    #[serde(default)]
    monitor: MonitorConfig,
}

#[derive(Deserialize)]
struct MqttConfig {
    host: String,
    port: u16,
}

#[derive(Deserialize, Default)]
struct NodeConfig {
    /// The hostname when missing.
    id: Option<String>,
}

#[derive(Deserialize)]
struct MonitorConfig {
    /// Mount points; entity ids follow this order.
    disks: Vec<String>,
    /// Interface names; entity ids follow this order.
    interfaces: Vec<String>,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            disks: vec!["/".into()],
            interfaces: Vec::new(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: oxihome-linux-monitor <linux-monitor.toml>")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let config: Config = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;
    let monitor = config.monitor;
    ensure!(
        monitor.disks.len() <= MAXIMUM_PER_RANGE && monitor.interfaces.len() <= MAXIMUM_PER_RANGE,
        "at most {MAXIMUM_PER_RANGE} disks and {MAXIMUM_PER_RANGE} interfaces"
    );

    let sampler = Sampler::new(monitor.disks.clone(), monitor.interfaces.clone());
    let (mounts, interfaces) = sampler.available();
    println!("mount points: {}", mounts.join(", "));
    println!("interfaces: {}", interfaces.join(", "));
    let device_id = match config.node.id {
        Some(id) => id,
        None => sampler.info().hostname,
    };
    ensure!(!device_id.is_empty(), "no node id and no hostname");
    println!("node {device_id}");

    let _handle = mqtt::spawn(
        MonitorNode::new(
            device_id,
            sampler,
            monitor.disks.len(),
            monitor.interfaces.len(),
        ),
        Broker {
            host: config.mqtt.host,
            port: config.mqtt.port,
            report_interval: REPORT_INTERVAL,
        },
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
