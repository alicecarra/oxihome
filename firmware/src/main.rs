mod button;
mod buzzer;
mod climate;
mod config;
mod ir;
mod ir_transmitter;
mod node;
mod ota;
mod peripheral;
mod potentiometer;
mod recovery;
mod settings;
mod transport;
mod wifi;

use std::time::Instant;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use oxihome_protocol::management::{HEARTBEAT_INTERVAL, WIFI_CHECK_INTERVAL};

use config::CONFIG;
use node::{Board, Node};
use ota::Ota;
use settings::Settings;
use transport::MqttTransport;

const POLL_INTERVAL_MS: u32 = 10;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let started = Instant::now();

    let peripherals = Peripherals::take()?;
    let factory = config::default_hardware(CONFIG.peripherals)?;
    if let Some(problem) = factory.validate(env!("MCU")).first() {
        anyhow::bail!("hardware in cfg.toml: {problem}");
    }
    let nvs = EspDefaultNvsPartition::take()?;
    let mut settings = Settings::load(nvs.clone())?;
    recovery::erase_on_boot_hold(&mut settings)?;
    let selection = recovery::select(&mut settings, factory)?;
    let (entities, failures) = peripheral::mount(&selection.hardware, peripherals.adc1);
    let mut problems = selection.problems;
    for problem in &problems {
        log::warn!("{problem}");
    }
    problems.extend(failures);

    let device_id = device_id()?;
    log::info!("Oxihome node {device_id} started");

    let mut wifi = wifi::connect(
        peripherals.modem,
        EspSystemEventLoop::take()?,
        nvs,
        CONFIG.wifi_ssid,
        CONFIG.wifi_password,
    )?;
    let transport = MqttTransport::new(CONFIG.mqtt_url, &device_id, CONFIG.mqtt_password)?;
    let board = Board {
        settings,
        hardware: selection.hardware,
        failed_hardware: selection.failed_hardware,
        problems,
        entities,
        pending_update: None,
        pending_restart: false,
    };
    let mut node = Node::new(device_id, transport, board, Ota::new()?);

    let mut last_wifi_check = Instant::now();
    let mut last_heartbeat = Instant::now();
    loop {
        if node.update_unverified() && started.elapsed() >= ota::VALIDATION_TIMEOUT {
            log::error!("New image did not reach the broker; restarting to roll back");
            esp_idf_svc::hal::reset::restart();
        }
        if last_wifi_check.elapsed() >= node.settings().interval(WIFI_CHECK_INTERVAL) {
            wifi::ensure_connected(&mut wifi)?;
            last_wifi_check = Instant::now();
        }
        if last_heartbeat.elapsed() >= node.settings().interval(HEARTBEAT_INTERVAL) {
            node.send_heartbeat(uptime_seconds(), wifi::rssi())?;
            last_heartbeat = Instant::now();
        }
        node.poll()?;
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
