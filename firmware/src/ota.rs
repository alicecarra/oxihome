use std::time::Duration;

use anyhow::anyhow;
use esp_idf_svc::http::Method;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::ota::{EspOta, SlotState};
use oxihome_protocol::ErrorCode;

/// A new image that has not reached the broker by then is rolled back.
pub const VALIDATION_TIMEOUT: Duration = Duration::from_secs(60);
const BUFFER_SIZE: usize = 4096;

/// Why an update failed; each kind goes to the hub with its own error code.
#[derive(Debug)]
pub enum UpdateError {
    /// No connection, an HTTP status other than 200, or the transfer stopped.
    Download(anyhow::Error),
    /// Downloaded, but the image failed the final check.
    Rejected(anyhow::Error),
    /// Writing to flash or another failure inside the board.
    Internal(anyhow::Error),
}

impl UpdateError {
    pub fn code(&self) -> ErrorCode {
        match self {
            UpdateError::Download(_) => ErrorCode::UpdateDownloadFailed,
            UpdateError::Rejected(_) => ErrorCode::UpdateRejected,
            UpdateError::Internal(_) => ErrorCode::Internal,
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (UpdateError::Download(error)
        | UpdateError::Rejected(error)
        | UpdateError::Internal(error)) = self;
        write!(formatter, "{error:#}")
    }
}

fn download(error: impl Into<anyhow::Error>) -> UpdateError {
    UpdateError::Download(error.into())
}

fn internal(error: impl Into<anyhow::Error>) -> UpdateError {
    UpdateError::Internal(error.into())
}

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
    pub fn update(&mut self, url: &str) -> Result<(), UpdateError> {
        log::info!("OTA: downloading {url}");
        let mut connection = EspHttpConnection::new(&Configuration {
            buffer_size: Some(BUFFER_SIZE),
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .map_err(internal)?;
        connection
            .initiate_request(Method::Get, url, &[])
            .map_err(download)?;
        connection.initiate_response().map_err(download)?;
        let status = connection.status();
        if status != 200 {
            return Err(download(anyhow!("HTTP {status}")));
        }
        let size: usize = connection
            .header("Content-Length")
            .and_then(|length| length.parse().ok())
            .ok_or_else(|| download(anyhow!("no Content-Length")))?;

        let mut update = self
            .ota
            .initiate_update_with_known_size(size)
            .map_err(internal)?;
        let mut buffer = vec![0; BUFFER_SIZE];
        let mut received = 0;
        while received < size {
            let read = connection.read(&mut buffer).map_err(download)?;
            if read == 0 {
                return Err(download(anyhow!(
                    "download ended at {received} of {size} bytes"
                )));
            }
            update.write(&buffer[..read]).map_err(|error| {
                // The first write already refuses data that is not an ESP32 image.
                if error.code() == esp_idf_svc::sys::ESP_ERR_OTA_VALIDATE_FAILED {
                    UpdateError::Rejected(error.into())
                } else {
                    internal(error)
                }
            })?;
            received += read;
            // A slow download must not pass for a stuck loop; a stalled one still times out above.
            unsafe { esp_idf_svc::sys::esp_task_wdt_reset() };
        }
        update
            .complete()
            .map_err(|error| UpdateError::Rejected(error.into()))?;
        log::info!("OTA: {size} bytes written, rebooting");
        Ok(())
    }
}
