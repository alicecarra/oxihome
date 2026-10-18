use anyhow::anyhow;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::WifiModemPeripheral;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{BlockingWifi, ClientConfiguration, Configuration, EspWifi};

pub type Wifi = BlockingWifi<EspWifi<'static>>;

pub fn connect(
    modem: impl WifiModemPeripheral + 'static,
    event_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    password: &str,
    power_save: i32,
) -> anyhow::Result<Wifi> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, event_loop.clone(), Some(nvs))?,
        event_loop,
    )?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi SSID is too long"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow!("Wi-Fi password is too long"))?,
        ..Default::default()
    }))?;

    wifi.start()?;
    set_power_save(power_save);
    match wifi.connect().and_then(|()| wifi.wait_netif_up()) {
        Ok(()) => {
            let ip = wifi.wifi().sta_netif().get_ip_info()?.ip;
            log::info!("Wi-Fi connected to {ssid}, IP {ip}");
        }
        Err(error) => log::warn!("Wi-Fi not connected to {ssid} ({error}), retrying"),
    }
    Ok(wifi)
}

/// Asks for a reconnection without waiting for it, so the loop keeps running without a network.
pub fn ensure_connected(wifi: &mut Wifi) {
    match wifi.is_connected() {
        Ok(true) => {}
        Ok(false) => {
            log::warn!("Wi-Fi lost, reconnecting");
            if let Err(error) = wifi.wifi_mut().connect() {
                log::warn!("Wi-Fi reconnection failed: {error}");
            }
        }
        Err(error) => log::warn!("Wi-Fi state unknown: {error}"),
    }
}

/// The `WifiPowerSave` parameter: 0 off, 1 light, 2 maximum. A failure only leaves a warning.
pub fn set_power_save(mode: i32) {
    use esp_idf_svc::sys::*;
    let mode = match mode {
        0 => wifi_ps_type_t_WIFI_PS_NONE,
        1 => wifi_ps_type_t_WIFI_PS_MIN_MODEM,
        _ => wifi_ps_type_t_WIFI_PS_MAX_MODEM,
    };
    if let Err(error) = esp!(unsafe { esp_wifi_set_ps(mode) }) {
        log::warn!("Wi-Fi power save not changed: {error}");
    }
}

pub fn rssi() -> Option<i8> {
    let mut record = esp_idf_svc::sys::wifi_ap_record_t::default();
    let result = unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut record) };
    (result == esp_idf_svc::sys::ESP_OK).then_some(record.rssi)
}

/// `None` while the station has no address.
pub fn ip() -> Option<std::net::Ipv4Addr> {
    use esp_idf_svc::sys::*;
    let netif = unsafe { esp_netif_get_handle_from_ifkey(c"WIFI_STA_DEF".as_ptr()) };
    if netif.is_null() {
        return None;
    }
    let mut info = esp_netif_ip_info_t::default();
    esp!(unsafe { esp_netif_get_ip_info(netif, &mut info) }).ok()?;
    // Stored in network byte order.
    let ip = std::net::Ipv4Addr::from(info.ip.addr.to_le_bytes());
    (!ip.is_unspecified()).then_some(ip)
}
