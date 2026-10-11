use std::time::{Duration, Instant};

use esp_idf_svc::hal::gpio::{Output, PinDriver};

pub const PATTERNS: [&str; 3] = ["Short", "Long", "Alarm"];

/// Milliseconds on, off, on... starting with on.
fn steps(pattern: &str) -> Option<&'static [u64]> {
    match pattern {
        "Short" => Some(&[100]),
        "Long" => Some(&[1000]),
        "Alarm" => Some(&[300, 200, 300, 200, 300]),
        _ => None,
    }
}

/// Active buzzer behind a transistor; patterns play without blocking the main loop.
pub struct Buzzer {
    pin: PinDriver<'static, Output>,
    steps: &'static [u64],
    step: usize,
    step_started: Instant,
}

impl Buzzer {
    pub fn new(mut pin: PinDriver<'static, Output>) -> anyhow::Result<Self> {
        pin.set_low()?;
        Ok(Self {
            pin,
            steps: &[],
            step: 0,
            step_started: Instant::now(),
        })
    }

    /// Replaces the pattern playing, if any. `false` for an unknown pattern.
    pub fn play(&mut self, pattern: &str) -> anyhow::Result<bool> {
        let Some(steps) = steps(pattern) else {
            return Ok(false);
        };
        self.steps = steps;
        self.step = 0;
        self.step_started = Instant::now();
        self.pin.set_high()?;
        Ok(true)
    }

    pub fn poll(&mut self) -> anyhow::Result<()> {
        let Some(&duration) = self.steps.get(self.step) else {
            return Ok(());
        };
        if self.step_started.elapsed() < Duration::from_millis(duration) {
            return Ok(());
        }
        self.step += 1;
        self.step_started = Instant::now();
        if self.step < self.steps.len() && self.step % 2 == 0 {
            self.pin.set_high()?;
        } else {
            self.pin.set_low()?;
        }
        Ok(())
    }
}
