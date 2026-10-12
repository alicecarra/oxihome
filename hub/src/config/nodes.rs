use std::collections::HashMap;

use anyhow::bail;
use oxihome_protocol::management::{HEARTBEAT_INTERVAL, Parameter, WIFI_CHECK_INTERVAL};
use serde::Deserialize;
use toml_edit::{DocumentMut, InlineTable, Item, Table, TableLike, Value};

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

#[derive(Debug, Default, PartialEq)]
pub struct EntityEdit {
    pub id: u8,
    /// `None` leaves the name as it is, `Some(None)` removes it.
    pub name: Option<Option<String>>,
    /// Channel id and name.
    pub channels: Vec<(u8, Option<String>)>,
}

/// Changes to one node's section. Names and parameters left out stay as they are; a `None`
/// value removes them.
#[derive(Debug, Default)]
pub struct NodeEdits {
    /// `None` leaves the name as it is, `Some(None)` removes it.
    pub name: Option<Option<String>>,
    pub entities: Vec<EntityEdit>,
    /// `nodes.toml` key and value.
    pub parameters: Vec<(&'static str, Option<i32>)>,
}

/// `nodes.toml` with one node's names and parameters changed, keeping the file's comments and
/// layout. Entities, channels and parameters left empty are removed.
pub fn with_node_edits(text: &str, device_id: &str, edits: &NodeEdits) -> anyhow::Result<String> {
    let mut document: DocumentMut = text.parse()?;
    let nodes = document
        .entry("nodes")
        .or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        })
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`nodes` is not a table"))?;
    let node = nodes
        .entry(device_id)
        .or_insert_with(|| Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(|| anyhow::anyhow!("node \"{device_id}\" is not a table"))?;

    if let Some(name) = &edits.name {
        set_or_remove(node, "name", name.clone().map(Value::from));
    }

    for EntityEdit { id, name, channels } in &edits.entities {
        let key = id.to_string();
        let entities = child_table(node, "entities", true)?;
        let entity_names = entities
            .entry(&key)
            .or_insert(Item::Value(Value::InlineTable(InlineTable::new())))
            .as_table_like_mut()
            .ok_or_else(|| anyhow::anyhow!("entity {key} is not a table"))?;
        if let Some(name) = name {
            set_or_remove(entity_names, "name", name.clone().map(Value::from));
        }
        for (channel, channel_name) in channels {
            let channel_names = child_table(entity_names, "channels", true)?;
            set_or_remove(
                channel_names,
                &channel.to_string(),
                channel_name.clone().map(Value::from),
            );
        }
        remove_if_empty(entity_names, "channels");
        remove_if_empty(entities, &key);
    }
    remove_if_empty(node, "entities");

    for (key, value) in &edits.parameters {
        let parameters = child_table(node, "parameters", false)?;
        set_or_remove(
            parameters,
            key,
            value.map(|value| Value::from(i64::from(value))),
        );
    }
    remove_if_empty(node, "parameters");
    // An emptied node keeps its `[nodes."..."]` line: comments above it live in its header.
    Ok(document.to_string())
}

fn set_or_remove(table: &mut dyn TableLike, key: &str, value: Option<Value>) {
    match value {
        Some(value) => {
            table.insert(key, Item::Value(value));
        }
        None => {
            table.remove(key);
        }
    }
}

/// A new one is an inline table, dotted (`entities.1 = ...`) when `dotted`.
fn child_table<'a>(
    parent: &'a mut dyn TableLike,
    key: &str,
    dotted: bool,
) -> anyhow::Result<&'a mut dyn TableLike> {
    parent
        .entry(key)
        .or_insert_with(|| {
            let mut table = InlineTable::new();
            table.set_dotted(dotted);
            Item::Value(Value::InlineTable(table))
        })
        .as_table_like_mut()
        .ok_or_else(|| anyhow::anyhow!("`{key}` is not a table"))
}

