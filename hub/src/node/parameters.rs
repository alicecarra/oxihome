use std::collections::HashMap;
use std::sync::Arc;

use anyhow::bail;
use oxihome_protocol::Value;
use oxihome_protocol::management::{self, HEARTBEAT_INTERVAL, Parameter, WIFI_CHECK_INTERVAL};
use serde::Deserialize;

use crate::hub::Hub;

/// Desired operation parameters from `nodes.toml`. The hub is the source of truth: a node
/// that reports a different value gets a `Set`.
#[derive(Debug, Default, Deserialize)]
pub struct NodeParameters {
    #[serde(default)]
    nodes: HashMap<String, DesiredParameters>,
}

/// The `nodes.toml` key of each parameter.
const KEYS: [(Parameter, &str); 2] = [
    (HEARTBEAT_INTERVAL, "heartbeat_interval"),
    (WIFI_CHECK_INTERVAL, "wifi_check_interval"),
];

pub fn key(parameter: Parameter) -> &'static str {
    KEYS.iter()
        .find(|(known, _)| *known == parameter)
        .map_or("unknown", |(_, key)| key)
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DesiredParameters {
    heartbeat_interval: Option<i32>,
    wifi_check_interval: Option<i32>,
}

impl DesiredParameters {
    fn values(&self) -> Vec<(Parameter, i32)> {
        [
            (HEARTBEAT_INTERVAL, self.heartbeat_interval),
            (WIFI_CHECK_INTERVAL, self.wifi_check_interval),
        ]
        .into_iter()
        .filter_map(|(parameter, value)| Some((parameter, value?)))
        .collect()
    }
}

impl NodeParameters {
    pub fn desired(&self, device_id: &str) -> Vec<(Parameter, i32)> {
        self.nodes
            .get(device_id)
            .map(DesiredParameters::values)
            .unwrap_or_default()
    }

    /// Rejects values the nodes would refuse.
    pub fn validate(&self) -> anyhow::Result<()> {
        for (device_id, desired) in &self.nodes {
            for (parameter, value) in desired.values() {
                if !parameter.accepts(value) {
                    bail!(
                        "{device_id}: {} = {value} outside {}..={}",
                        key(parameter),
                        parameter.min,
                        parameter.max
                    );
                }
            }
        }
        Ok(())
    }
}

/// Sends a `Set` for every parameter whose reported value differs from the desired one.
pub async fn apply(hub: Arc<Hub>, device_id: String) {
    for (parameter, value) in hub.parameter_differences(&device_id) {
        let channel = u8::from(parameter.channel);
        match hub
            .set(&device_id, management::ENTITY, channel, Value::Int(value))
            .await
        {
            Ok(()) => println!("{device_id}: {:?} set to {value}", parameter.channel),
            Err(error) => println!("{device_id}: {:?} not set: {error}", parameter.channel),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> NodeParameters {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn desired_values_of_a_node() {
        let parameters = parse(
            r#"
            [nodes."esp32-test"]
            heartbeat_interval = 60
            "#,
        );
        assert_eq!(
            parameters.desired("esp32-test"),
            vec![(HEARTBEAT_INTERVAL, 60)]
        );
        assert!(parameters.desired("other").is_empty());
    }

    #[test]
    fn out_of_range_value_is_rejected() {
        let parameters = parse(
            r#"
            [nodes."esp32-test"]
            wifi_check_interval = 0
            "#,
        );
        let message = parameters.validate().unwrap_err().to_string();
        assert!(message.contains("esp32-test"), "{message}");
    }

    #[test]
    fn unknown_parameter_is_rejected() {
        let result = toml::from_str::<NodeParameters>(
            r#"
            [nodes."esp32-test"]
            heartbeat = 60
            "#,
        );
        assert!(result.is_err());
    }
}
