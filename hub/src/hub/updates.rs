//! Over-the-air updates: starting one, and its outcome from the node's next `Hello`, an
//! `Error` or the timeout.

use std::time::Instant;

use anyhow::{Context, ensure};
use jiff::Timestamp;
use oxihome_protocol::management::{self, ManagementChannel};

use super::{Change, CommandOrigin, Hub, UpdateStatus};
use crate::ota::Update;
use crate::storage::{StoredUpdate, UpdateOutcome, Write};

impl Hub {
    pub fn is_updating(&self, device_id: &str) -> bool {
        self.ota
            .as_ref()
            .is_some_and(|ota| ota.updating(device_id).is_some())
    }

    /// Only for an online node that declares the `Ota` channel.
    pub fn update_status(&self, device_id: &str) -> Option<UpdateStatus> {
        let ota = self.ota.as_ref()?;
        if let Some(version) = ota.updating(device_id) {
            return Some(UpdateStatus::InProgress(version));
        }
        let running = self.updatable_version(device_id)?;
        let (version, _) = ota.newer_image(device_id, &running)?;
        Some(UpdateStatus::Available(version))
    }

    /// Sends the node a single-use link to its newest image; the outcome comes with its next
    /// `Hello` (see `settle_update`).
    pub async fn start_update(&self, device_id: &str) -> anyhow::Result<()> {
        let ota = self
            .ota
            .as_ref()
            .context("over-the-air updates are off: no [ota] in hub.toml")?;
        ensure!(
            ota.updating(device_id).is_none(),
            "{device_id} is already updating"
        );
        let running = self
            .updatable_version(device_id)
            .with_context(|| format!("{device_id} is offline or does not update over the air"))?;
        let (version, image) = ota
            .newer_image(device_id, &running)
            .with_context(|| format!("no image newer than {running} for {device_id}"))?;
        let url = ota.start(device_id, running.clone(), version.clone(), image)?;
        self.announce(Change::Nodes);
        let channel = u8::from(ManagementChannel::Ota);
        if let Err(error) = self
            .invoke(
                device_id,
                management::ENTITY,
                channel,
                Some(url),
                CommandOrigin::Hub,
            )
            .await
        {
            if let Some(update) = ota.finish(device_id) {
                let detail = format!("{error:#}");
                self.record_update(device_id, update, UpdateOutcome::Failed, Some(detail));
            }
            return Err(error);
        }
        let name = self.node_name(device_id);
        self.notify(
            format!("{name} updating"),
            format!("Firmware {running} -> {version}"),
            device_id.to_string(),
        );
        Ok(())
    }

    /// A `Hello` after an update tells which version the node came back with.
    pub fn settle_update(&self, device_id: &str, running: &str) {
        let Some(update) = self.ota.as_ref().and_then(|ota| ota.finish(device_id)) else {
            return;
        };
        let outcome = if update.version == running {
            UpdateOutcome::Updated
        } else {
            UpdateOutcome::RolledBack
        };
        self.record_update(device_id, update, outcome, None);
    }

    /// The node reported an `Error` about the update (e.g. the download failed).
    pub fn fail_update(&self, device_id: &str, detail: &str) {
        if let Some(update) = self.ota.as_ref().and_then(|ota| ota.finish(device_id)) {
            self.record_update(
                device_id,
                update,
                UpdateOutcome::Failed,
                Some(detail.into()),
            );
        }
    }

    pub fn check_updates(&self, now: Instant) {
        let Some(ota) = &self.ota else {
            return;
        };
        for (device_id, update) in ota.timed_out(now) {
            self.record_update(&device_id, update, UpdateOutcome::NoAnswer, None);
        }
    }

    /// Stored for the node page, and notified.
    fn record_update(
        &self,
        device_id: &str,
        update: Update,
        outcome: UpdateOutcome,
        detail: Option<String>,
    ) {
        self.updates_finished
            .lock()
            .unwrap()
            .insert(device_id.to_string(), Instant::now());
        let name = self.node_name(device_id);
        let (title, message) = match outcome {
            UpdateOutcome::Updated => (
                format!("{name} updated"),
                format!("Firmware {}", update.version),
            ),
            UpdateOutcome::RolledBack => (
                format!("{name} update failed"),
                format!(
                    "Rolled back to {}; {} did not reach the broker",
                    update.from_version, update.version
                ),
            ),
            UpdateOutcome::Failed => (
                format!("{name} update failed"),
                format!(
                    "{}: {}",
                    update.version,
                    detail.as_deref().unwrap_or("error")
                ),
            ),
            UpdateOutcome::NoAnswer => (
                format!("{name} update failed"),
                format!("{}: no answer after the update", update.version),
            ),
        };
        self.storage.write(Write::Update(StoredUpdate {
            device_id: device_id.to_string(),
            started_at: update.started_at,
            finished_at: Timestamp::now(),
            from_version: update.from_version,
            to_version: update.version,
            outcome,
            detail,
        }));
        self.notify(title, message, device_id.to_string());
        self.announce(Change::Nodes);
    }

    /// Newest first; empty when the database fails (logged).
    pub async fn updates(&self, device_id: &str, limit: usize) -> Vec<StoredUpdate> {
        self.storage
            .updates(device_id, limit)
            .await
            .unwrap_or_else(|error| {
                println!("storage: updates of {device_id}: {error:#}");
                Vec::new()
            })
    }

    fn updatable_version(&self, device_id: &str) -> Option<String> {
        let registry = self.registry.lock().unwrap();
        let node = registry.node(device_id)?;
        let declares = node.declares(management::ENTITY, ManagementChannel::Ota.into());
        (node.online && declares).then(|| node.build.version.clone())
    }
}
