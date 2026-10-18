//! The configuration file of a software node: the path is the first argument, the format TOML,
//! with an `[mqtt]` section that every node shares.

use std::path::Path;

use anyhow::Context;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::mqtt::{self, Broker};

/// The `[mqtt]` section.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BrokerConfig {
    pub host: String,
    pub port: u16,
    /// The node's password on the broker; anonymous when missing.
    pub password: Option<String>,
    /// Outgoing messages waiting for the connection; `mqtt::DEFAULT_QUEUE_SIZE` when missing.
    pub queue_size: Option<usize>,
}

impl BrokerConfig {
    /// Reports every `mqtt::REPORT_INTERVAL`.
    pub fn broker(self) -> Broker {
        Broker {
            host: self.host,
            port: self.port,
            password: self.password,
            report_interval: mqtt::REPORT_INTERVAL,
            queue_size: self.queue_size.unwrap_or(mqtt::DEFAULT_QUEUE_SIZE),
        }
    }
}

/// The file named by the first argument, with its path for messages; `usage` is the error
/// without one, e.g. "oxihome-rss <rss.toml>".
pub fn read_config<T: DeserializeOwned>(usage: &str) -> anyhow::Result<(String, T)> {
    let path = std::env::args()
        .nth(1)
        .with_context(|| format!("usage: {usage}"))?;
    let config = parse_file(&path)?;
    Ok((path, config))
}

pub fn parse_file<T: DeserializeOwned>(path: impl AsRef<Path>) -> anyhow::Result<T> {
    let path = path.as_ref();
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Config {
        mqtt: BrokerConfig,
    }

    fn write(name: &str, text: &str) -> String {
        let path =
            std::env::temp_dir().join(format!("oxihome-node-{name}-{}.toml", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn optional_fields_take_their_defaults() {
        let path = write("minimal", "[mqtt]\nhost = \"broker\"\nport = 1883\n");
        let broker = parse_file::<Config>(&path).unwrap().mqtt.broker();
        assert_eq!((broker.host.as_str(), broker.port), ("broker", 1883));
        assert_eq!(broker.password, None);
        assert_eq!(broker.queue_size, mqtt::DEFAULT_QUEUE_SIZE);
        assert_eq!(broker.report_interval, mqtt::REPORT_INTERVAL);

        let path = write(
            "full",
            "[mqtt]\nhost = \"broker\"\nport = 1883\npassword = \"secret\"\nqueue_size = 10\n",
        );
        let broker = parse_file::<Config>(&path).unwrap().mqtt.broker();
        assert_eq!(broker.password.as_deref(), Some("secret"));
        assert_eq!(broker.queue_size, 10);
    }

    #[test]
    fn errors_name_the_file() {
        let missing = parse_file::<Config>("/nonexistent/node.toml").unwrap_err();
        assert_eq!(missing.to_string(), "reading /nonexistent/node.toml");

        let path = write("broken", "[mqtt]\nhost = 1\n");
        let broken = parse_file::<Config>(&path).unwrap_err();
        assert_eq!(broken.to_string(), format!("parsing {path}"));
    }
}
