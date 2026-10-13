use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::config::AdcChannelConfig;
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::adc::{ADC1, ADCCH6, ADCU1};
use esp_idf_svc::hal::gpio::Gpio34;

const SAMPLES: u32 = 16;
/// ADC noise moves the reading by about one point; smaller changes are not reported.
const MINIMUM_CHANGE: u8 = 2;
const RAW_MAXIMUM: u32 = 4095;

pub struct Potentiometer {
    channel: AdcChannelDriver<'static, ADCCH6<ADCU1>, AdcDriver<'static, ADCU1>>,
    reported: Option<u8>,
}

impl Potentiometer {
    pub fn new(adc: ADC1<'static>, pin: Gpio34<'static>) -> anyhow::Result<Self> {
        let config = AdcChannelConfig {
            attenuation: DB_12,
            ..Default::default()
        };
        let channel = AdcChannelDriver::new(AdcDriver::new(adc)?, pin, &config)?;
        Ok(Self {
            channel,
            reported: None,
        })
    }

    /// `true` when the position moved enough to be reported.
    pub fn poll(&mut self) -> anyhow::Result<bool> {
        let mut sum = 0;
        for _ in 0..SAMPLES {
            sum += u32::from(self.channel.read_raw()?);
        }
        let percent = (sum / SAMPLES * 100 / RAW_MAXIMUM) as u8;
        let changed = match self.reported {
            None => true,
            // The ends always get through so 0 and 100 can be reached.
            Some(reported) => {
                percent.abs_diff(reported) >= MINIMUM_CHANGE
                    || (percent != reported && (percent == 0 || percent == 100))
            }
        };
        if changed {
            self.reported = Some(percent);
        }
        Ok(changed)
    }

    pub fn percent(&self) -> Option<u8> {
        self.reported
    }
}
