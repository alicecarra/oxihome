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
        if is_nec_repeat(durations) {
            return self.last_nec.as_ref().map(|code| format!("{code} repeat"));
        }
        if durations.len() < MIN_DURATIONS {
            return None;
        }
        if let Some(code) = nec(durations) {
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

/// Within 25 % of the expected duration in µs.
fn near(duration: u32, expected: u32) -> bool {
    duration.abs_diff(expected) <= expected / 4
}

fn is_nec_repeat(durations: &[u32]) -> bool {
    matches!(durations, [mark, space, end, ..] if near(*mark, 9000) && near(*space, 2250) && near(*end, 560))
}

/// `NEC 00 45` (8-bit address and command, in hex) or `NEC 1234 45` (extended 16-bit address).
fn nec(durations: &[u32]) -> Option<String> {
    let [leader_mark, leader_space, bits @ ..] = durations else {
        return None;
    };
    if !near(*leader_mark, 9000) || !near(*leader_space, 4500) || bits.len() < 64 {
        return None;
    }
    let mut value: u32 = 0;
    for (index, pair) in bits[..64].chunks(2).enumerate() {
        if !near(pair[0], 560) {
            return None;
        }
        if near(pair[1], 1690) {
            value |= 1 << index;
        } else if !near(pair[1], 560) {
            return None;
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
