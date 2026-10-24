//! A software node with the weather outside, from Open-Meteo.

mod node;
mod open_meteo;

use std::ops::RangeInclusive;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Context;
use oxihome_node::config::{BrokerConfig, read_config};
use oxihome_node::mqtt;
use serde::Deserialize;

use node::WeatherNode;

const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// After a failed fetch; doubles on each failure in a row, up to the fetch interval.
const FIRST_RETRY: Duration = Duration::from_secs(60);
const INTERVAL_MINUTES: RangeInclusive<u64> = 5..=60;
/// Fetches run this long after the clock passes a multiple of the interval, once Open-Meteo has
/// the new quarter-hour.
const FETCH_DELAY: i64 = 60;

#[derive(Deserialize)]
struct Config {
    mqtt: BrokerConfig,
    node: NodeConfig,
    location: Location,
    #[serde(default)]
    fetch: FetchConfig,
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

#[derive(Deserialize)]
#[serde(default)]
struct FetchConfig {
    /// Minutes between fetches.
    interval: u64,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self { interval: 15 }
    }
}

impl Config {
    fn check(&self) -> anyhow::Result<()> {
        let Location {
            latitude,
            longitude,
        } = self.location;
        anyhow::ensure!(
            (-90.0..=90.0).contains(&latitude),
            "[location] latitude must be between -90 and 90, not {latitude}"
        );
        anyhow::ensure!(
            (-180.0..=180.0).contains(&longitude),
            "[location] longitude must be between -180 and 180, not {longitude}"
        );
        let interval = self.fetch.interval;
        anyhow::ensure!(
            INTERVAL_MINUTES.contains(&interval),
            "[fetch] interval must be {} to {} minutes, not {interval}",
            INTERVAL_MINUTES.start(),
            INTERVAL_MINUTES.end()
        );
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (path, config): (String, Config) = read_config("oxihome-weather <weather.toml>")?;
    config.check().with_context(|| path.clone())?;
    let interval = Duration::from_secs(config.fetch.interval * 60);

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
                until_next_fetch(unix_now(), interval)
            }
            Err(error) => {
                println!("fetching the weather failed: {error:#}");
                if !handle.act(|node| node.failed(Instant::now())).await {
                    return Ok(());
                }
                let wait = retry;
                retry = (retry * 2).min(interval);
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

/// To the next multiple of `interval` in clock time, plus `FETCH_DELAY`.
fn until_next_fetch(unix_now: i64, interval: Duration) -> Duration {
    let interval = interval.as_secs() as i64;
    let next = (unix_now - FETCH_DELAY).div_euclid(interval) * interval + interval + FETCH_DELAY;
    Duration::from_secs((next - unix_now) as u64)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(extra: &str) -> anyhow::Result<()> {
        let text =
            format!("[mqtt]\nhost = \"broker\"\nport = 1883\n[node]\nid = \"weather\"\n{extra}");
        toml::from_str::<Config>(&text)?.check()
    }

    #[test]
    fn the_interval_defaults_to_a_quarter_hour_and_the_location_is_checked() {
        let place = "[location]\nlatitude = -30.04\nlongitude = -51.22\n";
        let text =
            format!("[mqtt]\nhost = \"broker\"\nport = 1883\n[node]\nid = \"weather\"\n{place}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap().fetch.interval, 15);
        assert!(config(place).is_ok());
        assert!(config(&format!("{place}[fetch]\ninterval = 10\n")).is_ok());
        assert!(config(&format!("{place}[fetch]\ninterval = 1\n")).is_err());
        assert!(config("[location]\nlatitude = -300\nlongitude = -51.22\n").is_err());
        assert!(config("[location]\nlatitude = -30\nlongitude = 200\n").is_err());
    }

    #[test]
    fn fetches_follow_the_clock() {
        let quarter = Duration::from_secs(15 * 60);
        let nine_fifteen = 1792840500;
        let at = |seconds: i64| until_next_fetch(nine_fifteen + seconds, quarter).as_secs();
        assert_eq!(at(30), 30, "a minute after the quarter-hour");
        assert_eq!(at(60), 15 * 60, "just fetched");
        assert_eq!(at(61), 15 * 60 - 1);
        assert_eq!(at(-5 * 60), 6 * 60);
    }
}
