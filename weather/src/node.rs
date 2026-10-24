use std::time::{Duration, Instant};

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    Message, Quantity, Unit, Value, ValueType,
};

use crate::open_meteo::{Day, Weather, condition_label, is_rain};

const NOW_ENTITY: u8 = 1;
const TODAY_ENTITY: u8 = 2;
const UPDATED_ENTITY: u8 = 3;
const CONNECTION_ENTITY: u8 = 4;
const NEXT_HOUR_ENTITY: u8 = 5;
const TOMORROW_ENTITY: u8 = 6;
const ENTITIES: [u8; 6] = [
    NOW_ENTITY,
    TODAY_ENTITY,
    UPDATED_ENTITY,
    CONNECTION_ENTITY,
    NEXT_HOUR_ENTITY,
    TOMORROW_ENTITY,
];
/// How long fetches fail, since the last one that worked, before Open-Meteo counts as unreachable.
const UNREACHABLE_AFTER: Duration = Duration::from_secs(30 * 60);
/// Forecast precipitation in the next hour from which rain is expected.
const RAIN_EXPECTED_FROM: f32 = 0.1;

pub struct WeatherNode {
    core: NodeCore,
    device: WeatherDevice,
    started_at: Instant,
}

struct WeatherDevice {
    /// `None` until the first fetch works.
    weather: Option<Weather>,
    /// Between sunrise and sunset, as of the last fetch or sun change.
    daytime: bool,
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
                daytime: false,
                reachable: None,
                failing_since: None,
                fetched_at: None,
                age: None,
            },
            started_at: Instant::now(),
        }
    }

    /// Only the entities whose values changed; `unix_now` places it between sunrise and sunset.
    pub fn update(&mut self, weather: Weather, now: Instant, unix_now: i64) -> Vec<Message> {
        self.changes(|device| {
            device.daytime = weather.is_day(unix_now);
            device.weather = Some(weather);
            device.reachable = Some(true);
            device.failing_since = None;
            device.fetched_at = Some(now);
            device.age = Some(Duration::ZERO);
        })
    }

    /// The data keeps its last values; the connection entity tells how old they are.
    pub fn failed(&mut self, now: Instant) -> Vec<Message> {
        self.changes(|device| {
            let since = device
                .fetched_at
                .unwrap_or(*device.failing_since.get_or_insert(now));
            if now - since >= UNREACHABLE_AFTER {
                device.reachable = Some(false);
            }
            device.age = device.fetched_at.map(|fetched_at| now - fetched_at);
        })
    }

    /// At sunrise and sunset, between fetches.
    pub fn sun(&mut self, unix_now: i64) -> Vec<Message> {
        self.changes(|device| {
            if let Some(weather) = &device.weather {
                device.daytime = weather.is_day(unix_now);
            }
        })
    }

    fn changes(&mut self, change: impl FnOnce(&mut WeatherDevice)) -> Vec<Message> {
        let before = ENTITIES.map(|entity| self.device.state(entity));
        change(&mut self.device);
        ENTITIES
            .into_iter()
            .zip(before)
            .filter(|(entity, before)| self.device.state(*entity) != *before)
            .filter_map(|(entity, _)| self.core.state(&self.device, entity))
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
        let entity = |id, channels| EntityDescriptor {
            id,
            name: None,
            channels,
        };
        vec![
            entity(
                NOW_ENTITY,
                vec![
                    temperature(0).named("Temperature"),
                    reading(1, 0.0, 100.0, Quantity::Humidity, Unit::Percent).named("Humidity"),
                    precipitation(2).named("Precipitation"),
                    wind(3).named("Wind"),
                    flag(4).named("Raining"),
                    temperature(5).named("Feels like"),
                    wind(6).named("Gusts"),
                    text(7).named("Condition"),
                    flag(8).named("Daytime"),
                ],
            )
            .named("Now"),
            entity(TODAY_ENTITY, day_channels()).named("Today"),
            entity(UPDATED_ENTITY, vec![text(0).named("At")]).named("Updated"),
            entity(
                CONNECTION_ENTITY,
                vec![
                    flag(0).named("Reachable"),
                    Channel {
                        value: Some(ValueType::Int {
                            min: 0,
                            max: i32::MAX,
                            step: 1,
                        }),
                        unit: Some(Unit::Second),
                        ..flag(1)
                    }
                    .named("Data age"),
                ],
            )
            .named("Connection"),
            entity(
                NEXT_HOUR_ENTITY,
                vec![
                    precipitation(0).named("Precipitation"),
                    chance(1).named("Chance of rain"),
                    flag(2).named("Rain expected"),
                ],
            )
            .named("Next hour"),
            entity(TOMORROW_ENTITY, day_channels()).named("Tomorrow"),
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
        Some(match entity {
            NOW_ENTITY => vec![
                float(0, weather.temperature),
                float(1, weather.humidity),
                float(2, weather.precipitation),
                float(3, weather.wind_speed),
                confirmed(4, Value::Bool(is_rain(weather.condition))),
                float(5, weather.feels_like),
                float(6, weather.wind_gusts),
                confirmed(7, Value::Text(condition_label(weather.condition))),
                confirmed(8, Value::Bool(self.daytime)),
            ],
            TODAY_ENTITY => day_values(weather.days.first()?),
            UPDATED_ENTITY => vec![confirmed(0, Value::Text(weather.updated_at.clone()))],
            NEXT_HOUR_ENTITY => vec![
                float(0, weather.next_hour.precipitation),
                float(1, weather.next_hour.chance_of_rain),
                confirmed(
                    2,
                    Value::Bool(weather.next_hour.precipitation >= RAIN_EXPECTED_FROM),
                ),
            ],
            TOMORROW_ENTITY => day_values(weather.days.get(1)?),
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

fn day_channels() -> Vec<Channel> {
    vec![
        temperature(0).named("Maximum"),
        temperature(1).named("Minimum"),
        chance(2).named("Chance of rain"),
        text(3).named("Condition"),
        Channel {
            value: Some(ValueType::Float {
                min: 0.0,
                max: 20.0,
                step: 0.1,
            }),
            ..flag(4)
        }
        .named("UV index"),
        moment(5).named("Sunrise"),
        moment(6).named("Sunset"),
    ]
}

fn day_values(day: &Day) -> Vec<ChannelValue> {
    let moment = |channel, unix: i64| {
        confirmed(channel, Value::Int(i32::try_from(unix).unwrap_or(i32::MAX)))
    };
    vec![
        float(0, day.maximum),
        float(1, day.minimum),
        float(2, day.chance_of_rain),
        confirmed(3, Value::Text(condition_label(day.condition))),
        float(4, day.uv_index),
        moment(5, day.sunrise),
        moment(6, day.sunset),
    ]
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

fn temperature(id: u8) -> Channel {
    reading(id, -50.0, 60.0, Quantity::Temperature, Unit::Celsius)
}

fn precipitation(id: u8) -> Channel {
    reading(id, 0.0, 500.0, Quantity::Precipitation, Unit::Millimeter)
}

fn wind(id: u8) -> Channel {
    reading(id, 0.0, 300.0, Quantity::WindSpeed, Unit::KilometerPerHour)
}

fn chance(id: u8) -> Channel {
    reading(id, 0.0, 100.0, Quantity::Precipitation, Unit::Percent)
}

fn flag(id: u8) -> Channel {
    Channel {
        id,
        name: None,
        access: Access::Read,
        value: Some(ValueType::Bool),
        quantity: None,
        unit: None,
    }
}

fn text(id: u8) -> Channel {
    Channel {
        value: Some(ValueType::Text { max_length: 32 }),
        ..flag(id)
    }
}

fn moment(id: u8) -> Channel {
    Channel {
        value: Some(ValueType::Int {
            min: 0,
            max: i32::MAX,
            step: 1,
        }),
        unit: Some(Unit::UnixTime),
        ..flag(id)
    }
}

fn float(channel: u8, value: f32) -> ChannelValue {
    confirmed(channel, Value::Float(value))
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
    use crate::open_meteo::NextHour;

    const SUNRISE: i64 = 1792831034;
    const SUNSET: i64 = 1792878062;

    fn weather(condition: u8) -> Weather {
        let day = Day {
            maximum: 26.1,
            minimum: 16.8,
            chance_of_rain: 45.0,
            condition: 3,
            uv_index: 6.2,
            sunrise: SUNRISE,
            sunset: SUNSET,
        };
        Weather {
            observed_at: SUNRISE - 3600,
            updated_at: "2026-10-24 04:37".into(),
            temperature: 21.4,
            feels_like: 22.0,
            humidity: 73.0,
            precipitation: 0.2,
            wind_speed: 11.5,
            wind_gusts: 20.3,
            condition,
            next_hour: NextHour {
                precipitation: 0.3,
                chance_of_rain: 60.0,
            },
            days: vec![day.clone(), day],
        }
    }

    fn entities(messages: &[Message]) -> Vec<u8> {
        messages
            .iter()
            .filter_map(|message| match message.payload {
                Payload::State { entity, .. } => Some(entity),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn nothing_to_report_before_the_first_fetch() {
        let mut node = WeatherNode::new("weather".into());
        let announcement = node.announcement();
        assert_eq!(announcement.len(), 1, "only the Hello");
        assert!(matches!(announcement[0].payload, Payload::Hello { .. }));
    }

    #[test]
    fn a_fetch_publishes_every_entity_and_the_announcement_repeats_them() {
        let mut node = WeatherNode::new("weather".into());
        let states = node.update(weather(61), Instant::now(), SUNRISE);
        assert_eq!(entities(&states), ENTITIES);
        assert_eq!(entities(&node.announcement()), ENTITIES);
        let now = node.device.state(NOW_ENTITY).unwrap();
        assert_eq!(now[4].value, Value::Bool(true), "light rain");
        assert_eq!(now[7].value, Value::Text("Light rain".into()));
        assert_eq!(now[8].value, Value::Bool(true), "daytime");
        assert_eq!(
            node.device.state(NEXT_HOUR_ENTITY).unwrap()[2].value,
            Value::Bool(true)
        );
        assert_eq!(
            node.device.state(TODAY_ENTITY).unwrap()[5].value,
            Value::Int(SUNRISE as i32)
        );
    }

    #[test]
    fn only_what_changed_is_published_again() {
        let mut node = WeatherNode::new("weather".into());
        let start = Instant::now();
        node.update(weather(3), start, SUNRISE);
        assert!(node.update(weather(3), start, SUNRISE).is_empty());

        let mut drizzle = weather(51);
        drizzle.updated_at = "2026-10-24 04:52".into();
        assert_eq!(
            entities(&node.update(drizzle, start, SUNRISE)),
            [NOW_ENTITY, UPDATED_ENTITY]
        );
        assert_eq!(
            node.device.state(NOW_ENTITY).unwrap()[4].value,
            Value::Bool(true)
        );
    }

    #[test]
    fn daytime_follows_the_sun_between_fetches() {
        let mut node = WeatherNode::new("weather".into());
        node.update(weather(3), Instant::now(), SUNRISE - 1);
        let daytime = |node: &WeatherNode| node.device.state(NOW_ENTITY).unwrap()[8].value.clone();
        assert_eq!(daytime(&node), Value::Bool(false));
        assert!(node.sun(SUNRISE - 1).is_empty());
        assert_eq!(entities(&node.sun(SUNRISE)), [NOW_ENTITY]);
        assert_eq!(daytime(&node), Value::Bool(true));
        node.sun(SUNSET);
        assert_eq!(daytime(&node), Value::Bool(false));
    }

    #[test]
    fn tomorrow_is_left_out_when_missing() {
        let mut node = WeatherNode::new("weather".into());
        let mut today_only = weather(3);
        today_only.days.truncate(1);
        let states = node.update(today_only, Instant::now(), SUNRISE);
        assert!(!entities(&states).contains(&TOMORROW_ENTITY));
    }

    #[test]
    fn failed_fetches_age_the_data_and_end_unreachable() {
        let mut node = WeatherNode::new("weather".into());
        let start = Instant::now();
        let connection = |node: &WeatherNode| node.device.state(CONNECTION_ENTITY).unwrap();

        assert_eq!(node.failed(start).len(), 0, "nothing known yet");
        node.update(weather(3), start, SUNRISE);
        assert_eq!(
            connection(&node),
            vec![confirmed(0, Value::Bool(true)), confirmed(1, Value::Int(0))]
        );

        let minutes = |minutes: u64| start + Duration::from_secs(minutes * 60);
        assert_eq!(entities(&node.failed(minutes(10))), [CONNECTION_ENTITY]);
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
        node.update(weather(3), Instant::now(), SUNRISE);
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
            Payload::State { values, .. } if values[0].value == Value::Text("2026-10-24 04:37".into())
        ));
    }
}
