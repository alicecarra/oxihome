//! Simulated nodes that talk to the hub over MQTT like real boards.

mod console;
mod node;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;

use oxihome_node::config::{BrokerConfig, parse_file, read_config};
use oxihome_node::mqtt::{self, Handle, SoftwareNode};
use oxihome_protocol::{Access, EntityDescriptor, Value};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use console::{Command, HELP};
use node::{Profile, SimulatedNode};

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    nodes: Vec<NodeConfig>,
}

#[derive(Deserialize)]
struct MqttConfig {
    /// Its `password` is ignored: each node takes its own from `secrets`.
    #[serde(flatten)]
    broker: BrokerConfig,
    /// A TOML file out of git with a `[passwords]` table (device id = password); anonymous when
    /// missing.
    secrets: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Secrets {
    passwords: HashMap<String, String>,
}

#[derive(Deserialize)]
struct NodeConfig {
    id: String,
    /// Numbered from 1 in this order.
    #[serde(default)]
    entities: Vec<Profile>,
    /// The entities another node declared, downloaded from its page in the hub.
    clone: Option<PathBuf>,
}

impl NodeConfig {
    fn descriptors(&self) -> anyhow::Result<Vec<EntityDescriptor>> {
        match (&self.clone, self.entities.is_empty()) {
            (Some(path), true) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?;
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            (None, false) => Ok(self
                .entities
                .iter()
                .zip(1..)
                .map(|(profile, id)| profile.descriptor(id))
                .collect()),
            _ => anyhow::bail!("node {}: either entities or clone", self.id),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (_, config): (String, Config) = read_config("oxihome-simulator <config.toml>")?;

    let mut passwords = match &config.mqtt.secrets {
        Some(secrets_path) => parse_file::<Secrets>(secrets_path)?.passwords,
        None => HashMap::new(),
    };

    let mut nodes = HashMap::new();
    for node_config in &config.nodes {
        let entities = node_config.descriptors()?;
        let node = SimulatedNode::new(node_config.id.clone(), entities.clone(), Instant::now());
        let broker = BrokerConfig {
            password: passwords.remove(&node_config.id),
            ..config.mqtt.broker.clone()
        }
        .broker();
        nodes.insert(
            node_config.id.clone(),
            (entities, mqtt::spawn(node, broker)),
        );
    }

    println!("{HELP}");
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut shutdown = std::pin::pin!(oxihome_node::shutdown_signal());
    loop {
        let line = tokio::select! {
            line = lines.next_line() => line?,
            () = &mut shutdown => break,
        };
        let Some(line) = line else { break };
        let command = match console::parse(&line) {
            Ok(Some(command)) => command,
            Ok(None) => continue,
            Err(error) => {
                println!("{error}");
                continue;
            }
        };
        let device_id = match &command {
            Command::Quit => break,
            Command::Help => {
                println!("{HELP}");
                continue;
            }
            Command::List => {
                for node_config in &config.nodes {
                    println!("{}", node_config.id);
                    for line in console::describe(&nodes[&node_config.id].0) {
                        println!("{line}");
                    }
                }
                continue;
            }
            Command::Offline(device_id)
            | Command::Online(device_id)
            | Command::Reboot(device_id)
            | Command::Set {
                node: device_id, ..
            }
            | Command::Auto {
                node: device_id, ..
            }
            | Command::Event {
                node: device_id, ..
            } => device_id.clone(),
        };
        let Some((entities, handle)) = nodes.get(&device_id) else {
            println!("unknown node {device_id}");
            continue;
        };
        if let Err(error) = apply(entities, handle, command).await {
            println!("{device_id}: {error}");
        }
    }
    Ok(())
}

async fn apply(
    entities: &[EntityDescriptor],
    handle: &Handle<SimulatedNode>,
    command: Command,
) -> Result<(), String> {
    match command {
        Command::Set {
            entity,
            channel,
            value,
            ..
        } => {
            let value = typed(entities, entity, Some(channel), None, &value)?;
            handle
                .act(move |node| {
                    node.set(entity, channel, value).unwrap_or_else(|error| {
                        println!("{} {error}", node.device_id());
                        Vec::new()
                    })
                })
                .await;
        }
        Command::Auto { channel, .. } => {
            handle.act(move |node| node.release(channel)).await;
        }
        Command::Event {
            entity,
            channel,
            value,
            ..
        } => {
            let value = value
                .map(|value| typed(entities, entity, channel, Some(Access::Event), &value))
                .transpose()?;
            handle
                .act(move |node| match node.event(entity, channel, value) {
                    Ok(event) => vec![event],
                    Err(error) => {
                        println!("{} {error}", node.device_id());
                        Vec::new()
                    }
                })
                .await;
        }
        Command::Reboot(_) => {
            handle
                .act(|node| {
                    println!("{} rebooting", node.device_id());
                    node.reboot(Instant::now());
                    node.announcement()
                })
                .await;
        }
        Command::Offline(_) => {
            handle.disconnect().await;
        }
        Command::Online(_) => {
            handle.connect().await;
        }
        Command::List | Command::Help | Command::Quit => {}
    }
    Ok(())
}

/// `text` read as the type of `channel`, or of the entity's first channel with `access`.
fn typed(
    entities: &[EntityDescriptor],
    entity: u8,
    channel: Option<u8>,
    access: Option<Access>,
    text: &str,
) -> Result<Value, String> {
    let declared = entities
        .iter()
        .find(|declared| declared.id == entity)
        .ok_or_else(|| format!("no entity {entity}"))?
        .channels
        .iter()
        .find(|declared| {
            channel.is_none_or(|channel| declared.id == channel)
                && access.is_none_or(|access| declared.access == access)
        })
        .ok_or_else(|| match channel {
            Some(channel) => format!("no channel {entity}.{channel}"),
            None => format!("entity {entity} has no event channel"),
        })?;
    let value_type = declared
        .value
        .as_ref()
        .ok_or_else(|| format!("{entity}.{} takes no value", declared.id))?;
    console::parse_value(value_type, text)
        .map_err(|error| format!("{entity}.{}: {error}", declared.id))
}
