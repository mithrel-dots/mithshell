use std::{process::Command, thread, time::Duration};

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use log::warn;
use serde::Deserialize;

use crate::state::{WeatherCondition, WeatherDay, WeatherState};

/// wttr.in's machine-readable `j1` format. No API key or location
/// configuration is required: without a path segment, wttr.in geolocates
/// the request by source IP.
const WTTR_URL: &str = "https://wttr.in/?format=j1";
const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Spawns a background thread that fetches the forecast from wttr.in on an
/// interval and pushes each successful result down `sender`. Follows the
/// same shape as `system::start_poller`: blocking I/O on a dedicated OS
/// thread, `send_blocking` into an `async_channel`. Failures (no network,
/// wttr.in unreachable, `curl` missing) are logged and retried sooner than
/// the normal interval, without ever sending a value.
pub fn start_poller(sender: Sender<WeatherState>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            let sleep_for = match fetch() {
                Ok(state) => {
                    if sender.send_blocking(state).is_err() {
                        return;
                    }
                    POLL_INTERVAL
                }
                Err(error) => {
                    warn!("failed to fetch weather from wttr.in: {error:#}");
                    RETRY_INTERVAL
                }
            };
            if sender.is_closed() {
                return;
            }
            thread::sleep(sleep_for);
        }
    })
}

/// Fetches and parses the current forecast. Shells out to `curl` rather
/// than adding an HTTP client dependency, following the same convention as
/// `cava`/`wpctl`/`pactl` elsewhere in this codebase.
pub fn fetch() -> Result<WeatherState> {
    let output = Command::new("curl")
        .args(["-fsS", "--max-time", "8", WTTR_URL])
        .output()
        .context("failed to run curl")?;
    if !output.status.success() {
        bail!("curl exited with {}", output.status);
    }
    let body = String::from_utf8(output.stdout).context("wttr.in response was not UTF-8")?;
    parse_response(&body)
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
    /// whole day's icon and summary.
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

/// wttr.in's free tier only reports three real days of forecast (today plus
/// two more; see `curl wttr.in/:help`), but the dashboard shows a fixed
/// seven-day grid. Rather than silently only filling in three cards, the
/// remaining slots are padded with clearly-marked placeholders: an
/// `Unknown` condition (rendered as a distinct "?" icon) and no temperature
/// guess, dated by walking the calendar forward from the last real day.
fn pad_forecast(days: &mut Vec<WeatherDay>) {
    let Some(mut date) = days.last().map(|day| day.date.clone()) else {
        return;
    };
    while days.len() < 7 {
        let Some(next) = add_days(&date, 1) else {
            break;
        };
        days.push(WeatherDay {
            weekday: weekday_label(&next),
            date: next.clone(),
            max_c: None,
            min_c: None,
            condition: WeatherCondition::Unknown,
            description: "No forecast data".to_owned(),
        });
        date = next;
    }
}

/// Adds `delta` days to an ISO `YYYY-MM-DD` date via a Julian day number
/// round trip, so month/year rollovers are handled without a date/time
/// dependency.
fn add_days(date: &str, delta: i64) -> Option<String> {
    let mut parts = date.splitn(3, '-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let (year, month, day) = from_julian_day(to_julian_day(year, month, day) + delta);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Richards' algorithm for converting a proleptic Gregorian date to a
/// Julian day number.
fn to_julian_day(year: i32, month: u32, day: u32) -> i64 {
    let a = (14 - i64::from(month)) / 12;
    let y = i64::from(year) + 4800 - a;
    let m = i64::from(month) + 12 * a - 3;
    i64::from(day) + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045
}

/// The inverse of `to_julian_day`.
fn from_julian_day(jdn: i64) -> (i32, u32, u32) {
    let a = jdn + 32044;
    let b = (4 * a + 3) / 146_097;
    let c = a - (146_097 * b) / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5 + 1) as u32;
    let month = (m + 3 - 12 * (m / 10)) as u32;
    let year = (100 * b + d - 4800 + m / 10) as i32;
    (year, month, day)
}

/// Maps a subset of World Weather Online condition codes (what wttr.in's
/// `j1` format reports `weatherCode` as) to the broader `WeatherCondition`
/// buckets used to pick a placeholder illustration. Unrecognized codes fall
/// back to `Unknown` rather than guessing.
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

/// Full weekday name for an ISO `YYYY-MM-DD` date, computed with Sakamoto's
/// algorithm rather than pulling in a date/time dependency for one lookup.
fn weekday_label(date: &str) -> String {
    const NAMES: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let mut parts = date.splitn(3, '-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return String::new();
    };
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<i32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return String::new();
    };
    NAMES
        .get(day_of_week(year, month, day) as usize)
        .map(|name| (*name).to_owned())
        .unwrap_or_default()
}

/// Sakamoto's algorithm. Returns 0 for Sunday through 6 for Saturday.
fn day_of_week(year: i32, month: u32, day: u32) -> i32 {
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut year = year;
    if month < 3 {
        year -= 1;
    }
    (year + year / 4 - year / 100 + year / 400 + OFFSETS[(month - 1) as usize] + day as i32)
        .rem_euclid(7)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_known_weekdays() {
        // 2024-01-01 was a Monday; 2000-01-01 was a Saturday.
        assert_eq!(weekday_label("2024-01-01"), "Monday");
        assert_eq!(weekday_label("2000-01-01"), "Saturday");
        assert_eq!(weekday_label("2026-08-03"), "Monday");
        assert_eq!(weekday_label("not-a-date"), "");
    }

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

    #[test]
    fn adds_days_across_month_and_year_boundaries() {
        assert_eq!(add_days("2026-08-03", 1).as_deref(), Some("2026-08-04"));
        assert_eq!(add_days("2026-01-31", 1).as_deref(), Some("2026-02-01"));
        assert_eq!(add_days("2026-12-31", 1).as_deref(), Some("2027-01-01"));
        // 2024 was a leap year.
        assert_eq!(add_days("2024-02-28", 1).as_deref(), Some("2024-02-29"));
        assert_eq!(add_days("not-a-date", 1), None);
    }

    #[test]
    fn pads_a_short_forecast_up_to_seven_days() {
        let mut days = vec![WeatherDay {
            date: "2026-08-03".to_owned(),
            weekday: weekday_label("2026-08-03"),
            max_c: Some(30),
            min_c: Some(20),
            condition: WeatherCondition::Clear,
            description: "Sunny".to_owned(),
        }];
        pad_forecast(&mut days);
        assert_eq!(days.len(), 7);
        assert_eq!(days[0].max_c, Some(30));
        assert!(days[1..].iter().all(|day| day.max_c.is_none()
            && day.min_c.is_none()
            && day.condition == WeatherCondition::Unknown));
        assert_eq!(days.last().unwrap().date, "2026-08-09");
    }
}
