use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use oxihome_protocol::legacy::{self, ReadError};
use oxihome_protocol::{Message, Payload};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

use crate::traffic::{Direction, Reading, Traffic};

const SOURCE: &str = "oxihome-hub";
const UP_TOPICS: &str = "oxihome/+/up";
const BROADCAST_TOPIC: &str = "oxihome/broadcast/down";
/// Above the broker's `max_packet_size` (64 KiB), so the broker is the one that drops oversized
/// messages; rumqttc's default of 10 KiB would close the connection over a large `Hello`.
const MAX_PACKET_SIZE: usize = 128 * 1024;

pub enum Incoming {
    /// (Re)connected to the broker.
    Connected,
    Message(Message),
    /// A node speaking a protocol version the hub does not read.
    OtherProtocol {
        device_id: String,
        version: u8,
    },
}

pub struct Mqtt {
    client: AsyncClient,
    next_message_id: AtomicU32,
    traffic: Arc<Traffic>,
}

impl Mqtt {
    /// The hub logs in as `oxihome-hub`; without a password it connects anonymously.
    pub fn connect(
        host: &str,
        port: u16,
        password: Option<String>,
    ) -> (Self, mpsc::Receiver<Incoming>) {
        let mut options = MqttOptions::new(SOURCE, host, port);
        options.set_max_packet_size(MAX_PACKET_SIZE, MAX_PACKET_SIZE);
        if let Some(password) = password {
            options.set_credentials(SOURCE, password);
        }
        let (client, mut event_loop) = AsyncClient::new(options, 64);
        let (sender, receiver) = mpsc::channel(64);

        let subscriber = client.clone();
        let traffic = Arc::new(Traffic::default());
        let recorder = traffic.clone();
        tokio::spawn(async move {
            loop {
                let incoming = match event_loop.poll().await {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        if let Err(error) = subscriber.subscribe(UP_TOPICS, QoS::AtLeastOnce).await
                        {
                            println!("subscribe failed: {error}");
                        }
                        Incoming::Connected
                    }
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        let text = String::from_utf8_lossy(&publish.payload);
                        let (incoming, reading) = read(&publish.topic, &text);
                        if let (None, Reading::Dropped(reason)) = (&incoming, &reading) {
                            println!("{reason}");
                        }
                        recorder.record(Direction::Up, &publish.topic, &text, reading);
                        match incoming {
                            Some(incoming) => incoming,
                            None => continue,
                        }
                    }
                    Ok(_) => continue,
                    Err(error) => {
                        println!("MQTT connection error: {error}, retrying");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                if sender.send(incoming).await.is_err() {
                    return;
                }
            }
        });

        (
            Self {
                client,
                next_message_id: AtomicU32::new(0),
                traffic,
            },
            receiver,
        )
    }

    pub fn traffic(&self) -> &Traffic {
        &self.traffic
    }

    pub fn next_message_id(&self) -> u32 {
        self.next_message_id.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn broadcast(&self, payload: Payload) -> anyhow::Result<()> {
        self.publish(self.next_message_id(), None, payload).await
    }

    /// `destination` is a device id, `None` broadcasts.
    pub async fn publish(
        &self,
        message_id: u32,
        destination: Option<&str>,
        payload: Payload,
    ) -> anyhow::Result<()> {
        let mut message = Message::new(SOURCE, message_id, payload);
        if let Some(device_id) = destination {
            message = message.to(device_id);
        }
        let topic = match destination {
            Some(device_id) => format!("oxihome/{device_id}/down"),
            None => BROADCAST_TOPIC.into(),
        };
        let reading = Reading::of(&message.payload);
        let text = message.to_json()?;
        self.client
            .publish(topic.as_str(), QoS::AtLeastOnce, false, text.as_str())
            .await?;
        self.traffic.record(Direction::Down, &topic, &text, reading);
        Ok(())
    }
}

/// What reaches the hub's loop, `None` for a message thrown away.
fn read(topic: &str, text: &str) -> (Option<Incoming>, Reading) {
    match legacy::read(text) {
        // The broker's ACL ties each topic to its node, but not the `src` field.
        Ok(message) if topic != format!("oxihome/{}/up", message.src) => (
            None,
            Reading::Dropped(format!("dropping message from {} on {topic}", message.src)),
        ),
        Ok(message) => {
            let reading = Reading::of(&message.payload);
            (Some(Incoming::Message(message)), reading)
        }
        Err(ReadError::Unsupported { version, src }) if topic == format!("oxihome/{src}/up") => (
            Some(Incoming::OtherProtocol {
                device_id: src,
                version,
            }),
            Reading::Dropped(format!("protocol version {version}")),
        ),
        Err(error) => (
            None,
            Reading::Dropped(format!("invalid message on {topic}: {error}")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traffic::MessageKind;

    fn heartbeat_from(source: &str) -> String {
        Message::new(
            source,
            1,
            Payload::Heartbeat {
                uptime_s: 10,
                rssi: None,
                battery: None,
            },
        )
        .to_json()
        .unwrap()
    }

    #[test]
    fn reads_a_message_on_its_own_topic() {
        let (incoming, reading) = read("oxihome/a/up", &heartbeat_from("a"));
        assert!(matches!(incoming, Some(Incoming::Message(_))));
        assert_eq!(
            reading,
            Reading::Read {
                kind: MessageKind::Heartbeat,
                entity: None,
                channels: Vec::new(),
            }
        );
    }

    #[test]
    fn drops_a_source_that_does_not_match_the_topic() {
        let (incoming, reading) = read("oxihome/a/up", &heartbeat_from("b"));
        assert!(incoming.is_none());
        assert_eq!(
            reading,
            Reading::Dropped("dropping message from b on oxihome/a/up".into())
        );
    }

    #[test]
    fn drops_invalid_json() {
        let (incoming, reading) = read("oxihome/a/up", "not json");
        assert!(incoming.is_none());
        assert!(
            matches!(reading, Reading::Dropped(reason) if reason.starts_with("invalid message"))
        );
    }
}
