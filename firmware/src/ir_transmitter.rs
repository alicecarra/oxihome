use std::time::{Duration, Instant};

use esp_idf_svc::hal::gpio::OutputPin;
use esp_idf_svc::hal::rmt::config::{CarrierConfig, DutyPercent, TransmitConfig, TxChannelConfig};
use esp_idf_svc::hal::rmt::encoder::CopyEncoder;
use esp_idf_svc::hal::rmt::{PinState, Pulse, PulseTicks, RmtChannel, Symbol, TxChannelDriver};
use esp_idf_svc::hal::units::Hertz;

/// 1 tick = 1 µs.
const RESOLUTION: Hertz = Hertz(1_000_000);
const MAX_TICKS: u32 = 32_767;
const MAX_RAW_DURATIONS: usize = 512;

/// IR LED behind a transistor, driven by the RMT peripheral. Sends the codes `IrReceiver` reports.
pub struct IrTransmitter {
    channel: TxChannelDriver<'static>,
    encoder: CopyEncoder,
    carrier: Option<u32>,
    /// RC5 and RC6 flip it on each press, so a receiver tells a new press from a held key.
    toggle: bool,
}

/// What to send for one code.
#[derive(Debug, PartialEq)]
struct Signal {
    carrier_hz: u32,
    /// Marks and spaces in µs, starting with a mark.
    durations: Vec<u32>,
    frames: u8,
    /// From the start of a frame to the start of the next.
    frame_period: Duration,
}

impl IrTransmitter {
    pub fn new(pin: impl OutputPin + 'static) -> anyhow::Result<Self> {
        let channel = TxChannelDriver::new(
            pin,
            &TxChannelConfig {
                resolution: RESOLUTION,
                ..Default::default()
            },
        )?;
        Ok(Self {
            channel,
            encoder: CopyEncoder::new()?,
            carrier: None,
            toggle: false,
        })
    }

    /// Sends `code` and waits until it is out (under 200 ms). `Err` with a reason the hub can show
    /// for a code that cannot be sent.
    pub fn send(&mut self, code: &str) -> Result<(), String> {
        self.toggle = !self.toggle;
        let signal = signal(code, self.toggle)?;
        self.transmit(&signal)
            .map_err(|error| format!("RMT: {error}"))
    }

    fn transmit(&mut self, signal: &Signal) -> anyhow::Result<()> {
        if self.carrier != Some(signal.carrier_hz) {
            if self.channel.is_enabled() {
                self.channel.disable()?;
            }
            self.channel.apply_carrier(Some(&CarrierConfig {
                frequency: Hertz(signal.carrier_hz),
                duty_cycle: DutyPercent::new(0.33)?,
                ..Default::default()
            }))?;
            self.carrier = Some(signal.carrier_hz);
        }
        if !self.channel.is_enabled() {
            self.channel.enable()?;
        }
        let symbols = symbols(&signal.durations)?;
        for frame in 0..signal.frames {
            let started = Instant::now();
            self.channel
                .send_and_wait(&mut self.encoder, &symbols, &TransmitConfig::default())?;
            self.channel.wait_all_done(Some(Duration::from_secs(1)))?;
            if frame + 1 < signal.frames {
                std::thread::sleep(signal.frame_period.saturating_sub(started.elapsed()));
            }
        }
        Ok(())
    }
}

/// Pairs the marks and spaces into RMT symbols, splitting durations past the 15-bit tick counter.
fn symbols(durations: &[u32]) -> anyhow::Result<Vec<Symbol>> {
    let mut pulses = Vec::new();
    for (index, duration) in durations.iter().enumerate() {
        let state = if index % 2 == 0 {
            PinState::High
        } else {
            PinState::Low
        };
        let mut left = *duration;
        while left > 0 {
            let ticks = left.min(MAX_TICKS);
            pulses.push(Pulse::new(state, PulseTicks::new(ticks as u16)?));
            left -= ticks;
        }
    }
    // A zero-length pulse ends the transmission, so an odd count ends with a short space instead.
    if pulses.len() % 2 == 1 {
        pulses.push(Pulse::new(PinState::Low, PulseTicks::new(100)?));
    }
    Ok(pulses
        .chunks(2)
        .map(|pair| Symbol::new(pair[0], pair[1]))
        .collect())
}

