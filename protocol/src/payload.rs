use serde::{Deserialize, Serialize};

use crate::{Channel, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum Payload {
    Hello {
        build: Build,
        /// The node's address on the local network, when it has one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ip: Option<String>,
        entities: Vec<EntityDescriptor>,
    },
    Heartbeat {
        uptime_s: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rssi: Option<i8>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        battery: Option<u8>,
    },
    State {
        entity: u8,
        values: Vec<ChannelValue>,
    },
    Event {
        entity: u8,
        channel: u8,
        event: EventKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Command {
        entity: u8,
        channel: u8,
        op: CommandOp,
    },
    Query {
        what: QueryKind,
    },
    Ack {
        ok: bool,
    },
    Error {
        code: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Something a person should know, from the node itself: the hub shows it and rules may pass
    /// it on. Up to `NOTICE_TITLE_MAX_LENGTH` and `NOTICE_MESSAGE_MAX_LENGTH` bytes.
    Notice {
        level: NoticeLevel,
        title: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
        /// A notice with the same key replaces the node's previous one instead of adding another.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
}

pub const NOTICE_TITLE_MAX_LENGTH: usize = 128;
pub const NOTICE_MESSAGE_MAX_LENGTH: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NoticeLevel {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDescriptor {
    pub id: u8,
    /// What the node calls it; the hub's own name in `nodes.toml` wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub channels: Vec<Channel>,
}

impl EntityDescriptor {
    pub fn named(self, name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelValue {
    pub channel: u8,
    pub value: Value,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Confidence {
    /// Read back from hardware.
    Confirmed,
    /// Actuator without feedback (e.g. IR).
    Assumed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CommandOp {
    Set(Value),
    Invoke(Option<Value>),
}

/// What runs on the node: the program, its version and what it was built for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    /// What the program is, e.g. "firmware" or "weather".
    pub name: String,
    /// `major.minor.patch`.
    pub version: String,
    /// Where it runs, e.g. "esp32", "esp32c6" or "linux-x86_64".
    pub target: String,
}

impl Build {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        target: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            target: target.into(),
        }
    }

    /// For a program running on the machine it was built for, as `<os>-<architecture>`.
    pub fn native(name: impl Into<String>, version: impl Into<String>) -> Self {
        let target = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        Self::new(name, version, target)
    }
}

impl std::fmt::Display for Build {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {} ({})",
            self.name, self.version, self.target
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryKind {
    Hello,
    /// `None` = all entities.
    State {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        entity: Option<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    Press,
    LongPress,
    DoublePress,
    /// E.g. IR code received, carried in `data`.
    Received,
}
