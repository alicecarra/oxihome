use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    Message, Quantity, Unit, Value, ValueType,
};

use crate::open_meteo::Weather;

const NOW_ENTITY: u8 = 1;
const TODAY_ENTITY: u8 = 2;
const UPDATED_ENTITY: u8 = 3;

pub struct WeatherNode {
    core: NodeCore,
    device: WeatherDevice,
    started_at: Instant,
}

struct WeatherDevice {
    /// `None` until the first fetch works.
    weather: Option<Weather>,
}

impl WeatherNode {
    pub fn new(device_id: String) -> Self {
        Self {
            core: NodeCore::new(
                device_id,
                Build::native("weather", env!("CARGO_PKG_VERSION")),
            ),
            device: WeatherDevice { weather: None },
            started_at: Instant::now(),
        }
    }

    pub fn update(&mut self, weather: Weather) -> Vec<Message> {
        self.device.weather = Some(weather);
        self.core.states(&self.device)
    }
}

impl SoftwareNode for WeatherNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.core.handle(&mut self.device, message)
    }

    fn announcement(&mut self) -> Vec<Message> {
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        vec![self.core.heartbeat(uptime_s, None, None)]
    }
}

impl Device for WeatherDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        vec![
            EntityDescriptor {
                id: NOW_ENTITY,
                name: None,
                channels: vec![
                    reading(0, -50.0, 60.0, Quantity::Temperature, Unit::Celsius)
                        .named("Temperature"),
                    reading(1, 0.0, 100.0, Quantity::Humidity, Unit::Percent).named("Humidity"),
                    reading(2, 0.0, 500.0, Quantity::Precipitation, Unit::Millimeter)
                        .named("Precipitation"),
                    reading(3, 0.0, 300.0, Quantity::WindSpeed, Unit::KilometerPerHour)
                        .named("Wind"),
                    Channel {
                        id: 4,
                        name: None,
                        access: Access::Read,
                        value: Some(ValueType::Bool),
                        quantity: None,
                        unit: None,
                    }
                    .named("Raining"),
                ],
            }
            .named("Now"),
            EntityDescriptor {
                id: TODAY_ENTITY,
                name: None,
                channels: vec![
                    reading(0, -50.0, 60.0, Quantity::Temperature, Unit::Celsius).named("Maximum"),
                    reading(1, -50.0, 60.0, Quantity::Temperature, Unit::Celsius).named("Minimum"),
                    reading(2, 0.0, 100.0, Quantity::Precipitation, Unit::Percent)
                        .named("Chance of rain"),
                ],
            }
            .named("Today"),
            EntityDescriptor {
                id: UPDATED_ENTITY,
                name: None,
                channels: vec![
                    Channel {
                        id: 0,
                        name: None,
                        access: Access::Read,
                        value: Some(ValueType::Text { max_length: 32 }),
                        quantity: None,
                        unit: None,
                    }
                    .named("At"),
                ],
            }
            .named("Updated"),
        ]
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        let weather = self.weather.as_ref()?;
        let float = |channel, value: f32| confirmed(channel, Value::Float(value));
        Some(match entity {
            NOW_ENTITY => vec![
                float(0, weather.temperature),
                float(1, weather.humidity),
                float(2, weather.precipitation),
                float(3, weather.wind_speed),
                confirmed(4, Value::Bool(weather.precipitation > 0.0)),
            ],
            TODAY_ENTITY => vec![
                float(0, weather.today_maximum),
                float(1, weather.today_minimum),
                float(2, weather.today_precipitation_probability),
            ],
            UPDATED_ENTITY => vec![confirmed(0, Value::Text(weather.updated_at.clone()))],
            _ => return None,
        })
    }

    /// Every channel is read only, so the core refuses commands before they get here.
    fn command(&mut self, entity: u8, channel: u8, _op: &CommandOp) -> Result<(), Rejection> {
        Err(Rejection::new(
            ErrorCode::InvalidOperation,
            format!("entity {entity} channel {channel} is read only"),
        ))
    }
}

fn reading(id: u8, min: f32, max: f32, quantity: Quantity, unit: Unit) -> Channel {
    Channel {
        id,
        name: None,
        access: Access::Read,
        value: Some(ValueType::Float {
            min,
            max,
            step: 0.1,
        }),
        quantity: Some(quantity),
        unit: Some(unit),
    }
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::{Payload, QueryKind};

    use super::*;

    fn weather(precipitation: f32) -> Weather {
        Weather {
            updated_at: "2026-10-12 09:15".into(),
            temperature: 21.4,
            humidity: 73.0,
            precipitation,
            wind_speed: 11.5,
            today_maximum: 26.1,
            today_minimum: 16.8,
            today_precipitation_probability: 45.0,
        }
    }

    #[test]
    fn nothing_to_report_before_the_first_fetch() {
        let mut node = WeatherNode::new("weather".into());
        let announcement = node.announcement();
        assert_eq!(announcement.len(), 1, "only the Hello");
        assert!(matches!(announcement[0].payload, Payload::Hello { .. }));
    }

    #[test]
    fn a_fetch_publishes_every_entity_and_rain_follows_precipitation() {
        let mut node = WeatherNode::new("weather".into());
        let states = node.update(weather(0.2));
        assert_eq!(states.len(), 3);
        assert_eq!(
            node.device.state(NOW_ENTITY).unwrap()[4].value,
            Value::Bool(true)
        );
        node.update(weather(0.0));
        assert_eq!(
            node.device.state(NOW_ENTITY).unwrap()[4].value,
            Value::Bool(false)
        );
    }

    #[test]
    fn readings_cannot_be_set() {
        let mut node = WeatherNode::new("weather".into());
        node.update(weather(0.0));
        let set = Message::new(
            "oxihome-hub",
            1,
            Payload::Command {
                entity: NOW_ENTITY,
                channel: 0,
                op: CommandOp::Set(Value::Float(30.0)),
            },
        )
        .to("weather");
        let answers = node.handle(&set);
        assert!(matches!(answers[0].payload, Payload::Error { code: 6, .. }));
        let query = Message::new(
            "oxihome-hub",
            2,
            Payload::Query {
                what: QueryKind::State {
                    entity: Some(UPDATED_ENTITY),
                },
            },
        );
        assert!(matches!(
            &node.handle(&query)[0].payload,
            Payload::State { values, .. } if values[0].value == Value::Text("2026-10-12 09:15".into())
        ));
    }
}
