//! Current conditions and today's forecast from Open-Meteo: free, no account or key.

use serde::Deserialize;

const URL: &str = "https://api.open-meteo.com/v1/forecast";

#[derive(Debug, Clone, PartialEq)]
pub struct Weather {
    /// Local time of the observation, `2026-10-12 09:15`.
    pub updated_at: String,
    pub temperature: f32,
    pub humidity: f32,
    pub precipitation: f32,
    pub wind_speed: f32,
    pub today_maximum: f32,
    pub today_minimum: f32,
    pub today_precipitation_probability: f32,
}

#[derive(Deserialize)]
struct Response {
    current: Current,
    daily: Daily,
}

#[derive(Deserialize)]
struct Current {
    time: String,
    temperature_2m: f32,
    relative_humidity_2m: f32,
    precipitation: f32,
    wind_speed_10m: f32,
}

/// One entry per day; only today is asked for.
#[derive(Deserialize)]
struct Daily {
    temperature_2m_max: Vec<f32>,
    temperature_2m_min: Vec<f32>,
    precipitation_probability_max: Vec<Option<f32>>,
}

pub async fn fetch(
    client: &reqwest::Client,
    latitude: f64,
    longitude: f64,
) -> anyhow::Result<Weather> {
    let response: Response = client
        .get(URL)
        .query(&[
            ("latitude", latitude.to_string()),
            ("longitude", longitude.to_string()),
            (
                "current",
                "temperature_2m,relative_humidity_2m,precipitation,wind_speed_10m".into(),
            ),
            (
                "daily",
                "temperature_2m_max,temperature_2m_min,precipitation_probability_max".into(),
            ),
            ("timezone", "auto".into()),
            ("forecast_days", "1".into()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    weather(response)
}

fn weather(response: Response) -> anyhow::Result<Weather> {
    let first = |values: &[f32], name: &str| {
        values
            .first()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("no {name} for today"))
    };
    Ok(Weather {
        updated_at: response.current.time.replace('T', " "),
        temperature: response.current.temperature_2m,
        humidity: response.current.relative_humidity_2m,
        precipitation: response.current.precipitation,
        wind_speed: response.current.wind_speed_10m,
        today_maximum: first(&response.daily.temperature_2m_max, "maximum")?,
        today_minimum: first(&response.daily.temperature_2m_min, "minimum")?,
        today_precipitation_probability: response
            .daily
            .precipitation_probability_max
            .first()
            .copied()
            .flatten()
            .unwrap_or(0.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real answer.
    const ANSWER: &str = r#"{
        "latitude": -30.0, "longitude": -51.25, "timezone": "America/Sao_Paulo",
        "current_units": {"time": "iso8601", "temperature_2m": "°C"},
        "current": {"time": "2026-10-12T09:15", "interval": 900, "temperature_2m": 21.4,
                    "relative_humidity_2m": 73, "precipitation": 0.2, "wind_speed_10m": 11.5},
        "daily": {"time": ["2026-10-12"], "temperature_2m_max": [26.1],
                  "temperature_2m_min": [16.8], "precipitation_probability_max": [45]}
    }"#;

    #[test]
    fn reads_current_conditions_and_today() {
        let response: Response = serde_json::from_str(ANSWER).unwrap();
        assert_eq!(
            weather(response).unwrap(),
            Weather {
                updated_at: "2026-10-12 09:15".into(),
                temperature: 21.4,
                humidity: 73.0,
                precipitation: 0.2,
                wind_speed: 11.5,
                today_maximum: 26.1,
                today_minimum: 16.8,
                today_precipitation_probability: 45.0,
            }
        );
    }

    #[test]
    fn a_missing_probability_counts_as_none() {
        let answer = ANSWER.replace("[45]", "[null]");
        let response: Response = serde_json::from_str(&answer).unwrap();
        assert_eq!(
            weather(response).unwrap().today_precipitation_probability,
            0.0
        );
    }
}
