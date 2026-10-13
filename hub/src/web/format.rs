use std::time::Duration;

use jiff::Timestamp;
use oxihome_protocol::{Quantity, Unit, Value};

/// Zero when `at` is in the future (clock adjusted).
pub fn elapsed(at: Timestamp, now: Timestamp) -> Duration {
    Duration::try_from(now.duration_since(at)).unwrap_or_default()
}

pub fn ago_label(ago: Duration) -> String {
    format!("{} ago", duration_label(ago))
}

pub fn duration_label(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        3600..86400 => format!("{} h", seconds / 3600),
        _ => format!("{} d", seconds / 86400),
    }
}

pub fn entity_label(entity: u8, name: &Option<String>) -> String {
    name.clone().unwrap_or_else(|| format!("entity {entity}"))
}

pub fn channel_label(channel: u8, name: &Option<String>, quantity: Option<Quantity>) -> String {
    name.clone()
        .unwrap_or_else(|| default_channel_name(channel, quantity))
}

/// What a channel is called until it gets a name: its quantity, or its id.
pub fn default_channel_name(channel: u8, quantity: Option<Quantity>) -> String {
    match quantity {
        Some(quantity) => format!("{quantity:?}"),
        None => format!("channel {channel}"),
    }
}

/// `22.3 °C`, `30 s`, `on`.
pub fn value_label(value: &Value, unit: Option<Unit>) -> String {
    let suffix = unit
        .map(|unit| format!(" {}", unit_symbol(unit)))
        .unwrap_or_default();
    match value {
        Value::Bool(on) => if *on { "on" } else { "off" }.into(),
        Value::Int(number) => format!("{number}{suffix}"),
        Value::Float(number) => format!("{number:.1}{suffix}"),
        Value::Enum(option) => option.clone(),
        Value::Text(text) => text.clone(),
        Value::Color(color) => format!("{color:?}"),
        Value::Bytes(bytes) => format!("{} bytes", bytes.len()),
    }
}

pub fn unit_symbol(unit: Unit) -> &'static str {
    match unit {
        Unit::Celsius => "°C",
        Unit::Percent => "%",
        Unit::Lux => "lx",
        Unit::Volt => "V",
        Unit::Ampere => "A",
        Unit::Watt => "W",
        Unit::Second => "s",
        Unit::Millimeter => "mm",
        Unit::KilometerPerHour => "km/h",
        Unit::Gigabyte => "GB",
        Unit::KilobytePerSecond => "kB/s",
        Unit::Megahertz => "MHz",
        Unit::Megabyte => "MB",
    }
}

/// The id of a rule's card on `/rules`, for links: `rule-living-room-hot`.
pub fn rule_anchor(name: &str) -> String {
    let words: Vec<String> = name
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    format!("rule-{}", words.join("-"))
}

/// Percent-encoded for a query string: `/rules/edit?name=Lamp%20at%20seven`.
pub fn query_value(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_anchors_are_url_friendly() {
        assert_eq!(
            rule_anchor("Living room hot: external LED on"),
            "rule-living-room-hot-external-led-on"
        );
        assert_eq!(
            rule_anchor("Remote key 0 toggles"),
            "rule-remote-key-0-toggles"
        );
    }

    #[test]
    fn float_has_one_decimal_and_unit() {
        assert_eq!(
            value_label(&Value::Float(22.34), Some(Unit::Celsius)),
            "22.3 °C"
        );
        assert_eq!(
            value_label(&Value::Float(54.0), Some(Unit::Percent)),
            "54.0 %"
        );
    }

    #[test]
    fn value_without_unit_is_just_the_number() {
        assert_eq!(value_label(&Value::Float(1.25), None), "1.2");
        assert_eq!(value_label(&Value::Int(30), Some(Unit::Second)), "30 s");
        assert_eq!(value_label(&Value::Bool(true), None), "on");
    }

    #[test]
    fn default_channel_name_prefers_the_quantity() {
        assert_eq!(
            default_channel_name(0, Some(Quantity::Temperature)),
            "Temperature"
        );
        assert_eq!(default_channel_name(1, None), "channel 1");
        assert_eq!(
            channel_label(0, &Some("power".into()), Some(Quantity::Power)),
            "power"
        );
    }
}
