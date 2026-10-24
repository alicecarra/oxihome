//! What a rule can name, and the choices the selects offer.

use oxihome_protocol::management;
use oxihome_protocol::{Access, Channel, ValueType};

use super::draft::{ChannelKey, pair, parse_pair};
use crate::config::nodes::NameTarget;
use crate::hub::Hub;
use crate::web::format::{channel_label, entity_label, value_label};

/// What the hub knows that a rule can name.
pub(super) struct Catalog {
    pub(super) channels: Vec<ChannelChoice>,
    /// Device id and name, by name.
    pub(super) nodes: Vec<(String, String)>,
    /// Remote and key.
    pub(super) remote_keys: Vec<(String, String)>,
}

pub(super) struct ChannelChoice {
    pub(super) key: ChannelKey,
    pub(super) node: String,
    /// `LED`, or `Climate sensor · Humidity` for a channel after the first.
    pub(super) name: String,
    pub(super) declared: Channel,
    /// The value it has now, with its unit.
    pub(super) now: Option<String>,
}

/// Which channels fit where.
#[derive(Clone, Copy)]
pub(super) enum Purpose {
    Event,
    Condition,
    Switch,
    Invoke,
}

impl Purpose {
    fn fits(self, channel: &Channel) -> bool {
        let readable = matches!(channel.access, Access::Read | Access::ReadWrite);
        match self {
            Purpose::Event => channel.access == Access::Event,
            Purpose::Condition => readable && channel.value.is_some(),
            Purpose::Switch => {
                channel.value == Some(ValueType::Bool)
                    && matches!(channel.access, Access::Write | Access::ReadWrite)
            }
            Purpose::Invoke => channel.access == Access::Trigger,
        }
    }
}

impl Catalog {
    /// Reads everything from one snapshot: no other `Hub` call while it lives.
    pub(super) fn new(hub: &Hub) -> Self {
        let snapshot = hub.snapshot();
        let name = |target: NameTarget| {
            snapshot
                .nodes
                .name(&target)
                .or_else(|| snapshot.registry.declared_name(&target))
                .map(String::from)
        };
        let mut nodes: Vec<(String, String)> = snapshot
            .registry
            .nodes()
            .into_iter()
            .map(|(device_id, _)| {
                let node_name = name(NameTarget::Node(device_id.clone()));
                (
                    device_id.clone(),
                    node_name.unwrap_or_else(|| device_id.clone()),
                )
            })
            .collect();
        nodes.sort_by(|first, second| first.1.cmp(&second.1));
        let mut channels = Vec::new();
        for (device_id, node_name) in &nodes {
            let Some(node) = snapshot.registry.node(device_id) else {
                continue;
            };
            for entity in node
                .entities
                .iter()
                .filter(|entity| entity.id != management::ENTITY)
            {
                let entity_name = entity_label(
                    entity.id,
                    &name(NameTarget::Entity(device_id.clone(), entity.id)),
                );
                for channel in &entity.channels {
                    let channel_name = if channel.id == 0 {
                        entity_name.clone()
                    } else {
                        let named = name(NameTarget::Channel(
                            device_id.clone(),
                            entity.id,
                            channel.id,
                        ));
                        format!(
                            "{entity_name} · {}",
                            channel_label(channel.id, &named, channel.quantity)
                        )
                    };
                    channels.push(ChannelChoice {
                        key: ChannelKey {
                            device: device_id.clone(),
                            entity: entity.id,
                            channel: channel.id,
                        },
                        node: node_name.clone(),
                        name: channel_name,
                        declared: channel.clone(),
                        now: snapshot
                            .state
                            .value(device_id, entity.id, channel.id)
                            .map(|value| value_label(value, channel.unit)),
                    });
                }
            }
        }
        let remote_keys = snapshot
            .remotes
            .all()
            .iter()
            .flat_map(|(remote_name, remote)| {
                remote
                    .keys
                    .keys()
                    .map(move |key| (remote_name.clone(), key.clone()))
            })
            .collect();
        Self {
            channels,
            nodes,
            remote_keys,
        }
    }

    pub(super) fn declared(&self, key: &str) -> Option<&Channel> {
        let key = ChannelKey::parse(key)?;
        self.channels
            .iter()
            .find(|choice| choice.key == key)
            .map(|choice| &choice.declared)
    }

    pub(super) fn now(&self, key: &str) -> Option<String> {
        let key = ChannelKey::parse(key)?;
        self.channels
            .iter()
            .find(|choice| choice.key == key)?
            .now
            .clone()
    }

