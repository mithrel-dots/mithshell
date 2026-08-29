//! The weather forecast view, including the placeholder pixel-art icons.

use super::*;

use gtk::{Align, Orientation};

use super::{IslandWindow, Metrics};
use crate::state::{WeatherCondition, WeatherDay, WeatherState};
use crate::weather::WeatherProvider;

pub(super) struct WeatherWidgets {
    pub(super) root: gtk::Box,
    pub(super) back_button: gtk::Button,
    pub(super) eyebrow: gtk::Label,
    pub(super) location: gtk::Label,
    pub(super) hero_icon: gtk::DrawingArea,
    pub(super) hero_temp: gtk::Label,
    pub(super) hero_description: gtk::Label,
    pub(super) status: gtk::Label,
    pub(super) forecast_row: gtk::Box,
}

pub(super) fn weather_provider_label(prefix: &str, provider: WeatherProvider) -> String {
    format!("{prefix}  //  {}", provider.name().to_ascii_uppercase())
}

pub(super) fn weather_view(metrics: Metrics, provider: WeatherProvider) -> WeatherWidgets {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(7));
    root.set_size_request(metrics.weather_width, metrics.weather_height);
    root.add_css_class("weather-content");
    root.set_valign(Align::Start);

    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    let back_button = icon::icon_button(Icon::Back, metrics.icons);
    back_button.add_css_class("close-button");
    back_button.set_tooltip_text(Some("Back to dashboard"));
    back_button.set_valign(Align::Center);

    let heading = gtk::Box::new(Orientation::Vertical, 0);
    heading.set_hexpand(true);
    heading.set_valign(Align::Center);
    let eyebrow = gtk::Label::new(Some(&weather_provider_label("WEATHER", provider)));
    eyebrow.add_css_class("eyebrow");
    eyebrow.set_halign(Align::Start);
    let location = gtk::Label::new(Some("Locating..."));
    location.add_css_class("weather-location");
    location.set_halign(Align::Start);
    location.set_ellipsize(gtk::pango::EllipsizeMode::End);
    heading.append(&eyebrow);
    heading.append(&location);

    let hero_icon = weather_icon_area("weather-hero-icon", metrics.spacing(44));
    let hero_temp = gtk::Label::new(Some("--°"));
    hero_temp.add_css_class("weather-hero-temp");
    hero_temp.set_valign(Align::Center);

    header.append(&back_button);
    header.append(&heading);
    header.append(&hero_icon);
    header.append(&hero_temp);
    root.append(&header);

    let hero_description = gtk::Label::new(Some("Waiting for the forecast"));
    hero_description.add_css_class("weather-description");
    hero_description.set_halign(Align::Start);
    let status = gtk::Label::new(Some("FETCHING FORECAST"));
    status.add_css_class("search-status");
    status.set_halign(Align::Start);
    let hero_meta = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    hero_meta.append(&hero_description);
    hero_meta.append(&status);
    root.append(&hero_meta);

    let forecast_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    forecast_row.set_homogeneous(true);
    forecast_row.set_hexpand(true);
    root.append(&forecast_row);

    let calendar = gtk::Calendar::new();
    calendar.set_vexpand(false);
    calendar.add_css_class("weather-calendar");
    calendar.set_hexpand(true);
    calendar.set_vexpand(true);
    root.append(&calendar);

    WeatherWidgets {
        root,
        back_button,
        eyebrow,
        location,
        hero_icon,
        hero_temp,
        hero_description,
        status,
        forecast_row,
    }
}

/// Builds a card for one day of the forecast: weekday, a placeholder
/// condition icon, and the high/low temperatures.
fn weather_day_card(day: &WeatherDay, metrics: Metrics) -> (gtk::Box, gtk::DrawingArea) {
    let card = gtk::Box::new(Orientation::Vertical, metrics.spacing(4));
    card.add_css_class("weather-day-card");
    card.set_halign(Align::Fill);
    card.set_hexpand(true);

    let weekday = gtk::Label::new(Some(short_weekday(&day.weekday)));
    weekday.add_css_class("weather-day-label");

    let icon = weather_icon_area("weather-day-icon", metrics.spacing(24));
    draw_weather_condition(&icon, day.condition);

    let high = gtk::Label::new(Some(&format_temp(day.max_c)));
    high.add_css_class("weather-day-high");
    let low = gtk::Label::new(Some(&format_temp(day.min_c)));
    low.add_css_class("weather-day-low");

    card.append(&weekday);
    card.append(&icon);
    card.append(&high);
    card.append(&low);
    (card, icon)
}

/// Formats a forecast temperature, or `--°` for padded placeholder days
/// that have no real reading (see `weather::pad_forecast`).
fn format_temp(value: Option<i32>) -> String {
    value.map_or_else(|| "--°".to_owned(), |value| format!("{value}°"))
}

