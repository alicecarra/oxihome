use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::PinDriver;
use esp_idf_svc::hal::peripherals::Peripherals;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let mut led = PinDriver::output(peripherals.pins.gpio2)?;

    log::info!("Oxihome firmware started");

    loop {
        led.toggle()?;
        log::info!("LED is {}", if led.is_set_high() { "on" } else { "off" });
        FreeRtos::delay_ms(1000);
    }
}
