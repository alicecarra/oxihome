//! Current conditions and the forecast for today and tomorrow from Open-Meteo: free, no account
//! or key.

use std::ops::RangeInclusive;

use serde::Deserialize;

const URL: &str = "https://api.open-meteo.com/v1/forecast";
const HOUR: i64 = 3600;

#[derive(Debug, Clone, PartialEq)]
pub struct Weather {
    /// Unix time of the observation.
    pub observed_at: i64,
    /// Local time of the observation, `2026-10-12 09:15`.
    pub updated_at: String,
    pub temperature: f32,
    pub feels_like: f32,
    pub humidity: f32,
    pub precipitation: f32,
    pub wind_speed: f32,
    pub wind_gusts: f32,
    /// WMO weather code.
    pub condition: u8,
    pub next_hour: NextHour,
    /// Today, then tomorrow when Open-Meteo has it.
    pub days: Vec<Day>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NextHour {
    pub precipitation: f32,
    pub chance_of_rain: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub maximum: f32,
    pub minimum: f32,
    pub chance_of_rain: f32,
    pub condition: u8,
    pub uv_index: f32,
    pub sunrise: i64,
    pub sunset: i64,
}

impl Weather {
    pub fn is_day(&self, now: i64) -> bool {
        self.days
            .iter()
            .any(|day| day.sunrise <= now && now < day.sunset)
    }

    /// The next sunrise or sunset after `now`.
    pub fn next_sun_change(&self, now: i64) -> Option<i64> {
        self.days
            .iter()
            .flat_map(|day| [day.sunrise, day.sunset])
            .filter(|&moment| moment > now)
            .min()
    }
}

/// Drizzle, rain, showers and thunderstorms; not snow.
pub fn is_rain(code: u8) -> bool {
    matches!(code, 51..=67 | 80..=82 | 95..=99)
}

pub fn condition_label(code: u8) -> String {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 => "Light drizzle",
        53 => "Drizzle",
        55 => "Dense drizzle",
        56 | 57 => "Freezing drizzle",
        61 => "Light rain",
        63 => "Rain",
        65 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71 => "Light snow",
        73 => "Snow",
        75 => "Heavy snow",
        77 => "Snow grains",
        80 => "Light showers",
        81 => "Showers",
        82 => "Violent showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => return format!("Code {code}"),
    }
    .into()
}

#[derive(Deserialize)]
struct Response {
    utc_offset_seconds: i32,
    current: Current,
    minutely_15: Slots,
    hourly: Slots,
    daily: Daily,
}

#[derive(Deserialize)]
struct Current {
    time: i64,
    temperature_2m: f32,
    apparent_temperature: f32,
    relative_humidity_2m: f32,
    precipitation: f32,
    weather_code: u8,
    wind_speed_10m: f32,
    wind_gusts_10m: f32,
}

/// Each value covers the time before its moment: a quarter of an hour or an hour.
#[derive(Deserialize)]
struct Slots {
    time: Vec<i64>,
    #[serde(alias = "precipitation_probability")]
    precipitation: Vec<Option<f32>>,
}

impl Slots {
    fn within(&self, moments: RangeInclusive<i64>) -> impl Iterator<Item = f32> {
        self.time
            .iter()
            .zip(&self.precipitation)
            .filter(move |(moment, _)| moments.contains(moment))
            .map(|(_, value)| value.unwrap_or(0.0))
    }
}

/// One entry per day.
#[derive(Deserialize)]
struct Daily {
    temperature_2m_max: Vec<f32>,
    temperature_2m_min: Vec<f32>,
    precipitation_probability_max: Vec<Option<f32>>,
    weather_code: Vec<u8>,
    uv_index_max: Vec<Option<f32>>,
    sunrise: Vec<i64>,
    sunset: Vec<i64>,
}

