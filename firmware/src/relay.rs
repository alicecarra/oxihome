//! Frames between a leaf board and its gateway over ESP-NOW. A frame carries one part of a
//! message (the JSON of a `Message`, untouched), so messages larger than a frame go in parts.

const FRAME_VERSION: u8 = 1;
const HEADER_LENGTH: usize = 6;
/// Fits the 250 bytes of ESP-NOW v1 with the header.
pub const PART_LENGTH: usize = 240;
/// The MQTT buffer of the boards.
pub const MESSAGE_MAX_LENGTH: usize = 4096;
const MAXIMUM_PARTS: usize = MESSAGE_MAX_LENGTH.div_ceil(PART_LENGTH);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Data,
    /// Gateway to leaf: the gateway is on the broker; the leaf announces itself again.
    Connected,
    /// Leaf to gateway: looks for the gateway's channel and asks whether it is on the broker.
    Probe,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Data),
            1 => Some(Self::Connected),
            2 => Some(Self::Probe),
            _ => None,
        }
    }

    fn byte(self) -> u8 {
        match self {
            Self::Data => 0,
            Self::Connected => 1,
            Self::Probe => 2,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub kind: Kind,
    pub number: u16,
    pub part: u8,
    pub parts: u8,
    pub data: &'a [u8],
}

impl<'a> Frame<'a> {
    pub fn decode(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() < HEADER_LENGTH || bytes[0] != FRAME_VERSION {
            return None;
        }
        let frame = Self {
            kind: Kind::from_byte(bytes[1])?,
            number: u16::from_le_bytes([bytes[2], bytes[3]]),
            part: bytes[4],
            parts: bytes[5],
            data: &bytes[HEADER_LENGTH..],
        };
        (frame.part < frame.parts && usize::from(frame.parts) <= MAXIMUM_PARTS).then_some(frame)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LENGTH + self.data.len());
        bytes.push(FRAME_VERSION);
        bytes.push(self.kind.byte());
        bytes.extend_from_slice(&self.number.to_le_bytes());
        bytes.push(self.part);
        bytes.push(self.parts);
        bytes.extend_from_slice(self.data);
        bytes
    }
}

/// The frames of one message; `None` when it is longer than `MESSAGE_MAX_LENGTH`.
pub fn split(kind: Kind, number: u16, message: &[u8]) -> Option<Vec<Vec<u8>>> {
    if message.len() > MESSAGE_MAX_LENGTH {
        return None;
    }
    let chunks: Vec<&[u8]> = if message.is_empty() {
        vec![&[]]
    } else {
        message.chunks(PART_LENGTH).collect()
    };
    let parts = chunks.len() as u8;
    Some(
        chunks
            .into_iter()
            .enumerate()
            .map(|(part, data)| {
                Frame {
                    kind,
                    number,
                    part: part as u8,
                    parts,
                    data,
                }
                .encode()
            })
            .collect(),
    )
}

/// Puts back together the messages of one sender. Parts arrive in order over ESP-NOW; a part of
/// another message drops the one in progress.
#[derive(Default)]
pub struct Reassembler {
    number: u16,
    next_part: u8,
    message: Vec<u8>,
}

impl Reassembler {
    /// The whole message once its last part arrives.
    pub fn push(&mut self, frame: &Frame) -> Option<Vec<u8>> {
        if frame.part == 0 {
            self.number = frame.number;
            self.message.clear();
        } else if frame.number != self.number || frame.part != self.next_part {
            self.message.clear();
            self.next_part = 0;
            return None;
        }
        self.message.extend_from_slice(frame.data);
        self.next_part = frame.part + 1;
        if self.next_part < frame.parts {
            return None;
        }
        self.next_part = 0;
        Some(std::mem::take(&mut self.message))
    }
}

/// `esp32-<12 hex digits>` to the station MAC it was made from.
pub fn mac_from_device_id(device_id: &str) -> Option<[u8; 6]> {
    let bytes = hex(device_id.strip_prefix("esp32-")?)?;
    bytes.try_into().ok()
}

pub fn hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

/// The comma-separated device ids of `cfg.toml`.
pub fn device_ids(list: &str) -> impl Iterator<Item = &str> {
    list.split(',').map(str::trim).filter(|id| !id.is_empty())
}
