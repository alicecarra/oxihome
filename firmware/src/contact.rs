use std::time::{Duration, Instant};

use esp_idf_svc::hal::gpio::{Input, PinDriver};

const DEBOUNCE: Duration = Duration::from_millis(50);

/// Open or closed contact, read by polling; closed is low unless `inverted`.
pub struct Contact {
    pin: PinDriver<'static, Input>,
    inverted: bool,
    last_raw: bool,
    raw_since: Instant,
    closed: bool,
}

impl Contact {
    pub fn new(pin: PinDriver<'static, Input>, inverted: bool) -> Self {
        let closed = pin.is_low() != inverted;
        Self {
            pin,
            inverted,
            last_raw: closed,
            raw_since: Instant::now(),
            closed,
        }
    }

    pub fn closed(&self) -> bool {
        self.closed
    }

    /// Whether the contact changed.
    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        let raw = self.pin.is_low() != self.inverted;

        if raw != self.last_raw {
            self.last_raw = raw;
            self.raw_since = now;
            return false;
        }
        if raw == self.closed || now - self.raw_since < DEBOUNCE {
            return false;
        }
        self.closed = raw;
        true
    }
}
