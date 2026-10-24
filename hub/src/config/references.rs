//! Where the config names a node's channels, to tell when the node stops declaring them.

use oxihome_protocol::EntityDescriptor;

use super::dashboard::{Block, Dashboards};
use super::nodes::Nodes;
use super::rules::Rules;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// `rule "Hot"`, `dashboard "Home", block "Outside"`, `nodes.toml`.
    pub place: String,
    pub entity: u8,
    /// `None` for an entity's name in `nodes.toml`.
    pub channel: Option<u8>,
}

impl Reference {
    /// `rule "Hot": 3/0`.
    pub fn label(&self) -> String {
        match self.channel {
            Some(channel) => format!("{}: {}/{channel}", self.place, self.entity),
            None => format!("{}: entity {}", self.place, self.entity),
        }
    }
}

/// In file order: rules, dashboards, then names in `nodes.toml`.
pub fn references(
    device_id: &str,
    rules: &Rules,
    dashboards: &Dashboards,
    nodes: &Nodes,
) -> Vec<Reference> {
    let mut references: Vec<Reference> = Vec::new();
    let mut add = |place: &str, entity, channel| {
        let reference = Reference {
            place: place.to_string(),
            entity,
            channel,
        };
        if !references.contains(&reference) {
            references.push(reference);
        }
    };
    for rule in rules.all() {
        let place = format!("rule \"{}\"", rule.name);
        for (device, entity, channel) in rule.channels() {
            if device == device_id {
                add(&place, entity, Some(channel));
            }
        }
    }
    for dashboard in dashboards.all() {
        for block in &dashboard.blocks {
            let place = format!(
                "dashboard \"{}\", block \"{}\"",
                dashboard.name,
                block_name(block)
            );
            for (entity, channel) in block_channels(block, device_id) {
                add(&place, entity, Some(channel));
            }
        }
    }
    for entity in nodes.entities(device_id) {
        if entity.name.is_some() {
            add("nodes.toml", entity.id, None);
        }
        for (channel, _) in &entity.channels {
            add("nodes.toml", entity.id, Some(*channel));
        }
    }
    references
}

/// The references to entities or channels the node does not declare.
pub fn missing(references: Vec<Reference>, entities: &[EntityDescriptor]) -> Vec<Reference> {
    references
        .into_iter()
        .filter(|reference| {
            let entity = entities.iter().find(|entity| entity.id == reference.entity);
            match (entity, reference.channel) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(entity), Some(channel)) => !entity
                    .channels
                    .iter()
                    .any(|declared| declared.id == channel),
            }
        })
        .collect()
}

fn block_name(block: &Block) -> String {
    block
        .title
        .clone()
        .unwrap_or_else(|| format!("{:?}", block.kind).to_lowercase())
}

fn block_channels(block: &Block, device_id: &str) -> Vec<(u8, u8)> {
    let mut channels: Vec<(u8, u8)> = block
        .channels
        .iter()
        .chain(&block.channel)
        .filter(|reference| reference.device == device_id)
        .map(|reference| (reference.entity, reference.channel))
        .collect();
    if block.device.as_deref() == Some(device_id) {
        channels.extend(
            block
                .state
                .iter()
                .chain(&block.progress)
                .chain(&block.left)
                .chain(&block.details)
                .map(|job| (job.entity, job.channel)),
        );
    }
    channels
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::{Access, Channel, ValueType};

    use super::*;

    const RULES: &str = r#"
        [[rules]]
        name = "Started raining"
        when = { device = "weather", entity = 1, channel = 2, above = 0 }
        if = [{ device = "weather", entity = 3, channel = 0, equals = "x" }]
        then = [{ notify = { title = "Rain", message = "{value}" } }]

        [[rules]]
        name = "Other node"
        when = { device = "board", entity = 3, channel = 0, above = 0 }
        then = [{ notify = { title = "Board", message = "{value}" } }]
    "#;

    const DASHBOARDS: &str = r#"
        [[dashboards]]
        name = "Home"

        [[dashboards.blocks]]
        type = "channels"
        channels = [
            { device = "weather", entity = 1, channel = 0 },
            { device = "weather", entity = 1, channel = 9 },
        ]

        [[dashboards.blocks]]
        type = "value"
        title = "Updated"
        channel = { device = "weather", entity = 3, channel = 0 }

        [[dashboards.blocks]]
        type = "job"
        device = "weather"
        state = { entity = 4, channel = 0 }
    "#;

    const NODES: &str = r#"
        [nodes.weather]
        name = "Weather"
        entities.1 = { name = "Now", channels.0 = "Outside" }
        entities.3 = { name = "Old" }
    "#;

    fn entity(id: u8, channels: &[u8]) -> EntityDescriptor {
        EntityDescriptor {
            id,
            name: None,
            channels: channels
                .iter()
                .map(|&id| Channel {
                    id,
                    name: None,
                    access: Access::Read,
                    value: Some(ValueType::Bool),
                    quantity: None,
                    unit: None,
                })
                .collect(),
        }
    }

    fn labels(references: &[Reference]) -> Vec<String> {
        references.iter().map(Reference::label).collect()
    }

    #[test]
    fn rules_dashboards_and_names_that_point_at_undeclared_channels() {
        let rules: Rules = toml::from_str(RULES).unwrap();
        let dashboards: Dashboards = toml::from_str(DASHBOARDS).unwrap();
        let nodes: Nodes = toml::from_str(NODES).unwrap();
        let references = references("weather", &rules, &dashboards, &nodes);
        assert_eq!(references.len(), 9);

        let declared = [entity(1, &[0, 1, 2]), entity(4, &[0, 1])];
        assert_eq!(
            labels(&missing(references, &declared)),
            [
                "rule \"Started raining\": 3/0",
                "dashboard \"Home\", block \"channels\": 1/9",
                "dashboard \"Home\", block \"Updated\": 3/0",
                "nodes.toml: entity 3",
            ]
        );
    }

    #[test]
    fn nothing_is_missing_when_every_channel_is_declared() {
        let rules: Rules = toml::from_str(RULES).unwrap();
        let references = references("board", &rules, &Dashboards::default(), &Nodes::default());
        assert_eq!(labels(&references), ["rule \"Other node\": 3/0"]);
        assert!(missing(references, &[entity(3, &[0])]).is_empty());
    }
}