impl Daily {
    fn day(&self, index: usize) -> Option<Day> {
        Some(Day {
            maximum: *self.temperature_2m_max.get(index)?,
            minimum: *self.temperature_2m_min.get(index)?,
            chance_of_rain: self
                .precipitation_probability_max
                .get(index)
                .copied()
                .flatten()
                .unwrap_or(0.0),
            condition: *self.weather_code.get(index)?,
            uv_index: self
                .uv_index_max
                .get(index)
                .copied()
                .flatten()
                .unwrap_or(0.0),
            sunrise: *self.sunrise.get(index)?,
            sunset: *self.sunset.get(index)?,
        })
    }
}

pub async fn fetch(
    client: &reqwest::Client,
    latitude: f64,
    longitude: f64,
) -> anyhow::Result<Weather> {
    let response = client
        .get(URL)
        .query(&[
            ("latitude", latitude.to_string()),
            ("longitude", longitude.to_string()),
            (
                "current",
                "temperature_2m,apparent_temperature,relative_humidity_2m,precipitation,\
                 weather_code,wind_speed_10m,wind_gusts_10m"
                    .into(),
            ),
            ("minutely_15", "precipitation".into()),
            ("forecast_minutely_15", "5".into()),
            ("hourly", "precipitation_probability".into()),
            ("forecast_hours", "3".into()),
            (
                "daily",
                "temperature_2m_max,temperature_2m_min,precipitation_probability_max,\
                 weather_code,uv_index_max,sunrise,sunset"
                    .into(),
            ),
            ("timezone", "auto".into()),
            ("timeformat", "unixtime".into()),
            ("forecast_days", "2".into()),
        ])
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("{status}: {}", error_reason(&body));
    }
    weather(response.json().await?)
}

/// Open-Meteo explains a refusal in `{"error": true, "reason": "..."}`.
fn error_reason(body: &str) -> String {
    #[derive(Deserialize)]
    struct Refusal {
        reason: String,
    }
    match serde_json::from_str::<Refusal>(body) {
        Ok(refusal) => refusal.reason,
        Err(_) => body.chars().take(200).collect(),
    }
}

fn weather(response: Response) -> anyhow::Result<Weather> {
    let current = &response.current;
    let observed_at = current.time;
    let next_quarters = observed_at + 1..=observed_at + HOUR;
    // An hour slot ending after the observation and starting before the hour ahead ends.
    let next_hours = observed_at + 1..=observed_at + 2 * HOUR - 1;
    let today = response
        .daily
        .day(0)
        .ok_or_else(|| anyhow::anyhow!("no forecast for today"))?;
    Ok(Weather {
        observed_at,
        updated_at: local_time(observed_at, response.utc_offset_seconds)?,
        temperature: current.temperature_2m,
        feels_like: current.apparent_temperature,
        humidity: current.relative_humidity_2m,
        precipitation: current.precipitation,
        wind_speed: current.wind_speed_10m,
        wind_gusts: current.wind_gusts_10m,
        condition: current.weather_code,
        next_hour: NextHour {
            precipitation: response.minutely_15.within(next_quarters).sum(),
            chance_of_rain: response.hourly.within(next_hours).fold(0.0, f32::max),
        },
        days: std::iter::once(today)
            .chain(response.daily.day(1))
            .collect(),
    })
}

