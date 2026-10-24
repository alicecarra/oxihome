//! Telling when the config names channels a node no longer declares.

use super::Hub;
use crate::config::references::{missing, references};

const NOTICE_KEY: &str = "missing-channels";

impl Hub {
    /// After the node's `Hello` or a config change: notifies when what is missing changes.
    pub fn check_references(&self, device_id: &str) {
        let Some(entities) = self.entities(device_id) else {
            return;
        };
        let found = {
            let rules = self.rules.lock().unwrap();
            let dashboards = self.dashboards.lock().unwrap();
            let nodes = self.nodes.lock().unwrap();
            references(device_id, &rules, &dashboards, &nodes)
        };
        let labels: Vec<String> = missing(found, &entities)
            .iter()
            .map(|reference| reference.label())
            .collect();
        let before = self
            .missing_references
            .lock()
            .unwrap()
            .insert(device_id.to_string(), labels.clone())
            .unwrap_or_default();
        if labels == before {
            return;
        }
        let name = self.node_name(device_id);
        let (title, message) = if labels.is_empty() {
            (
                format!("{name}: the config uses only channels it has"),
                "Every rule, dashboard block and name points at a declared channel".to_string(),
            )
        } else {
            println!(
                "{device_id} does not declare what the config uses: {}",
                labels.join("; ")
            );
            (
                format!("{name}: the config uses channels it no longer has"),
                labels.join("; "),
            )
        };
        self.notify_keyed(
            title,
            message,
            device_id.to_string(),
            Some(NOTICE_KEY.to_string()),
        );
    }

    pub fn check_all_references(&self) {
        for device_id in self.device_ids() {
            self.check_references(&device_id);
        }
    }
}
