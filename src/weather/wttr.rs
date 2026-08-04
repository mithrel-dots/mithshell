use anyhow::{Context, Result};
use log::warn;
use serde::Deserialize;

use crate::state::{WeatherCondition, WeatherDay, WeatherState};

use super::{curl, pad_forecast, url_encode, weekday_label};

/// wttr.in's machine-readable `j1` format; without a path segment it
/// geolocates by source IP, so no location needs to be configured.
const WTTR_URL: &str = "https://wttr.in/?format=j1";

pub(super) fn fetch(city: Option<&str>) -> Result<WeatherState> {
    let url = match city {
        Some(city) => format!("https://wttr.in/{}?format=j1", url_encode(city)),
        None => WTTR_URL.to_owned(),
    };
    match curl(&url) {
        Ok(body) => parse_response(&body),
        Err(error) if city.is_some() => {
            warn!(
                "wttr.in rejected the configured city, falling back to IP geolocation: {error:#}"
            );
            parse_response(&curl(WTTR_URL)?)
        }
        Err(error) => Err(error),
    }
}

fn parse_response(body: &str) -> Result<WeatherState> {
    let response: WttrResponse =
        serde_json::from_str(body).context("failed to parse wttr.in response")?;
    let current = response
        .current_condition
        .first()
        .context("wttr.in response had no current conditions")?;
    let location = response
        .nearest_area
        .first()
        .map(WttrArea::label)
        .unwrap_or_else(|| "Unknown location".to_owned());
    let mut days: Vec<_> = response
        .weather
        .iter()
        .take(7)
        .map(WttrDay::forecast)
        .collect();
    pad_forecast(&mut days);
    Ok(WeatherState {
        location,
        current_c: current.temp_c.parse().unwrap_or_default(),
        condition: condition_from_code(&current.weather_code),
        description: current
            .weather_desc
            .first()
            .map(|value| value.value.clone())
            .unwrap_or_default(),
        days,
    })
}

#[derive(Debug, Deserialize)]
struct WttrResponse {
    current_condition: Vec<WttrCurrent>,
    nearest_area: Vec<WttrArea>,
    weather: Vec<WttrDay>,
}

#[derive(Debug, Deserialize)]
struct WttrValue {
    value: String,
}

#[derive(Debug, Deserialize)]
struct WttrCurrent {
    #[serde(rename = "temp_C")]
    temp_c: String,
    #[serde(rename = "weatherCode")]
    weather_code: String,
    #[serde(rename = "weatherDesc")]
    weather_desc: Vec<WttrValue>,
}

#[derive(Debug, Deserialize)]
struct WttrArea {
    #[serde(rename = "areaName")]
    area_name: Vec<WttrValue>,
    country: Vec<WttrValue>,
}

