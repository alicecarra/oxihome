//! A board on the broker that carries the messages of its ESP-NOW leaves: what a leaf sends goes to
//! `oxihome/<leaf>/up`, and what comes on `oxihome/<leaf>/down` goes to the leaf, untouched.

use anyhow::anyhow;
use esp_idf_svc::mqtt::client::{EspMqttClient, QoS};

use crate::espnow::{self, Mac, Radio};
use crate::relay::{self, Frame, Kind, Reassembler};

/// Frames handled per poll, so the main loop keeps its pace when the leaves are busy.
const FRAMES_PER_POLL: usize = 8;

struct Leaf {
    device_id: String,
    mac: Mac,
    up_topic: String,
    down_topic: String,
    reassembler: Reassembler,
}

pub struct Gateway {
    radio: Radio,
    leaves: Vec<Leaf>,
    on_broker: bool,
}

impl Gateway {
    /// `leaves`: comma-separated device ids. `secret`: hex, the leaves' keys come from it.
    pub fn new(leaves: &str, secret: &str) -> anyhow::Result<Self> {
        let secret = relay::hex(secret)
            .filter(|secret| secret.len() >= 16)
            .ok_or_else(|| anyhow!("espnow_secret must have at least 32 hex digits"))?;
        let leaves = relay::device_ids(leaves)
            .map(|device_id| {
                let mac = relay::mac_from_device_id(device_id)
                    .ok_or_else(|| anyhow!("\"{device_id}\" in espnow_leaves is not a device id"))?;
                Ok(Leaf {
                    device_id: device_id.to_string(),
                    mac,
                    up_topic: format!("oxihome/{device_id}/up"),
                    down_topic: format!("oxihome/{device_id}/down"),
                    reassembler: Reassembler::default(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let peers = leaves
            .iter()
            .map(|leaf| Ok((leaf.mac, espnow::leaf_key(&secret, &leaf.device_id)?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let radio = Radio::new(&peers)?;
        log::info!(
            "ESP-NOW gateway for {} on channel {}",
            leaves
                .iter()
                .map(|leaf| leaf.device_id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            espnow::channel().map_or_else(|| "?".into(), |channel| channel.to_string())
        );
        Ok(Self {
            radio,
            leaves,
            on_broker: false,
        })
    }

    pub fn down_topics(&self) -> impl Iterator<Item = &str> {
        self.leaves.iter().map(|leaf| leaf.down_topic.as_str())
    }

    /// Tells every leaf, so each one announces itself to the hub again.
    pub fn broker_connected(&mut self) {
        self.on_broker = true;
        for index in 0..self.leaves.len() {
            self.send(index, Kind::Connected, &[]);
        }
    }

    pub fn broker_disconnected(&mut self) {
        self.on_broker = false;
    }

    /// `true` when `topic` belonged to a leaf.
    pub fn deliver(&mut self, topic: &str, message: &[u8]) -> bool {
        let Some(index) = self.leaves.iter().position(|leaf| leaf.down_topic == topic) else {
            return false;
        };
        self.send(index, Kind::Data, message);
        true
    }

    pub fn broadcast(&mut self, message: &[u8]) {
        for index in 0..self.leaves.len() {
            self.send(index, Kind::Data, message);
        }
    }

    /// Carries what the leaves sent to the broker.
    pub fn poll(&mut self, client: &mut EspMqttClient<'static>) {
        for _ in 0..FRAMES_PER_POLL {
            let Some((mac, bytes)) = self.radio.receive() else {
                return;
            };
            let Some(index) = self.leaves.iter().position(|leaf| leaf.mac == mac) else {
                continue;
            };
            let Some(frame) = Frame::decode(&bytes) else {
                log::warn!("Invalid ESP-NOW frame from {}", self.leaves[index].device_id);
                continue;
            };
            match frame.kind {
                Kind::Probe if self.on_broker => self.send(index, Kind::Connected, &[]),
                Kind::Data => {
                    let leaf = &mut self.leaves[index];
                    if let Some(message) = leaf.reassembler.push(&frame) {
                        if let Err(error) =
                            client.enqueue(&leaf.up_topic, QoS::AtLeastOnce, false, &message)
                        {
                            log::warn!("Message of {} dropped: {error}", leaf.device_id);
                        }
                    }
                }
                Kind::Probe | Kind::Connected => {}
            }
        }
    }

    fn send(&mut self, index: usize, kind: Kind, message: &[u8]) {
        let leaf = &self.leaves[index];
        if let Err(error) = self.radio.send(leaf.mac, kind, message) {
            log::warn!("To {}: {error:#}", leaf.device_id);
        }
    }
}