/// First three letters of a weekday name (all of `weekday_label`'s output
/// is ASCII, so byte slicing is safe).
fn short_weekday(weekday: &str) -> &str {
    &weekday[..weekday.len().min(3)]
}

/// Bare drawing area for a placeholder weather icon; the actual pixels are
/// (re)drawn by `draw_weather_condition`.
fn weather_icon_area(css_class: &str, size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("weather-icon");
    area.add_css_class(css_class);
    area.set_content_width(size);
    area.set_content_height(size);
    area
}

/// 8x8 placeholder pixel-art grids, one per `WeatherCondition`. Cell values
/// select which looked-up theme color fills that pixel: 0 is empty, 1 is the
/// warm accent (`ms_tertiary`), 2 is the cloud body (`ms_on_surface_variant`),
/// 3 is a fine outline/fog tone (`ms_outline`), and 4 is the cool accent
/// (`ms_primary`) used for rain/snow marks. These are intentionally simple,
/// blocky placeholders rather than a polished icon set.
type PixelGrid = [[u8; 8]; 8];

const GRID_CLEAR: PixelGrid = [
    [0, 0, 1, 0, 0, 1, 0, 0],
    [0, 0, 0, 1, 1, 0, 0, 0],
    [1, 0, 1, 1, 1, 1, 0, 1],
    [0, 1, 1, 1, 1, 1, 1, 0],
    [0, 1, 1, 1, 1, 1, 1, 0],
    [1, 0, 1, 1, 1, 1, 0, 1],
    [0, 0, 0, 1, 1, 0, 0, 0],
    [0, 0, 1, 0, 0, 1, 0, 0],
];

