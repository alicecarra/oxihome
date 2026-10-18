/// Known values of `Payload::Error::code`. The wire keeps a plain `u16` so that codes added by newer
/// nodes still deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    /// Not valid JSON, or required fields missing.
    InvalidMessage = 1,
    /// Message kind the node does not handle.
    Unsupported = 2,
    UnknownEntity = 3,
    UnknownChannel = 4,
    /// Wrong value type or out of range.
    InvalidValue = 5,
    /// Operation not allowed by the channel's `Access`, e.g. `Set` on a `Read` channel.
    InvalidOperation = 6,
    /// Failure inside the node, e.g. a GPIO error.
    Internal = 7,
    /// The node could not download the image of an `Ota` command: no connection, an HTTP status
    /// other than 200, or the transfer stopped. Sent on its own, after the command's `Ack`.
    UpdateDownloadFailed = 8,
    /// The image was downloaded but failed the final check, e.g. corrupt or for another chip.
    /// Sent on its own, after the command's `Ack`.
    UpdateRejected = 9,
}

impl From<ErrorCode> for u16 {
    fn from(code: ErrorCode) -> Self {
        code as u16
    }
}

impl TryFrom<u16> for ErrorCode {
    /// The unknown code.
    type Error = u16;

    fn try_from(code: u16) -> Result<Self, Self::Error> {
        match code {
            1 => Ok(Self::InvalidMessage),
            2 => Ok(Self::Unsupported),
            3 => Ok(Self::UnknownEntity),
            4 => Ok(Self::UnknownChannel),
            5 => Ok(Self::InvalidValue),
            6 => Ok(Self::InvalidOperation),
            7 => Ok(Self::Internal),
            8 => Ok(Self::UpdateDownloadFailed),
            9 => Ok(Self::UpdateRejected),
            unknown => Err(unknown),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_roundtrip() {
        for code in [
            ErrorCode::InvalidMessage,
            ErrorCode::Unsupported,
            ErrorCode::UnknownEntity,
            ErrorCode::UnknownChannel,
            ErrorCode::InvalidValue,
            ErrorCode::InvalidOperation,
            ErrorCode::Internal,
            ErrorCode::UpdateDownloadFailed,
            ErrorCode::UpdateRejected,
        ] {
            assert_eq!(ErrorCode::try_from(u16::from(code)), Ok(code));
        }
        assert_eq!(ErrorCode::try_from(0), Err(0));
        assert_eq!(ErrorCode::try_from(10), Err(10));
    }
}
