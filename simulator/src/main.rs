//! Simulated nodes that talk to the hub over MQTT like real boards.

mod node;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Context;
use oxihome_protocol::Message;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

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

#[derive(Deserialize, Clone)]
struct MqttConfig {
    host: String,
    port: u16,
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

    let mut controls = HashMap::new();
    for node_config in &config.nodes {
        let node = SimulatedNode::new(
            node_config.id.clone(),
            &node_config.entities,
            Instant::now(),
        );
        let (sender, receiver) = mpsc::channel(8);
        controls.insert(node_config.id.clone(), sender);
        tokio::spawn(run_node(node, config.mqtt.clone(), receiver));
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
        match controls.get(device_id) {
            Some(sender) => sender.send(control).await?,
            None => println!("unknown node {device_id}"),
        }
    }
    Ok(())
}

/// One MQTT connection per node, like a real board.
async fn run_node(
    mut node: SimulatedNode,
    mqtt: MqttConfig,
    mut controls: mpsc::Receiver<Control>,
) {
    let device_id = node.device_id().to_string();
    let up_topic = format!("oxihome/{device_id}/up");
    let down_topic = format!("oxihome/{device_id}/down");
    let mut online = true;

    loop {
        if !online {
            match controls.recv().await {
                Some(Control::Online) => online = true,
                Some(_) => println!("{} is offline", device_id),
                None => return,
            }
            continue;
        }

        let options = MqttOptions::new(device_id.clone(), mqtt.host.clone(), mqtt.port);
        let (client, mut event_loop) = AsyncClient::new(options, 32);
        let mut report = tokio::time::interval(REPORT_INTERVAL);
        report.tick().await;

        loop {
            let outgoing: Vec<Message> = tokio::select! {
                event = event_loop.poll() => match event {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        println!("{} connected", device_id);
                        for topic in [down_topic.as_str(), "oxihome/broadcast/down"] {
                            if let Err(error) = client.subscribe(topic, QoS::AtLeastOnce).await {
                                println!("{} subscribe failed: {error}", device_id);
                            }
                        }
                        node.announcement()
                    }
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        match Message::from_json(&String::from_utf8_lossy(&publish.payload)) {
                            Ok(message) => node.handle(&message),
                            Err(error) => {
                                println!("{} invalid message: {error}", device_id);
                                Vec::new()
                            }
                        }
                    }
                    Ok(_) => Vec::new(),
                    Err(error) => {
                        println!("{} connection error: {error}", device_id);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        Vec::new()
                    }
                },
                _ = report.tick() => {
                    let mut messages = vec![node.heartbeat(Instant::now())];
                    messages.extend(node.sensor_states());
                    messages
                }
                control = controls.recv() => match control {
                    Some(Control::Press(entity)) => match node.press(entity) {
                        Some(event) => vec![event],
                        None => {
                            println!("{} entity {entity} is not a button", device_id);
                            Vec::new()
                        }
                    },
                    Some(Control::Reboot) => {
                        println!("{} rebooting", device_id);
                        node.reboot(Instant::now());
                        node.announcement()
                    }
                    Some(Control::Offline) => {
                        println!("{} going offline", device_id);
                        let _ = client.disconnect().await;
                        online = false;
                        break;
                    }
                    Some(Control::Climate { temperature, humidity }) => {
                        let states = node.set_climate(temperature, humidity);
                        if states.is_empty() {
                            println!("{} has no climate sensor", device_id);
                        }
                        states
                    }
                    Some(Control::Online) => Vec::new(),
                    None => return,
                },
            };

            for message in outgoing {
                let json = match message.to_json() {
                    Ok(json) => json,
                    Err(error) => {
                        println!("{} encoding failed: {error}", device_id);
                        continue;
                    }
                };
                if let Err(error) = client
                    .publish(up_topic.as_str(), QoS::AtLeastOnce, false, json)
                    .await
                {
                    println!("{} publish failed: {error}", device_id);
                }
            }
        }
    }
}
