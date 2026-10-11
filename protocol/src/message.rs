use serde::{Deserialize, Serialize};

use crate::Payload;

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
    /// Version 1, direct delivery (`ttl` 1), broadcast, no reply, timestamp or auth.
    pub fn new(src: impl Into<String>, msg_id: u32, payload: Payload) -> Self {
        Self {
            version: 1,
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
        assert_eq!(msg.version, 1);
        assert_eq!(msg.ttl, 1);
        assert_eq!((msg.dst.clone(), msg.reply_to), (None, None));
        assert_eq!((msg.timestamp, msg.auth.clone()), (None, None));

        let msg = msg.to("esp32-a1b2c3").replying_to(3);
        assert_eq!(msg.dst.as_deref(), Some("esp32-a1b2c3"));
        assert_eq!(msg.reply_to, Some(3));
    }

    #[test]
    fn hello_with_descriptors() {
        roundtrip(Payload::Hello {
            fw_version: "0.1.0".into(),
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
    fn optional_fields_can_be_omitted() {
        let json = r#"{"version":1,"msg_id":42,"src":"esp32-a1b2c3","dst":null,"ttl":1,"reply_to":null,"kind":"Ack","payload":{"ok":true}}"#;
        let msg = Message::from_json(json).unwrap();
        assert_eq!(msg.timestamp, None);
        assert_eq!(msg.auth, None);
    }
}
