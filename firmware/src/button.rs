use std::time::{Duration, Instant};

use esp_idf_svc::hal::gpio::{Input, PinDriver};
use oxihome_protocol::EventKind;

const DEBOUNCE: Duration = Duration::from_millis(30);
const LONG_PRESS: Duration = Duration::from_secs(1);

/// Active-low push button, read by polling.
pub struct Button {
    pin: PinDriver<'static, Input>,
    last_raw: bool,
    raw_since: Instant,
    pressed: bool,
    pressed_at: Instant,
    long_press_reported: bool,
}

impl Button {
    pub fn new(pin: PinDriver<'static, Input>) -> Self {
        let now = Instant::now();
        Self {
            pin,
            last_raw: false,
            raw_since: now,
            pressed: false,
            pressed_at: now,
            long_press_reported: false,
        }
    }

    /// `Press` fires on release, `LongPress` fires while still held.
    pub fn poll(&mut self) -> Option<EventKind> {
        let now = Instant::now();
        let raw = self.pin.is_low();

        if raw != self.last_raw {
            self.last_raw = raw;
            self.raw_since = now;
            return None;
        }

        if raw == self.pressed || now - self.raw_since < DEBOUNCE {
            if self.pressed && !self.long_press_reported && now - self.pressed_at >= LONG_PRESS {
                self.long_press_reported = true;
                return Some(EventKind::LongPress);
            }
            return None;
        }

        self.pressed = raw;
        if raw {
            self.pressed_at = now;
            self.long_press_reported = false;
            None
        } else if self.long_press_reported {
            None
        } else {
            Some(EventKind::Press)
        }
    }
}