/// The inverse of `IrReceiver`'s codes: `NEC 00 45`, `NEC 1234 45`, `SONY12 01 15`, `RC5 00 0C`,
/// `RC6 00 0C` or `raw 9000 4500 …`. A trailing `repeat` is ignored.
fn signal(code: &str, toggle: bool) -> Result<Signal, String> {
    let words: Vec<&str> = code
        .split_whitespace()
        .filter(|word| *word != "repeat" && *word != "…")
        .collect();
    let invalid = || format!("not a code this transmitter knows: \"{code}\"");
    let hex = |text: &str, digits: &[usize]| {
        if digits.contains(&text.len()) {
            u32::from_str_radix(text, 16).map_err(|_| invalid())
        } else {
            Err(invalid())
        }
    };
    match words.as_slice() {
        ["NEC", address, command] => Ok(nec(
            hex(address, &[2, 4])?,
            address.len() == 4,
            hex(command, &[2])?,
        )),
        [sony, address, command] if sony.starts_with("SONY") => {
            let length: usize = sony["SONY".len()..].parse().map_err(|_| invalid())?;
            let address_digits = match length {
                12 | 15 => 2,
                20 => 4,
                _ => return Err(invalid()),
            };
            let address = hex(address, &[address_digits])?;
            let command = hex(command, &[2])?;
            if command > 0x7F || address >= 1 << (length - 7) {
                return Err(invalid());
            }
            Ok(self::sony(length, address, command))
        }
        ["RC5", address, command] => {
            let (address, command) = (hex(address, &[2])?, hex(command, &[2])?);
            if address > 0x1F || command > 0x7F {
                return Err(invalid());
            }
            Ok(rc5(address, command, toggle))
        }
        ["RC6", address, command] => Ok(rc6(hex(address, &[2])?, hex(command, &[2])?, toggle)),
        ["raw", durations @ ..] if !durations.is_empty() => {
            if durations.len() > MAX_RAW_DURATIONS {
                return Err(format!("more than {MAX_RAW_DURATIONS} durations"));
            }
            let durations = durations
                .iter()
                .map(|duration| match duration.parse::<u32>() {
                    Ok(value) if value > 0 => Ok(value),
                    _ => Err(invalid()),
                })
                .collect::<Result<Vec<u32>, String>>()?;
            Ok(Signal {
                carrier_hz: 38_000,
                durations,
                frames: 1,
                frame_period: Duration::ZERO,
            })
        }
        _ => Err(invalid()),
    }
}

fn nec(address: u32, extended: bool, command: u32) -> Signal {
    let address = if extended {
        address
    } else {
        address | (!address & 0xFF) << 8
    };
    let value = address | command << 16 | (!command & 0xFF) << 24;
    let mut durations = vec![9000, 4500];
    for bit in 0..32 {
        durations.push(560);
        durations.push(if value >> bit & 1 == 1 { 1690 } else { 560 });
    }
    durations.push(560);
    Signal {
        carrier_hz: 38_000,
        durations,
        frames: 1,
        frame_period: Duration::ZERO,
    }
}

/// Least significant bit first: 7 command bits, then the address. Sony receivers want the frame
/// at least three times.
fn sony(length: usize, address: u32, command: u32) -> Signal {
    let value = command | address << 7;
    let mut durations = vec![2400];
    for bit in 0..length {
        durations.push(600);
        durations.push(if value >> bit & 1 == 1 { 1200 } else { 600 });
    }
    Signal {
        carrier_hz: 40_000,
        durations,
        frames: 3,
        frame_period: Duration::from_millis(45),
    }
}

/// Manchester, a space then a mark for 1: two start bits (the second clear for commands of 64 and
/// up), the toggle, 5 address bits and 6 command bits.
fn rc5(address: u32, command: u32, toggle: bool) -> Signal {
    let mut bits = vec![true, command < 0x40, toggle];
    bits.extend((0..5).rev().map(|bit| address >> bit & 1 == 1));
    bits.extend((0..6).rev().map(|bit| command >> bit & 1 == 1));
    let halves: Vec<bool> = bits.iter().flat_map(|bit| [!bit, *bit]).collect();
    Signal {
        carrier_hz: 36_000,
        durations: run_lengths(&halves, 889),
        frames: 1,
        frame_period: Duration::ZERO,
    }
}

/// A 6-unit mark and 2-unit space, then Manchester bits of 2 units, a mark first for 1: the start
/// bit, mode 0, the double-length toggle, 8 address bits and 8 command bits.
fn rc6(address: u32, command: u32, toggle: bool) -> Signal {
    let mut halves = vec![true; 6];
    halves.extend([false; 2]);
    let mut bits = vec![true, false, false, false];
    bits.extend((0..8).rev().map(|bit| address >> bit & 1 == 1));
    bits.extend((0..8).rev().map(|bit| command >> bit & 1 == 1));
    for (index, bit) in bits.iter().enumerate() {
        halves.extend([*bit, !bit]);
        if index == 3 {
            halves.extend([toggle, toggle, !toggle, !toggle]);
        }
    }
    Signal {
        carrier_hz: 36_000,
        durations: run_lengths(&halves, 444),
        frames: 1,
        frame_period: Duration::ZERO,
    }
}

/// Joins equal half bits of `unit` µs into marks and spaces; leading and trailing spaces are idle.
fn run_lengths(halves: &[bool], unit: u32) -> Vec<u32> {
    let start = halves.iter().position(|half| *half).unwrap_or(halves.len());
    let end = halves
        .iter()
        .rposition(|half| *half)
        .map_or(start, |last| last + 1);
    let mut durations: Vec<u32> = Vec::new();
    let mut previous = None;
    for half in &halves[start..end] {
        if previous == Some(*half) {
            *durations.last_mut().unwrap() += unit;
        } else {
            durations.push(unit);
        }
        previous = Some(*half);
    }
    durations
}
