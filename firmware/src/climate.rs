use dht_sensor::dht11;
use esp_idf_svc::hal::delay::Delay;
use esp_idf_svc::hal::gpio::{InputOutput, PinDriver};

/// DHT11 on an open-drain pin; the module has its own pull-up.
pub struct Climate {
    pin: PinDriver<'static, InputOutput>,
    delay: Delay,
    last: Option<dht11::Reading>,
}

impl Climate {
    pub fn new(mut pin: PinDriver<'static, InputOutput>) -> anyhow::Result<Self> {
        // Idle high: a low line before the first read makes the sensor time out.
        pin.set_high()?;
        Ok(Self {
            pin,
            delay: Delay::new_default(),
            last: None,
        })
    }

    /// `None` when the read fails (timeouts and checksum errors happen now and then).
    pub fn read(&mut self) -> Option<dht11::Reading> {
        match dht11::blocking::read(&mut self.delay, &mut self.pin) {
            Ok(reading) => {
                self.last = Some(reading);
                Some(reading)
            }
            Err(error) => {
                log::warn!("DHT11: {error:?}");
                None
            }
        }
    }

    pub fn last(&self) -> Option<dht11::Reading> {
        self.last
    }
}
