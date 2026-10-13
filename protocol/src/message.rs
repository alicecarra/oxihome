use serde::{Deserialize, Serialize};

use crate::Payload;

/// Raised with every change of the wire format that older nodes or hubs cannot read.
pub const PROTOCOL_VERSION: u8 = 2;

/// The part of a message every protocol version keeps, to tell who speaks another version.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Envelope {
    pub version: u8,
    pub src: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub version: u8,
    /// Sequential per sender.
    pub msg_id: u32,
    /// Claimed sender, not authenticated.
    pub src: String,
    /// `None` = broadcast.
    pub dst: Option<String>,
    /// Remaining hops (mesh); 1 = direct delivery.
    pub ttl: u8,
    pub reply_to: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(flatten)]
    pub payload: Payload,
    /// Reserved for HMAC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<Vec<u8>>,
}

impl Message {
    /// `PROTOCOL_VERSION`, direct delivery (`ttl` 1), broadcast, no reply, timestamp or auth.
    pub fn new(src: impl Into<String>, msg_id: u32, payload: Payload) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            msg_id,
            src: src.into(),
            dst: None,
            ttl: 1,
            reply_to: None,
            timestamp: None,
            payload,
            auth: None,
        }
    }

    /// Addressed to one device.
    pub fn to(mut self, dst: impl Into<String>) -> Self {
        self.dst = Some(dst.into());
        self
    }

    /// An answer to the message `msg_id`.
    pub fn replying_to(mut self, msg_id: u32) -> Self {
        self.reply_to = Some(msg_id);
        self
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }

    /// Reads only `version` and `src`, which any version can.
    pub fn envelope(s: &str) -> serde_json::Result<Envelope> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn roundtrip(payload: Payload) {
        let msg = Message::new("esp32-a1b2c3", 42, payload);

        let json = msg.to_json().unwrap();
        assert_eq!(Message::from_json(&json).unwrap(), msg, "{json}");
    }

    #[test]
    fn constructor_defaults_and_builders() {
        let msg = Message::new("hub", 7, Payload::Ack { ok: true });
        assert_eq!(msg.version, PROTOCOL_VERSION);
        assert_eq!(msg.ttl, 1);
        assert_eq!((msg.dst.clone(), msg.reply_to), (None, None));
        assert_eq!((msg.timestamp, msg.auth.clone()), (None, None));

        let msg = msg.to("esp32-a1b2c3").replying_to(3);
        assert_eq!(msg.dst.as_deref(), Some("esp32-a1b2c3"));
        assert_eq!(msg.reply_to, Some(3));
    }

    #[test]
    fn envelope_reads_an_older_message() {
        let json = r#"{"version":1,"msg_id":7,"src":"esp32-a","dst":null,"ttl":1,"reply_to":null,"kind":"Hello","payload":{"fw_version":"0.1.0","entities":[]}}"#;
        assert!(Message::from_json(json).is_err());
        let envelope = Message::envelope(json).unwrap();
        assert_eq!((envelope.version, envelope.src.as_str()), (1, "esp32-a"));
    }

    #[test]
    fn hello_with_descriptors() {
        roundtrip(Payload::Hello {
            build: Build::new("firmware", "0.1.0", "esp32"),
            entities: vec![
                EntityDescriptor {
                    id: 1,
                    channels: vec![Channel {
                        id: 0,
                        access: Access::Read,
                        value: Some(ValueType::Float {
                            min: -40.0,
                            max: 85.0,
                            step: 0.1,
                        }),
                        quantity: Some(Quantity::Temperature),
                        unit: Some(Unit::Celsius),
                    }],
                },
                EntityDescriptor {
                    id: 2,
                    channels: vec![Channel {
                        id: 0,
                        access: Access::ReadWrite,
                        value: Some(ValueType::Bool),
                        quantity: None,
                        unit: None,
                    }],
                },
                EntityDescriptor {
                    id: 3,
                    channels: vec![Channel {
                        id: 0,
                        access: Access::Event,
                        value: None,
                        quantity: None,
                        unit: None,
                    }],
                },
            ],
        });
    }

    #[test]
    fn all_payloads() {
        let payloads = [
            Payload::Heartbeat {
                uptime_s: 3600,
                rssi: Some(-60),
                battery: None,
            },
            Payload::State {
                entity: 1,
                values: vec![ChannelValue {
                    channel: 0,
                    value: Value::Float(23.4),
                    confidence: Confidence::Confirmed,
                }],
            },
            Payload::Event {
                entity: 3,
                channel: 0,
                event: EventKind::Received,
                data: Some(Value::Bytes(vec![0x20, 0xdf, 0x10, 0xef])),
            },
            Payload::Event {
                entity: 6,
                channel: 0,
                event: EventKind::Received,
                data: Some(Value::Text("NEC 00 45".into())),
            },
            Payload::Command {
                entity: 2,
                channel: 0,
                op: CommandOp::Set(Value::Color(Color::Rgb {
                    r: 255,
                    g: 128,
                    b: 0,
                })),
            },
            Payload::Command {
                entity: 0,
                channel: ManagementChannel::Reboot.into(),
                op: CommandOp::Invoke(None),
            },
            Payload::Query {
                what: QueryKind::State { entity: None },
            },
            Payload::Ack { ok: true },
            Payload::Error {
                code: 1,
                detail: Some("unknown channel".into()),
            },
        ];

        for payload in payloads {
            roundtrip(payload);
        }
    }

    #[test]
    fn text_values_are_plain_json_strings() {
        let json = serde_json::to_string(&Value::Text("NEC 00 45".into())).unwrap();
        assert_eq!(json, r#"{"Text":"NEC 00 45"}"#);
        let json = serde_json::to_string(&ValueType::Text { max_length: 1024 }).unwrap();
        assert_eq!(json, r#"{"Text":{"max_length":1024}}"#);
    }

    #[test]
    fn optional_fields_can_be_omitted() {
        let json = r#"{"version":2,"msg_id":42,"src":"esp32-a1b2c3","dst":null,"ttl":1,"reply_to":null,"kind":"Ack","payload":{"ok":true}}"#;
        let msg = Message::from_json(json).unwrap();
        assert_eq!(msg.timestamp, None);
        assert_eq!(msg.auth, None);
    }
}
