use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use oxihome_protocol::Value;
use oxihome_protocol::hardware::Hardware;
use oxihome_protocol::management::{self, ManagementChannel};

use crate::hub::Hub;

/// What the hub knows about sending each node its hardware, by device id.
#[derive(Default)]
pub struct HardwareSync {
    /// From the send until the node's next `Hello`.
    pub sending: HashSet<String>,
    /// The hardware the node did not take and why; not sent again until `nodes.toml` changes it
    /// or the config is reloaded.
    pub refused: HashMap<String, (Hardware, String)>,
    /// What the node reported on its own (safe mode, drivers that failed) since its last `Hello`.
    pub reports: HashMap<String, Vec<String>>,
}

/// Where a node stands against the hardware `nodes.toml` lists for it.
#[derive(Debug, Clone, PartialEq)]
pub enum HardwareState {
    /// No `hardware` for the node in `nodes.toml`.
    NotConfigured,
    Offline,
    /// The node does not declare the `Hardware` channel.
    Unsupported,
    /// Not sent: the node's target would refuse it.
    Invalid(Vec<String>),
    Matches,
    Sending,
    Refused(String),
    /// Differs, and is sent at the next chance.
    Differs,
}

#[derive(Clone)]
pub struct HardwareStatus {
    pub desired: Option<Hardware>,
    /// The hardware the node mounts, from its `State`.
    pub reported: Option<Hardware>,
    pub state: HardwareState,
    pub reports: Vec<String>,
}

/// Sends the desired hardware when the node mounts another; the node restarts to apply it.
pub async fn apply(hub: Arc<Hub>, device_id: String) {
    let status = hub.hardware_status(&device_id);
    let (HardwareState::Differs, Some(desired)) = (status.state, status.desired) else {
        return;
    };
    if !hub.start_hardware_send(&device_id) {
        return;
    }
    let channel = u8::from(ManagementChannel::Hardware);
    let result = hub
        .set(
            &device_id,
            management::ENTITY,
            channel,
            Value::Text(desired.to_json()),
        )
        .await;
    match &result {
        Ok(()) => println!("{device_id}: hardware set, the node restarts"),
        Err(error) => println!("{device_id}: hardware not set: {error:#}"),
    }
    hub.finish_hardware_send(
        &device_id,
        desired,
        result.err().map(|error| format!("{error:#}")),
    );
}
