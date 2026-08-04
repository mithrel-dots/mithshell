use std::{thread, time::Duration};

use anyhow::Result;
use async_channel::Sender;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::state::{WeatherCondition, WeatherDay, WeatherState};

mod open_meteo;
mod wttr;

const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Which upstream forecast service the weather tile polls.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WeatherProvider {
    #[default]
    Wttr,
    OpenMeteo,
}

impl WeatherProvider {
    fn name(self) -> &'static str {
        match self {
            Self::Wttr => "wttr.in",
            Self::OpenMeteo => "open-meteo.com",
        }
    }

    fn fetch(self, city: Option<&str>) -> Result<WeatherState> {
        match self {
            Self::Wttr => wttr::fetch(city),
            Self::OpenMeteo => open_meteo::fetch(city),
        }
    }
}

/// Spawns a background thread that fetches the forecast from `provider` on
/// an interval and pushes each successful result down `sender`. Failures are
/// logged and retried sooner than the normal interval, without ever sending
/// a value.
pub fn start_poller(
    provider: WeatherProvider,
    city: Option<String>,
    sender: Sender<WeatherState>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let city = city
            .as_deref()
            .map(str::trim)
            .filter(|city| !city.is_empty());
        loop {
            let sleep_for = match provider.fetch(city) {
                Ok(state) => {
                    if sender.send_blocking(state).is_err() {
                        return;
                    }
                    POLL_INTERVAL
                }
                Err(error) => {
                    warn!(
                        "failed to fetch weather from {}: {error:#}",
                        provider.name()
                    );
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

/// Julian day number of a proleptic Gregorian date, per Richards' algorithm.
fn to_julian_day(year: i32, month: u32, day: u32) -> i64 {
    let a = (14 - i64::from(month)) / 12;
    let y = i64::from(year) + 4800 - a;
    let m = i64::from(month) + 12 * a - 3;
    i64::from(day) + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045
}

/// Percent-encodes a location name for use in a URL path or query string.
fn url_encode(value: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
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

/// Adds `delta` days to an ISO `YYYY-MM-DD` date via a Julian day round
/// trip, so month/year rollovers are handled without a date/time dependency.
fn add_days(date: &str, delta: i64) -> Option<String> {
    let mut parts = date.splitn(3, '-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    let (year, month, day) = from_julian_day(to_julian_day(year, month, day) + delta);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Full weekday name for an ISO `YYYY-MM-DD` date, computed with Sakamoto's
/// algorithm rather than pulling in a date/time dependency.
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

/// Some providers report fewer than seven days of forecast, but the dashboard
/// shows a fixed seven-day grid. The remaining slots are padded with `Unknown`
/// placeholder cards (no temperature guess) dated forward from the last real
/// day.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_known_weekdays() {
        assert_eq!(weekday_label("2024-01-01"), "Monday");
        assert_eq!(weekday_label("2000-01-01"), "Saturday");
        assert_eq!(weekday_label("2026-08-03"), "Monday");
        assert_eq!(weekday_label("not-a-date"), "");
    }

    #[test]
    fn julian_day_round_trips() {
        for (year, month, day) in [
            (2026, 8, 3),
            (2026, 1, 31),
            (2026, 12, 31),
            (2024, 2, 29),
            (1970, 1, 1),
            (2100, 6, 15),
        ] {
            assert_eq!(
                from_julian_day(to_julian_day(year, month, day)),
                (year, month, day)
            );
        }
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
    fn percent_encodes_location_names() {
        assert_eq!(url_encode("London"), "London");
        assert_eq!(url_encode("New York"), "New%20York");
        assert_eq!(url_encode("København"), "K%C3%B8benhavn");
        assert_eq!(url_encode("a/b"), "a%2Fb");
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
