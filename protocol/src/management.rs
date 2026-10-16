//! Node management entity.

use crate::hardware::HARDWARE_MAX_LENGTH;
use crate::{Access, Channel, EntityDescriptor, Unit, ValueType};

pub const ENTITY: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ManagementChannel {
    /// `Bool`: Wi-Fi on/off.
    Wifi = 0,
    /// `Trigger` with `Text`: downloads the image at that URL over HTTP and reboots into it.
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
            unknown => Err(unknown),
        }
    }
}

/// Operation parameter: an `Int` channel in seconds that the node stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parameter {
    pub channel: ManagementChannel,
    pub default: i32,
    pub min: i32,
    pub max: i32,
}

pub const HEARTBEAT_INTERVAL: Parameter = Parameter {
    channel: ManagementChannel::HeartbeatInterval,
    default: 30,
    min: 5,
    max: 300,
};

pub const WIFI_CHECK_INTERVAL: Parameter = Parameter {
    channel: ManagementChannel::WifiCheckInterval,
    default: 5,
    min: 1,
    max: 60,
};

pub const OTA_URL_MAX_LENGTH: u16 = 512;

pub const PARAMETERS: [Parameter; 2] = [HEARTBEAT_INTERVAL, WIFI_CHECK_INTERVAL];

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
            unit: Some(Unit::Second),
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
        ] {
            assert_eq!(ManagementChannel::try_from(u8::from(channel)), Ok(channel));
        }
        assert_eq!(ManagementChannel::try_from(4), Err(4));
        assert_eq!(ManagementChannel::try_from(8), Err(8));
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