const GRID_PARTLY_CLOUDY: PixelGrid = [
    [0, 1, 1, 0, 0, 0, 0, 0],
    [1, 1, 1, 1, 0, 0, 0, 0],
    [0, 1, 1, 0, 0, 2, 2, 0],
    [0, 0, 0, 2, 2, 2, 2, 2],
    [0, 0, 2, 2, 2, 2, 2, 2],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_CLOUDY: PixelGrid = [
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 2, 2, 2, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [2, 2, 2, 2, 2, 2, 2, 2],
    [2, 2, 2, 2, 2, 2, 2, 2],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_FOG: PixelGrid = [
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 3, 3, 3, 3, 3, 0, 0],
    [3, 3, 3, 3, 3, 3, 3, 0],
    [0, 3, 3, 3, 3, 3, 0, 0],
    [3, 3, 3, 3, 3, 3, 3, 0],
    [0, 3, 3, 3, 3, 3, 0, 0],
    [3, 3, 3, 3, 3, 3, 3, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_DRIZZLE: PixelGrid = [
    [0, 0, 2, 2, 2, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [2, 2, 2, 2, 2, 2, 2, 2],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [0, 0, 4, 0, 4, 0, 4, 0],
    [0, 0, 0, 4, 0, 4, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_RAIN: PixelGrid = [
    [0, 0, 2, 2, 2, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [2, 2, 2, 2, 2, 2, 2, 2],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [0, 4, 0, 4, 0, 4, 0, 4],
    [4, 0, 4, 0, 4, 0, 4, 0],
    [0, 4, 0, 4, 0, 4, 0, 4],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_SLEET: PixelGrid = [
    [0, 0, 2, 2, 2, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [2, 2, 2, 2, 2, 2, 2, 2],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [0, 4, 0, 1, 0, 4, 0, 1],
    [0, 0, 1, 0, 4, 0, 1, 0],
    [0, 1, 0, 4, 0, 1, 0, 4],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_SNOW: PixelGrid = [
    [0, 0, 2, 2, 2, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [2, 2, 2, 2, 2, 2, 2, 2],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [0, 1, 0, 1, 0, 1, 0, 1],
    [0, 0, 1, 0, 1, 0, 1, 0],
    [0, 1, 0, 1, 0, 1, 0, 1],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_THUNDER: PixelGrid = [
    [0, 0, 2, 2, 2, 0, 0, 0],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [2, 2, 2, 2, 2, 2, 2, 2],
    [0, 2, 2, 2, 2, 2, 2, 0],
    [0, 0, 0, 1, 1, 0, 0, 0],
    [0, 0, 1, 1, 0, 0, 0, 0],
    [0, 0, 0, 1, 1, 0, 0, 0],
    [0, 0, 0, 0, 1, 0, 0, 0],
];

const GRID_WIND: PixelGrid = [
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 3, 3, 3, 3, 3, 0, 0],
    [0, 0, 0, 0, 0, 0, 3, 0],
    [3, 3, 3, 3, 3, 3, 0, 0],
    [0, 0, 0, 3, 0, 0, 0, 0],
    [0, 3, 3, 3, 3, 3, 3, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

const GRID_UNKNOWN: PixelGrid = [
    [0, 0, 3, 3, 3, 0, 0, 0],
    [0, 3, 0, 0, 0, 3, 0, 0],
    [0, 0, 0, 0, 0, 3, 0, 0],
    [0, 0, 0, 3, 3, 0, 0, 0],
    [0, 0, 0, 3, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 3, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0],
];

fn weather_pixel_grid(condition: WeatherCondition) -> &'static PixelGrid {
    match condition {
        WeatherCondition::Clear => &GRID_CLEAR,
        WeatherCondition::PartlyCloudy => &GRID_PARTLY_CLOUDY,
        WeatherCondition::Cloudy => &GRID_CLOUDY,
        WeatherCondition::Fog => &GRID_FOG,
        WeatherCondition::Drizzle => &GRID_DRIZZLE,
        WeatherCondition::Rain => &GRID_RAIN,
        WeatherCondition::Sleet => &GRID_SLEET,
        WeatherCondition::Snow => &GRID_SNOW,
        WeatherCondition::Thunder => &GRID_THUNDER,
        WeatherCondition::Wind => &GRID_WIND,
        WeatherCondition::Unknown => &GRID_UNKNOWN,
    }
}

/// Sets (or replaces) `area`'s draw function to render `condition`'s
/// placeholder pixel-art icon, recolored from the active theme's
/// `@ms_*` colors each time it draws.
///
/// `StyleContext::lookup_color` has been deprecated since GTK 4.10 with no
/// direct replacement for resolving a named color to RGBA outside of a
/// stylesheet; it remains the only way to recolor custom Cairo drawing from
/// the active theme, and continues to function correctly (see the same
/// rationale in `theme::generate_gtk_from_style_context`).
#[allow(deprecated)]
pub(super) fn draw_weather_condition(area: &gtk::DrawingArea, condition: WeatherCondition) {
    area.set_draw_func(move |area, context, width, height| {
        let style = area.style_context();
        let lookup = |name: &str, fallback: (f64, f64, f64, f64)| -> (f64, f64, f64, f64) {
            style.lookup_color(name).map_or(fallback, |rgba| {
                (
                    f64::from(rgba.red()),
                    f64::from(rgba.green()),
                    f64::from(rgba.blue()),
                    f64::from(rgba.alpha()),
                )
            })
        };
        let accent = lookup("ms_tertiary", (0.98, 0.85, 0.45, 1.0));
        let body = lookup("ms_on_surface_variant", (0.68, 0.68, 0.74, 1.0));
        let outline = lookup("ms_outline", (0.5, 0.5, 0.56, 1.0));
        let cool = lookup("ms_primary", (0.4, 0.6, 0.92, 1.0));

        let size = f64::from(width.min(height));
        let cell = size / 8.0;
        let offset_x = (f64::from(width) - size) / 2.0;
        let offset_y = (f64::from(height) - size) / 2.0;
        for (row, cells) in weather_pixel_grid(condition).iter().enumerate() {
            for (col, value) in cells.iter().enumerate() {
                let color = match value {
                    1 => accent,
                    2 => body,
                    3 => outline,
                    4 => cool,
                    _ => continue,
                };
                context.set_source_rgba(color.0, color.1, color.2, color.3);
                context.rectangle(
                    offset_x + col as f64 * cell,
                    offset_y + row as f64 * cell,
                    cell.ceil(),
                    cell.ceil(),
                );
                let _ = context.fill();
            }
        }
    });
    area.queue_draw();
}

impl IslandWindow {
    /// Renders a forecast pushed by `Controller::attach_weather`, or `None`
    /// when a fetch failed and there is nothing cached yet.
    pub fn update_weather(&self, state: Option<&WeatherState>) {
        let Some(state) = state else {
            if self.latest_weather.borrow().is_none() {
                self.weather_status.set_label("WEATHER UNAVAILABLE");
            }
            return;
        };
        self.weather_location.set_label(&state.location);
        self.weather_eyebrow
            .set_label(&weather_provider_label("WEATHER", state.provider));
        self.weather_hero_temp
            .set_label(&format!("{}°", state.current_c));
        self.weather_hero_description.set_label(&state.description);
        draw_weather_condition(&self.weather_hero_icon, state.condition);
        self.weather_status
            .set_label(&weather_provider_label("UPDATED", state.provider));

        clear_box(&self.weather_forecast_row);
        let mut icons = vec![self.weather_hero_icon.clone()];
        for day in &state.days {
            let (card, icon) = weather_day_card(day, self.metrics);
            icons.push(icon);
            self.weather_forecast_row.append(&card);
        }
        *self.weather_icons.borrow_mut() = icons;
        *self.latest_weather.borrow_mut() = Some(state.clone());
    }
}
