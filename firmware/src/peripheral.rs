use std::time::{Duration, Instant};

use esp_idf_svc::hal::adc::oneshot::AdcDriver;
use esp_idf_svc::hal::adc::{ADC1, ADCU1};
use esp_idf_svc::hal::gpio::{AnyIOPin, Output, PinDriver, Pull};
use oxihome_protocol::hardware::{Driver, Hardware, HardwareEntity};

use crate::button::Button;
use crate::buzzer::Buzzer;
use crate::climate::Climate;
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
        last_read: Option<Instant>,
    },
    Buzzer(Buzzer),
    IrReceiver(IrReceiver),
    IrTransmitter(IrTransmitter),
    Potentiometer {
        potentiometer: Potentiometer,
        last_read: Instant,
    },
}

/// Mounts the entities of an already validated `hardware`. An entity that fails is left out, and
/// its error comes back as a message.
pub fn mount(hardware: &Hardware, adc: ADC1<'static>) -> (Vec<Entity>, Vec<String>) {
    let mut adc = Some(adc);
    let mut shared_adc: Option<&'static AdcDriver<'static, ADCU1>> = None;
    let mut entities = Vec::new();
    let mut failures = Vec::new();
    for entity in &hardware.entities {
        match mount_one(entity, &mut adc, &mut shared_adc) {
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
            climate: Climate::new(PinDriver::input_output_od(pin(), Pull::Floating)?)?,
            read_interval: Duration::from_secs((*read_interval_seconds).into()),
            last_read: None,
        },
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
    })
}
