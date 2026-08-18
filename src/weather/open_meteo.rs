use anyhow::{Context, Result, bail};
use log::warn;
use serde::Deserialize;

use crate::http;
use crate::state::{WeatherCondition, WeatherDay, WeatherState};

use super::{pad_forecast, url_encode, weekday_label};

/// Free, keyless IP geolocation used to resolve coordinates for the forecast
/// request, matching wttr.in's "no configuration needed" behavior.
const GEO_URL: &str = "https://ipwho.is/";

/// Resolves a configured city name to coordinates; no API key required.
const GEOCODING_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";

const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";

pub(super) fn fetch(city: Option<&str>) -> Result<WeatherState> {
    let geo = match city {
        Some(city) => match geocode(city) {
            Ok(geo) => geo,
            Err(error) => {
                warn!(
                    "open-meteo geocoding failed for {city:?}, falling back to IP geolocation: \
                     {error:#}"
                );
                ip_geocode()?
            }
        },
        None => ip_geocode()?,
    };
    let url = format!(
        "{FORECAST_URL}?latitude={lat}&longitude={lon}\
         &current=temperature_2m,weather_code\
         &daily=weather_code,temperature_2m_max,temperature_2m_min\
         &timezone=auto&forecast_days=7",
        lat = geo.latitude,
        lon = geo.longitude,
    );
    let body = http::get(&url)?;
    parse_response(&body, &geo)
}

fn ip_geocode() -> Result<GeoResponse> {
    let geo: GeoResponse = http::get_json(GEO_URL)?;
    if !geo.success {
        bail!("IP geolocation failed");
    }
    Ok(geo)
}

fn geocode(city: &str) -> Result<GeoResponse> {
    let url = format!(
        "{GEOCODING_URL}?name={}&count=1&language=en&format=json",
        url_encode(city)
    );
    let response: GeocodingResponse = http::get_json(&url)?;
    let result = response.results.first().context("no matching city found")?;
    Ok(GeoResponse {
        success: true,
        latitude: result.latitude,
        longitude: result.longitude,
        city: result.name.clone(),
        country: result.country.clone().unwrap_or_default(),
    })
}

fn parse_response(body: &str, geo: &GeoResponse) -> Result<WeatherState> {
    let response: ForecastResponse =
        serde_json::from_str(body).context("failed to parse open-meteo response")?;
    let current = response
        .current
        .context("open-meteo response had no current conditions")?;
    let (condition, description) = match current.weather_code {
        Some(code) => {
            let (condition, description) = describe(code);
            (condition, description.to_owned())
        }
        None => (WeatherCondition::Unknown, String::new()),
    };
    let mut days: Vec<WeatherDay> = response
        .daily
        .time
        .iter()
        .zip(&response.daily.weather_code)
        .zip(&response.daily.temperature_2m_max)
        .zip(&response.daily.temperature_2m_min)
        .map(|(((date, code), max_c), min_c)| {
            let (condition, description) = describe(*code);
            WeatherDay {
                weekday: weekday_label(date),
                date: date.clone(),
                max_c: (*max_c).map(|value| value.round() as i32),
                min_c: (*min_c).map(|value| value.round() as i32),
                condition,
                description: description.to_owned(),
            }
        })
        .collect();
    pad_forecast(&mut days);
    Ok(WeatherState {
        provider: super::WeatherProvider::OpenMeteo,
        location: geo.label(),
        current_c: current
            .temperature_2m
            .map(|value| value.round() as i32)
            .unwrap_or_default(),
        condition,
        description: description.to_owned(),
        days,
    })
}

/// ipwho.is' geolocation payload; only the fields the forecast needs.
#[derive(Debug, Deserialize)]
struct GeoResponse {
    success: bool,
    latitude: f64,
    longitude: f64,
    city: String,
    country: String,
}

#[derive(Debug, Deserialize)]
struct GeocodingResponse {
    results: Vec<GeocodingResult>,
}

#[derive(Debug, Deserialize)]
struct GeocodingResult {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
}

