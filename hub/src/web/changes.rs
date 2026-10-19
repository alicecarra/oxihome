use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{self, Stream};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, sleep_until};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::sse::{Event, KeepAlive, Sse},
        route,
    },
};

use crate::hub::{Change, Hub};

/// After an event goes out, more of the same kind wait this long and leave as one.
const QUIET: Duration = Duration::from_secs(3);

/// What a page that missed changes is told, so it refreshes everything.
const EVERYTHING: [Change; 5] = [
    Change::Nodes,
    Change::Presence,
    Change::Config,
    Change::Notifications,
    Change::Timeline,
];

/// Sends the first change of a kind at once and holds the ones that follow within `QUIET`, so a
/// node sending a value every second does not make the pages reload every second; the held ones
/// leave together when the quiet time ends, so the last value always shows. Each node's changes
/// are a kind of their own.
#[derive(Default)]
struct Coalescer {
    kinds: HashMap<Change, Quiet>,
}

struct Quiet {
    until: Instant,
    held: bool,
}

impl Coalescer {
    /// Whether the change goes out now.
    fn receive(&mut self, change: Change, now: Instant) -> bool {
        // Kinds whose quiet time ended with nothing held are forgotten, so a node that went
        // away does not stay in the map.
        self.kinds
            .retain(|_, quiet| quiet.held || now < quiet.until);
        match self.kinds.get_mut(&change) {
            Some(quiet) => {
                quiet.held = true;
                false
            }
            None => {
                self.kinds.insert(
                    change,
                    Quiet {
                        until: now + QUIET,
                        held: false,
                    },
                );
                true
            }
        }
    }

    /// When the next held change is due.
    fn next_due(&self) -> Option<Instant> {
        self.kinds
            .values()
            .filter(|quiet| quiet.held)
            .map(|quiet| quiet.until)
            .min()
    }

    /// The held changes whose quiet time is over; each starts a new quiet time.
    fn due(&mut self, now: Instant) -> Vec<Change> {
        let mut due = Vec::new();
        for (change, quiet) in &mut self.kinds {
            if quiet.held && quiet.until <= now {
                quiet.held = false;
                quiet.until = now + QUIET;
                due.push(change.clone());
            }
        }
        due
    }
}

/// One event per change, named after what changed; the pages fetch the new HTML themselves.
#[route(GET "/events")]
pub async fn events(cx: &Cx) -> Result<Sse<impl Stream<Item = Result<Event>> + use<>>> {
    let hub: &Arc<Hub> = app_context(cx);
    // Ends when the hub shuts down, so the web server does not wait for open pages.
    let shutdown = Box::pin(hub.shutdown());
    let changes = stream::unfold(
        (
            hub.subscribe(),
            shutdown,
            Coalescer::default(),
            VecDeque::<Change>::new(),
        ),
        |(mut receiver, mut shutdown, mut coalescer, mut ready)| async move {
            loop {
                if let Some(change) = ready.pop_front() {
                    let name = change.event_name();
                    // Browsers drop events with empty data.
                    return Some((
                        Ok(Event::new().event(name.clone()).data(name)),
                        (receiver, shutdown, coalescer, ready),
                    ));
                }
                let due = coalescer.next_due();
                let received = tokio::select! {
                    received = receiver.recv() => received,
                    () = sleep_until(due.unwrap_or_else(Instant::now)), if due.is_some() => {
                        ready.extend(coalescer.due(Instant::now()));
                        continue;
                    }
                    () = &mut shutdown => return None,
                };
                let received = match received {
                    Ok(change) => vec![change],
                    // Some changes were dropped; refreshing everything covers them.
                    Err(RecvError::Lagged(_)) => EVERYTHING.to_vec(),
                    Err(RecvError::Closed) => return None,
                };
                let now = Instant::now();
                ready.extend(
                    received
                        .into_iter()
                        .filter(|change| coalescer.receive(change.clone(), now)),
                );
            }
        },
    );
    Ok(Sse::new(changes).keep_alive(KeepAlive::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(changes: Vec<Change>) -> Vec<String> {
        changes.iter().map(Change::event_name).collect()
    }

    #[test]
    fn first_change_goes_out_at_once() {
        let mut coalescer = Coalescer::default();
        assert!(coalescer.receive(Change::Nodes, Instant::now()));
        assert_eq!(coalescer.next_due(), None);
    }

    #[test]
    fn changes_within_the_quiet_time_leave_as_one_at_its_end() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        assert!(coalescer.receive(Change::Nodes, start));
        for second in 1..3 {
            assert!(!coalescer.receive(Change::Nodes, start + Duration::from_secs(second)));
        }
        assert_eq!(coalescer.next_due(), Some(start + QUIET));
        assert!(coalescer.due(start + Duration::from_secs(2)).is_empty());
        assert_eq!(names(coalescer.due(start + QUIET)), ["nodes"]);
        assert_eq!(coalescer.next_due(), None);
        // The held one started a new quiet time.
        assert!(!coalescer.receive(Change::Nodes, start + QUIET + Duration::from_secs(1)));
    }

    #[test]
    fn kinds_are_held_apart() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        assert!(coalescer.receive(Change::Nodes, start));
        assert!(coalescer.receive(Change::Notifications, start));
        assert!(!coalescer.receive(Change::Nodes, start + Duration::from_secs(1)));
        assert_eq!(names(coalescer.due(start + QUIET)), ["nodes"]);
    }

    #[test]
    fn each_node_is_held_apart() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        let printer = Change::Node("printer".into());
        let weather = Change::Node("weather".into());
        assert!(coalescer.receive(printer.clone(), start));
        assert!(coalescer.receive(weather, start + Duration::from_secs(1)));
        assert!(!coalescer.receive(printer, start + Duration::from_secs(1)));
        assert_eq!(names(coalescer.due(start + QUIET)), ["node-printer"]);
    }

    #[test]
    fn quiet_kinds_are_forgotten() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        assert!(coalescer.receive(Change::Node("printer".into()), start));
        assert!(coalescer.receive(Change::Nodes, start + QUIET));
        assert_eq!(coalescer.kinds.len(), 1);
    }

    #[test]
    fn a_change_after_the_quiet_time_goes_out_at_once() {
        let start = Instant::now();
        let mut coalescer = Coalescer::default();
        assert!(coalescer.receive(Change::Timeline, start));
        assert!(coalescer.receive(Change::Timeline, start + QUIET));
    }
}
