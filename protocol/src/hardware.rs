//! What is wired to a board: carried as JSON in the `Hardware` channel of the management entity.

use serde::{Deserialize, Serialize};

use crate::management;

pub const HARDWARE_MAX_LENGTH: u16 = 2048;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hardware {
    pub entities: Vec<HardwareEntity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareEntity {
    pub id: u8,
    pub pin: u8,
    #[serde(flatten)]
    pub driver: Driver,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "driver", rename_all = "snake_case")]
pub enum Driver {
    /// LED or relay.
    Output {
        #[serde(default)]
        inverted: bool,
    },
    Button {
        #[serde(default = "enabled")]
        pull_up: bool,
        /// Output entity that a press toggles on the board itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        toggles: Option<u8>,
    },
    Dht11 {
        #[serde(default = "default_read_interval")]
        read_interval_seconds: u16,
    },
    Buzzer,
    IrReceiver,
    /// IR LED behind a transistor.
    IrTransmitter,
    Potentiometer {
        /// Smaller changes, in percent, are ADC noise and not reported.
        #[serde(default = "default_minimum_change")]
        minimum_change: u8,
    },
}

fn enabled() -> bool {
    true
}

fn default_read_interval() -> u16 {
    30
}

fn default_minimum_change() -> u8 {
    2
}

const DHT11_READ_INTERVAL_RANGE: std::ops::RangeInclusive<u16> = 2..=3600;
const MINIMUM_CHANGE_RANGE: std::ops::RangeInclusive<u8> = 1..=50;

impl Driver {
    pub fn name(&self) -> &'static str {
        match self {
            Driver::Output { .. } => "output",
            Driver::Button { .. } => "button",
            Driver::Dht11 { .. } => "dht11",
            Driver::Buzzer => "buzzer",
            Driver::IrReceiver => "ir_receiver",
            Driver::IrTransmitter => "ir_transmitter",
            Driver::Potentiometer { .. } => "potentiometer",
        }
    }

    fn drives_pin(&self) -> bool {
        matches!(
            self,
            Driver::Output { .. } | Driver::Dht11 { .. } | Driver::Buzzer | Driver::IrTransmitter
        )
    }
}

/// The chips whose pins `Hardware::validate` knows, by the `target` of their `Build`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Esp32,
    Esp32C6,
}

impl Target {
    pub fn from_build(target: &str) -> Option<Self> {
        match target {
            "esp32" => Some(Self::Esp32),
            "esp32c6" => Some(Self::Esp32C6),
            _ => None,
        }
    }

    fn pin_problem(self, pin: u8) -> Option<PinProblem> {
        let reserved = match self {
            // 1 and 3: UART0, used by the log.
            Target::Esp32 => matches!(pin, 1 | 3 | 6..=11),
            // 12 and 13: USB; 16 and 17: UART0; 24 to 30: flash.
            Target::Esp32C6 => matches!(pin, 12 | 13 | 16 | 17 | 24..=30),
        };
        let exists = match self {
            Target::Esp32 => matches!(pin, 0..=19 | 21..=23 | 25..=27 | 32..=39),
            Target::Esp32C6 => pin <= 30,
        };
        if !exists {
            Some(PinProblem::Missing)
        } else if reserved {
            Some(PinProblem::Reserved)
        } else {
            None
        }
    }

    fn input_only(self, pin: u8) -> bool {
        self == Target::Esp32 && (34..=39).contains(&pin)
    }

