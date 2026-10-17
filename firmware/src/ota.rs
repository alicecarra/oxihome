use std::time::Duration;

use anyhow::{Context, bail};
use esp_idf_svc::http::Method;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::ota::{EspOta, SlotState};

/// A new image that has not reached the broker by then is rolled back.
pub const VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);
const BUFFER_SIZE: usize = 4096;

pub struct Ota {
    ota: EspOta,
    unverified: bool,
}

impl Ota {
    pub fn new() -> anyhow::Result<Self> {
        let ota = EspOta::new()?;
        let unverified = ota.get_running_slot()?.state == SlotState::Unverified;
        if unverified {
            log::warn!("Running a new image on trial; rolled back unless it reaches the broker");
        }
        Ok(Self { ota, unverified })
    }

    pub fn unverified(&self) -> bool {
        self.unverified
    }

    pub fn mark_valid(&mut self) -> anyhow::Result<()> {
        if self.unverified {
            self.ota.mark_running_slot_valid()?;
            self.unverified = false;
            log::info!("New image marked valid");
        }
        Ok(())
    }

    /// Writes the image to the other slot and boots from it next time; the caller restarts.
    pub fn update(&mut self, url: &str) -> anyhow::Result<()> {
        log::info!("OTA: downloading {url}");
        let mut connection = EspHttpConnection::new(&Configuration {
            buffer_size: Some(BUFFER_SIZE),
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })?;
        connection.initiate_request(Method::Get, url, &[])?;
        connection.initiate_response()?;
        let status = connection.status();
        if status != 200 {
            bail!("HTTP {status}");
        }
        let size: usize = connection
            .header("Content-Length")
            .and_then(|length| length.parse().ok())
            .context("no Content-Length")?;

        let mut update = self.ota.initiate_update_with_known_size(size)?;
        let mut buffer = vec![0; BUFFER_SIZE];
        let mut received = 0;
        while received < size {
            let read = connection.read(&mut buffer)?;
            if read == 0 {
                bail!("download ended at {received} of {size} bytes");
            }
            update.write(&buffer[..read])?;
            received += read;
            // A slow download must not pass for a stuck loop; a stalled one still times out above.
            unsafe { esp_idf_svc::sys::esp_task_wdt_reset() };
        }
        update.complete().context("image rejected")?;
        log::info!("OTA: {size} bytes written, rebooting");
        Ok(())
    }
}
