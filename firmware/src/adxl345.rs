use std::time::{Duration, Instant};

use anyhow::Context;
use esp_idf_svc::hal::delay::TickType;
use esp_idf_svc::hal::i2c::I2cDriver;

use crate::aht20_bmp280::scan;

/// 0x53 with SDO low (the usual module), 0x1D with SDO high.
const ADDRESSES: [u8; 2] = [0x53, 0x1D];
const DEVICE_ID: u8 = 0xE5;
const TIMEOUT_MS: u64 = 100;

const REGISTER_DEVICE_ID: u8 = 0x00;
const REGISTER_POWER_CONTROL: u8 = 0x2D;
const REGISTER_DATA_FORMAT: u8 = 0x31;
const REGISTER_DATA: u8 = 0x32;
const REGISTER_FIFO_CONTROL: u8 = 0x38;
const REGISTER_FIFO_STATUS: u8 = 0x39;

const POWER_MEASURE: u8 = 0x08;
/// Full resolution, ±16 g.
const FULL_RESOLUTION_16_G: u8 = 0x0B;
/// Keeps the latest 32 samples, taken at the default 100 Hz.
const FIFO_STREAM: u8 = 0x80;
const FIFO_ENTRIES_MASK: u8 = 0x3F;
const G_PER_STEP: f32 = 1.0 / 256.0;
/// Quiet time before the vibration counts as over.
const QUIET: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Reading {
    pub fn pitch(&self) -> f32 {
        (-self.x).atan2(self.y.hypot(self.z)).to_degrees()
    }

    pub fn roll(&self) -> f32 {
        self.y.atan2(self.z).to_degrees()
    }
}

/// The chip's FIFO holds the samples between checks, so a check sees all of them. Vibration is the
/// swing inside one check: gravity and a slow tilt do not count.
pub struct Adxl345 {
    i2c: I2cDriver<'static>,
    address: u8,
    threshold_g: f32,
    last_vibration: Option<Instant>,
    vibrating: bool,
    /// The average of the samples of the latest check.
    latest: Option<Reading>,
    last: Option<Reading>,
}

impl Adxl345 {
    pub fn new(mut i2c: I2cDriver<'static>, threshold_milli_g: u16) -> anyhow::Result<Self> {
        let address = ADDRESSES
            .into_iter()
            .find(|&address| {
                let mut id = [0];
                i2c.write_read(address, &[REGISTER_DEVICE_ID], &mut id, timeout())
                    .is_ok()
                    && id[0] == DEVICE_ID
            })
            .with_context(|| format!("ADXL345 not answering; {}", scan(&mut i2c)))?;
        // A restart of the board does not reset the chip, so it may still be measuring.
        for (register, value) in [
            (REGISTER_POWER_CONTROL, 0),
            (REGISTER_DATA_FORMAT, FULL_RESOLUTION_16_G),
            (REGISTER_FIFO_CONTROL, FIFO_STREAM),
            (REGISTER_POWER_CONTROL, POWER_MEASURE),
        ] {
            i2c.write(address, &[register, value], timeout())?;
        }
        log::info!("ADXL345 at 0x{address:02x}");
        Ok(Self {
            i2c,
            address,
            threshold_g: f32::from(threshold_milli_g) / 1000.0,
            last_vibration: None,
            vibrating: false,
            latest: None,
            last: None,
        })
    }

    /// Whether the vibration started or stopped since the last check.
    pub fn check_vibration(&mut self) -> anyhow::Result<bool> {
        let mut status = [0];
        self.i2c.write_read(
            self.address,
            &[REGISTER_FIFO_STATUS],
            &mut status,
            timeout(),
        )?;
        let entries = status[0] & FIFO_ENTRIES_MASK;

        let mut lowest = [f32::MAX; 3];
        let mut highest = [f32::MIN; 3];
        let mut sum = [0.0; 3];
        for _ in 0..entries {
            let mut data = [0; 6];
            self.i2c
                .write_read(self.address, &[REGISTER_DATA], &mut data, timeout())?;
            for axis in 0..3 {
                let raw = i16::from_le_bytes([data[axis * 2], data[axis * 2 + 1]]);
                let value = f32::from(raw) * G_PER_STEP;
                lowest[axis] = lowest[axis].min(value);
                highest[axis] = highest[axis].max(value);
                sum[axis] += value;
            }
        }
        if entries > 0 {
            let count = f32::from(entries);
            self.latest = Some(Reading {
                x: sum[0] / count,
                y: sum[1] / count,
                z: sum[2] / count,
            });
            let swing = (0..3)
                .map(|axis| highest[axis] - lowest[axis])
                .fold(0.0, f32::max);
            if swing > self.threshold_g {
                if !self.vibrating {
                    log::info!("ADXL345: vibration started, swing of {swing:.3} g");
                }
                self.last_vibration = Some(Instant::now());
            }
        }

        let vibrating = self
            .last_vibration
            .is_some_and(|instant| instant.elapsed() < QUIET);
        let changed = vibrating != self.vibrating;
        if changed && !vibrating {
            log::info!("ADXL345: vibration stopped");
        }
        self.vibrating = vibrating;
        Ok(changed)
    }

    /// Makes the latest check's samples the reading to report; false before the first of them.
    pub fn read(&mut self) -> bool {
        let Some(reading) = self.latest else {
            return false;
        };
        log::info!(
            "ADXL345: {:.2} {:.2} {:.2} g, pitch {:.1}°, roll {:.1}°",
            reading.x,
            reading.y,
            reading.z,
            reading.pitch(),
            reading.roll()
        );
        self.last = Some(reading);
        true
    }

    pub fn vibrating(&self) -> bool {
        self.vibrating
    }

    pub fn last(&self) -> Option<Reading> {
        self.last
    }
}

fn timeout() -> u32 {
    TickType::new_millis(TIMEOUT_MS).ticks()
}