    pub(super) fn channel_groups(&self, purpose: Purpose, chosen: &str) -> Vec<ChoiceGroup> {
        let mut groups: Vec<ChoiceGroup> = Vec::new();
        for choice in self
            .channels
            .iter()
            .filter(|choice| purpose.fits(&choice.declared))
        {
            let option = Choice {
                value: ChannelKey::text(&choice.key.device, choice.key.entity, choice.key.channel),
                text: choice.name.clone(),
            };
            match groups.last_mut() {
                Some(group) if group.label == choice.node => group.choices.push(option),
                _ => groups.push(ChoiceGroup {
                    label: choice.node.clone(),
                    choices: vec![option],
                }),
            }
        }
        keep_chosen(groups, chosen, || {
            ChannelKey::parse(chosen).map_or_else(
                || chosen.to_string(),
                |key| {
                    format!(
                        "{} {}/{} (not declared)",
                        key.device, key.entity, key.channel
                    )
                },
            )
        })
    }

    pub(super) fn node_groups(&self, chosen: &str, any: bool) -> Vec<ChoiceGroup> {
        let mut choices: Vec<Choice> = Vec::new();
        if any {
            choices.push(Choice::new("*", "Any node"));
        }
        choices.extend(
            self.nodes
                .iter()
                .map(|(device_id, node_name)| Choice::new(device_id, node_name)),
        );
        keep_chosen(ungrouped(choices), chosen, || {
            format!("{chosen} (not seen)")
        })
    }
}

/// `(remote, key)` pairs grouped by remote.
pub(super) fn pair_groups(pairs: &[(String, String)], chosen: &str) -> Vec<ChoiceGroup> {
    let mut groups: Vec<ChoiceGroup> = Vec::new();
    for (group_name, item) in pairs {
        let option = Choice {
            value: pair(group_name, item),
            text: item.clone(),
        };
        match groups.last_mut() {
            Some(group) if &group.label == group_name => group.choices.push(option),
            _ => groups.push(ChoiceGroup {
                label: group_name.clone(),
                choices: vec![option],
            }),
        }
    }
    keep_chosen(groups, chosen, || {
        parse_pair(chosen).map_or_else(
            || chosen.to_string(),
            |(group_name, item)| format!("{group_name} › {item} (missing)"),
        )
    })
}

#[derive(Clone)]
pub(super) struct Choice {
    pub(super) value: String,
    pub(super) text: String,
}

impl Choice {
    pub(super) fn new(value: &str, text: &str) -> Self {
        Self {
            value: value.into(),
            text: text.into(),
        }
    }
}

/// Shown under a heading, or loose when `label` is empty.
#[derive(Clone)]
pub(super) struct ChoiceGroup {
    pub(super) label: String,
    pub(super) choices: Vec<Choice>,
}

pub(super) fn ungrouped(choices: Vec<Choice>) -> Vec<ChoiceGroup> {
    vec![ChoiceGroup {
        label: String::new(),
        choices,
    }]
}

/// A rule may name something the hub does not know (any more); it stays chosen.
fn keep_chosen(
    mut groups: Vec<ChoiceGroup>,
    chosen: &str,
    text: impl FnOnce() -> String,
) -> Vec<ChoiceGroup> {
    let known = groups
        .iter()
        .any(|group| group.choices.iter().any(|choice| choice.value == chosen));
    if !chosen.is_empty() && !known {
        groups.insert(
            0,
            ChoiceGroup {
                label: String::new(),
                choices: vec![Choice {
                    value: chosen.into(),
                    text: text(),
                }],
            },
        );
    }
    groups
}

/// How a value is entered, from the channel's type.
#[derive(Clone)]
pub(super) enum Control {
    OnOff,
    Options(Vec<String>),
    Number,
    Text,
    /// The channel takes no value.
    Nothing,
}

impl Control {
    pub(super) fn for_condition(declared: Option<&Channel>) -> Self {
        match declared.and_then(|channel| channel.value.as_ref()) {
            Some(ValueType::Bool) => Control::OnOff,
            Some(ValueType::Enum(options)) => Control::Options(options.clone()),
            Some(ValueType::Int { .. } | ValueType::Float { .. }) => Control::Number,
            _ => Control::Text,
        }
    }

    pub(super) fn for_invoke(declared: Option<&Channel>) -> Self {
        match declared.and_then(|channel| channel.value.as_ref()) {
            Some(ValueType::Enum(options)) => Control::Options(options.clone()),
            Some(ValueType::Text { .. }) => Control::Text,
            Some(_) => Control::Nothing,
            None if declared.is_some() => Control::Nothing,
            None => Control::Text,
        }
    }

    pub(super) fn allows_limits(&self) -> bool {
        matches!(self, Control::Number | Control::Text)
    }

    pub(super) fn settle(&self, value: &mut String) {
        match self {
            Control::OnOff if value != "false" => *value = "true".into(),
            Control::Options(options) if !options.contains(value) => {
                *value = options.first().cloned().unwrap_or_default();
            }
            Control::Nothing => value.clear(),
            _ => {}
        }
    }

    pub(super) fn choices(&self) -> Vec<ChoiceGroup> {
        match self {
            Control::OnOff => {
                ungrouped(vec![Choice::new("true", "on"), Choice::new("false", "off")])
            }
            Control::Options(options) => ungrouped(
                options
                    .iter()
                    .map(|option| Choice::new(option, option))
                    .collect(),
            ),
            _ => Vec::new(),
        }
    }
}