/// `2026-10-12 09:15`.
fn local_time(moment: i64, utc_offset_seconds: i32) -> anyhow::Result<String> {
    let offset = jiff::tz::Offset::from_seconds(utc_offset_seconds)?;
    let moment = jiff::Timestamp::from_second(moment)?;
    Ok(moment
        .to_zoned(jiff::tz::TimeZone::fixed(offset))
        .strftime("%Y-%m-%d %H:%M")
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real answer, at 00:45 in Porto Alegre.
    const ANSWER: &str = r#"{
        "latitude": -29.98, "longitude": -51.19, "utc_offset_seconds": -10800,
        "timezone": "America/Sao_Paulo",
        "current": {"time": 1792813500, "interval": 900, "temperature_2m": 21.8,
                    "relative_humidity_2m": 89, "apparent_temperature": 24.5,
                    "precipitation": 0.2, "weather_code": 61, "wind_speed_10m": 6.7,
                    "wind_gusts_10m": 12.2},
        "minutely_15": {"time": [1792813500, 1792814400, 1792815300, 1792816200, 1792817100],
                        "precipitation": [0.2, 0.25, 0.5, null, 0.25]},
        "hourly": {"time": [1792810800, 1792814400, 1792818000],
                   "precipitation_probability": [90, 57, 56]},
        "daily": {"time": [1792810800, 1792897200], "weather_code": [81, 80],
                  "temperature_2m_max": [25.6, 20.6], "temperature_2m_min": [16.3, 13.7],
                  "precipitation_probability_max": [80, null],
                  "sunrise": [1792831034, 1792917376], "sunset": [1792878062, 1792964505],
                  "uv_index_max": [1.95, 7.35]}
    }"#;

    fn read(answer: &str) -> anyhow::Result<Weather> {
        weather(serde_json::from_str(answer).unwrap())
    }

    #[test]
    fn reads_current_conditions_the_next_hour_and_the_days() {
        let weather = read(ANSWER).unwrap();
        assert_eq!(weather.observed_at, 1792813500);
        assert_eq!(weather.updated_at, "2026-10-24 00:45");
        assert_eq!(
            (weather.temperature, weather.feels_like, weather.humidity),
            (21.8, 24.5, 89.0)
        );
        assert_eq!(
            (
                weather.precipitation,
                weather.wind_speed,
                weather.wind_gusts
            ),
            (0.2, 6.7, 12.2)
        );
        assert_eq!(weather.condition, 61);
        assert_eq!(
            weather.next_hour,
            NextHour {
                precipitation: 1.0,
                chance_of_rain: 57.0
            },
            "the quarter already observed and the hour already gone are left out"
        );
        assert_eq!(
            weather.days,
            vec![
                Day {
                    maximum: 25.6,
                    minimum: 16.3,
                    chance_of_rain: 80.0,
                    condition: 81,
                    uv_index: 1.95,
                    sunrise: 1792831034,
                    sunset: 1792878062,
                },
                Day {
                    maximum: 20.6,
                    minimum: 13.7,
                    chance_of_rain: 0.0,
                    condition: 80,
                    uv_index: 7.35,
                    sunrise: 1792917376,
                    sunset: 1792964505,
                },
            ]
        );
    }

    #[test]
    fn tomorrow_may_be_missing_but_today_may_not() {
        let one_day = ANSWER.replace("[81, 80]", "[81]");
        assert_eq!(read(&one_day).unwrap().days.len(), 1);
        let no_day = ANSWER.replace("[25.6, 20.6]", "[]");
        assert!(read(&no_day).is_err());
    }

    #[test]
    fn daylight_runs_from_sunrise_to_sunset() {
        let weather = read(ANSWER).unwrap();
        let sunrise = 1792831034;
        let sunset = 1792878062;
        assert!(!weather.is_day(sunrise - 1));
        assert!(weather.is_day(sunrise));
        assert!(!weather.is_day(sunset));
        assert_eq!(weather.next_sun_change(1792813500), Some(sunrise));
        assert_eq!(weather.next_sun_change(sunrise), Some(sunset));
        assert_eq!(weather.next_sun_change(sunset), Some(1792917376));
        assert_eq!(weather.next_sun_change(1792964505), None);
    }

    #[test]
    fn rain_codes_and_labels() {
        assert!(is_rain(51) && is_rain(65) && is_rain(81) && is_rain(95));
        assert!(!is_rain(3) && !is_rain(45) && !is_rain(73) && !is_rain(85));
        assert_eq!(condition_label(2), "Partly cloudy");
        assert_eq!(condition_label(42), "Code 42");
    }

    #[test]
    fn a_refusal_gives_its_reason() {
        let body =
            r#"{"reason":"Latitude must be in range of -90 to 90°. Given: -300.0.","error":true}"#;
        assert_eq!(
            error_reason(body),
            "Latitude must be in range of -90 to 90°. Given: -300.0."
        );
        assert_eq!(error_reason("Bad Gateway"), "Bad Gateway");
    }
}
