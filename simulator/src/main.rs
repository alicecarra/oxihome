//! Simulated nodes that talk to the hub over MQTT like real boards.

mod node;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use oxihome_node::mqtt::{self, Broker, Handle, SoftwareNode};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use node::{Profile, SimulatedNode};

const REPORT_INTERVAL: Duration = Duration::from_secs(30);

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
    host: String,
    port: u16,
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
    let path = std::env::args()
        .nth(1)
        .context("usage: oxihome-simulator <config.toml>")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let config: Config = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;

    let mut passwords = match &config.mqtt.secrets {
        Some(secrets_path) => {
            let text = std::fs::read_to_string(secrets_path)
                .with_context(|| format!("reading {}", secrets_path.display()))?;
            let secrets: Secrets = toml::from_str(&text)
                .with_context(|| format!("parsing {}", secrets_path.display()))?;
            secrets.passwords
        }
        None => HashMap::new(),
    };

    let mut handles = HashMap::new();
    for node_config in &config.nodes {
        let node = SimulatedNode::new(
            node_config.id.clone(),
            &node_config.entities,
            Instant::now(),
        );
        let broker = Broker {
            host: config.mqtt.host.clone(),
            port: config.mqtt.port,
            password: passwords.remove(&node_config.id),
            report_interval: REPORT_INTERVAL,
        };
        handles.insert(node_config.id.clone(), mqtt::spawn(node, broker));
    }

    println!("{HELP}");
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
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
