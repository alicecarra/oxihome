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
/// After a failed fetch; doubles on each failure in a row, up to `FETCH_INTERVAL`.
const FIRST_RETRY: Duration = Duration::from_secs(60);

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
    let shutdown = oxihome_node::shutdown_signal();
    tokio::pin!(shutdown);
    let mut retry = FIRST_RETRY;
    loop {
        let fetched = tokio::select! {
            fetched = open_meteo::fetch(&client, config.location.latitude, config.location.longitude) => fetched,
            () = &mut shutdown => return Ok(()),
        };
        let wait = match fetched {
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
                retry = FIRST_RETRY;
                FETCH_INTERVAL
            }
            Err(error) => {
                println!("fetching the weather failed: {error:#}");
                if !handle.act(|node| node.failed(Instant::now())).await {
                    return Ok(());
                }
                let wait = retry;
                retry = (retry * 2).min(FETCH_INTERVAL);
                wait
            }
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = &mut shutdown => return Ok(()),
        }
    }
}
