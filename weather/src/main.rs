//! A software node with the weather outside, from Open-Meteo.

mod node;
mod open_meteo;

use std::time::{Duration, Instant, SystemTime};

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
    // For the sunrise and sunset between fetches.
    let mut known: Option<open_meteo::Weather> = None;
    loop {
        let fetched = tokio::select! {
            fetched = open_meteo::fetch(&client, config.location.latitude, config.location.longitude) => fetched,
            () = &mut shutdown => return Ok(()),
        };
        let wait = match fetched {
            Ok(weather) => {
                println!(
                    "{}: {}, {:.1} °C, {:.0} %, {:.1} mm, {:.1} km/h",
                    weather.updated_at,
                    open_meteo::condition_label(weather.condition),
                    weather.temperature,
                    weather.humidity,
                    weather.precipitation,
                    weather.wind_speed
                );
                known = Some(weather.clone());
                if !handle
                    .act(move |node| node.update(weather, Instant::now(), unix_now()))
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
        let fetch_at = tokio::time::Instant::now() + wait;
        loop {
            let sun_change = known
                .as_ref()
                .and_then(|weather| weather.next_sun_change(unix_now()));
            let sun = async {
                match sun_change {
                    Some(at) => {
                        let seconds = (at - unix_now()).max(0) as u64 + 1;
                        tokio::time::sleep(Duration::from_secs(seconds)).await;
                    }
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = tokio::time::sleep_until(fetch_at) => break,
                () = sun => {
                    if !handle.act(|node| node.sun(unix_now())).await {
                        return Ok(());
                    }
                }
                () = &mut shutdown => return Ok(()),
            }
        }
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}
