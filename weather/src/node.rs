use std::time::{Duration, Instant};

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
const CONNECTION_ENTITY: u8 = 4;
/// How long fetches fail, since the last one that worked, before Open-Meteo counts as unreachable.
const UNREACHABLE_AFTER: Duration = Duration::from_secs(30 * 60);

pub struct WeatherNode {
    core: NodeCore,
    device: WeatherDevice,
    started_at: Instant,
}

struct WeatherDevice {
    /// `None` until the first fetch works.
    weather: Option<Weather>,
    /// `None` until a fetch works or enough of them fail.
    reachable: Option<bool>,
    /// The first of the failed fetches in a row, while none has worked yet.
    failing_since: Option<Instant>,
    fetched_at: Option<Instant>,
    /// Since the last fetch that worked, as of the last attempt.
    age: Option<Duration>,
}

impl WeatherNode {
    pub fn new(device_id: String) -> Self {
        Self {
            core: NodeCore::new(
                device_id,
                Build::native("weather", env!("CARGO_PKG_VERSION")),
            ),
            device: WeatherDevice {
                weather: None,
                reachable: None,
                failing_since: None,
                fetched_at: None,
                age: None,
            },
            started_at: Instant::now(),
        }
    }

    pub fn update(&mut self, weather: Weather, now: Instant) -> Vec<Message> {
        let device = &mut self.device;
        device.weather = Some(weather);
        device.reachable = Some(true);
        device.failing_since = None;
        device.fetched_at = Some(now);
        device.age = Some(Duration::ZERO);
        self.core.states(&self.device)
    }

    /// The data keeps its last values; the connection entity tells how old they are.
    pub fn failed(&mut self, now: Instant) -> Vec<Message> {
        let device = &mut self.device;
        let since = device
            .fetched_at
            .unwrap_or(*device.failing_since.get_or_insert(now));
        if now - since >= UNREACHABLE_AFTER {
            device.reachable = Some(false);
        }
        device.age = device.fetched_at.map(|fetched_at| now - fetched_at);
        self.core
            .state(&self.device, CONNECTION_ENTITY)
            .into_iter()
            .collect()
    }
}

impl SoftwareNode for WeatherNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.core.handle(&mut self.device, message)
    }

    fn invalid_message(&mut self, detail: String) -> Vec<Message> {
        vec![self.core.invalid_message(detail)]
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
            EntityDescriptor {
                id: CONNECTION_ENTITY,
                name: None,
                channels: vec![
                    Channel {
                        id: 0,
                        name: None,
                        access: Access::Read,
                        value: Some(ValueType::Bool),
                        quantity: None,
                        unit: None,
                    }
                    .named("Reachable"),
                    Channel {
                        id: 1,
                        name: None,
                        access: Access::Read,
                        value: Some(ValueType::Int {
                            min: 0,
                            max: i32::MAX,
                            step: 1,
                        }),
                        quantity: None,
                        unit: Some(Unit::Second),
                    }
                    .named("Data age"),
                ],
            }
            .named("Connection"),
        ]
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        if entity == CONNECTION_ENTITY {
            let mut values = Vec::new();
            if let Some(reachable) = self.reachable {
                values.push(confirmed(0, Value::Bool(reachable)));
            }
            if let Some(age) = self.age {
                let seconds = i32::try_from(age.as_secs()).unwrap_or(i32::MAX);
                values.push(confirmed(1, Value::Int(seconds)));
            }
            return (!values.is_empty()).then_some(values);
        }
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
        let states = node.update(weather(0.2), Instant::now());
        assert_eq!(states.len(), 4);
        assert_eq!(
            node.device.state(NOW_ENTITY).unwrap()[4].value,
            Value::Bool(true)
        );
        node.update(weather(0.0), Instant::now());
        assert_eq!(
            node.device.state(NOW_ENTITY).unwrap()[4].value,
            Value::Bool(false)
        );
    }

    #[test]
    fn failed_fetches_age_the_data_and_end_unreachable() {
        let mut node = WeatherNode::new("weather".into());
        let start = Instant::now();
        let connection = |node: &WeatherNode| node.device.state(CONNECTION_ENTITY).unwrap();

        assert_eq!(node.failed(start).len(), 0, "nothing known yet");
        node.update(weather(0.0), start);
        assert_eq!(
            connection(&node),
            vec![confirmed(0, Value::Bool(true)), confirmed(1, Value::Int(0))]
        );

        let minutes = |minutes: u64| start + Duration::from_secs(minutes * 60);
        assert_eq!(node.failed(minutes(10)).len(), 1);
        node.failed(minutes(20));
        assert_eq!(connection(&node)[0].value, Value::Bool(true));
        node.failed(minutes(30));
        assert_eq!(
            connection(&node),
            vec![
                confirmed(0, Value::Bool(false)),
                confirmed(1, Value::Int(1800))
            ]
        );
        assert_eq!(
            node.device.state(NOW_ENTITY).unwrap()[0].value,
            Value::Float(21.4),
            "the last values stay"
        );
    }

    #[test]
    fn without_any_fetch_working_it_ends_unreachable_after_half_an_hour() {
        let mut node = WeatherNode::new("weather".into());
        let start = Instant::now();
        let minutes = |minutes: u64| start + Duration::from_secs(minutes * 60);
        for minute in [0, 1, 3, 7, 15, 25] {
            assert!(node.failed(minutes(minute)).is_empty());
        }
        assert_eq!(
            node.failed(minutes(35))[0].payload,
            Payload::State {
                entity: CONNECTION_ENTITY,
                values: vec![confirmed(0, Value::Bool(false))]
            }
        );
    }

    #[test]
    fn readings_cannot_be_set() {
        let mut node = WeatherNode::new("weather".into());
        node.update(weather(0.0), Instant::now());
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
