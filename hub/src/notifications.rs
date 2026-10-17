use std::collections::VecDeque;

use crate::node::registry::NodeEvent;

const CAPACITY: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub message: String,
    /// What produced it, e.g. a rule name.
    pub source: String,
}

/// Recent notifications for the web interface, in memory only.
#[derive(Default)]
pub struct Notifications {
    recent: VecDeque<Notification>,
    last_id: u64,
}

impl Notifications {
    /// Taken from the clock at start, `first_id` keeps ids growing across restarts, so a page left
    /// open through a restart still gets the new toasts.
    pub fn starting_at(first_id: u64) -> Self {
        Self {
            recent: VecDeque::new(),
            last_id: first_id.saturating_sub(1),
        }
    }

    pub fn push(&mut self, title: String, message: String, source: String) -> u64 {
        self.last_id += 1;
        self.recent.push_back(Notification {
            id: self.last_id,
            title,
            message,
            source,
        });
        if self.recent.len() > CAPACITY {
            self.recent.pop_front();
        }
        self.last_id
    }

    pub fn last_id(&self) -> u64 {
        self.last_id
    }

    /// Notifications newer than `id`, oldest first.
    pub fn since(&self, id: u64) -> Vec<Notification> {
        self.recent
            .iter()
            .filter(|notification| notification.id > id)
            .cloned()
            .collect()
    }
}

/// Title and message of the toast for a node event, if it deserves one. `Registered` is left out
/// because every node registers when the hub starts.
pub fn for_node_event(event: &NodeEvent, name: &str) -> Option<(String, String)> {
    match event {
        NodeEvent::Offline(_) => {
            Some((format!("{name} offline"), "Stopped sending messages".into()))
        }
        NodeEvent::Online(_) => Some((
            format!("{name} back online"),
            "Sending messages again".into(),
        )),
        NodeEvent::Rebooted {
            previous_uptime,
            uptime,
            ..
        } => Some((
            format!("{name} rebooted"),
            format!("Uptime {previous_uptime} s -> {uptime} s"),
        )),
        NodeEvent::Registered { .. } | NodeEvent::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_events_that_become_toasts() {
        let offline = for_node_event(&NodeEvent::Offline("sim".into()), "Bedroom").unwrap();
        assert_eq!(offline.0, "Bedroom offline");

        let rebooted = for_node_event(
            &NodeEvent::Rebooted {
                device_id: "sim".into(),
                previous_uptime: 3485,
                uptime: 2,
            },
            "Test board",
        )
        .unwrap();
        assert_eq!(
            rebooted,
            ("Test board rebooted".into(), "Uptime 3485 s -> 2 s".into())
        );

        assert!(for_node_event(&NodeEvent::Unknown("sim".into()), "sim").is_none());
        assert!(
            for_node_event(
                &NodeEvent::Registered {
                    device_id: "sim".into(),
                    build: oxihome_protocol::Build::new("firmware", "0.4.0", "esp32"),
                    entity_count: 2,
                },
                "sim"
            )
            .is_none()
        );
    }

    fn push(notifications: &mut Notifications, title: &str) -> u64 {
        notifications.push(title.into(), "message".into(), "test".into())
    }

    #[test]
    fn since_returns_only_newer_notifications() {
        let mut notifications = Notifications::default();
        push(&mut notifications, "first");
        let seen = push(&mut notifications, "second");
        push(&mut notifications, "third");

        let titles: Vec<String> = notifications
            .since(seen)
            .into_iter()
            .map(|notification| notification.title)
            .collect();
        assert_eq!(titles, vec!["third"]);
        assert!(notifications.since(notifications.last_id()).is_empty());
    }

    #[test]
    fn ids_after_a_restart_are_newer() {
        let mut before = Notifications::starting_at(1_000);
        let seen = push(&mut before, "before");
        let mut after = Notifications::starting_at(2_000);
        push(&mut after, "after");

        assert_eq!(after.since(seen).len(), 1);
    }

    #[test]
    fn keeps_only_the_most_recent() {
        let mut notifications = Notifications::default();
        for index in 0..CAPACITY + 10 {
            push(&mut notifications, &index.to_string());
        }
        let all = notifications.since(0);
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].title, "10");
        assert_eq!(notifications.last_id(), (CAPACITY + 10) as u64);
    }
}