fn remove_if_empty(parent: &mut dyn TableLike, key: &str) {
    if parent
        .get(key)
        .and_then(Item::as_table_like)
        .is_some_and(TableLike::is_empty)
    {
        parent.remove(key);
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

    const FILE: &str = r#"# Friendly names and parameters.

[nodes."esp32-test"]
name = "Test board"
entities.1 = { name = "LED", channels.0 = "Power" }
parameters = { heartbeat_interval = 30, wifi_check_interval = 5 }
"#;

    #[test]
    fn edits_keep_comments_and_style() {
        let edits = NodeEdits {
            name: Some(Some("Bench board".into())),
            entities: vec![
                EntityEdit {
                    id: 1,
                    name: Some(Some("Blue LED".into())),
                    channels: vec![(0, Some("On".into()))],
                },
                EntityEdit {
                    id: 4,
                    name: Some(Some("External LED".into())),
                    channels: vec![(0, Some("Power".into()))],
                },
            ],
            parameters: vec![("heartbeat_interval", Some(60))],
        };
        let text = with_node_edits(FILE, DEVICE, &edits).unwrap();
        assert!(
            text.starts_with("# Friendly names and parameters."),
            "{text}"
        );
        assert!(text.contains("name = \"Bench board\""), "{text}");
        assert!(
            text.contains("entities.1 = { name = \"Blue LED\", channels.0 = \"On\" }"),
            "{text}"
        );
        assert!(
            text.contains("entities.4 = { name = \"External LED\", channels.0 = \"Power\" }"),
            "{text}"
        );
        assert!(
            text.contains("heartbeat_interval = 60, wifi_check_interval = 5"),
            "{text}"
        );
        let nodes = parse(&text).unwrap();
        assert_eq!(
            nodes.name(&NameTarget::Entity(DEVICE.into(), 4)),
            Some("External LED")
        );
        assert_eq!(nodes.desired(DEVICE)[0], (HEARTBEAT_INTERVAL, 60));
    }

    #[test]
    fn empty_values_remove_keys_and_empty_parts() {
        let edits = NodeEdits {
            name: Some(None),
            entities: vec![EntityEdit {
                id: 1,
                name: Some(None),
                channels: vec![(0, None)],
            }],
            parameters: vec![("heartbeat_interval", None), ("wifi_check_interval", None)],
        };
        let text = with_node_edits(FILE, DEVICE, &edits).unwrap();
        assert_eq!(
            text,
            "# Friendly names and parameters.\n\n[nodes.\"esp32-test\"]\n"
        );
    }

    #[test]
    fn names_left_out_stay() {
        let edits = NodeEdits {
            name: None,
            entities: vec![EntityEdit {
                id: 1,
                name: None,
                channels: vec![(0, Some("On".into()))],
            }],
            parameters: vec![],
        };
        let text = with_node_edits(FILE, DEVICE, &edits).unwrap();
        assert!(text.contains("name = \"Test board\""), "{text}");
        assert!(
            text.contains("entities.1 = { name = \"LED\", channels.0 = \"On\" }"),
            "{text}"
        );
    }

    #[test]
    fn a_new_node_gets_its_own_section() {
        let edits = NodeEdits {
            name: Some(Some("Kitchen".into())),
            entities: vec![EntityEdit {
                id: 2,
                name: None,
                channels: vec![(0, Some("Press".into()))],
            }],
            parameters: vec![],
        };
        let text = with_node_edits(FILE, "esp32-new", &edits).unwrap();
        assert!(text.contains("[nodes.esp32-new]"), "{text}");
        let nodes = parse(&text).unwrap();
        assert_eq!(
            nodes.name(&NameTarget::Node("esp32-new".into())),
            Some("Kitchen")
        );
        assert_eq!(
            nodes.name(&NameTarget::Channel("esp32-new".into(), 2, 0)),
            Some("Press")
        );
        assert_eq!(
            nodes.name(&NameTarget::Node(DEVICE.into())),
            Some("Test board")
        );
    }
}
