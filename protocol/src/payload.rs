use serde::{Deserialize, Serialize};

use crate::{Channel, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum Payload {
    Hello {
        fw_version: String,
        entities: Vec<EntityDescriptor>,
    },
    Heartbeat {
        uptime_s: u32,
        rssi: Option<i8>,
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
        detail: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDescriptor {
    pub id: u8,
    pub channels: Vec<Channel>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryKind {
    Hello,
    /// `None` = all entities.
    State {
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
