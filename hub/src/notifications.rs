use std::collections::VecDeque;

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

#[cfg(test)]
mod tests {
    use super::*;

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
