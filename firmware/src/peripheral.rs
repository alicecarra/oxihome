use std::time::{Duration, Instant};

use esp_idf_svc::hal::adc::oneshot::AdcDriver;
use esp_idf_svc::hal::adc::{ADC1, ADCU1};
use esp_idf_svc::hal::gpio::{AnyIOPin, Output, PinDriver, Pull};
use esp_idf_svc::hal::i2c::{I2C0, I2cConfig, I2cDriver};
use esp_idf_svc::hal::uart::UART1;
use esp_idf_svc::hal::units::FromValueType;
use oxihome_protocol::hardware::{Driver, Hardware, HardwareEntity};

use crate::aht20_bmp280::Aht20Bmp280;
use crate::button::Button;
use crate::buzzer::Buzzer;
use crate::climate::Climate;
use crate::gps::{Gps, Reading};
use crate::ir::IrReceiver;
use crate::ir_transmitter::IrTransmitter;
use crate::potentiometer::Potentiometer;

pub const POTENTIOMETER_READ_INTERVAL: Duration = Duration::from_millis(100);

pub struct Entity {
    pub id: u8,
    pub peripheral: Peripheral,
}

pub enum Peripheral {
    Output {
        pin: PinDriver<'static, Output>,
        inverted: bool,
    },
    Button {
        button: Button,
        toggles: Option<u8>,
    },
    Climate {
        climate: Climate,
        read_interval: Duration,
        next_read: Instant,
        failures: u8,
    },
    Buzzer(Buzzer),
    IrReceiver(IrReceiver),
    IrTransmitter(IrTransmitter),
    Potentiometer {
        potentiometer: Potentiometer,
        last_read: Instant,
    },
    Gps {
        gps: Gps,
        report_interval: Duration,
        next_report: Instant,
        reported: Option<Reading>,
    },
}

/// Mounts the entities of an already validated `hardware`. An entity that fails is left out, and
/// its error comes back as a message.
pub fn mount(
    hardware: &Hardware,
    adc: ADC1<'static>,
    i2c: I2C0<'static>,
    uart: UART1<'static>,
) -> (Vec<Entity>, Vec<String>) {
    let mut adc = Some(adc);
    let mut i2c = Some(i2c);
    let mut uart = Some(uart);
    let mut shared_adc: Option<&'static AdcDriver<'static, ADCU1>> = None;
    let mut entities = Vec::new();
    let mut failures = Vec::new();
    for entity in &hardware.entities {
        match mount_one(entity, &mut adc, &mut shared_adc, &mut i2c, &mut uart) {
            Ok(peripheral) => entities.push(Entity {
                id: entity.id,
                peripheral,
            }),
            Err(error) => {
                let failure = format!(
                    "entity {} ({} on pin {}): {error:#}",
                    entity.id,
                    entity.driver.name(),
                    entity.pin
                );
                log::error!("{failure}");
                failures.push(failure);
            }
        }
    }
    (entities, failures)
}

fn mount_one(
    entity: &HardwareEntity,
    adc: &mut Option<ADC1<'static>>,
    shared_adc: &mut Option<&'static AdcDriver<'static, ADCU1>>,
    i2c: &mut Option<I2C0<'static>>,
    uart: &mut Option<UART1<'static>>,
) -> anyhow::Result<Peripheral> {
    // Validation keeps pins unique and away from the flash, so no other driver holds this one.
    let pin = || unsafe { AnyIOPin::steal(entity.pin) };
    Ok(match &entity.driver {
        Driver::Output { inverted } => {
            let mut pin = PinDriver::output(pin())?;
            if *inverted {
                pin.set_high()?;
            }
            Peripheral::Output {
                pin,
                inverted: *inverted,
            }
        }
        Driver::Button { pull_up, toggles } => {
            let pull = if *pull_up { Pull::Up } else { Pull::Floating };
            Peripheral::Button {
                button: Button::new(PinDriver::input(pin(), pull)?),
                toggles: *toggles,
            }
        }
        Driver::Dht11 {
            read_interval_seconds,
        } => Peripheral::Climate {
            climate: Climate::dht11(PinDriver::input_output_od(pin(), Pull::Floating)?)?,
            read_interval: Duration::from_secs((*read_interval_seconds).into()),
            next_read: Instant::now(),
            failures: 0,
        },
        Driver::Aht20Bmp280 {
            clock_pin,
            read_interval_seconds,
        } => {
            let unit = i2c
                .take()
                .ok_or_else(|| anyhow::anyhow!("the I2C bus is taken by another entity"))?;
            let clock = unsafe { AnyIOPin::steal(*clock_pin) };
            // The default is 1 MHz; the AHT20 takes up to 400 kHz.
            let config = I2cConfig::new().baudrate(100.kHz().into());
            let driver = I2cDriver::new(unit, pin(), clock, &config)?;
            Peripheral::Climate {
                climate: Climate::aht20_bmp280(Aht20Bmp280::new(driver)?),
                read_interval: Duration::from_secs((*read_interval_seconds).into()),
                next_read: Instant::now(),
                failures: 0,
            }
        }
        Driver::Buzzer => Peripheral::Buzzer(Buzzer::new(PinDriver::output(pin())?)?),
        Driver::IrReceiver => Peripheral::IrReceiver(IrReceiver::new(pin())?),
        Driver::IrTransmitter => Peripheral::IrTransmitter(IrTransmitter::new(pin())?),
        Driver::Potentiometer { minimum_change } => {
            let adc = match (*shared_adc, adc.take()) {
                (Some(driver), _) => driver,
                (None, Some(unit)) => {
                    let driver: &'static _ = Box::leak(Box::new(AdcDriver::new(unit)?));
                    *shared_adc = Some(driver);
                    driver
                }
                (None, None) => anyhow::bail!("ADC1 failed to start"),
            };
            Peripheral::Potentiometer {
                potentiometer: Potentiometer::new(adc, entity.pin, *minimum_change)?,
                last_read: Instant::now(),
            }
        }
        Driver::Gps {
            report_interval_seconds,
        } => {
            let unit = uart
                .take()
                .ok_or_else(|| anyhow::anyhow!("UART1 is taken by another entity"))?;
            Peripheral::Gps {
                gps: Gps::new(unit, pin())?,
                report_interval: Duration::from_secs((*report_interval_seconds).into()),
                next_report: Instant::now(),
                reported: None,
            }
        }
    })
}
