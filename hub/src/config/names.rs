use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NameTarget {
    Node(String),
    Entity(String, u8),
    Channel(String, u8, u8),
}

/// Friendly names from `names.toml`.
#[derive(Debug, Default, Deserialize)]
pub struct Names {
    #[serde(default)]
    nodes: HashMap<String, NodeNames>,
}

#[derive(Debug, Default, Deserialize)]
struct NodeNames {
    name: Option<String>,
    /// Keyed by entity id; TOML keys are strings.
    #[serde(default)]
    entities: HashMap<String, EntityNames>,
}

#[derive(Debug, Default, Deserialize)]
struct EntityNames {
    name: Option<String>,
    /// Channel id to name.
    #[serde(default)]
    channels: HashMap<String, String>,
}

impl Names {
    pub fn get(&self, target: &NameTarget) -> Option<&str> {
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
    "#;

    #[test]
    fn names_by_target() {
        let names: Names = toml::from_str(EXAMPLE).unwrap();

        assert_eq!(
            names.get(&NameTarget::Node(DEVICE.into())),
            Some("Test board")
        );
        assert_eq!(
            names.get(&NameTarget::Entity(DEVICE.into(), 1)),
            Some("LED")
        );
        assert_eq!(
            names.get(&NameTarget::Channel(DEVICE.into(), 1, 0)),
            Some("Power")
        );
        assert_eq!(
            names.get(&NameTarget::Entity(DEVICE.into(), 2)),
            Some("BOOT button")
        );
        assert_eq!(names.get(&NameTarget::Channel(DEVICE.into(), 2, 0)), None);
        assert_eq!(names.get(&NameTarget::Node("other".into())), None);
    }

    #[test]
    fn empty_file_has_no_names() {
        let names: Names = toml::from_str("").unwrap();
        assert_eq!(names.get(&NameTarget::Node(DEVICE.into())), None);
    }
}
