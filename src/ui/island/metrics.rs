//! Design-time dimensions and the runtime `Metrics` that scales them for
//! one output.

use super::*;

use gtk::{gdk, prelude::MonitorExt};

use crate::ui::{automatic_scale, resolved_scale, scale_class, scaled};

#[derive(Debug, Clone, Copy)]
pub(super) struct Metrics {
    pub(super) scale: f64,
    pub(super) window_width: i32,
    pub(super) window_height: i32,
    pub(super) compact_width: i32,
    pub(super) compact_height: i32,
    pub(super) compact_min_width: i32,
    pub(super) compact_workspaces_max_width: i32,
    pub(super) compact_clock_max_width: i32,
    pub(super) compact_battery_max_width: i32,
    pub(super) compact_tray_max_width: i32,
    pub(super) tray_icon_size: i32,
    pub(super) media_max_width: i32,
    pub(super) media_height: i32,
    pub(super) dashboard_width: i32,
    pub(super) dashboard_height: i32,
    pub(super) osd_width: i32,
    pub(super) osd_height: i32,
    pub(super) notification_width: i32,
    pub(super) notification_height: i32,
    pub(super) search_width: i32,
    pub(super) search_height: i32,
    pub(super) search_y: i32,
    pub(super) weather_width: i32,
    pub(super) weather_height: i32,
}

impl Metrics {
    pub(super) fn new(
        monitor: &gdk::Monitor,
        configured_scale: f64,
        media_width_factor: f64,
    ) -> Self {
        let scale = resolved_scale(configured_scale, automatic_scale(monitor));
        let media_width_factor =
            media_width_factor.clamp(1.0, f64::from(DASHBOARD_WIDTH) / f64::from(COMPACT_WIDTH));
        let search_height = scaled(SEARCH_HEIGHT, scale);
        let monitor_height = monitor.geometry().height();
        let search_y = (f64::from(monitor_height) * 0.2).round() as i32;
        let search_y = search_y.min((monitor_height - search_height - scaled(20, scale)).max(0));
        Self {
            scale,
            window_width: scaled(WINDOW_WIDTH, scale),
            window_height: search_y + search_height,
            compact_width: scaled(COMPACT_WIDTH, scale),
            compact_height: scaled(COMPACT_HEIGHT, scale),
            compact_min_width: scaled(COMPACT_MIN_WIDTH, scale),
            compact_workspaces_max_width: scaled(COMPACT_WORKSPACES_MAX_WIDTH, scale),
            compact_clock_max_width: scaled(COMPACT_CLOCK_MAX_WIDTH, scale),
            compact_battery_max_width: scaled(COMPACT_BATTERY_MAX_WIDTH, scale),
            compact_tray_max_width: scaled(COMPACT_TRAY_MAX_WIDTH, scale),
            tray_icon_size: (f64::from(COMPACT_TRAY_ICON_SIZE) * COMPACT_TRAY_ICON_SCALE * scale)
                .round() as i32,
            media_max_width: (f64::from(COMPACT_WIDTH) * media_width_factor * scale).round() as i32,
            media_height: scaled(MEDIA_HEIGHT, scale),
            dashboard_width: scaled(DASHBOARD_WIDTH, scale),
            dashboard_height: scaled(DASHBOARD_HEIGHT, scale),
            osd_width: scaled(OSD_WIDTH, scale),
            osd_height: scaled(OSD_HEIGHT, scale),
            notification_width: scaled(NOTIFICATION_WIDTH, scale),
            notification_height: scaled(NOTIFICATION_HEIGHT, scale),
            search_width: scaled(SEARCH_WIDTH, scale),
            search_height,
            search_y,
            weather_width: scaled(WEATHER_WIDTH, scale),
            weather_height: scaled(WEATHER_HEIGHT, scale),
        }
    }

    pub(super) fn spacing(self, value: i32) -> i32 {
        scaled(value, self.scale)
    }

    pub(super) fn css_class(self) -> Option<&'static str> {
        scale_class(self.scale)
    }
}
