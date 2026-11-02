use std::sync::mpsc::{self, Receiver};
use std::thread;

use esp_idf_svc::mqtt::client::{
    Details, EspMqttClient, EventPayload, MqttClientConfiguration, QoS,
};

use crate::gateway::Gateway;

pub enum TransportEvent {
    /// (Re)connected: the peer may have missed messages.
    Connected,
    Received(Vec<u8>),
}

pub trait Transport {
    fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()>;
    /// Never blocks.
    fn poll(&mut self) -> Option<TransportEvent>;
    /// Before an update is downloaded from the hub over HTTP.
    fn prepare_download(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn download_failed(&mut self) {}
}

const BUFFER_SIZE: usize = 4096;
const BROADCAST_TOPIC: &str = "oxihome/broadcast/down";

enum Incoming {
    Connected,
    Disconnected,
    Received { topic: String, data: Vec<u8> },
}

pub struct MqttTransport {
    client: EspMqttClient<'static>,
    events: Receiver<Incoming>,
    up_topic: String,
    down_topic: String,
    /// The leaves this board carries to the broker, when it is an ESP-NOW gateway.
    gateway: Option<Gateway>,
}

impl MqttTransport {
    pub fn new(
        url: &str,
        device_id: &str,
        password: &str,
        gateway: Option<Gateway>,
    ) -> anyhow::Result<Self> {
        let anonymous = password.is_empty();
        let (client, mut connection) = EspMqttClient::new(
            url,
            &MqttClientConfiguration {
                client_id: Some(device_id),
                username: (!anonymous).then_some(device_id),
                password: (!anonymous).then_some(password),
                // A hardware configuration, escaped inside a message, takes more than the default 1024.
                buffer_size: BUFFER_SIZE,
                out_buffer_size: BUFFER_SIZE,
                ..Default::default()
            },
        )?;

        let (sender, events) = mpsc::channel();
        thread::Builder::new().stack_size(6144).spawn(move || {
            while let Ok(event) = connection.next() {
                let forwarded = match event.payload() {
                    EventPayload::Connected(_) => {
                        log::info!("MQTT connected");
                        Some(Incoming::Connected)
                    }
                    EventPayload::Disconnected => {
                        log::warn!("MQTT disconnected");
                        Some(Incoming::Disconnected)
                    }
                    EventPayload::Received {
                        topic: Some(topic),
                        data,
                        details: Details::Complete,
                        ..
                    } => Some(Incoming::Received {
                        topic: topic.to_string(),
                        data: data.to_vec(),
                    }),
                    EventPayload::Received { .. } => {
                        log::warn!("Dropping fragmented MQTT message");
                        None
                    }
                    _ => None,
                };
                if let Some(forwarded) = forwarded {
                    if sender.send(forwarded).is_err() {
                        break;
                    }
                }
            }
        })?;

        Ok(Self {
            client,
            events,
            up_topic: format!("oxihome/{device_id}/up"),
            down_topic: format!("oxihome/{device_id}/down"),
            gateway,
        })
    }

    fn subscribe(&mut self) {
        let leaf_topics = self.gateway.iter().flat_map(Gateway::down_topics);
        let topics: Vec<String> = [self.down_topic.as_str(), BROADCAST_TOPIC]
            .into_iter()
            .chain(leaf_topics)
            .map(String::from)
            .collect();
        for topic in topics {
            if let Err(error) = self.client.subscribe(&topic, QoS::AtLeastOnce) {
                log::error!("Subscribing to {topic} failed: {error}");
            }
        }
    }
}

impl Transport for MqttTransport {
    fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.client
            .enqueue(&self.up_topic, QoS::AtLeastOnce, false, bytes)?;
        Ok(())
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        if let Some(gateway) = &mut self.gateway {
            gateway.poll(&mut self.client);
        }
        loop {
            match self.events.try_recv().ok()? {
                Incoming::Connected => {
                    self.subscribe();
                    if let Some(gateway) = &mut self.gateway {
                        gateway.broker_connected();
                    }
                    return Some(TransportEvent::Connected);
                }
                Incoming::Disconnected => {
                    if let Some(gateway) = &mut self.gateway {
                        gateway.broker_disconnected();
                    }
                }
                Incoming::Received { topic, data } => {
                    if topic == BROADCAST_TOPIC {
                        if let Some(gateway) = &mut self.gateway {
                            gateway.broadcast(&data);
                        }
                    } else if topic != self.down_topic {
                        if let Some(gateway) = &mut self.gateway {
                            if !gateway.deliver(&topic, &data) {
                                log::warn!("Message on unexpected topic {topic}");
                            }
                        }
                        continue;
                    }
                    return Some(TransportEvent::Received(data));
                }
            }
        }
    }
}
