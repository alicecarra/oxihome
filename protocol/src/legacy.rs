//! Reading messages of the previous protocol version, so a hub can talk to nodes not updated yet.
//! A node speaks only its own version; when `PROTOCOL_VERSION` goes up, this module follows it.

use serde_json::Value as Json;

use crate::{Message, PROTOCOL_VERSION};

/// The oldest version `read` accepts.
pub const OLDEST_PROTOCOL_VERSION: u8 = PROTOCOL_VERSION - 1;

/// Target of a version 1 node, which did not say.
pub const UNKNOWN_TARGET: &str = "unknown";

#[derive(Debug)]
pub enum ReadError {
    /// Older than `OLDEST_PROTOCOL_VERSION` or newer than `PROTOCOL_VERSION`.
    Unsupported {
        version: u8,
        src: String,
    },
    Invalid(serde_json::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Unsupported { version, src } => {
                write!(formatter, "{src} speaks protocol {version}")
            }
            ReadError::Invalid(error) => write!(formatter, "{error}"),
        }
    }
}

/// A message of any supported version, in the current types; its `version` stays the sender's.
pub fn read(json: &str) -> Result<Message, ReadError> {
    let envelope = Message::envelope(json).map_err(ReadError::Invalid)?;
    match envelope.version {
        PROTOCOL_VERSION => Message::from_json(json).map_err(ReadError::Invalid),
        1 => {
            let mut message: Json = serde_json::from_str(json).map_err(ReadError::Invalid)?;
            upgrade_version_1(&mut message);
            serde_json::from_value(message).map_err(ReadError::Invalid)
        }
        version => Err(ReadError::Unsupported {
            version,
            src: envelope.src,
        }),
    }
}

/// Version 1 `Hello` had `fw_version`: "<name> <version>" for software nodes, the bare version for
/// the ESP32 firmware. Everything else is the same as version 2.
fn upgrade_version_1(message: &mut Json) {
    if message["kind"] != "Hello" {
        return;
    }
    let Some(payload) = message["payload"].as_object_mut() else {
        return;
    };
    let Some(Json::String(text)) = payload.remove("fw_version") else {
        return;
    };
    let (name, version) = text.split_once(' ').unwrap_or(("firmware", &text));
    payload.insert(
        "build".into(),
        serde_json::json!({ "name": name, "version": version, "target": UNKNOWN_TARGET }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Build, Payload};

    fn hello_version_1(fw_version: &str) -> String {
        format!(
            r#"{{"version":1,"msg_id":7,"src":"node-a","dst":null,"ttl":1,"reply_to":null,"kind":"Hello","payload":{{"fw_version":"{fw_version}","entities":[]}}}}"#
        )
    }

    fn build_of(message: Message) -> Build {
        match message.payload {
            Payload::Hello { build, .. } => build,
            other => panic!("not a Hello: {other:?}"),
        }
    }

    #[test]
    fn version_1_hello_of_a_software_node() {
        let message = read(&hello_version_1("weather 0.1.0")).unwrap();
        assert_eq!(message.version, 1);
        assert_eq!(
            build_of(message),
            Build::new("weather", "0.1.0", UNKNOWN_TARGET)
        );
    }

    #[test]
    fn version_1_hello_of_the_firmware() {
        let message = read(&hello_version_1("0.12.1")).unwrap();
        assert_eq!(
            build_of(message),
            Build::new("firmware", "0.12.1", UNKNOWN_TARGET)
        );
    }

    #[test]
    fn version_1_other_messages_read_as_they_are() {
        let json = r#"{"version":1,"msg_id":8,"src":"node-a","dst":null,"ttl":1,"reply_to":3,"kind":"Ack","payload":{"ok":true}}"#;
        let message = read(json).unwrap();
        assert_eq!((message.version, message.reply_to), (1, Some(3)));
    }

    #[test]
    fn current_version_reads_directly() {
        let message = Message::new("node-a", 1, Payload::Ack { ok: true });
        assert_eq!(read(&message.to_json().unwrap()).unwrap(), message);
    }

    #[test]
    fn unsupported_versions_are_refused() {
        let json = r#"{"version":0,"msg_id":1,"src":"node-a","dst":null,"ttl":1,"reply_to":null,"kind":"Ack","payload":{"ok":true}}"#;
        assert!(matches!(
            read(json),
            Err(ReadError::Unsupported { version: 0, .. })
        ));
        let newer = json.replace(r#""version":0"#, r#""version":3"#);
        assert!(matches!(
            read(&newer),
            Err(ReadError::Unsupported { version: 3, .. })
        ));
    }
}
