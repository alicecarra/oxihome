//! Node management entity.

use crate::hardware::HARDWARE_MAX_LENGTH;
use crate::{Access, Channel, EntityDescriptor, Unit, ValueType};

pub const ENTITY: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ManagementChannel {
    /// `Bool`: Wi-Fi on/off.
    Wifi = 0,
    /// `Trigger` with `Text`: downloads the image at that URL over HTTP and reboots into it. The
    /// URL may end in the image's digest (see `ota_command`).
    Ota = 1,
    /// `Trigger`: reboots the node.
    Reboot = 2,
    /// `Int` (s): periodic `State` interval.
    ReportInterval = 3,
    // 4: reserved for local bindings.
    /// `Int` (s): `Heartbeat` interval.
    HeartbeatInterval = 5,
    /// `Int` (s): Wi-Fi connection check interval.
    WifiCheckInterval = 6,
    /// `Text`: the `Hardware` JSON; a `Set` makes the node store it and reboot.
    Hardware = 7,
    /// `Int`: Wi-Fi power save, 0 off, 1 light (the radio sleeps between beacons), 2 maximum.
    WifiPowerSave = 8,
}

impl From<ManagementChannel> for u8 {
    fn from(channel: ManagementChannel) -> Self {
        channel as u8
    }
}

impl TryFrom<u8> for ManagementChannel {
    /// The unknown channel id.
    type Error = u8;

    fn try_from(id: u8) -> Result<Self, Self::Error> {
        match id {
            0 => Ok(Self::Wifi),
            1 => Ok(Self::Ota),
            2 => Ok(Self::Reboot),
            3 => Ok(Self::ReportInterval),
            5 => Ok(Self::HeartbeatInterval),
            6 => Ok(Self::WifiCheckInterval),
            7 => Ok(Self::Hardware),
            8 => Ok(Self::WifiPowerSave),
            unknown => Err(unknown),
        }
    }
}

/// Operation parameter: an `Int` channel that the node stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parameter {
    pub channel: ManagementChannel,
    pub default: i32,
    pub min: i32,
    pub max: i32,
    pub unit: Option<Unit>,
}

pub const HEARTBEAT_INTERVAL: Parameter = Parameter {
    channel: ManagementChannel::HeartbeatInterval,
    default: 30,
    min: 5,
    max: 300,
    unit: Some(Unit::Second),
};

pub const WIFI_CHECK_INTERVAL: Parameter = Parameter {
    channel: ManagementChannel::WifiCheckInterval,
    default: 5,
    min: 1,
    max: 60,
    unit: Some(Unit::Second),
};

/// Off by default: the boards are on mains power, and power save delays commands.
pub const WIFI_POWER_SAVE: Parameter = Parameter {
    channel: ManagementChannel::WifiPowerSave,
    default: 0,
    min: 0,
    max: 2,
    unit: None,
};

pub const OTA_URL_MAX_LENGTH: u16 = 512;

/// A URL fragment never reaches the server, so a node that does not check the digest still
/// downloads the image.
const OTA_DIGEST_PREFIX: &str = "#sha256=";

/// The `Ota` command for the image at `url`, whose SHA-256 is `digest`.
pub fn ota_command(url: &str, digest: &[u8; 32]) -> String {
    let mut command = format!("{url}{OTA_DIGEST_PREFIX}");
    for byte in digest {
        command.push_str(&format!("{byte:02x}"));
    }
    command
}

/// The URL and, when the command has one, the digest of an `Ota` command. `Err` for a digest that
/// is not 64 hex digits.
pub fn parse_ota_command(command: &str) -> Result<(&str, Option<[u8; 32]>), String> {
    let Some((url, hex)) = command.split_once(OTA_DIGEST_PREFIX) else {
        return Ok((command, None));
    };
    let invalid = || format!("not a SHA-256 digest: \"{hex}\"");
    if hex.len() != 64 || !hex.is_ascii() {
        return Err(invalid());
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| invalid())?;
    }
    Ok((url, Some(digest)))
}

pub const PARAMETERS: [Parameter; 3] = [HEARTBEAT_INTERVAL, WIFI_CHECK_INTERVAL, WIFI_POWER_SAVE];

impl Parameter {
    pub fn accepts(&self, value: i32) -> bool {
        (self.min..=self.max).contains(&value)
    }

    pub fn descriptor(&self) -> Channel {
        Channel {
            id: self.channel.into(),
            name: None,
            access: Access::ReadWrite,
            value: Some(ValueType::Int {
                min: self.min,
                max: self.max,
                step: 1,
            }),
            quantity: None,
            unit: self.unit,
        }
    }
}

pub fn parameter(channel: u8) -> Option<Parameter> {
    PARAMETERS
        .into_iter()
        .find(|parameter| u8::from(parameter.channel) == channel)
}

/// For nodes that update over the air; added to `descriptor()` by the node.
pub fn ota_descriptor() -> Channel {
    Channel {
        id: ManagementChannel::Ota.into(),
        name: None,
        access: Access::Trigger,
        value: Some(ValueType::Text {
            max_length: OTA_URL_MAX_LENGTH,
        }),
        quantity: None,
        unit: None,
    }
}

/// For nodes whose hardware the hub configures; added to `descriptor()` by the node.
pub fn hardware_descriptor() -> Channel {
    Channel {
        id: ManagementChannel::Hardware.into(),
        name: None,
        access: Access::ReadWrite,
        value: Some(ValueType::Text {
            max_length: HARDWARE_MAX_LENGTH,
        }),
        quantity: None,
        unit: None,
    }
}

/// The management entity as announced in `Hello`.
pub fn descriptor() -> EntityDescriptor {
    EntityDescriptor {
        id: ENTITY,
        name: None,
        channels: PARAMETERS.iter().map(Parameter::descriptor).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_roundtrip() {
        for channel in [
            ManagementChannel::Wifi,
            ManagementChannel::Ota,
            ManagementChannel::Reboot,
            ManagementChannel::ReportInterval,
            ManagementChannel::HeartbeatInterval,
            ManagementChannel::WifiCheckInterval,
            ManagementChannel::Hardware,
            ManagementChannel::WifiPowerSave,
        ] {
            assert_eq!(ManagementChannel::try_from(u8::from(channel)), Ok(channel));
        }
        assert_eq!(ManagementChannel::try_from(4), Err(4));
        assert_eq!(ManagementChannel::try_from(9), Err(9));
    }

    #[test]
    fn ota_command_roundtrip() {
        let digest: [u8; 32] = std::array::from_fn(|index| index as u8 * 8);
        let command = ota_command("http://hub:8081/ota/abc", &digest);
        assert_eq!(
            command,
            "http://hub:8081/ota/abc#sha256=\
             0008101820283038404850586068707880889098a0a8b0b8c0c8d0d8e0e8f0f8"
        );
        assert!(command.len() <= OTA_URL_MAX_LENGTH.into());
        assert_eq!(
            parse_ota_command(&command),
            Ok(("http://hub:8081/ota/abc", Some(digest)))
        );
        assert_eq!(
            parse_ota_command("http://hub:8081/ota/abc"),
            Ok(("http://hub:8081/ota/abc", None))
        );
        assert!(parse_ota_command("http://hub/ota/abc#sha256=00ff").is_err());
        assert!(parse_ota_command(&command.replace("f8", "zz")).is_err());
    }

    #[test]
    fn parameter_defaults_are_accepted() {
        for parameter in PARAMETERS {
            assert!(parameter.accepts(parameter.default));
            assert!(!parameter.accepts(parameter.min - 1));
            assert!(!parameter.accepts(parameter.max + 1));
        }
    }
}
