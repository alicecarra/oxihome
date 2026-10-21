use dht_sensor::dht11;
use esp_idf_svc::hal::delay::Delay;
use esp_idf_svc::hal::gpio::{InputOutput, PinDriver};
use oxihome_protocol::ValueType;

use crate::aht20_bmp280::Aht20Bmp280;

#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub temperature: f32,
    pub humidity: f32,
    /// hPa; only sensors that measure it.
    pub pressure: Option<f32>,
}

enum Sensor {
    /// On an open-drain pin; the module has its own pull-up.
    Dht11 {
        pin: PinDriver<'static, InputOutput>,
        delay: Delay,
    },
    Aht20Bmp280(Aht20Bmp280),
}

pub struct Climate {
    sensor: Sensor,
    last: Option<Reading>,
}

impl Climate {
    pub fn dht11(mut pin: PinDriver<'static, InputOutput>) -> anyhow::Result<Self> {
        // Idle high: a low line before the first read makes the sensor time out.
        pin.set_high()?;
        Ok(Self::with(Sensor::Dht11 {
            pin,
            delay: Delay::new_default(),
        }))
    }

    pub fn aht20_bmp280(module: Aht20Bmp280) -> Self {
        Self::with(Sensor::Aht20Bmp280(module))
    }

    fn with(sensor: Sensor) -> Self {
        Self { sensor, last: None }
    }

    pub fn name(&self) -> &'static str {
        match self.sensor {
            Sensor::Dht11 { .. } => "DHT11",
            Sensor::Aht20Bmp280(_) => "AHT20 + BMP280",
        }
    }

    /// The datasheet ranges of temperature, humidity and pressure (`None` when not measured).
    pub fn ranges(&self) -> (ValueType, ValueType, Option<ValueType>) {
        let range = |min, max, step| ValueType::Float { min, max, step };
        match self.sensor {
            Sensor::Dht11 { .. } => (range(0.0, 50.0, 1.0), range(20.0, 90.0, 1.0), None),
            Sensor::Aht20Bmp280(_) => (
                range(-40.0, 85.0, 0.1),
                range(0.0, 100.0, 0.1),
                Some(range(300.0, 1100.0, 0.1)),
            ),
        }
    }

    /// `None` when the read fails (timeouts and checksum errors happen now and then).
    pub fn read(&mut self) -> Option<Reading> {
        let reading = match &mut self.sensor {
            Sensor::Dht11 { pin, delay } => dht11::blocking::read(delay, pin)
                .map(|reading| Reading {
                    temperature: reading.temperature.into(),
                    humidity: reading.relative_humidity.into(),
                    pressure: None,
                })
                .map_err(|error| format!("{error:?}")),
            Sensor::Aht20Bmp280(module) => module
                .measure()
                .map(|measurement| Reading {
                    temperature: measurement.temperature,
                    humidity: measurement.humidity,
                    pressure: Some(measurement.pressure),
                })
                .map_err(|error| format!("{error:#}")),
        };
        match reading {
            Ok(reading) => {
                self.last = Some(reading);
                Some(reading)
            }
            Err(error) => {
                log::warn!("{}: {error}", self.name());
                None
            }
        }
    }

    pub fn last(&self) -> Option<Reading> {
        self.last
    }
}
