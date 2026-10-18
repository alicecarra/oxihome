//! How well each node stays connected: for the network page, to find the board with bad Wi-Fi.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How far back reconnections, reboots and signal samples go.
pub const NETWORK_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connection {
    pub since: Instant,
    /// False when the hub only knows it was already connected by then, e.g. after a hub restart.
    pub exact: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkHistory {
    /// `None` while the node is offline.
    pub connection: Option<Connection>,
    /// Each `Hello` the node sent on its own after its first, which it does on every connection
    /// to the broker.
    reconnections: VecDeque<Instant>,
    reboots: VecDeque<Instant>,
    /// From its heartbeats, oldest first.
    signal: VecDeque<(Instant, i8)>,
}

impl NetworkHistory {
    pub fn connected(&mut self, now: Instant) {
        self.connection = Some(Connection {
            since: now,
            exact: true,
        });
    }

    pub fn reconnected(&mut self, now: Instant) {
        self.connected(now);
        push(&mut self.reconnections, now, now, |at| *at);
    }

    /// The node talks, so it is connected, but the hub did not see when it connected.
    pub fn seen(&mut self, now: Instant) {
        self.connection.get_or_insert(Connection {
            since: now,
            exact: false,
        });
    }

    pub fn disconnected(&mut self) {
        self.connection = None;
    }

    pub fn rebooted(&mut self, now: Instant) {
        push(&mut self.reboots, now, now, |at| *at);
    }

    pub fn signal_sample(&mut self, rssi: i8, now: Instant) {
        push(&mut self.signal, (now, rssi), now, |(at, _)| *at);
    }

    /// Reconnections in the window that did not come from a reboot: the network dropped.
    pub fn reconnections(&self, now: Instant) -> usize {
        let reconnections = recent(&self.reconnections, now, |at| *at).count();
        reconnections.saturating_sub(self.reboots(now))
    }

    pub fn reboots(&self, now: Instant) -> usize {
        recent(&self.reboots, now, |at| *at).count()
    }

    /// Signal samples in the window, oldest first.
    pub fn signal(&self, now: Instant) -> Vec<i8> {
        recent(&self.signal, now, |(at, _)| *at)
            .map(|(_, rssi)| *rssi)
            .collect()
    }
}

fn push<T>(samples: &mut VecDeque<T>, sample: T, now: Instant, at: impl Fn(&T) -> Instant) {
    while samples
        .front()
        .is_some_and(|oldest| now.saturating_duration_since(at(oldest)) > NETWORK_WINDOW)
    {
        samples.pop_front();
    }
    samples.push_back(sample);
}

fn recent<T>(
    samples: &VecDeque<T>,
    now: Instant,
    at: impl Fn(&T) -> Instant,
) -> impl Iterator<Item = &T> {
    samples
        .iter()
        .filter(move |sample| now.saturating_duration_since(at(sample)) <= NETWORK_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reboots_are_not_reconnections() {
        let start = Instant::now();
        let mut history = NetworkHistory::default();
        history.connected(start);
        history.reconnected(start + Duration::from_secs(60));
        history.reconnected(start + Duration::from_secs(90));
        history.reconnected(start + Duration::from_secs(120));
        history.rebooted(start + Duration::from_secs(125));

        let now = start + Duration::from_secs(130);
        assert_eq!(history.reboots(now), 1);
        assert_eq!(history.reconnections(now), 2);
    }

    #[test]
    fn old_samples_leave_the_window() {
        let start = Instant::now();
        let mut history = NetworkHistory::default();
        history.signal_sample(-80, start);
        history.reconnected(start);
        let later = start + NETWORK_WINDOW + Duration::from_secs(1);
        history.signal_sample(-60, later);

        assert_eq!(history.signal(later), vec![-60]);
        assert_eq!(history.reconnections(later), 0);
        assert_eq!(history.signal.len(), 1);
    }

    #[test]
    fn seen_keeps_a_known_connection() {
        let start = Instant::now();
        let mut history = NetworkHistory::default();
        history.seen(start);
        history.seen(start + Duration::from_secs(5));
        assert_eq!(
            history.connection,
            Some(Connection {
                since: start,
                exact: false
            })
        );

        history.connected(start + Duration::from_secs(10));
        history.seen(start + Duration::from_secs(15));
        assert!(history.connection.unwrap().exact);
    }
}
