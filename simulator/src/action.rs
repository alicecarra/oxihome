//! What can be done to a simulated node, from the console or from the hub.

use std::time::Instant;

use oxihome_node::mqtt::{Handle, SoftwareNode};
use oxihome_protocol::Value;

use crate::node::SimulatedNode;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Online,
    Offline,
    Reboot,
    Set {
        entity: u8,
        channel: u8,
        value: Value,
    },
    /// Back to the wave for one channel, or for all of them.
    Auto {
        channel: Option<(u8, u8)>,
    },
    Event {
        entity: u8,
        channel: Option<u8>,
        value: Option<Value>,
    },
}

pub async fn apply(handle: &Handle<SimulatedNode>, action: Action) {
    match action {
        Action::Set {
            entity,
            channel,
            value,
        } => {
            handle
                .act(move |node| {
                    node.set(entity, channel, value).unwrap_or_else(|error| {
                        println!("{} {error}", node.device_id());
                        Vec::new()
                    })
                })
                .await;
        }
        Action::Auto { channel } => {
            handle.act(move |node| node.release(channel)).await;
        }
        Action::Event {
            entity,
            channel,
            value,
        } => {
            handle
                .act(move |node| match node.event(entity, channel, value) {
                    Ok(event) => vec![event],
                    Err(error) => {
                        println!("{} {error}", node.device_id());
                        Vec::new()
                    }
                })
                .await;
        }
        Action::Reboot => {
            handle
                .act(|node| {
                    println!("{} rebooting", node.device_id());
                    node.reboot(Instant::now());
                    node.announcement()
                })
                .await;
        }
        Action::Offline => {
            handle.disconnect().await;
        }
        Action::Online => {
            handle.connect().await;
        }
    }
}