impl GeoResponse {
    fn label(&self) -> String {
        match (self.city.as_str(), self.country.as_str()) {
            (city, country) if !city.is_empty() && !country.is_empty() => {
                format!("{city}, {country}")
            }
            (city, _) if !city.is_empty() => city.to_owned(),
            (_, country) if !country.is_empty() => country.to_owned(),
            _ => "Unknown location".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ForecastResponse {
    current: Option<CurrentConditions>,
    daily: DailyForecast,
}

#[derive(Debug, Deserialize)]
struct CurrentConditions {
    temperature_2m: Option<f64>,
    weather_code: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct DailyForecast {
    time: Vec<String>,
    weather_code: Vec<u16>,
    temperature_2m_max: Vec<Option<f64>>,
    temperature_2m_min: Vec<Option<f64>>,
}

/// Maps Open-Meteo's WMO weather codes to the `WeatherCondition` buckets and
/// a short human-readable description. Unrecognized codes fall back to
/// `Unknown`.
fn describe(code: u16) -> (WeatherCondition, &'static str) {
    match code {
        0 => (WeatherCondition::Clear, "Clear sky"),
        1 | 2 => (WeatherCondition::PartlyCloudy, "Partly cloudy"),
        3 => (WeatherCondition::Cloudy, "Overcast"),
        45 | 48 => (WeatherCondition::Fog, "Fog"),
        51..=57 => (WeatherCondition::Drizzle, "Drizzle"),
        61..=67 => (WeatherCondition::Rain, "Rain"),
        71..=77 => (WeatherCondition::Snow, "Snow"),
        80..=82 => (WeatherCondition::Rain, "Rain showers"),
        85..=86 => (WeatherCondition::Snow, "Snow showers"),
        95..=99 => (WeatherCondition::Thunder, "Thunderstorm"),
        _ => (WeatherCondition::Unknown, "Unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_wmo_codes_to_conditions() {
        assert_eq!(describe(0).0, WeatherCondition::Clear);
        assert_eq!(describe(2).0, WeatherCondition::PartlyCloudy);
        assert_eq!(describe(3).0, WeatherCondition::Cloudy);
        assert_eq!(describe(45).0, WeatherCondition::Fog);
        assert_eq!(describe(55).0, WeatherCondition::Drizzle);
        assert_eq!(describe(65).0, WeatherCondition::Rain);
        assert_eq!(describe(75).0, WeatherCondition::Snow);
        assert_eq!(describe(81).0, WeatherCondition::Rain);
        assert_eq!(describe(95).0, WeatherCondition::Thunder);
        assert_eq!(describe(999).0, WeatherCondition::Unknown);
    }

    #[test]
    fn parses_a_full_open_meteo_response() {
        let body = r#"{
            "latitude": 37.98,
            "longitude": 23.72,
            "current": {
                "time": "2026-08-04T09:15",
                "temperature_2m": 27.1,
                "weather_code": 2
            },
            "daily": {
                "time": ["2026-08-04", "2026-08-05"],
                "weather_code": [2, 61],
                "temperature_2m_max": [33.2, 28.6],
                "temperature_2m_min": [22.4, 19.8]
            }
        }"#;
        let geo = GeoResponse {
            success: true,
            latitude: 37.98,
            longitude: 23.72,
            city: "Athens".to_owned(),
            country: "Greece".to_owned(),
        };
        let state = parse_response(body, &geo).unwrap();
        assert_eq!(state.provider, crate::weather::WeatherProvider::OpenMeteo);
        assert_eq!(state.location, "Athens, Greece");
        assert_eq!(state.current_c, 27);
        assert_eq!(state.condition, WeatherCondition::PartlyCloudy);
        assert_eq!(state.description, "Partly cloudy");
        // Two real days in the response; the rest is padded out to a full
        // week of placeholder cards.
        assert_eq!(state.days.len(), 7);
        let day = &state.days[0];
        assert_eq!(day.weekday, "Tuesday");
        assert_eq!(day.date, "2026-08-04");
        assert_eq!(day.max_c, Some(33));
        assert_eq!(day.min_c, Some(22));
        assert_eq!(day.condition, WeatherCondition::PartlyCloudy);

        let rainy = &state.days[1];
        assert_eq!(rainy.condition, WeatherCondition::Rain);
        assert_eq!(rainy.description, "Rain");
        assert_eq!(state.days[6].date, "2026-08-10");
    }

    #[test]
    fn tolerates_missing_temperatures_and_current() {
        let body = r#"{
            "current": {"time": "2026-08-04T09:15", "temperature_2m": null, "weather_code": null},
            "daily": {
                "time": ["2026-08-04"],
                "weather_code": [0],
                "temperature_2m_max": [null],
                "temperature_2m_min": [null]
            }
        }"#;
        let geo = GeoResponse {
            success: true,
            latitude: 0.0,
            longitude: 0.0,
            city: String::new(),
            country: String::new(),
        };
        let state = parse_response(body, &geo).unwrap();
        assert_eq!(state.current_c, 0);
        assert_eq!(state.condition, WeatherCondition::Unknown);
        assert_eq!(state.location, "Unknown location");
        assert_eq!(state.days[0].max_c, None);
        assert_eq!(state.days[0].condition, WeatherCondition::Clear);
    }

    #[test]
    fn labels_location_from_geolocation() {
        let geo = GeoResponse {
            success: true,
            latitude: 0.0,
            longitude: 0.0,
            city: "Athens".to_owned(),
            country: String::new(),
        };
        assert_eq!(geo.label(), "Athens");
    }

    #[test]
    fn parses_geocoding_results() {
        let body = r#"{
            "results": [
                {"name": "London", "latitude": 51.50853, "longitude": -0.12574,
                 "country": "United Kingdom"}
            ],
            "generationtime_ms": 0.6
        }"#;
        let response: GeocodingResponse = serde_json::from_str(body).unwrap();
        let result = response.results.first().unwrap();
        assert_eq!(result.name, "London");
        assert_eq!(result.country.as_deref(), Some("United Kingdom"));
        assert_eq!(result.latitude, 51.50853);
    }
}
