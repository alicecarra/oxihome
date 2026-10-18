//! Simulated nodes that talk to the hub over MQTT like real boards.

mod node;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use oxihome_node::config::{BrokerConfig, parse_file, read_config};
use oxihome_node::mqtt::{self, Handle, SoftwareNode};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use node::{Profile, SimulatedNode};

const HELP: &str = "\
commands:
  list
  press <node> <entity>
  offline <node>
  online <node>
  reboot <node>
  climate <node> <temperature> [humidity]   fixed readings, sent right away
  climate <node> auto                       back to the slow wave
  help
  quit";

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
    entities: Vec<Profile>,
}

enum Control {
    Press(u8),
    Offline,
    Online,
    Reboot,
    Climate {
        temperature: Option<f32>,
        humidity: Option<f32>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (_, config): (String, Config) = read_config("oxihome-simulator <config.toml>")?;

    let mut passwords = match &config.mqtt.secrets {
        Some(secrets_path) => parse_file::<Secrets>(secrets_path)?.passwords,
        None => HashMap::new(),
    };

    let mut handles = HashMap::new();
    for node_config in &config.nodes {
        let node = SimulatedNode::new(
            node_config.id.clone(),
            &node_config.entities,
            Instant::now(),
        );
        let broker = BrokerConfig {
            password: passwords.remove(&node_config.id),
            ..config.mqtt.broker.clone()
        }
        .broker();
        handles.insert(node_config.id.clone(), mqtt::spawn(node, broker));
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
        let words: Vec<&str> = line.split_whitespace().collect();
        let (device_id, control) = match words.as_slice() {
            [] => continue,
            ["quit"] => break,
            ["help"] => {
                println!("{HELP}");
                continue;
            }
            ["list"] => {
                for node_config in &config.nodes {
                    let entities: Vec<String> = node_config
                        .entities
                        .iter()
                        .enumerate()
                        .map(|(index, profile)| format!("{}={profile:?}", index + 1))
                        .collect();
                    println!("{} {}", node_config.id, entities.join(" "));
                }
                continue;
            }
            ["press", device_id, entity] => match entity.parse() {
                Ok(entity) => (*device_id, Control::Press(entity)),
                Err(_) => {
                    println!("entity must be a number");
                    continue;
                }
            },
            ["offline", device_id] => (*device_id, Control::Offline),
            ["online", device_id] => (*device_id, Control::Online),
            ["reboot", device_id] => (*device_id, Control::Reboot),
            ["climate", device_id, "auto"] => (
                *device_id,
                Control::Climate {
                    temperature: None,
                    humidity: None,
                },
            ),
            ["climate", device_id, temperature, rest @ ..] if rest.len() <= 1 => {
                let temperature = temperature.parse::<f32>();
                let humidity = rest.first().map(|humidity| humidity.parse::<f32>());
                match (temperature, humidity.transpose()) {
                    (Ok(temperature), Ok(humidity)) => (
                        *device_id,
                        Control::Climate {
                            temperature: Some(temperature),
                            humidity,
                        },
                    ),
                    _ => {
                        println!("temperature and humidity must be numbers");
                        continue;
                    }
                }
            }
            _ => {
                println!("unknown command, type `help`");
                continue;
            }
        };
        match handles.get(device_id) {
            Some(handle) => apply(handle, control).await,
            None => println!("unknown node {device_id}"),
        }
    }
    Ok(())
}

async fn apply(handle: &Handle<SimulatedNode>, control: Control) {
    match control {
        Control::Press(entity) => {
            handle
                .act(move |node| match node.press(entity) {
                    Some(event) => vec![event],
                    None => {
                        println!("{} entity {entity} is not a button", node.device_id());
                        Vec::new()
                    }
                })
                .await
        }
        Control::Reboot => {
            handle
                .act(|node| {
                    println!("{} rebooting", node.device_id());
                    node.reboot(Instant::now());
                    node.announcement()
                })
                .await
        }
        Control::Climate {
            temperature,
            humidity,
        } => {
            handle
                .act(move |node| {
                    let states = node.set_climate(temperature, humidity);
                    if states.is_empty() {
                        println!("{} has no climate sensor", node.device_id());
                    }
                    states
                })
                .await
        }
        Control::Offline => handle.disconnect().await,
        Control::Online => handle.connect().await,
    };
}
