use esp_idf_svc::hal::adc::ADCU1;
use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::config::AdcChannelConfig;
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::gpio::{Gpio32, Gpio33, Gpio34, Gpio35, Gpio36, Gpio37, Gpio38, Gpio39};
use esp_idf_svc::sys::EspError;

const SAMPLES: u32 = 16;
const RAW_MAXIMUM: u32 = 4095;

type Reader = Box<dyn FnMut() -> Result<u16, EspError>>;

pub struct Potentiometer {
    read_raw: Reader,
    /// Smaller changes, in percent, are ADC noise and not reported.
    minimum_change: u8,
    reported: Option<u8>,
}

impl Potentiometer {
    pub fn new(
        adc: &'static AdcDriver<'static, ADCU1>,
        pin: u8,
        minimum_change: u8,
    ) -> anyhow::Result<Self> {
        let config = AdcChannelConfig {
            attenuation: DB_12,
            ..Default::default()
        };
        // Each ADC pin has its own channel type, so the pin number picks the type here.
        macro_rules! reader {
            ($($number:literal => $gpio:ident),*) => {
                match pin {
                    $($number => {
                        let mut channel =
                            AdcChannelDriver::new(adc, unsafe { $gpio::steal() }, &config)?;
                        Box::new(move || channel.read_raw()) as Reader
                    })*
                    _ => anyhow::bail!("pin {pin} is not an ADC1 pin"),
                }
            };
        }
        let read_raw = reader!(
            32 => Gpio32, 33 => Gpio33, 34 => Gpio34, 35 => Gpio35,
            36 => Gpio36, 37 => Gpio37, 38 => Gpio38, 39 => Gpio39
        );
        Ok(Self {
            read_raw,
            minimum_change,
            reported: None,
        })
    }

    /// `true` when the position moved enough to be reported.
    pub fn poll(&mut self) -> anyhow::Result<bool> {
        let mut sum = 0;
        for _ in 0..SAMPLES {
            sum += u32::from((self.read_raw)()?);
        }
        let percent = (sum / SAMPLES * 100 / RAW_MAXIMUM) as u8;
        let changed = match self.reported {
            None => true,
            // The ends always get through so 0 and 100 can be reached.
            Some(reported) => {
                percent.abs_diff(reported) >= self.minimum_change
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
