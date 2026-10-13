use oxihome_protocol::hardware::{Driver, Hardware, HardwareEntity};

#[toml_cfg::toml_config]
pub struct Config {
    #[default("")]
    wifi_ssid: &'static str,
    #[default("")]
    wifi_password: &'static str,
    #[default("")]
    mqtt_url: &'static str,
    /// The board logs in with its device id as the user name; empty connects anonymously.
    #[default("")]
    mqtt_password: &'static str,
    #[default("climate, external_led, buzzer, ir, potentiometer")]
    peripherals: &'static str,
}

/// The factory hardware: the onboard LED and BOOT button, plus `peripherals` on today's pins.
pub fn default_hardware(peripherals: &str) -> anyhow::Result<Hardware> {
    let mut entities = vec![
        entity(1, 2, Driver::Output { inverted: false }),
        entity(
            2,
            0,
            Driver::Button {
                pull_up: true,
                toggles: Some(1),
            },
        ),
    ];
    for name in peripherals
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        entities.push(match name {
            "climate" => entity(
                3,
                4,
                Driver::Dht11 {
                    read_interval_seconds: 30,
                },
            ),
            "external_led" => entity(4, 18, Driver::Output { inverted: false }),
            "buzzer" => entity(5, 19, Driver::Buzzer),
            "ir" => entity(6, 23, Driver::IrReceiver),
            "potentiometer" => entity(7, 34, Driver::Potentiometer { minimum_change: 2 }),
            _ => anyhow::bail!("unknown peripheral \"{name}\" in cfg.toml"),
        });
    }
    entities.sort_by_key(|entity| entity.id);
    Ok(Hardware { entities })
}

fn entity(id: u8, pin: u8, driver: Driver) -> HardwareEntity {
    HardwareEntity { id, pin, driver }
}