    /// Wi-Fi takes ADC2, so only ADC1 is usable.
    fn analog(self, pin: u8) -> bool {
        match self {
            Target::Esp32 => (32..=39).contains(&pin),
            Target::Esp32C6 => pin <= 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinProblem {
    Missing,
    Reserved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HardwareProblem {
    TooLong { length: usize },
    UnknownTarget(String),
    ManagementEntity,
    RepeatedEntity(u8),
    RepeatedPin { pin: u8, entities: (u8, u8) },
    MissingPin { entity: u8, pin: u8 },
    ReservedPin { entity: u8, pin: u8 },
    InputOnlyPin { entity: u8, pin: u8 },
    NoPullUp { entity: u8, pin: u8 },
    NotAnalog { entity: u8, pin: u8 },
    OutOfRange { entity: u8, option: &'static str },
    NotAnOutput { entity: u8, toggles: u8 },
}

impl std::fmt::Display for HardwareProblem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HardwareProblem::TooLong { length } => write!(
                formatter,
                "the configuration has {length} bytes, more than {HARDWARE_MAX_LENGTH}"
            ),
            HardwareProblem::UnknownTarget(target) => {
                write!(formatter, "the pins of target \"{target}\" are unknown")
            }
            HardwareProblem::ManagementEntity => write!(
                formatter,
                "entity {} is the management entity",
                management::ENTITY
            ),
            HardwareProblem::RepeatedEntity(entity) => {
                write!(formatter, "entity {entity} appears more than once")
            }
            HardwareProblem::RepeatedPin { pin, entities } => write!(
                formatter,
                "pin {pin} is used by entities {} and {}",
                entities.0, entities.1
            ),
            HardwareProblem::MissingPin { entity, pin } => {
                write!(formatter, "entity {entity}: pin {pin} does not exist")
            }
            HardwareProblem::ReservedPin { entity, pin } => {
                write!(formatter, "entity {entity}: pin {pin} is reserved")
            }
            HardwareProblem::InputOnlyPin { entity, pin } => {
                write!(formatter, "entity {entity}: pin {pin} is input only")
            }
            HardwareProblem::NoPullUp { entity, pin } => write!(
                formatter,
                "entity {entity}: pin {pin} has no internal pull-up"
            ),
            HardwareProblem::NotAnalog { entity, pin } => {
                write!(formatter, "entity {entity}: pin {pin} is not an ADC1 pin")
            }
            HardwareProblem::OutOfRange { entity, option } => {
                write!(formatter, "entity {entity}: {option} is out of range")
            }
            HardwareProblem::NotAnOutput { entity, toggles } => write!(
                formatter,
                "entity {entity}: toggles entity {toggles}, which is not an output"
            ),
        }
    }
}

impl Hardware {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Hardware always serializes")
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    /// `target` is the `target` of the board's `Build`. Empty when the board can take it.
    pub fn validate(&self, target: &str) -> Vec<HardwareProblem> {
        let mut problems = Vec::new();
        let length = self.to_json().len();
        if length > usize::from(HARDWARE_MAX_LENGTH) {
            problems.push(HardwareProblem::TooLong { length });
        }
        let Some(chip) = Target::from_build(target) else {
            problems.push(HardwareProblem::UnknownTarget(target.to_string()));
            return problems;
        };

        for (index, entity) in self.entities.iter().enumerate() {
            let earlier = &self.entities[..index];
            if entity.id == management::ENTITY {
                problems.push(HardwareProblem::ManagementEntity);
            } else if earlier.iter().any(|other| other.id == entity.id) {
                problems.push(HardwareProblem::RepeatedEntity(entity.id));
            }
            if let Some(other) = earlier.iter().find(|other| other.pin == entity.pin) {
                problems.push(HardwareProblem::RepeatedPin {
                    pin: entity.pin,
                    entities: (other.id, entity.id),
                });
            }
            problems.extend(self.entity_problems(chip, entity));
        }
        problems
    }

    fn entity_problems(&self, chip: Target, entity: &HardwareEntity) -> Vec<HardwareProblem> {
        let (id, pin) = (entity.id, entity.pin);
        match chip.pin_problem(pin) {
            Some(PinProblem::Missing) => {
                return vec![HardwareProblem::MissingPin { entity: id, pin }];
            }
            Some(PinProblem::Reserved) => {
                return vec![HardwareProblem::ReservedPin { entity: id, pin }];
            }
            None => {}
        }

        let mut problems = Vec::new();
        if entity.driver.drives_pin() && chip.input_only(pin) {
            problems.push(HardwareProblem::InputOnlyPin { entity: id, pin });
        }
        match &entity.driver {
            Driver::Button { pull_up, toggles } => {
                if *pull_up && chip.input_only(pin) {
                    problems.push(HardwareProblem::NoPullUp { entity: id, pin });
                }
                if let Some(toggles) = *toggles {
                    let target = self.entities.iter().find(|other| other.id == toggles);
                    if !matches!(
                        target.map(|other| &other.driver),
                        Some(Driver::Output { .. })
                    ) {
                        problems.push(HardwareProblem::NotAnOutput {
                            entity: id,
                            toggles,
                        });
                    }
                }
            }
            Driver::Dht11 {
                read_interval_seconds,
            } if !DHT11_READ_INTERVAL_RANGE.contains(read_interval_seconds) => {
                problems.push(HardwareProblem::OutOfRange {
                    entity: id,
                    option: "read_interval_seconds",
                });
            }
            Driver::Potentiometer { minimum_change } => {
                if !chip.analog(pin) {
                    problems.push(HardwareProblem::NotAnalog { entity: id, pin });
                }
                if !MINIMUM_CHANGE_RANGE.contains(minimum_change) {
                    problems.push(HardwareProblem::OutOfRange {
                        entity: id,
                        option: "minimum_change",
                    });
                }
            }
            _ => {}
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(id: u8, pin: u8, driver: Driver) -> HardwareEntity {
        HardwareEntity { id, pin, driver }
    }

    /// The test board as wired today.
    fn test_board() -> Hardware {
        Hardware {
            entities: vec![
                entity(1, 2, Driver::Output { inverted: false }),
                entity(
                    2,
                    0,
                    Driver::Button {
                        pull_up: true,
                        toggles: Some(1),
                    },
                ),
                entity(
                    3,
                    4,
                    Driver::Dht11 {
                        read_interval_seconds: 30,
                    },
                ),
                entity(4, 18, Driver::Output { inverted: false }),
                entity(5, 19, Driver::Buzzer),
                entity(6, 23, Driver::IrReceiver),
                entity(7, 34, Driver::Potentiometer { minimum_change: 2 }),
            ],
        }
    }

    #[test]
    fn test_board_is_valid() {
        assert_eq!(test_board().validate("esp32"), vec![]);
    }

    #[test]
    fn json_roundtrip_and_defaults() {
        let hardware = test_board();
        assert_eq!(Hardware::from_json(&hardware.to_json()).unwrap(), hardware);

        let json = r#"{"entities":[
            {"id":1,"pin":2,"driver":"output"},
            {"id":2,"pin":0,"driver":"button"},
            {"id":3,"pin":4,"driver":"dht11"},
            {"id":7,"pin":34,"driver":"potentiometer"}
        ]}"#;
        let hardware = Hardware::from_json(json).unwrap();
        assert_eq!(
            hardware.entities[0].driver,
            Driver::Output { inverted: false }
        );
        assert_eq!(
            hardware.entities[1].driver,
            Driver::Button {
                pull_up: true,
                toggles: None
            }
        );
        assert_eq!(
            hardware.entities[2].driver,
            Driver::Dht11 {
                read_interval_seconds: 30
            }
        );
        assert_eq!(
            hardware.entities[3].driver,
            Driver::Potentiometer { minimum_change: 2 }
        );
    }

    #[test]
    fn unknown_driver_does_not_parse() {
        assert!(
            Hardware::from_json(r#"{"entities":[{"id":1,"pin":2,"driver":"servo"}]}"#).is_err()
        );
    }

    #[test]
    fn entity_problems() {
        let hardware = Hardware {
            entities: vec![
                entity(0, 2, Driver::Buzzer),
                entity(1, 5, Driver::Buzzer),
                entity(1, 5, Driver::Buzzer),
            ],
        };
        assert_eq!(
            hardware.validate("esp32"),
            vec![
                HardwareProblem::ManagementEntity,
                HardwareProblem::RepeatedEntity(1),
                HardwareProblem::RepeatedPin {
                    pin: 5,
                    entities: (1, 1)
                },
            ]
        );
    }

    #[test]
    fn esp32_pins() {
        let check = |pin, driver| {
            Hardware {
                entities: vec![entity(1, pin, driver)],
            }
            .validate("esp32")
        };
        assert_eq!(
            check(20, Driver::Buzzer),
            vec![HardwareProblem::MissingPin { entity: 1, pin: 20 }]
        );
        assert_eq!(
            check(7, Driver::Buzzer),
            vec![HardwareProblem::ReservedPin { entity: 1, pin: 7 }]
        );
        assert_eq!(
            check(35, Driver::Output { inverted: false }),
            vec![HardwareProblem::InputOnlyPin { entity: 1, pin: 35 }]
        );
        assert_eq!(
            check(
                36,
                Driver::Button {
                    pull_up: true,
                    toggles: None
                }
            ),
            vec![HardwareProblem::NoPullUp { entity: 1, pin: 36 }]
        );
        assert_eq!(
            check(
                36,
                Driver::Button {
                    pull_up: false,
                    toggles: None
                }
            ),
            vec![]
        );
        assert_eq!(check(36, Driver::IrReceiver), vec![]);
        assert_eq!(
            check(36, Driver::IrTransmitter),
            vec![HardwareProblem::InputOnlyPin { entity: 1, pin: 36 }]
        );
        assert_eq!(
            check(25, Driver::Potentiometer { minimum_change: 2 }),
            vec![HardwareProblem::NotAnalog { entity: 1, pin: 25 }]
        );
    }

    #[test]
    fn esp32c6_pins() {
        let check = |pin, driver| {
            Hardware {
                entities: vec![entity(1, pin, driver)],
            }
            .validate("esp32c6")
        };
        assert_eq!(
            check(35, Driver::Output { inverted: false }),
            vec![HardwareProblem::MissingPin { entity: 1, pin: 35 }]
        );
        assert_eq!(
            check(24, Driver::Buzzer),
            vec![HardwareProblem::ReservedPin { entity: 1, pin: 24 }]
        );
        assert_eq!(
            check(3, Driver::Potentiometer { minimum_change: 2 }),
            vec![]
        );
        assert_eq!(check(20, Driver::Output { inverted: true }), vec![]);
    }

    #[test]
    fn options_and_toggles() {
        let hardware = Hardware {
            entities: vec![
                entity(
                    1,
                    0,
                    Driver::Button {
                        pull_up: true,
                        toggles: Some(2),
                    },
                ),
                entity(2, 19, Driver::Buzzer),
                entity(
                    3,
                    4,
                    Driver::Dht11 {
                        read_interval_seconds: 1,
                    },
                ),
                entity(4, 34, Driver::Potentiometer { minimum_change: 0 }),
                entity(
                    5,
                    5,
                    Driver::Button {
                        pull_up: true,
                        toggles: Some(9),
                    },
                ),
            ],
        };
        assert_eq!(
            hardware.validate("esp32"),
            vec![
                HardwareProblem::NotAnOutput {
                    entity: 1,
                    toggles: 2
                },
                HardwareProblem::OutOfRange {
                    entity: 3,
                    option: "read_interval_seconds"
                },
                HardwareProblem::OutOfRange {
                    entity: 4,
                    option: "minimum_change"
                },
                HardwareProblem::NotAnOutput {
                    entity: 5,
                    toggles: 9
                },
            ]
        );
    }

    #[test]
    fn unknown_target_and_length() {
        assert_eq!(
            test_board().validate("linux-x86_64"),
            vec![HardwareProblem::UnknownTarget("linux-x86_64".into())]
        );
        let hardware = Hardware {
            entities: (1..=200).map(|id| entity(id, 2, Driver::Buzzer)).collect(),
        };
        assert!(matches!(
            hardware.validate("esp32")[0],
            HardwareProblem::TooLong { .. }
        ));
    }
}
