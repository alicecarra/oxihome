//! Simulated nodes that talk to the hub over MQTT like real boards.

mod action;
mod console;
mod control;
mod node;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context;
use oxihome_node::config::{BrokerConfig, parse_file, read_config};
use oxihome_node::mqtt;
use oxihome_protocol::{Access, EntityDescriptor, Value};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use action::Action;
use console::{Command, HELP};
use control::ControlNode;
use node::{Profile, SimulatedNode};

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    nodes: Vec<NodeConfig>,
    /// Without it, the nodes are controlled from the console only.
    control: Option<ControlConfig>,
}

#[derive(Deserialize)]
struct ControlConfig {
    /// The control node's device id, which logs in to the broker like the others.
    id: String,
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

    let (actions, mut requested) = tokio::sync::mpsc::unbounded_channel();
    let control = config.control.as_ref().map(|control_config| {
        let simulated: Vec<(String, Vec<EntityDescriptor>)> = config
            .nodes
            .iter()
            .map(|node_config| (node_config.id.clone(), nodes[&node_config.id].0.clone()))
            .collect();
        let control = ControlNode::new(control_config.id.clone(), &simulated, actions);
        let entities = control.entities().to_vec();
        let broker = BrokerConfig {
            password: passwords.remove(&control_config.id),
            ..config.mqtt.broker.clone()
        }
        .broker();
        (
            control_config.id.clone(),
            entities,
            mqtt::spawn(control, broker),
        )
    });

    println!("{HELP}");
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut shutdown = std::pin::pin!(oxihome_node::shutdown_signal());
    loop {
        let line = tokio::select! {
            line = lines.next_line() => line?,
            Some((device_id, action)) = requested.recv() => {
                if let Some((_, handle)) = nodes.get(&device_id) {
                    action::apply(handle, action).await;
                }
                continue;
            }
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
                if let Some((device_id, entities, _)) = &control {
                    println!("{device_id} (controls)");
                    for line in console::describe(entities) {
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
        match typed_action(entities, command) {
            Ok(action) => {
                let online = match action {
                    Action::Online => Some(true),
                    Action::Offline => Some(false),
                    _ => None,
                };
                action::apply(handle, action).await;
                if let (Some(online), Some((_, _, control_handle))) = (online, &control) {
                    control_handle
                        .act(move |control| control.reflect(&device_id, online))
                        .await;
                }
            }
            Err(error) => println!("{device_id}: {error}"),
        }
    }
    Ok(())
}

/// The command with its values read as the node's channels declare them.
fn typed_action(entities: &[EntityDescriptor], command: Command) -> Result<Action, String> {
    Ok(match command {
        Command::Online(_) => Action::Online,
        Command::Offline(_) => Action::Offline,
        Command::Reboot(_) => Action::Reboot,
        Command::Set {
            entity,
            channel,
            value,
            ..
        } => Action::Set {
            entity,
            channel,
            value: typed(entities, entity, Some(channel), None, &value)?,
        },
        Command::Auto { channel, .. } => Action::Auto { channel },
        Command::Event {
            entity,
            channel,
            value,
            ..
        } => Action::Event {
            entity,
            channel,
            value: value
                .map(|value| typed(entities, entity, channel, Some(Access::Event), &value))
                .transpose()?,
        },
        Command::List | Command::Help | Command::Quit => {
            return Err("not a node command".into());
        }
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn entities() -> Vec<EntityDescriptor> {
        [Profile::Lamp, Profile::Button, Profile::ClimateSensor]
            .iter()
            .zip(1..)
            .map(|(profile, id)| profile.descriptor(id))
            .collect()
    }

    fn action(line: &str) -> Result<Action, String> {
        typed_action(&entities(), console::parse(line).unwrap().unwrap())
    }

    #[test]
    fn typed_values_follow_the_channels() {
        assert_eq!(
            action("set sim 3.0 31"),
            Ok(Action::Set {
                entity: 3,
                channel: 0,
                value: Value::Float(31.0),
            })
        );
        assert_eq!(
            action("press sim 2"),
            Ok(Action::Event {
                entity: 2,
                channel: None,
                value: None,
            })
        );
        assert_eq!(action("offline sim"), Ok(Action::Offline));
        assert!(action("set sim 3.0 99").is_err());
        assert!(action("set sim 3.7 20").is_err());
        assert!(action("event sim 2 now").is_err());
    }
}
