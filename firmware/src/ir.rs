use std::time::Duration;

use esp_idf_svc::hal::gpio::InputPin;
use esp_idf_svc::hal::rmt::config::{MemoryAccess, ReceiveConfig, RxChannelConfig};
use esp_idf_svc::hal::rmt::{RxChannelDriver, Symbol};
use esp_idf_svc::hal::units::Hertz;
use esp_idf_svc::sys::ESP_ERR_TIMEOUT;

/// 1 tick = 1 µs.
const RESOLUTION: Hertz = Hertz(1_000_000);
const MAX_SYMBOLS: usize = 256;
/// Longer than any mark or space inside a frame (the NEC leader mark is 9 ms), shorter than the
/// gap between frames.
const FRAME_END: Duration = Duration::from_millis(12);
/// Shorter frames are noise from lamps and sunlight.
const MIN_DURATIONS: usize = 8;
const MAX_RAW_DURATIONS: usize = 128;
/// `raw` and 128 durations of up to 5 digits fit with room to spare.
pub const MAX_CODE_LENGTH: u16 = 1024;

/// VS1838B demodulating receiver, read by the RMT peripheral.
pub struct IrReceiver {
    channel: RxChannelDriver<'static>,
    symbols: Vec<Symbol>,
    last_nec: Option<String>,
}

impl IrReceiver {
    pub fn new(pin: impl InputPin + 'static) -> anyhow::Result<Self> {
        let channel = RxChannelDriver::new(
            pin,
            &RxChannelConfig {
                resolution: RESOLUTION,
                memory_access: MemoryAccess::Indirect {
                    memory_block_symbols: MAX_SYMBOLS,
                },
                ..Default::default()
            },
        )?;
        Ok(Self {
            channel,
            symbols: vec![Symbol::default(); MAX_SYMBOLS],
            last_nec: None,
        })
    }

    /// The code of a frame received since the last call, without waiting.
    pub fn poll(&mut self) -> Option<String> {
        let config = ReceiveConfig {
            signal_range_min: Duration::from_nanos(1000),
            signal_range_max: FRAME_END,
            timeout: Some(0),
            ..Default::default()
        };
        let count = match self.channel.receive(&mut self.symbols, &config) {
            Ok(count) => count,
            Err(error) if error.code() == ESP_ERR_TIMEOUT => return None,
            Err(error) => {
                log::warn!("IR: {error}");
                return None;
            }
        };
        let durations: Vec<u32> = self.symbols[..count]
            .iter()
            .flat_map(|symbol| [symbol.level0(), symbol.level1()])
            .map(|pulse| u32::from(pulse.ticks.ticks()))
            .take_while(|ticks| *ticks > 0)
            .collect();
        self.describe(&durations)
    }

    fn describe(&mut self, durations: &[u32]) -> Option<String> {
        let merged = merge_glitches(durations);
        if is_nec_repeat(&merged) {
            return self.last_nec.as_ref().map(|code| format!("{code} repeat"));
        }
        if durations.len() < MIN_DURATIONS {
            return None;
        }
        if let Some(code) = nec(&merged) {
            self.last_nec = Some(code.clone());
            return Some(code);
        }
        self.last_nec = None;
        let mut raw = String::from("raw");
        for duration in durations.iter().take(MAX_RAW_DURATIONS) {
            raw.push_str(&format!(" {duration}"));
        }
        if durations.len() > MAX_RAW_DURATIONS {
            raw.push_str(" …");
        }
        Some(raw)
    }
}

/// Pulses shorter than this are noise (lamps, a remote at an angle): they are merged with their
/// neighbours, which keeps marks and spaces alternating.
const GLITCH: u32 = 250;

fn merge_glitches(durations: &[u32]) -> Vec<u32> {
    let mut merged: Vec<u32> = Vec::with_capacity(durations.len());
    let mut index = 0;
    while index < durations.len() {
        let duration = durations[index];
        match (merged.last_mut(), durations.get(index + 1)) {
            (Some(previous), Some(next)) if duration < GLITCH => {
                *previous += duration + next;
                index += 2;
            }
            _ => {
                merged.push(duration);
                index += 1;
            }
        }
    }
    merged
}

/// A NEC repeat: a long mark, a 2.25 ms space and a short mark, alone in its frame.
fn is_nec_repeat(durations: &[u32]) -> bool {
    matches!(durations, [mark, space, end] | [mark, space, end, _]
        if *mark >= 6000 && (1800..=2700).contains(space) && (200..=1100).contains(end))
}

/// `NEC 00 45` (8-bit address and command, in hex) or `NEC 1234 45` (extended 16-bit address).
/// The header's 4.5 ms space may come anywhere (noise before it is skipped); each bit is a short
/// mark and a space below 1.1 ms for 0 or up to 2.4 ms for 1. The inverted command byte rules out
/// wrong readings.
fn nec(durations: &[u32]) -> Option<String> {
    (1..durations.len())
        .step_by(2)
        .filter(|&index| (3500..=5500).contains(&durations[index]))
        .find_map(|index| nec_bits(&durations[index + 1..]))
}

fn nec_bits(bits: &[u32]) -> Option<String> {
    if bits.len() < 64 {
        return None;
    }
    let mut value: u32 = 0;
    for (index, pair) in bits[..64].chunks(2).enumerate() {
        if !(200..=1100).contains(&pair[0]) {
            return None;
        }
        match pair[1] {
            200..1100 => {}
            1100..=2400 => value |= 1 << index,
            _ => return None,
        }
    }
    let [address, address_check, command, command_check] = value.to_le_bytes();
    if command ^ command_check != 0xFF {
        return None;
    }
    Some(if address ^ address_check == 0xFF {
        format!("NEC {address:02X} {command:02X}")
    } else {
        format!(
            "NEC {:04X} {command:02X}",
            u16::from_le_bytes([address, address_check])
        )
    })
}
