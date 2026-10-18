//! A software node with the weather outside, from Open-Meteo.

mod node;
mod open_meteo;

use std::time::{Duration, Instant};

use oxihome_node::config::{BrokerConfig, read_config};
use oxihome_node::mqtt;
use serde::Deserialize;

use node::WeatherNode;

const FETCH_INTERVAL: Duration = Duration::from_secs(10 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
struct Config {
    mqtt: BrokerConfig,
    node: NodeConfig,
    location: Location,
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
    let (_, config): (String, Config) = read_config("oxihome-weather <weather.toml>")?;

    let handle = mqtt::spawn(
        WeatherNode::new(config.node.id.clone()),
        config.mqtt.broker(),
    );
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("oxihome-weather/", env!("CARGO_PKG_VERSION")))
        .build()?;
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
                if !handle
                    .act(move |node| node.update(weather, Instant::now()))
                    .await
                {
                    return Ok(());
                }
            }
            Err(error) => {
                println!("fetching the weather failed: {error:#}");
                if !handle.act(|node| node.failed(Instant::now())).await {
                    return Ok(());
                }
            }
        }
    }
}
