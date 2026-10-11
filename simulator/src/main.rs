//! Simulated nodes that talk to the hub over MQTT like real boards.

mod node;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Context;
use oxihome_protocol::{Message, Payload};
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
    let up_topic = format!("oxihome/{}/up", node.device_id);
    let down_topic = format!("oxihome/{}/down", node.device_id);
    let mut online = true;

    loop {
        if !online {
            match controls.recv().await {
                Some(Control::Online) => online = true,
                Some(_) => println!("{} is offline", node.device_id),
                None => return,
            }
            continue;
        }

        let options = MqttOptions::new(node.device_id.clone(), mqtt.host.clone(), mqtt.port);
        let (client, mut event_loop) = AsyncClient::new(options, 32);
        let mut report = tokio::time::interval(REPORT_INTERVAL);
        report.tick().await;

        loop {
            let outgoing: Vec<(Payload, Option<u32>)> = tokio::select! {
                event = event_loop.poll() => match event {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        println!("{} connected", node.device_id);
                        for topic in [down_topic.as_str(), "oxihome/broadcast/down"] {
                            if let Err(error) = client.subscribe(topic, QoS::AtLeastOnce).await {
                                println!("{} subscribe failed: {error}", node.device_id);
                            }
                        }
                        announcement(&node)
                    }
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        match Message::from_json(&String::from_utf8_lossy(&publish.payload)) {
                            Ok(message) => node.handle(&message, Instant::now()),
                            Err(error) => {
                                println!("{} invalid message: {error}", node.device_id);
                                Vec::new()
                            }
                        }
                    }
                    Ok(_) => Vec::new(),
                    Err(error) => {
                        println!("{} connection error: {error}", node.device_id);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        Vec::new()
                    }
                },
                _ = report.tick() => {
                    let now = Instant::now();
                    let mut payloads = vec![node.heartbeat(now)];
                    payloads.extend(node.sensor_states(now));
                    payloads.into_iter().map(|payload| (payload, None)).collect()
                }
                control = controls.recv() => match control {
                    Some(Control::Press(entity)) => match node.press(entity) {
                        Some(event) => vec![(event, None)],
                        None => {
                            println!("{} entity {entity} is not a button", node.device_id);
                            Vec::new()
                        }
                    },
                    Some(Control::Reboot) => {
                        println!("{} rebooting", node.device_id);
                        node.reboot(Instant::now());
                        announcement(&node)
                    }
                    Some(Control::Offline) => {
                        println!("{} going offline", node.device_id);
                        let _ = client.disconnect().await;
                        online = false;
                        break;
                    }
                    Some(Control::Online) => Vec::new(),
                    None => return,
                },
            };

            for (payload, reply_to) in outgoing {
                let message = node.message(payload, reply_to);
                let json = match message.to_json() {
                    Ok(json) => json,
                    Err(error) => {
                        println!("{} encoding failed: {error}", node.device_id);
                        continue;
                    }
                };
                if let Err(error) = client
                    .publish(up_topic.as_str(), QoS::AtLeastOnce, false, json)
                    .await
                {
                    println!("{} publish failed: {error}", node.device_id);
                }
            }
        }
    }
}

/// What a node sends after connecting or rebooting.
fn announcement(node: &SimulatedNode) -> Vec<(Payload, Option<u32>)> {
    let mut payloads = vec![node.hello()];
    payloads.extend(node.states(Instant::now()));
    payloads
        .into_iter()
        .map(|payload| (payload, None))
        .collect()
}
