use std::sync::Arc;

use oxihome_protocol::Value;
use oxihome_protocol::management;

use crate::hub::{CommandOrigin, Hub};

/// Sends a `Set` for every parameter whose reported value differs from the desired one.
pub async fn apply(hub: Arc<Hub>, device_id: String) {
    for (parameter, value) in hub.parameter_differences(&device_id) {
        let channel = u8::from(parameter.channel);
        match hub
            .set(
                &device_id,
                management::ENTITY,
                channel,
                Value::Int(value),
                CommandOrigin::Hub,
            )
            .await
        {
            Ok(()) => println!("{device_id}: {:?} set to {value}", parameter.channel),
            Err(error) => println!("{device_id}: {:?} not set: {error}", parameter.channel),
        }
    }
}