impl WttrArea {
    fn label(&self) -> String {
        let name = self
            .area_name
            .first()
            .map(|value| value.value.as_str())
            .filter(|value| !value.is_empty());
        let country = self
            .country
            .first()
            .map(|value| value.value.as_str())
            .filter(|value| !value.is_empty());
        match (name, country) {
            (Some(name), Some(country)) => format!("{name}, {country}"),
            (Some(name), None) => name.to_owned(),
            (None, Some(country)) => country.to_owned(),
            (None, None) => "Unknown location".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WttrDay {
    date: String,
    #[serde(rename = "maxtempC")]
    maxtemp_c: String,
    #[serde(rename = "mintempC")]
    mintemp_c: String,
    hourly: Vec<WttrHour>,
}

#[derive(Debug, Deserialize)]
struct WttrHour {
    time: String,
    #[serde(rename = "weatherCode")]
    weather_code: String,
    #[serde(rename = "weatherDesc")]
    weather_desc: Vec<WttrValue>,
}

impl WttrDay {
    /// wttr.in reports weather per 3-hour slot, not once per day, so the
    /// slot closest to noon is used as the representative condition for the
    /// whole day.
    fn forecast(&self) -> WeatherDay {
        let midday = self
            .hourly
            .iter()
            .find(|hour| hour.time == "1200")
            .or_else(|| self.hourly.get(self.hourly.len() / 2))
            .or_else(|| self.hourly.first());
        let (condition, description) =
            midday.map_or((WeatherCondition::Unknown, String::new()), |hour| {
                (
                    condition_from_code(&hour.weather_code),
                    hour.weather_desc
                        .first()
                        .map(|value| value.value.clone())
                        .unwrap_or_default(),
                )
            });
        WeatherDay {
            weekday: weekday_label(&self.date),
            date: self.date.clone(),
            max_c: self.maxtemp_c.parse().ok(),
            min_c: self.mintemp_c.parse().ok(),
            condition,
            description,
        }
    }
}

/// Maps a subset of World Weather Online condition codes (wttr.in's `j1`
/// `weatherCode`) to the broader `WeatherCondition` buckets. Unrecognized
/// codes fall back to `Unknown`.
fn condition_from_code(code: &str) -> WeatherCondition {
    match code.trim() {
        "113" => WeatherCondition::Clear,
        "116" => WeatherCondition::PartlyCloudy,
        "119" | "122" => WeatherCondition::Cloudy,
        "143" | "248" | "260" => WeatherCondition::Fog,
        "176" | "263" | "266" | "293" | "296" | "353" => WeatherCondition::Drizzle,
        "179" | "182" | "185" | "281" | "284" | "311" | "314" | "317" | "320" | "350" | "362"
        | "365" | "374" | "377" => WeatherCondition::Sleet,
        "200" | "386" | "389" | "392" | "395" => WeatherCondition::Thunder,
        "227" | "230" | "323" | "326" | "329" | "332" | "335" | "338" | "368" | "371" => {
            WeatherCondition::Snow
        }
        "299" | "302" | "305" | "308" | "356" | "359" => WeatherCondition::Rain,
        _ => WeatherCondition::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_condition_codes_and_falls_back_to_unknown() {
        assert_eq!(condition_from_code("113"), WeatherCondition::Clear);
        assert_eq!(condition_from_code("116"), WeatherCondition::PartlyCloudy);
        assert_eq!(condition_from_code("200"), WeatherCondition::Thunder);
        assert_eq!(condition_from_code("302"), WeatherCondition::Rain);
        assert_eq!(condition_from_code("329"), WeatherCondition::Snow);
        assert_eq!(condition_from_code("999999"), WeatherCondition::Unknown);
    }

    #[test]
    fn parses_a_full_wttr_response() {
        let body = r#"{
            "current_condition": [
                {"temp_C": "21", "weatherCode": "116", "weatherDesc": [{"value": "Partly cloudy"}]}
            ],
            "nearest_area": [
                {"areaName": [{"value": "Athens"}], "country": [{"value": "Greece"}]}
            ],
            "weather": [
                {
                    "date": "2026-08-03",
                    "maxtempC": "34",
                    "mintempC": "22",
                    "hourly": [
                        {"time": "0", "weatherCode": "113", "weatherDesc": [{"value": "Sunny"}]},
                        {"time": "1200", "weatherCode": "200", "weatherDesc": [{"value": "Thundery outbreaks"}]}
                    ]
                }
            ]
        }"#;
        let state = parse_response(body).unwrap();
        assert_eq!(state.location, "Athens, Greece");
        assert_eq!(state.current_c, 21);
        assert_eq!(state.condition, WeatherCondition::PartlyCloudy);
        assert_eq!(state.description, "Partly cloudy");
        // Only one real day was in the response; the rest is padded out to
        // a full week of placeholder cards.
        assert_eq!(state.days.len(), 7);
        let day = &state.days[0];
        assert_eq!(day.weekday, "Monday");
        assert_eq!(day.max_c, Some(34));
        assert_eq!(day.min_c, Some(22));
        // The midday (1200) slot should be picked over the first hourly entry.
        assert_eq!(day.condition, WeatherCondition::Thunder);

        let padded = &state.days[1];
        assert_eq!(padded.date, "2026-08-04");
        assert_eq!(padded.weekday, "Tuesday");
        assert_eq!(padded.max_c, None);
        assert_eq!(padded.condition, WeatherCondition::Unknown);
        assert_eq!(state.days[6].date, "2026-08-09");
    }
}
