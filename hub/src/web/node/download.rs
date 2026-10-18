//! The node's entities as a JSON file, for a `clone` node in the simulator.

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body,
        error::not_found,
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
        path_param,
        response::IntoResponse,
        route,
    },
};

use super::DeviceId;
use crate::hub::Hub;

#[route(GET "/nodes/{device_id}/entities.json")]
pub async fn entities_download(cx: &Cx) -> Result<impl IntoResponse> {
    let hub: &Arc<Hub> = app_context(cx);
    let device_id = path_param::<DeviceId>(cx).to_string();
    let (file_name, json) = entities_file(hub, &device_id).ok_or_else(not_found)?;
    Ok((
        [
            (CONTENT_TYPE, "application/json".to_string()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"{file_name}\""),
            ),
        ],
        Body::from(json),
    ))
}

/// The file name keeps only letters, digits, `-` and `_` of the id, which comes from the URL.
fn entities_file(hub: &Hub, device_id: &str) -> Option<(String, String)> {
    let entities = hub.entities(device_id)?;
    let safe_id: String = device_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let json = serde_json::to_string_pretty(&entities).ok()?;
    Some((format!("{safe_id}.json"), json + "\n"))
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use oxihome_protocol::{Access, Build, Channel, EntityDescriptor, Message, Payload, ValueType};

    use super::*;

    #[tokio::test]
    async fn the_file_holds_what_the_node_declared() {
        let hub = Hub::for_tests("entities-download", "").await;
        let entities = vec![EntityDescriptor {
            id: 1,
            name: None,
            channels: vec![Channel {
                id: 0,
                name: None,
                access: Access::ReadWrite,
                value: Some(ValueType::Bool),
                quantity: None,
                unit: None,
            }],
        }];
        let hello = Message::new(
            "esp32-test",
            0,
            Payload::Hello {
                build: Build::new("firmware", "0.17.0", "esp32"),
                ip: None,
                entities: entities.clone(),
            },
        );
        hub.handle(&hello, Instant::now());

        let (file_name, json) = entities_file(&hub, "esp32-test").unwrap();
        assert_eq!(file_name, "esp32-test.json");
        let read: Vec<EntityDescriptor> = serde_json::from_str(&json).unwrap();
        assert_eq!(read, entities);
        assert!(entities_file(&hub, "unknown").is_none());
    }
}
