use std::time::Duration;

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use oxihome_protocol::hardware::HARDWARE_MAX_LENGTH;
use oxihome_protocol::management::{PARAMETERS, Parameter};

const NAMESPACE: &str = "oxihome";
const HARDWARE_KEY: &str = "hardware";
const FAILED_BOOTS_KEY: &str = "failed_boots";

/// What the hub sets on the board, persisted in NVS: operation parameters and the hardware.
pub struct Settings {
    nvs: EspNvs<NvsDefault>,
    values: Vec<(Parameter, i32)>,
}

impl Settings {
    /// Missing or out-of-range stored values fall back to the default.
    pub fn load(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        let nvs = EspNvs::new(partition, NAMESPACE, true)?;
        let values = PARAMETERS
            .into_iter()
            .map(|parameter| {
                let value = match nvs.get_i32(&key(parameter))? {
                    Some(stored) if parameter.accepts(stored) => stored,
                    _ => parameter.default,
                };
                log::info!("{:?} = {value}", parameter.channel);
                Ok((parameter, value))
            })
            .collect::<anyhow::Result<_>>()?;
        Ok(Self { nvs, values })
    }

    pub fn get(&self, parameter: Parameter) -> i32 {
        self.values
            .iter()
            .find(|(known, _)| *known == parameter)
            .map_or(parameter.default, |(_, value)| *value)
    }

    /// The caller validates the value with `Parameter::accepts`.
    pub fn set(&mut self, parameter: Parameter, value: i32) -> anyhow::Result<()> {
        self.nvs.set_i32(&key(parameter), value)?;
        if let Some((_, current)) = self
            .values
            .iter_mut()
            .find(|(known, _)| *known == parameter)
        {
            *current = value;
        }
        Ok(())
    }

    /// The stored hardware JSON, unchecked.
    pub fn hardware(&self) -> anyhow::Result<Option<String>> {
        let mut buffer = vec![0; usize::from(HARDWARE_MAX_LENGTH) + 1];
        Ok(self
            .nvs
            .get_str(HARDWARE_KEY, &mut buffer)?
            .map(str::to_string))
    }

    pub fn set_hardware(&mut self, json: &str) -> anyhow::Result<()> {
        Ok(self.nvs.set_str(HARDWARE_KEY, json)?)
    }

    pub fn remove_hardware(&mut self) -> anyhow::Result<()> {
        self.nvs.remove(HARDWARE_KEY)?;
        Ok(())
    }

    /// Boots since the board last reached the broker.
    pub fn failed_boots(&self) -> anyhow::Result<u8> {
        Ok(self.nvs.get_u8(FAILED_BOOTS_KEY)?.unwrap_or(0))
    }

    pub fn set_failed_boots(&mut self, boots: u8) -> anyhow::Result<()> {
        Ok(self.nvs.set_u8(FAILED_BOOTS_KEY, boots)?)
    }

    pub fn interval(&self, parameter: Parameter) -> Duration {
        Duration::from_secs(self.get(parameter).unsigned_abs().into())
    }
}

fn key(parameter: Parameter) -> String {
    format!("parameter_{}", u8::from(parameter.channel))
}
