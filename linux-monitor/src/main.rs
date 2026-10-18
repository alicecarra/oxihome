//! A software node with the state of the Linux machine it runs on.

mod docker;
mod node;
mod sample;

use std::path::PathBuf;

use anyhow::ensure;
use oxihome_node::config::{BrokerConfig, read_config};
use oxihome_node::mqtt;
use serde::Deserialize;

use docker::Docker;
use node::{MAXIMUM_PER_RANGE, MonitorNode};
use sample::Sampler;

#[derive(Deserialize)]
struct Config {
    mqtt: BrokerConfig,
    #[serde(default)]
    node: NodeConfig,
    #[serde(default)]
    monitor: MonitorConfig,
}

#[derive(Deserialize, Default)]
struct NodeConfig {
    /// The hostname when missing.
    id: Option<String>,
}

#[derive(Deserialize)]
#[serde(default)]
struct MonitorConfig {
    /// Mount points; entity ids follow this order.
    disks: Vec<String>,
    /// Interface names; entity ids follow this order.
    interfaces: Vec<String>,
    /// Container names, as `docker ps` shows them; entity ids follow this order.
    containers: Vec<String>,
    /// Podman without root: `$XDG_RUNTIME_DIR/podman/podman.sock`.
    docker_socket: PathBuf,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            disks: vec!["/".into()],
            interfaces: Vec::new(),
            containers: Vec::new(),
            docker_socket: "/var/run/docker.sock".into(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (_, config): (String, Config) = read_config("oxihome-linux-monitor <linux-monitor.toml>")?;
    let monitor = config.monitor;
    ensure!(
        [&monitor.disks, &monitor.interfaces, &monitor.containers]
            .iter()
            .all(|list| list.len() <= MAXIMUM_PER_RANGE),
        "at most {MAXIMUM_PER_RANGE} disks, interfaces and containers"
    );

    let sampler = Sampler::new(
        monitor.disks.clone(),
        monitor.interfaces.clone(),
        monitor.containers.clone(),
        Docker::new(monitor.docker_socket),
    );
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
            monitor.disks,
            monitor.interfaces,
            monitor.containers,
        ),
        config.mqtt.broker(),
    );
    oxihome_node::shutdown_signal().await;
    Ok(())
}
