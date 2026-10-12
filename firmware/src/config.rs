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
}
