mod button;
mod node;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{AnyIOPin, PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::uart::{config::Config, UartDriver};
use esp_idf_svc::hal::units::Hertz;

use button::Button;
use node::Node;

const POLL_INTERVAL_MS: u32 = 10;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    let uart = UartDriver::new(
        peripherals.uart0,
        pins.gpio1,
        pins.gpio3,
        Option::<AnyIOPin>::None,
        Option::<AnyIOPin>::None,
        &Config::default().baudrate(Hertz(115_200)),
    )?;
    let led = PinDriver::output(pins.gpio2)?;
    let mut button = Button::new(PinDriver::input(pins.gpio0, Pull::Up)?);

    let device_id = device_id()?;
    log::info!("Oxihome node {device_id} started");

    let mut node = Node::new(device_id, uart, led);
    node.announce()?;

    loop {
        node.poll_serial()?;
        if let Some(event) = button.poll() {
            node.handle_button(event)?;
        }
        FreeRtos::delay_ms(POLL_INTERVAL_MS);
    }
}

/// `esp32-<factory MAC>`, stable across reboots.
fn device_id() -> anyhow::Result<String> {
    let mut mac = [0u8; 6];
    esp_idf_svc::sys::esp!(unsafe {
        esp_idf_svc::sys::esp_efuse_mac_get_default(mac.as_mut_ptr())
    })?;
    let hex: String = mac.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("esp32-{hex}"))
}
