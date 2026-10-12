use std::collections::HashMap;

use anyhow::bail;
use oxihome_protocol::management::{HEARTBEAT_INTERVAL, Parameter, WIFI_CHECK_INTERVAL};
use serde::Deserialize;

/// What `nodes.toml` says about each node, by the id from its `Hello`: friendly names and the
/// desired operation parameters. The hub is the source of truth for the parameters: a node that
/// reports another value gets a `Set`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Nodes {
    #[serde(default)]
    nodes: HashMap<String, NodeConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeConfig {
    name: Option<String>,
    /// Keyed by entity id; TOML keys are strings.
    #[serde(default)]
    entities: HashMap<String, EntityConfig>,
    #[serde(default)]
    parameters: Parameters,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityConfig {
    name: Option<String>,
    /// Channel id to name.
    #[serde(default)]
    channels: HashMap<String, String>,
}

/// In seconds; a missing one keeps the node's value.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Parameters {
    heartbeat_interval: Option<i32>,
    wifi_check_interval: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NameTarget {
    Node(String),
    Entity(String, u8),
    Channel(String, u8, u8),
}

/// The `nodes.toml` key of each parameter.
const PARAMETER_KEYS: [(Parameter, &str); 2] = [
    (HEARTBEAT_INTERVAL, "heartbeat_interval"),
    (WIFI_CHECK_INTERVAL, "wifi_check_interval"),
];

pub fn parameter_key(parameter: Parameter) -> &'static str {
    PARAMETER_KEYS
        .iter()
        .find(|(known, _)| *known == parameter)
        .map_or("unknown", |(_, key)| key)
}

impl Parameters {
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

impl Nodes {
    pub fn name(&self, target: &NameTarget) -> Option<&str> {
        match target {
            NameTarget::Node(device_id) => self.nodes.get(device_id)?.name.as_deref(),
            NameTarget::Entity(device_id, entity) => self
                .nodes
                .get(device_id)?
                .entities
                .get(&entity.to_string())?
                .name
                .as_deref(),
            NameTarget::Channel(device_id, entity, channel) => self
                .nodes
                .get(device_id)?
                .entities
                .get(&entity.to_string())?
                .channels
                .get(&channel.to_string())
                .map(String::as_str),
        }
    }

    pub fn desired(&self, device_id: &str) -> Vec<(Parameter, i32)> {
        self.nodes
            .get(device_id)
            .map(|node| node.parameters.values())
            .unwrap_or_default()
    }

    /// Rejects parameter values the nodes would refuse.
    pub fn validate(&self) -> anyhow::Result<()> {
        for (device_id, node) in &self.nodes {
            for (parameter, value) in node.parameters.values() {
                if !parameter.accepts(value) {
                    bail!(
                        "{device_id}: {} = {value} outside {}..={}",
                        parameter_key(parameter),
                        parameter.min,
                        parameter.max
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "esp32-test";

    const EXAMPLE: &str = r#"
        [nodes."esp32-test"]
        name = "Test board"
        entities.1 = { name = "LED", channels.0 = "Power" }
        entities.2 = { name = "BOOT button" }
        parameters = { heartbeat_interval = 60 }
    "#;

    fn parse(text: &str) -> Result<Nodes, toml::de::Error> {
        toml::from_str(text)
    }

    #[test]
    fn names_by_target() {
        let nodes = parse(EXAMPLE).unwrap();
        assert_eq!(
            nodes.name(&NameTarget::Node(DEVICE.into())),
            Some("Test board")
        );
        assert_eq!(
            nodes.name(&NameTarget::Entity(DEVICE.into(), 1)),
            Some("LED")
        );
        assert_eq!(
            nodes.name(&NameTarget::Channel(DEVICE.into(), 1, 0)),
            Some("Power")
        );
        assert_eq!(
            nodes.name(&NameTarget::Entity(DEVICE.into(), 2)),
            Some("BOOT button")
        );
        assert_eq!(nodes.name(&NameTarget::Channel(DEVICE.into(), 2, 0)), None);
        assert_eq!(nodes.name(&NameTarget::Node("other".into())), None);
    }

    #[test]
    fn desired_parameters_of_a_node() {
        let nodes = parse(EXAMPLE).unwrap();
        assert_eq!(nodes.desired(DEVICE), vec![(HEARTBEAT_INTERVAL, 60)]);
        assert!(nodes.desired("other").is_empty());
    }

    #[test]
    fn empty_file_has_nothing() {
        let nodes = parse("").unwrap();
        assert_eq!(nodes.name(&NameTarget::Node(DEVICE.into())), None);
        assert!(nodes.desired(DEVICE).is_empty());
    }

    #[test]
    fn out_of_range_parameter_is_rejected() {
        let nodes =
            parse("[nodes.\"esp32-test\"]\nparameters = { wifi_check_interval = 0 }").unwrap();
        let message = nodes.validate().unwrap_err().to_string();
        assert!(message.contains("esp32-test"), "{message}");
    }

    #[test]
    fn misspelled_keys_are_rejected() {
        assert!(parse("[nodes.\"esp32-test\"]\nparameters = { heartbeat = 60 }").is_err());
        assert!(parse("[nodes.\"esp32-test\"]\nnmae = \"Test board\"").is_err());
        assert!(parse("[nodes.\"esp32-test\"]\nentities.1 = { nmae = \"LED\" }").is_err());
    }
}
