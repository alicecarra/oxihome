//! A board without the broker: it talks to its gateway over ESP-NOW, and the gateway carries its
//! messages to and from the broker.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail};

use crate::espnow::{self, Mac, Radio};
use crate::relay::{self, Frame, Kind, Reassembler};
use crate::transport::{Transport, TransportEvent};
use crate::wifi::Wifi;

/// Between probes while the gateway is not answering or not on the broker.
const PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// The 2.4 GHz channels in Brazil.
const CHANNELS: std::ops::RangeInclusive<u8> = 1..=13;
/// Unconfirmed messages in a row before looking for the gateway again.
const FAILURES_BEFORE_PROBE: u8 = 3;

pub struct RelayTransport {
    radio: Radio,
    /// Started but not connected; connected only to download an update.
    wifi: Wifi,
    gateway: Mac,
    channel: Option<u8>,
    connected: bool,
    last_probe: Option<Instant>,
    failures: u8,
    reassembler: Reassembler,
    events: VecDeque<TransportEvent>,
}

impl RelayTransport {
    pub fn new(wifi: Wifi, gateway: &str, key: &str) -> anyhow::Result<Self> {
        let gateway_mac = relay::mac_from_device_id(gateway)
            .ok_or_else(|| anyhow!("espnow_gateway \"{gateway}\" is not a device id"))?;
        let radio = Radio::new(&[(gateway_mac, espnow::parse_key(key)?)])?;
        log::info!("ESP-NOW leaf of {gateway}");
        Ok(Self {
            radio,
            wifi,
            gateway: gateway_mac,
            channel: None,
            connected: false,
            last_probe: None,
            failures: 0,
            reassembler: Reassembler::default(),
            events: VecDeque::new(),
        })
    }

    /// Tries the known channel first, then all of them; the gateway answers a probe with
    /// `Connected` while it is on the broker.
    fn probe(&mut self) {
        self.last_probe = Some(Instant::now());
        let candidates = self
            .channel
            .into_iter()
            .chain(CHANNELS.filter(|channel| Some(*channel) != self.channel));
        for channel in candidates.collect::<Vec<_>>() {
            if espnow::set_channel(channel).is_err() {
                continue;
            }
            if self.radio.send(self.gateway, Kind::Probe, &[]).is_ok() {
                if self.channel != Some(channel) {
                    log::info!("Gateway found on channel {channel}");
                }
                self.channel = Some(channel);
                self.failures = 0;
                return;
            }
        }
        if self.channel.take().is_some() {
            log::warn!("Gateway lost");
        }
    }

    fn receive(&mut self) {
        while let Some((mac, bytes)) = self.radio.receive() {
            if mac != self.gateway {
                continue;
            }
            let Some(frame) = Frame::decode(&bytes) else {
                log::warn!("Invalid ESP-NOW frame from the gateway");
                continue;
            };
            match frame.kind {
                Kind::Connected => {
                    if !self.connected {
                        log::info!("Gateway on the broker");
                    }
                    self.connected = true;
                    self.reassembler = Reassembler::default();
                    self.events.push_back(TransportEvent::Connected);
                }
                Kind::Data => {
                    if let Some(message) = self.reassembler.push(&frame) {
                        self.events.push_back(TransportEvent::Received(message));
                    }
                }
                Kind::Probe => {}
            }
        }
    }
}

impl Transport for RelayTransport {
    fn send(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if self.channel.is_none() {
            bail!("gateway not found");
        }
        let result = self.radio.send(self.gateway, Kind::Data, bytes);
        if result.is_ok() {
            self.failures = 0;
        } else {
            self.failures += 1;
            if self.failures >= FAILURES_BEFORE_PROBE {
                self.connected = false;
            }
        }
        result
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        if self.events.is_empty() {
            if !self.connected
                && self
                    .last_probe
                    .is_none_or(|probe| probe.elapsed() >= PROBE_INTERVAL)
            {
                self.probe();
            }
            self.receive();
        }
        self.events.pop_front()
    }

    fn prepare_download(&mut self) -> anyhow::Result<()> {
        log::info!("Connecting to the Wi-Fi to download the update");
        self.wifi.connect()?;
        self.wifi.wait_netif_up()?;
        Ok(())
    }

    fn download_failed(&mut self) {
        if let Err(error) = self.wifi.disconnect() {
            log::warn!("Wi-Fi disconnection failed: {error}");
        }
        if let Some(channel) = self.channel {
            if let Err(error) = espnow::set_channel(channel) {
                log::warn!("Back to channel {channel} failed: {error:#}");
            }
        }
    }
}
