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

/// What is wired to this board besides the onboard LED and BOOT button.
#[derive(Default)]
pub struct Wiring {
    pub climate: bool,
    pub external_led: bool,
    pub buzzer: bool,
    pub ir: bool,
    pub potentiometer: bool,
}

impl Wiring {
    pub fn parse(list: &str) -> anyhow::Result<Self> {
        let mut peripherals = Self::default();
        for name in list.split(',').map(str::trim).filter(|name| !name.is_empty()) {
            let flag = match name {
                "climate" => &mut peripherals.climate,
                "external_led" => &mut peripherals.external_led,
                "buzzer" => &mut peripherals.buzzer,
                "ir" => &mut peripherals.ir,
                "potentiometer" => &mut peripherals.potentiometer,
                _ => anyhow::bail!("unknown peripheral \"{name}\" in cfg.toml"),
            };
            *flag = true;
        }
        Ok(peripherals)
    }
}
