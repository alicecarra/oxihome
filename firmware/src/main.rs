mod adxl345;
mod aht20_bmp280;
mod button;
mod buzzer;
mod climate;
mod contact;
mod config;
mod espnow;
mod gateway;
mod gps;
mod ir;
mod ir_transmitter;
mod leaf;
mod nmea;
mod node;
mod ota;
mod peripheral;
mod potentiometer;
mod recovery;
mod relay;
mod settings;
mod storage;
mod transport;
mod wifi;

use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::task::watchdog::{TWDTConfig, TWDTDriver};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use oxihome_protocol::management::{HEARTBEAT_INTERVAL, WIFI_CHECK_INTERVAL, WIFI_POWER_SAVE};

use config::CONFIG;
use gateway::Gateway;
use leaf::RelayTransport;
use node::{Board, Node};
use ota::Ota;
use settings::Settings;
use transport::{MqttTransport, Transport};

const POLL_INTERVAL_MS: u32 = 10;
/// Longer than the blocking parts of the loop, the over-the-air download included.
const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(120);
/// Before restarting after a startup error, so a lasting error does not wear out the NVS.
const RESTART_DELAY_MS: u32 = 10_000;

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    if let Err(error) = run() {
        log::error!("{error:#}; restarting");
        FreeRtos::delay_ms(RESTART_DELAY_MS);
        esp_idf_svc::hal::reset::restart();
    }
}

fn run() -> anyhow::Result<()> {
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
    let (entities, failures) = peripheral::mount(
        &selection.hardware,
        peripherals.adc1,
        peripherals.i2c0,
        peripherals.uart1,
    );
    let mut problems = selection.problems;
    for problem in &problems {
        log::warn!("{problem}");
    }
    problems.extend(failures);

    let device_id = device_id()?;
    log::info!("Oxihome node {device_id} started");
    match storage::mount() {
        Ok(true) => log::info!("Storage partition mounted at /storage"),
        Ok(false) => {}
        Err(error) => log::warn!("Storage partition: {error:#}"),
    }
    let restart_reason = restart_reason();
    if let Some(reason) = &restart_reason {
        log::warn!("{reason}");
    }

    let board = Board {
        settings,
        hardware: selection.hardware,
        failed_hardware: selection.failed_hardware,
        problems,
        entities,
        pending_update: None,
        pending_restart: false,
        restart_reason,
    };
    let mut watchdog = TWDTDriver::new(
        peripherals.twdt,
        &TWDTConfig {
            duration: WATCHDOG_TIMEOUT,
            panic_on_trigger: true,
            ..Default::default()
        },
    )?;

    if !CONFIG.espnow_gateway.is_empty() {
        let wifi = wifi::start(
            peripherals.modem,
            EspSystemEventLoop::take()?,
            nvs,
            CONFIG.wifi_ssid,
            CONFIG.wifi_password,
        )?;
        let transport = RelayTransport::new(wifi, CONFIG.espnow_gateway, CONFIG.espnow_key)?;
        let node = Node::new(device_id, transport, board, Ota::new()?);
        return serve(node, None, &mut watchdog, started);
    }

    let power_save = board.settings.get(WIFI_POWER_SAVE);
    let wifi = wifi::connect(
        peripherals.modem,
        EspSystemEventLoop::take()?,
        nvs,
        CONFIG.wifi_ssid,
        CONFIG.wifi_password,
        power_save,
    )?;
    let gateway = if CONFIG.espnow_leaves.is_empty() {
        None
    } else {
        Some(Gateway::new(CONFIG.espnow_leaves, CONFIG.espnow_secret)?)
    };
    let transport = MqttTransport::new(
        CONFIG.mqtt_url,
        &device_id,
        CONFIG.mqtt_password,
        gateway,
    )?;
    let node = Node::new(device_id, transport, board, Ota::new()?);
    serve(node, Some(wifi), &mut watchdog, started)
}

/// The main loop. `wifi` is the connected station; a leaf has none (its transport keeps it).
fn serve<T: Transport>(
    mut node: Node<T>,
    mut wifi: Option<wifi::Wifi>,
    watchdog: &mut TWDTDriver<'_>,
    started: Instant,
) -> anyhow::Result<()> {
    let mut subscription = watchdog.watch_current_task()?;
    let mut last_wifi_check = Instant::now();
    let mut last_heartbeat = Instant::now();
    loop {
        subscription.feed()?;
        if node.update_unverified() && started.elapsed() >= ota::VALIDATION_TIMEOUT {
            log::error!("New image did not reach the broker; restarting to roll back");
            esp_idf_svc::hal::reset::restart();
        }
        if let Some(wifi) = &mut wifi {
            if last_wifi_check.elapsed() >= node.settings().interval(WIFI_CHECK_INTERVAL) {
                wifi::ensure_connected(wifi);
                last_wifi_check = Instant::now();
            }
        }
        if last_heartbeat.elapsed() >= node.settings().interval(HEARTBEAT_INTERVAL) {
            node.send_heartbeat(uptime_seconds(), wifi::rssi());
            last_heartbeat = Instant::now();
        }
        node.poll();
        FreeRtos::delay_ms(POLL_INTERVAL_MS);
    }
}

/// `esp32-<factory MAC>`, stable across reboots. The Wi-Fi station MAC, because the ESP32-C6
/// factory MAC in eFuse is 8 bytes (EUI-64).
fn device_id() -> anyhow::Result<String> {
    let mut mac = [0u8; 6];
    esp_idf_svc::sys::esp!(unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_WIFI_STA,
        )
    })?;
    let hex: String = mac.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("esp32-{hex}"))
}

/// Why the board restarted, when it was not a power-on or a restart of its own.
fn restart_reason() -> Option<String> {
    use esp_idf_svc::sys::*;
    #[allow(non_upper_case_globals)]
    let reason = match unsafe { esp_reset_reason() } {
        esp_reset_reason_t_ESP_RST_TASK_WDT => "the task watchdog (the main loop stopped)",
        esp_reset_reason_t_ESP_RST_INT_WDT | esp_reset_reason_t_ESP_RST_WDT => "a watchdog",
        esp_reset_reason_t_ESP_RST_PANIC => "a panic",
        esp_reset_reason_t_ESP_RST_BROWNOUT => "a brownout (the supply voltage dropped)",
        esp_reset_reason_t_ESP_RST_PWR_GLITCH => "a power glitch",
        esp_reset_reason_t_ESP_RST_CPU_LOCKUP => "a CPU lockup",
        esp_reset_reason_t_ESP_RST_UNKNOWN => "an unknown cause",
        _ => return None,
    };
    Some(format!("restarted by {reason}"))
}

fn uptime_seconds() -> u32 {
    let microseconds = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    (microseconds / 1_000_000) as u32
}
