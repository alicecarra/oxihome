//! The commands typed on the simulator's console.

use oxihome_protocol::{Access, EntityDescriptor, Value, ValueType};

pub const HELP: &str = "\
commands:
  list
  set <node> <entity>.<channel> <value>     a fixed reading, or a change made by the device itself
  auto <node> [<entity>.<channel>]          readings back to their wave
  event <node> <entity>[.<channel>] [value]
  press <node> <entity>                     the same as event without a value
  offline <node>
  online <node>
  reboot <node>
  help
  quit";

#[derive(Debug, PartialEq)]
pub enum Command {
    List,
    Help,
    Quit,
    Offline(String),
    Online(String),
    Reboot(String),
    Set {
        node: String,
        entity: u8,
        channel: u8,
        value: String,
    },
    Auto {
        node: String,
        channel: Option<(u8, u8)>,
    },
    Event {
        node: String,
        entity: u8,
        channel: Option<u8>,
        value: Option<String>,
    },
}

/// `None` for an empty line. Values stay as text: their type comes from the node's channels.
pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let (word, rest) = next_word(line);
    let command = match word {
        "" => return Ok(None),
        "list" => Command::List,
        "help" => Command::Help,
        "quit" => Command::Quit,
        "offline" | "online" | "reboot" => {
            let (node, rest) = next_word(rest);
            if node.is_empty() || !rest.is_empty() {
                return Err(format!("usage: {word} <node>"));
            }
            match word {
                "offline" => Command::Offline(node.into()),
                "online" => Command::Online(node.into()),
                _ => Command::Reboot(node.into()),
            }
        }
        "set" => {
            let (node, rest) = next_word(rest);
            let (channel, value) = next_word(rest);
            let value = value.trim_end();
            let (entity, channel) = match parse_channel(channel)? {
                (entity, Some(channel)) => (entity, channel),
                _ => return Err("usage: set <node> <entity>.<channel> <value>".into()),
            };
            if node.is_empty() || value.is_empty() {
                return Err("usage: set <node> <entity>.<channel> <value>".into());
            }
            Command::Set {
                node: node.into(),
                entity,
                channel,
                value: value.into(),
            }
        }
        "auto" => {
            let (node, rest) = next_word(rest);
            if node.is_empty() {
                return Err("usage: auto <node> [<entity>.<channel>]".into());
            }
            let channel = match next_word(rest) {
                ("", _) => None,
                (channel, "") => match parse_channel(channel)? {
                    (entity, Some(channel)) => Some((entity, channel)),
                    _ => return Err("usage: auto <node> [<entity>.<channel>]".into()),
                },
                _ => return Err("usage: auto <node> [<entity>.<channel>]".into()),
            };
            Command::Auto {
                node: node.into(),
                channel,
            }
        }
        "event" | "press" => {
            let (node, rest) = next_word(rest);
            let (channel, value) = next_word(rest);
            let value = value.trim_end();
            if node.is_empty() || channel.is_empty() || (word == "press" && !value.is_empty()) {
                return Err(match word {
                    "press" => "usage: press <node> <entity>".into(),
                    _ => "usage: event <node> <entity>[.<channel>] [value]".into(),
                });
            }
            let (entity, channel) = parse_channel(channel)?;
            Command::Event {
                node: node.into(),
                entity,
                channel,
                value: (!value.is_empty()).then(|| value.into()),
            }
        }
        _ => return Err("unknown command, type `help`".into()),
    };
    Ok(Some(command))
}

/// A typed value as the channel declares it; text takes the rest of the line as it is.
pub fn parse_value(value_type: &ValueType, text: &str) -> Result<Value, String> {
    match value_type {
        ValueType::Bool => match text {
            "true" | "on" | "1" => Ok(Value::Bool(true)),
            "false" | "off" | "0" => Ok(Value::Bool(false)),
            _ => Err("must be true, false, on or off".into()),
        },
        ValueType::Int { min, max, .. } => match text.parse::<i32>() {
            Ok(value) if (*min..=*max).contains(&value) => Ok(Value::Int(value)),
            _ => Err(format!("must be a whole number from {min} to {max}")),
        },
        ValueType::Float { min, max, .. } => match text.parse::<f32>() {
            Ok(value) if (*min..=*max).contains(&value) => Ok(Value::Float(value)),
            _ => Err(format!("must be a number from {min} to {max}")),
        },
        ValueType::Enum(options) => options
            .iter()
            .find(|option| option.eq_ignore_ascii_case(text))
            .map(|option| Value::Enum(option.clone()))
            .ok_or_else(|| format!("must be one of {}", options.join(", "))),
        ValueType::Text { max_length } => {
            if text.len() <= usize::from(*max_length) {
                Ok(Value::Text(text.into()))
            } else {
                Err(format!("must be up to {max_length} bytes"))
            }
        }
        ValueType::Color(_) | ValueType::Bytes => {
            Err("this type cannot be typed on the console".into())
        }
    }
}

