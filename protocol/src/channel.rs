use serde::{Deserialize, Serialize};

/// Primitive property of an entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: u8,
    pub access: Access,
    /// `None` for channels without a value: a `Trigger` with no argument, an `Event` with no data.
    pub value: Option<ValueType>,
    pub quantity: Option<Quantity>,
    pub unit: Option<Unit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
    /// Stateless action, fired by `CommandOp::Invoke`.
    Trigger,
    /// Emits stateless occurrences (`Payload::Event`).
    Event,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueType {
    Bool,
    Int {
        min: i32,
        max: i32,
        step: i32,
    },
    Float {
        min: f32,
        max: f32,
        step: f32,
    },
    Enum(Vec<String>),
    Color(ColorModel),
    /// Free text, e.g. a received IR code or a chat message; `max_length` in bytes of UTF-8.
    Text {
        max_length: u16,
    },
    /// Binary data.
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Float(f32),
    Enum(String),
    Color(Color),
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorModel {
    Rgb,
    Hsv,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Color {
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
    /// `h` in degrees (0–360), `s` and `v` in 0–1.
    Hsv {
        h: f32,
        s: f32,
        v: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantity {
    Temperature,
    Humidity,
    Illuminance,
    Voltage,
    Current,
    Power,
    Precipitation,
    WindSpeed,
    DataSize,
    DataRate,
    Frequency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Celsius,
    Percent,
    Lux,
    Volt,
    Ampere,
    Watt,
    Second,
    Millimeter,
    KilometerPerHour,
    Gigabyte,
    KilobytePerSecond,
    Megahertz,
}
