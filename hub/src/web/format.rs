use std::sync::OnceLock;
use std::time::Duration;

use jiff::{Timestamp, tz::TimeZone};
use oxihome_protocol::{Color, Quantity, Unit, Value};

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

/// The hub's, for `UnixTime` values; UTC until set at startup.
static TIME_ZONE: OnceLock<TimeZone> = OnceLock::new();

pub fn set_time_zone(time_zone: TimeZone) {
    let _ = TIME_ZONE.set(time_zone);
}

/// `14:02` today, `Mon 14:02` within a week, `2026-10-19 14:02` further; `—` for zero, which
/// nodes send for "not yet".
pub fn moment_label(seconds: i64, now: Timestamp) -> String {
    let time_zone = TIME_ZONE.get().cloned().unwrap_or(TimeZone::UTC);
    let Some(moment) = Timestamp::from_second(seconds).ok().filter(|_| seconds > 0) else {
        return "—".into();
    };
    let moment = moment.to_zoned(time_zone.clone());
    let now = now.to_zoned(time_zone);
    let format = if moment.date() == now.date() {
        "%H:%M"
    } else if (now.date() - moment.date()).get_days().abs() <= 6 {
        "%a %H:%M"
    } else {
        "%Y-%m-%d %H:%M"
    };
    moment.strftime(format).to_string()
}

/// `1 h 30 min`: the two largest units, the smaller left out when zero.
pub fn long_duration_label(seconds: u64) -> String {
    let units = [(86400, "d"), (3600, "h"), (60, "min"), (1, "s")];
    let Some(first) = units.iter().position(|(length, _)| seconds >= *length) else {
        return "0 s".into();
    };
    let (length, symbol) = units[first];
    let mut label = format!("{} {symbol}", seconds / length);
    if let Some(&(next_length, next_symbol)) = units.get(first + 1) {
        let rest = seconds % length / next_length;
        if rest > 0 {
            label.push_str(&format!(" {rest} {next_symbol}"));
        }
    }
    label
}

/// `22.3 °C`, `1 h 30 min`, `on`.
pub fn value_label(value: &Value, unit: Option<Unit>) -> String {
    let suffix = unit
        .map(|unit| format!(" {}", unit_symbol(unit)))
        .unwrap_or_default();
    match (value, unit) {
        (Value::Int(number), Some(Unit::Second)) if *number >= 0 => {
            return long_duration_label(number.unsigned_abs().into());
        }
        (Value::Float(number), Some(Unit::Second)) if *number >= 0.0 => {
            return long_duration_label(number.round() as u64);
        }
        (Value::Int(number), Some(Unit::UnixTime)) => {
            return moment_label(i64::from(*number), Timestamp::now());
        }
        (Value::Float(number), Some(Unit::UnixTime)) => {
            return moment_label(*number as i64, Timestamp::now());
        }
        _ => {}
    }
    match value {
        Value::Bool(on) => if *on { "on" } else { "off" }.into(),
        Value::Int(number) => format!("{number}{suffix}"),
        Value::Float(number) => format!("{number:.1}{suffix}"),
        Value::Enum(option) => option.clone(),
        Value::Text(text) => text.clone(),
        Value::Color(Color::Rgb { r, g, b }) => format!("#{r:02x}{g:02x}{b:02x}"),
        Value::Color(Color::Hsv { h, s, v }) => {
            format!("hsv {h:.0}° {:.0} % {:.0} %", s * 100.0, v * 100.0)
        }
        Value::Bytes(bytes) => format!("{} bytes", bytes.len()),
    }
}

/// A switch is on or off; a sensor or flag the node only reports is yes or no.
pub fn bool_label(on: bool, switchable: bool) -> &'static str {
    match (switchable, on) {
        (true, true) => "on",
        (true, false) => "off",
        (false, true) => "yes",
        (false, false) => "no",
    }
}

/// For a CSS `background-color`: `#b2a1e1`, `hsl(...)`; `None` for other values.
pub fn css_color(value: &Value) -> Option<String> {
    match value {
        Value::Color(Color::Rgb { .. }) => Some(value_label(value, None)),
        Value::Color(Color::Hsv { h, s, v }) => {
            // HSV to HSL: same hue, the lightness and saturation recomputed.
            let lightness = v * (1.0 - s / 2.0);
            let saturation = if lightness == 0.0 || lightness == 1.0 {
                0.0
            } else {
                (v - lightness) / lightness.min(1.0 - lightness)
            };
            Some(format!(
                "hsl({h:.0} {:.0}% {:.0}%)",
                saturation * 100.0,
                lightness * 100.0
            ))
        }
        _ => None,
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
        Unit::UnixTime => "",
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
        assert_eq!(value_label(&Value::Bool(true), None), "on");
    }

    #[test]
    fn only_switches_are_on_or_off() {
        assert_eq!(bool_label(true, true), "on");
        assert_eq!(bool_label(false, true), "off");
        assert_eq!(bool_label(true, false), "yes");
        assert_eq!(bool_label(false, false), "no");
    }

    #[test]
    fn moments_read_as_a_time_of_day() {
        set_time_zone(TimeZone::get("America/Sao_Paulo").unwrap());
        let now: Timestamp = "2026-10-19T20:00:00Z".parse().unwrap();
        let at = |text: &str| text.parse::<Timestamp>().unwrap().as_second();
        assert_eq!(moment_label(at("2026-10-19T17:02:00Z"), now), "14:02");
        assert_eq!(moment_label(at("2026-10-17T17:02:00Z"), now), "Sat 14:02");
        assert_eq!(
            moment_label(at("2026-09-01T17:02:00Z"), now),
            "2026-09-01 14:02"
        );
        assert_eq!(moment_label(0, now), "—");
    }

    #[test]
    fn colors_fill_a_dot() {
        let red = Value::Color(Color::Hsv {
            h: 0.0,
            s: 1.0,
            v: 1.0,
        });
        assert_eq!(css_color(&red).as_deref(), Some("hsl(0 100% 50%)"));
        assert_eq!(css_color(&Value::Int(1)), None);
    }

    #[test]
    fn colors_read_as_hex() {
        let color = Value::Color(Color::Rgb {
            r: 178,
            g: 161,
            b: 225,
        });
        assert_eq!(value_label(&color, None), "#b2a1e1");
    }

    #[test]
    fn seconds_read_as_a_duration() {
        assert_eq!(value_label(&Value::Int(30), Some(Unit::Second)), "30 s");
        assert_eq!(
            value_label(&Value::Int(5423), Some(Unit::Second)),
            "1 h 30 min"
        );
        assert_eq!(
            value_label(&Value::Int(312), Some(Unit::Second)),
            "5 min 12 s"
        );
        assert_eq!(
            value_label(&Value::Int(183_600), Some(Unit::Second)),
            "2 d 3 h"
        );
        assert_eq!(value_label(&Value::Int(7200), Some(Unit::Second)), "2 h");
        assert_eq!(value_label(&Value::Int(0), Some(Unit::Second)), "0 s");
        assert_eq!(
            value_label(&Value::Float(90.4), Some(Unit::Second)),
            "1 min 30 s"
        );
        assert_eq!(value_label(&Value::Int(-5), Some(Unit::Second)), "-5 s");
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
