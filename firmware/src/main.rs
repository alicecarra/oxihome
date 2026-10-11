mod button;
mod config;
mod node;
mod settings;
mod transport;
mod wifi;

use std::time::Instant;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use oxihome_protocol::management::{HEARTBEAT_INTERVAL, WIFI_CHECK_INTERVAL};

use button::Button;
use config::CONFIG;
use node::Node;
use settings::Settings;
use transport::MqttTransport;

const POLL_INTERVAL_MS: u32 = 10;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    let led = PinDriver::output(pins.gpio2)?;
    let mut button = Button::new(PinDriver::input(pins.gpio0, Pull::Up)?);

    let device_id = device_id()?;
    log::info!("Oxihome node {device_id} started");

    let nvs = EspDefaultNvsPartition::take()?;
    let settings = Settings::load(nvs.clone())?;
    let mut wifi = wifi::connect(
        peripherals.modem,
        EspSystemEventLoop::take()?,
        nvs,
        CONFIG.wifi_ssid,
        CONFIG.wifi_password,
    )?;
    let transport = MqttTransport::new(CONFIG.mqtt_url, &device_id)?;
    let mut node = Node::new(device_id, transport, led, settings);

    let mut last_wifi_check = Instant::now();
    let mut last_heartbeat = Instant::now();
    loop {
        if last_wifi_check.elapsed() >= node.settings().interval(WIFI_CHECK_INTERVAL) {
            wifi::ensure_connected(&mut wifi)?;
            last_wifi_check = Instant::now();
        }
        if last_heartbeat.elapsed() >= node.settings().interval(HEARTBEAT_INTERVAL) {
            node.send_heartbeat(uptime_seconds(), wifi::rssi())?;
            last_heartbeat = Instant::now();
        }
        node.poll()?;
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

fn uptime_seconds() -> u32 {
    let microseconds = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    (microseconds / 1_000_000) as u32
}
