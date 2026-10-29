use esp_idf_svc::hal::delay::NON_BLOCK;
use esp_idf_svc::hal::gpio::{AnyIOPin, AnyOutputPin, InputPin};
use esp_idf_svc::hal::uart::config::Config;
use esp_idf_svc::hal::uart::{UART1, UartRxDriver};
use esp_idf_svc::hal::units::Hertz;

use crate::nmea::{self, Position, Sentence};

const BAUD_RATE: Hertz = Hertz(9600);
/// NMEA caps a sentence at 82 characters.
const LINE_MAX_LENGTH: usize = 82;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Reading {
    pub fix: bool,
    pub satellites: u8,
    pub position: Option<Position>,
    pub altitude_meters: Option<f32>,
    pub speed_kilometers_per_hour: Option<f32>,
    pub unix_time: Option<i64>,
}

/// NMEA GPS module such as the NEO-6M; only its TX is wired, to the board's RX.
pub struct Gps {
    uart: UartRxDriver<'static>,
    line: Vec<u8>,
    reading: Reading,
    /// Nothing before the first good sentence: the module may not be wired.
    heard: bool,
}

impl Gps {
    pub fn new(uart: UART1<'static>, rx: impl InputPin + 'static) -> anyhow::Result<Self> {
        let config = Config::new().baudrate(BAUD_RATE);
        let uart = UartRxDriver::new(uart, rx, None::<AnyIOPin>, None::<AnyOutputPin>, &config)?;
        Ok(Self {
            uart,
            line: Vec::with_capacity(LINE_MAX_LENGTH),
            reading: Reading::default(),
            heard: false,
        })
    }

    /// Reads what arrived without waiting; `true` when the fix came or went.
    pub fn poll(&mut self) -> bool {
        let fix = self.reading.fix;
        let mut buffer = [0; 128];
        while let Ok(count @ 1..) = self.uart.read(&mut buffer, NON_BLOCK) {
            for &byte in &buffer[..count] {
                self.push(byte);
            }
        }
        self.reading.fix != fix
    }

    pub fn reading(&self) -> Option<Reading> {
        self.heard.then_some(self.reading)
    }

    fn push(&mut self, byte: u8) {
        match byte {
            b'$' => {
                self.line.clear();
                self.line.push(byte);
            }
            b'\n' => {
                let sentence = std::str::from_utf8(&self.line).ok().and_then(nmea::parse);
                self.line.clear();
                if let Some(sentence) = sentence {
                    self.apply(sentence);
                }
            }
            // A longer line fails its checksum.
            _ if self.line.len() < LINE_MAX_LENGTH => self.line.push(byte),
            _ => {}
        }
    }

    fn apply(&mut self, sentence: Sentence) {
        self.heard = true;
        match sentence {
            Sentence::Rmc(rmc) => {
                self.reading.fix = rmc.position.is_some();
                self.reading.position = rmc.position;
                self.reading.speed_kilometers_per_hour = rmc.speed_kilometers_per_hour;
                self.reading.unix_time = rmc.unix_time;
            }
            Sentence::Gga(gga) => {
                self.reading.satellites = gga.satellites;
                self.reading.altitude_meters = gga.altitude_meters;
            }
        }
    }
}
