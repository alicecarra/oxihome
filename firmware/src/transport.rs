use std::sync::mpsc::{self, Receiver};
use std::thread;

use esp_idf_svc::mqtt::client::{
    Details, EspMqttClient, EventPayload, MqttClientConfiguration, QoS,
};

pub enum TransportEvent {
    /// (Re)connected: the peer may have missed messages.
    Connected,
    Received(Vec<u8>),
}

pub trait Transport {
    fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()>;
    /// Never blocks.
    fn poll(&mut self) -> Option<TransportEvent>;
}

pub struct MqttTransport {
    client: EspMqttClient<'static>,
    events: Receiver<TransportEvent>,
    up_topic: String,
    down_topics: [String; 2],
}

impl MqttTransport {
    pub fn new(url: &str, device_id: &str, password: &str) -> anyhow::Result<Self> {
        let anonymous = password.is_empty();
        let (client, mut connection) = EspMqttClient::new(
            url,
            &MqttClientConfiguration {
                client_id: Some(device_id),
                username: (!anonymous).then_some(device_id),
                password: (!anonymous).then_some(password),
                ..Default::default()
            },
        )?;

        let (sender, events) = mpsc::channel();
        thread::Builder::new().stack_size(6144).spawn(move || {
            while let Ok(event) = connection.next() {
                let forwarded = match event.payload() {
                    EventPayload::Connected(_) => {
                        log::info!("MQTT connected");
                        Some(TransportEvent::Connected)
                    }
                    EventPayload::Disconnected => {
                        log::warn!("MQTT disconnected");
                        None
                    }
                    EventPayload::Received {
                        data,
                        details: Details::Complete,
                        ..
                    } => Some(TransportEvent::Received(data.to_vec())),
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
            down_topics: [
                format!("oxihome/{device_id}/down"),
                "oxihome/broadcast/down".into(),
            ],
        })
    }
}

impl Transport for MqttTransport {
    fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.client
            .enqueue(&self.up_topic, QoS::AtLeastOnce, false, bytes)?;
        Ok(())
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        let event = self.events.try_recv().ok()?;
        if let TransportEvent::Connected = event {
            for topic in &self.down_topics {
                if let Err(error) = self.client.subscribe(topic, QoS::AtLeastOnce) {
                    log::error!("Subscribing to {topic} failed: {error}");
                }
            }
        }
        Some(event)
    }
}
