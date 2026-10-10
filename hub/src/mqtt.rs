use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use oxihome_protocol::{Message, Payload};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

const SOURCE: &str = "oxihome-hub";
const UP_TOPICS: &str = "oxihome/+/up";
const BROADCAST_TOPIC: &str = "oxihome/broadcast/down";

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
    pub fn connect(host: &str, port: u16) -> (Self, mpsc::Receiver<Incoming>) {
        let (client, mut event_loop) = AsyncClient::new(MqttOptions::new(SOURCE, host, port), 64);
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
        let message = Message {
            version: 1,
            msg_id: message_id,
            src: SOURCE.into(),
            dst: destination.map(String::from),
            ttl: 1,
            reply_to: None,
            timestamp: None,
            payload,
            auth: None,
        };
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
