//! The ESP-NOW radio shared by a leaf and its gateway: encrypted peers, messages sent in parts and
//! confirmed frame by frame.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use anyhow::{anyhow, bail};
use esp_idf_svc::espnow::{EspNow, PeerInfo, SendStatus};
use esp_idf_svc::sys::{
    esp, mbedtls_md_hmac, mbedtls_md_info_from_type, mbedtls_md_type_t_MBEDTLS_MD_SHA256,
    wifi_interface_t_WIFI_IF_STA, wifi_second_chan_t_WIFI_SECOND_CHAN_NONE,
};

use crate::relay::{self, Kind};

/// Only wraps the per-peer keys inside the driver; the secret part is each peer's key.
const PRIMARY_MASTER_KEY: &[u8; 16] = b"oxihome-espnow-1";
const KEY_LENGTH: usize = 16;
/// Longer than the radio's own retries.
const CONFIRMATION_TIMEOUT: Duration = Duration::from_millis(100);
const SEND_ATTEMPTS: u8 = 3;
/// Received frames waiting for the main loop; more are dropped.
const QUEUE_LENGTH: usize = 32;

pub type Mac = [u8; 6];

pub struct Radio {
    espnow: EspNow<'static>,
    frames: Receiver<(Mac, Vec<u8>)>,
    confirmations: Receiver<bool>,
    next_number: u16,
}

impl Radio {
    /// Needs the Wi-Fi started. `peers`: each peer's MAC and key.
    pub fn new(peers: &[(Mac, [u8; KEY_LENGTH])]) -> anyhow::Result<Self> {
        let espnow = EspNow::take()?;
        espnow.set_pmk(PRIMARY_MASTER_KEY)?;
        for (mac, key) in peers {
            espnow.add_peer(PeerInfo {
                peer_addr: *mac,
                lmk: *key,
                channel: 0,
                ifidx: wifi_interface_t_WIFI_IF_STA,
                encrypt: true,
                ..Default::default()
            })?;
        }

        let (frame_sender, frames) = mpsc::sync_channel(QUEUE_LENGTH);
        espnow.register_recv_cb(move |info, data| {
            if frame_sender.try_send((*info.src_addr, data.to_vec())).is_err() {
                log::warn!("ESP-NOW queue full; frame dropped");
            }
        })?;
        let (confirmation_sender, confirmations) = mpsc::sync_channel(QUEUE_LENGTH);
        espnow.register_send_cb(move |_, status| {
            let _ = confirmation_sender.try_send(matches!(status, SendStatus::SUCCESS));
        })?;

        Ok(Self {
            espnow,
            frames,
            confirmations,
            next_number: 0,
        })
    }

    /// Sends `message` in parts; fails when a part is not confirmed by the peer's radio.
    pub fn send(&mut self, peer: Mac, kind: Kind, message: &[u8]) -> anyhow::Result<()> {
        let number = self.next_number;
        self.next_number = self.next_number.wrapping_add(1);
        let frames = relay::split(kind, number, message)
            .ok_or_else(|| anyhow!("message of {} bytes is too long", message.len()))?;
        for frame in frames {
            self.send_frame(peer, &frame)?;
        }
        Ok(())
    }

    fn send_frame(&mut self, peer: Mac, frame: &[u8]) -> anyhow::Result<()> {
        while self.confirmations.try_recv().is_ok() {}
        for _ in 0..SEND_ATTEMPTS {
            self.espnow.send(peer, frame)?;
            match self.confirmations.recv_timeout(CONFIRMATION_TIMEOUT) {
                Ok(true) => return Ok(()),
                Ok(false) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => bail!("ESP-NOW stopped"),
            }
        }
        bail!("no answer from {}", mac_text(&peer))
    }

    /// A frame from any peer; never blocks.
    pub fn receive(&mut self) -> Option<(Mac, Vec<u8>)> {
        self.frames.try_recv().ok()
    }
}

/// The key of a leaf: the first 16 bytes of HMAC-SHA256(the gateway's secret, the leaf's device id).
/// `espnow-key.sh` makes the same one for the leaf's configuration.
pub fn leaf_key(secret: &[u8], device_id: &str) -> anyhow::Result<[u8; KEY_LENGTH]> {
    let mut digest = [0u8; 32];
    let result = unsafe {
        mbedtls_md_hmac(
            mbedtls_md_info_from_type(mbedtls_md_type_t_MBEDTLS_MD_SHA256),
            secret.as_ptr(),
            secret.len(),
            device_id.as_ptr(),
            device_id.len(),
            digest.as_mut_ptr(),
        )
    };
    if result != 0 {
        bail!("HMAC failed ({result})");
    }
    Ok(digest[..KEY_LENGTH].try_into().unwrap())
}

pub fn parse_key(text: &str) -> anyhow::Result<[u8; KEY_LENGTH]> {
    relay::hex(text)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| anyhow!("espnow_key must have 32 hex digits"))
}

/// Only while the station is not connected; connected, it follows the access point.
pub fn set_channel(channel: u8) -> anyhow::Result<()> {
    esp!(unsafe {
        esp_idf_svc::sys::esp_wifi_set_channel(channel, wifi_second_chan_t_WIFI_SECOND_CHAN_NONE)
    })?;
    Ok(())
}

pub fn channel() -> Option<u8> {
    let mut primary = 0;
    let mut second = wifi_second_chan_t_WIFI_SECOND_CHAN_NONE;
    esp!(unsafe { esp_idf_svc::sys::esp_wifi_get_channel(&mut primary, &mut second) }).ok()?;
    Some(primary)
}

pub fn mac_text(mac: &Mac) -> String {
    mac.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
