//! Keeping each node's parameters and hardware as `nodes.toml` wants them.
//!
//! Lock order: the snapshot first, then the hardware sync, never the other way around.

use oxihome_protocol::Value;
use oxihome_protocol::hardware::Hardware;
use oxihome_protocol::management::{self, ManagementChannel, Parameter};

use super::{Change, Hub};
use crate::node::hardware::{HardwareState, HardwareStatus};

impl Hub {
    /// Desired parameters the node declares but reports with another value; empty while offline.
    pub fn parameter_differences(&self, device_id: &str) -> Vec<(Parameter, i32)> {
        let snapshot = self.snapshot();
        let Some(node) = snapshot.registry.node(device_id).filter(|node| node.online) else {
            return Vec::new();
        };
        snapshot
            .nodes
            .desired(device_id)
            .into_iter()
            .filter(|(parameter, value)| {
                let channel = u8::from(parameter.channel);
                node.declares(management::ENTITY, channel)
                    && snapshot.state.value(device_id, management::ENTITY, channel)
                        != Some(&Value::Int(*value))
            })
            .collect()
    }

    /// Locks the snapshot, then the hardware sync: never the other way around.
    pub fn hardware_status(&self, device_id: &str) -> HardwareStatus {
        let snapshot = self.snapshot();
        let desired = snapshot.nodes.hardware(device_id);
        let reported = match snapshot.state.value(
            device_id,
            management::ENTITY,
            ManagementChannel::Hardware.into(),
        ) {
            Some(Value::Text(json)) => Hardware::from_json(json).ok(),
            _ => None,
        };
        let node = snapshot.registry.node(device_id);
        let sync = self.hardware.lock().unwrap();
        let state = match (&desired, node) {
            (None, _) => HardwareState::NotConfigured,
            (Some(_), None) => HardwareState::Offline,
            (Some(_), Some(node)) if !node.online => HardwareState::Offline,
            (Some(desired), Some(node)) => {
                let problems = desired.validate(&node.build.target);
                if !node.declares(management::ENTITY, ManagementChannel::Hardware.into()) {
                    HardwareState::Unsupported
                } else if !problems.is_empty() {
                    HardwareState::Invalid(problems.iter().map(ToString::to_string).collect())
                } else if reported.as_ref() == Some(desired) {
                    HardwareState::Matches
                } else if sync.sending.contains(device_id) {
                    HardwareState::Sending
                } else if let Some((refused, reason)) = sync.refused.get(device_id)
                    && refused == desired
                {
                    HardwareState::Refused(reason.clone())
                } else {
                    HardwareState::Differs
                }
            }
        };
        HardwareStatus {
            desired,
            reported,
            state,
            reports: sync.reports.get(device_id).cloned().unwrap_or_default(),
        }
    }

    /// `false` when a send to the node is already under way.
    pub fn start_hardware_send(&self, device_id: &str) -> bool {
        let started = self
            .hardware
            .lock()
            .unwrap()
            .sending
            .insert(device_id.to_string());
        if started {
            self.announce(Change::Nodes);
        }
        started
    }

    /// Taken, the send lasts until the node's next `Hello`: until it restarts, the node still
    /// reports the hardware it mounts.
    pub fn finish_hardware_send(&self, device_id: &str, sent: Hardware, refusal: Option<String>) {
        let mut sync = self.hardware.lock().unwrap();
        match refusal {
            Some(reason) => {
                sync.sending.remove(device_id);
                sync.refused.insert(device_id.to_string(), (sent, reason));
            }
            None => {
                sync.refused.remove(device_id);
            }
        }
        drop(sync);
        self.announce(Change::Nodes);
    }

    /// An `Error` the node sent on its own, kept until its next `Hello`.
    pub fn record_node_report(&self, device_id: &str, detail: &str) {
        self.hardware
            .lock()
            .unwrap()
            .reports
            .entry(device_id.to_string())
            .or_default()
            .push(detail.to_string());
        self.announce(Change::Nodes);
    }

    /// A `Hello`: the node (re)started, and resends what it has to report.
    pub fn hardware_restarted(&self, device_id: &str) {
        let mut sync = self.hardware.lock().unwrap();
        sync.reports.remove(device_id);
        sync.sending.remove(device_id);
    }
}
