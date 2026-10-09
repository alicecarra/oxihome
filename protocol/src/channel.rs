use serde::{Deserialize, Serialize};

/// Primitive property of an entity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: u8,
    pub access: Access,
    pub value: ValueType,
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
    /// E.g. raw IR code.
    Bytes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Float(f32),
    Enum(String),
    Color(Color),
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
}
