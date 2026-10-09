//! Node management entity.

pub const ENTITY: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ManagementChannel {
    /// `Bool`: Wi-Fi on/off.
    Wifi = 0,
    /// `Trigger`: starts an over-the-air update (URL or version in the value).
    Ota = 1,
    /// `Trigger`: reboots the node.
    Reboot = 2,
    /// `Int` (s): periodic `State` interval.
    ReportInterval = 3,
    // 4: reserved for local bindings.
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
            unknown => Err(unknown),
        }
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
        ] {
            assert_eq!(ManagementChannel::try_from(u8::from(channel)), Ok(channel));
        }
        assert_eq!(ManagementChannel::try_from(4), Err(4));
    }
}
