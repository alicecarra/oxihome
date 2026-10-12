use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use oxihome_protocol::{Message, Payload};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

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
}

pub struct Mqtt {
    client: AsyncClient,
    next_message_id: AtomicU32,
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
                        match Message::from_json(&text) {
                            // The broker's ACL ties each topic to its node, but not the `src` field.
                            Ok(message)
                                if publish.topic != format!("oxihome/{}/up", message.src) =>
                            {
                                println!(
                                    "dropping message from {} on {}",
                                    message.src, publish.topic
                                );
                                continue;
                            }
                            Ok(message) => Incoming::Message(message),
                            Err(error) => {
                                println!("invalid message on {}: {error}", publish.topic);
                                continue;
                            }
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
            },
            receiver,
        )
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
        self.client
            .publish(topic, QoS::AtLeastOnce, false, message.to_json()?)
            .await?;
        Ok(())
    }
}
