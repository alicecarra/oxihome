use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver, Pull};
use oxihome_protocol::hardware::Hardware;

use crate::settings::Settings;

const PRESS_WINDOW: Duration = Duration::from_secs(3);
const HOLD: Duration = Duration::from_secs(5);
/// Boots without reaching the broker before the stored hardware is ignored.
pub const SAFE_MODE_BOOTS: u8 = 3;

/// The hardware to mount on this boot.
pub struct Selection {
    pub hardware: Hardware,
    /// The stored hardware set aside by the safe mode.
    pub failed_hardware: Option<Hardware>,
    pub problems: Vec<String>,
}

/// Erases the stored hardware when BOOT is pressed within `PRESS_WINDOW` and held for `HOLD`.
/// Held during reset, BOOT selects the download mode instead, so the press comes after boot.
pub fn erase_on_boot_hold(settings: &mut Settings) -> anyhow::Result<()> {
    let boot_pin = match env!("MCU") {
        "esp32c6" => 9,
        _ => 0,
    };
    let button = PinDriver::input(unsafe { AnyIOPin::steal(boot_pin) }, Pull::Up)?;
    log::info!("Hold BOOT for 5 s now to erase the hardware configuration");
    let window = Instant::now();
    while button.is_high() {
        if window.elapsed() >= PRESS_WINDOW {
            return Ok(());
        }
        FreeRtos::delay_ms(10);
    }
    let pressed = Instant::now();
    while button.is_low() {
        if pressed.elapsed() >= HOLD {
            settings.remove_hardware()?;
            settings.set_failed_boots(0)?;
            log::warn!("Hardware configuration erased");
            return Ok(());
        }
        FreeRtos::delay_ms(10);
    }
    Ok(())
}

/// The stored hardware when it is valid and the safe mode is off, otherwise `factory`.
pub fn select(settings: &mut Settings, factory: Hardware) -> anyhow::Result<Selection> {
    let failed_boots = settings.failed_boots()?;
    settings.set_failed_boots(failed_boots.saturating_add(1))?;
    let mut selection = Selection {
        hardware: factory,
        failed_hardware: None,
        problems: Vec::new(),
    };
    let Some(json) = settings.hardware()? else {
        return Ok(selection);
    };
    let stored = match Hardware::from_json(&json) {
        Ok(stored) => stored,
        Err(error) => {
            selection.problems.push(format!(
                "stored hardware ignored, not a configuration: {error}"
            ));
            return Ok(selection);
        }
    };
    if let Some(problem) = stored.validate(env!("MCU")).first() {
        selection
            .problems
            .push(format!("stored hardware ignored: {problem}"));
    } else if failed_boots >= SAFE_MODE_BOOTS {
        selection.problems.push(format!(
            "safe mode: stored hardware ignored after {failed_boots} boots without the broker"
        ));
        selection.failed_hardware = Some(stored);
    } else {
        selection.hardware = stored;
    }
    Ok(selection)
}
