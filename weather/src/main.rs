//! A software node with the weather outside, from Open-Meteo.

mod node;
mod open_meteo;

use std::time::Duration;

use anyhow::Context;
use oxihome_node::mqtt::{self, Broker};
use serde::Deserialize;

use node::WeatherNode;

const REPORT_INTERVAL: Duration = Duration::from_secs(30);
const FETCH_INTERVAL: Duration = Duration::from_secs(10 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
struct Config {
    mqtt: MqttConfig,
    node: NodeConfig,
    location: Location,
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
struct Location {
    latitude: f64,
    longitude: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: oxihome-weather <weather.toml>")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    let config: Config = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;

    let handle = mqtt::spawn(
        WeatherNode::new(config.node.id.clone()),
        Broker {
            host: config.mqtt.host,
            port: config.mqtt.port,
            password: config.mqtt.password,
            report_interval: REPORT_INTERVAL,
            queue_size: config.mqtt.queue_size.unwrap_or(mqtt::DEFAULT_QUEUE_SIZE),
        },
    );
    let client = reqwest::Client::builder().timeout(FETCH_TIMEOUT).build()?;
    let mut fetch = tokio::time::interval(FETCH_INTERVAL);
    loop {
        tokio::select! {
            _ = fetch.tick() => {}
            () = oxihome_node::shutdown_signal() => return Ok(()),
        }

        match open_meteo::fetch(&client, config.location.latitude, config.location.longitude).await
        {
            Ok(weather) => {
                println!(
                    "{}: {:.1} °C, {:.0} %, {:.1} mm, {:.1} km/h",
                    weather.updated_at,
                    weather.temperature,
                    weather.humidity,
                    weather.precipitation,
                    weather.wind_speed
                );
                if !handle.act(move |node| node.update(weather)).await {
                    return Ok(());
                }
            }
            Err(error) => println!("fetching the weather failed: {error:#}"),
        }
    }
}