/// One line per channel: `<entity>.<channel> <entity name> / <channel name> <access> <type>`.
pub fn describe(entities: &[EntityDescriptor]) -> Vec<String> {
    entities
        .iter()
        .flat_map(|entity| {
            entity.channels.iter().map(move |channel| {
                let names: Vec<&str> = [entity.name.as_deref(), channel.name.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect();
                let access = match channel.access {
                    Access::Read => "read",
                    Access::Write => "write",
                    Access::ReadWrite => "read-write",
                    Access::Trigger => "trigger",
                    Access::Event => "event",
                };
                let value = channel
                    .value
                    .as_ref()
                    .map(|value| format!(" {value:?}"))
                    .unwrap_or_default();
                let names = if names.is_empty() {
                    String::new()
                } else {
                    format!(" {}", names.join(" / "))
                };
                format!("  {}.{}{names} {access}{value}", entity.id, channel.id)
            })
        })
        .collect()
}

/// `3.0` or `3`.
fn parse_channel(text: &str) -> Result<(u8, Option<u8>), String> {
    let invalid = || format!("`{text}` is not <entity>.<channel>");
    match text.split_once('.') {
        Some((entity, channel)) => Ok((
            entity.parse().map_err(|_| invalid())?,
            Some(channel.parse().map_err(|_| invalid())?),
        )),
        None => Ok((text.parse().map_err(|_| invalid())?, None)),
    }
}

fn next_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim_start()),
        None => (text, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_keeps_the_rest_of_the_line_as_the_value() {
        assert_eq!(
            parse("set  sim-remote 3.0  NEC 00  45 ").unwrap(),
            Some(Command::Set {
                node: "sim-remote".into(),
                entity: 3,
                channel: 0,
                value: "NEC 00  45".into(),
            })
        );
        assert!(parse("set sim-remote 3 on").is_err());
        assert!(parse("set sim-remote 3.0").is_err());
    }

    #[test]
    fn event_channel_and_value_are_optional() {
        assert_eq!(
            parse("press sim 2").unwrap(),
            Some(Command::Event {
                node: "sim".into(),
                entity: 2,
                channel: None,
                value: None,
            })
        );
        assert_eq!(
            parse("event sim 6.0 NEC 00 45").unwrap(),
            Some(Command::Event {
                node: "sim".into(),
                entity: 6,
                channel: Some(0),
                value: Some("NEC 00 45".into()),
            })
        );
        assert!(parse("press sim 2 now").is_err());
    }

    #[test]
    fn auto_takes_a_node_and_maybe_a_channel() {
        assert_eq!(
            parse("auto sim").unwrap(),
            Some(Command::Auto {
                node: "sim".into(),
                channel: None,
            })
        );
        assert_eq!(
            parse("auto sim 3.1").unwrap(),
            Some(Command::Auto {
                node: "sim".into(),
                channel: Some((3, 1)),
            })
        );
        assert!(parse("auto").is_err());
        assert!(parse("auto sim 3").is_err());
    }

    #[test]
    fn empty_and_unknown_lines() {
        assert_eq!(parse("   ").unwrap(), None);
        assert!(parse("climate sim 30").is_err());
        assert!(parse("offline").is_err());
        assert!(parse("offline sim extra").is_err());
    }

    #[test]
    fn values_follow_the_declared_type() {
        assert_eq!(parse_value(&ValueType::Bool, "on"), Ok(Value::Bool(true)));
        assert!(parse_value(&ValueType::Bool, "yes").is_err());
        let int = ValueType::Int {
            min: 5,
            max: 300,
            step: 1,
        };
        assert_eq!(parse_value(&int, "30"), Ok(Value::Int(30)));
        assert!(parse_value(&int, "1").is_err());
        let float = ValueType::Float {
            min: 0.0,
            max: 50.0,
            step: 1.0,
        };
        assert_eq!(parse_value(&float, "31.5"), Ok(Value::Float(31.5)));
        assert!(parse_value(&float, "NaN").is_err());
        let patterns = ValueType::Enum(vec!["Short".into(), "Alarm".into()]);
        assert_eq!(
            parse_value(&patterns, "alarm"),
            Ok(Value::Enum("Alarm".into()))
        );
        assert!(parse_value(&patterns, "Siren").is_err());
        let text = ValueType::Text { max_length: 4 };
        assert_eq!(parse_value(&text, "a b"), Ok(Value::Text("a b".into())));
        assert!(parse_value(&text, "too long").is_err());
    }
}
