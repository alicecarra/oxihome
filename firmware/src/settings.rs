use std::time::Duration;

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use oxihome_protocol::management::{PARAMETERS, Parameter};

const NAMESPACE: &str = "oxihome";

/// Operation parameters set by the hub, persisted in NVS.
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

    pub fn interval(&self, parameter: Parameter) -> Duration {
        Duration::from_secs(self.get(parameter).unsigned_abs().into())
    }
}

fn key(parameter: Parameter) -> String {
    format!("parameter_{}", u8::from(parameter.channel))
}
