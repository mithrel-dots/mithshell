//! The dynamic island: a layer-shell window that morphs between the compact
//! pill, dashboard, weather, OSD, notifications, and an optional integrated launcher.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

use gtk::{ApplicationWindow, Fixed, Orientation, Overflow, gdk::prelude::*, glib, prelude::*};
use gtk4_layer_shell::LayerShell;

mod actions;
mod battery_wave;
pub(crate) mod circle;
mod circle_integration;
mod compact;
mod dashboard;
mod interactions;
mod media;
pub(crate) mod media_circle;
mod metrics;
mod notification;
pub(crate) mod notification_circle;
mod osd;
mod search;
mod tray;
pub(crate) mod tray_circle;
mod view;
mod weather;
mod window;

#[cfg(test)]
mod tests;

pub use actions::IslandActions;
use actions::OverlayButtons;
use metrics::Metrics;
use notification::{CurrentNotification, NotificationToasts, PendingNotification, PillOverlay};

use crate::config::{IconStyle, LauncherPresentation, NotificationConfig};
use crate::media::VisualizerLevels;
use crate::state::{HyprlandSnapshot, MediaState, WeatherState};
use crate::tarragon::{TarragonSnapshot, TarragonStatus};
use crate::ui::icon::{self, Icon};

const WINDOW_WIDTH: i32 = 860;
// A stable native canvas avoids compositor squash/stretch on hover and stale
// rectangles during launcher animations. Animate only the child widgets.
const WINDOW_HEIGHT: i32 = 900;
const COMPACT_WIDTH: i32 = 224;
const COMPACT_HEIGHT: i32 = 32;
/// Minimum content-driven width, even when only the clock is visible.
const COMPACT_MIN_WIDTH: i32 = 128;
/// Maximum tray contribution to the legacy media pill's width solver.
const COMPACT_TRAY_MAX_WIDTH: i32 = 120;
const COMPACT_TRAY_ICON_SIZE: i32 = 16;
const COMPACT_TRAY_ICON_SCALE: f64 = 0.9;
const MEDIA_HEIGHT: i32 = 32;
const DASHBOARD_WIDTH: i32 = 448;
const DASHBOARD_HEIGHT: i32 = 400;
/// Header depth and click-to-close hit zone, ending before the status strip.
const DASHBOARD_HEADER_HEIGHT: i32 = 44;
const OSD_WIDTH: i32 = 292;
const OSD_HEIGHT: i32 = 36;
const NOTIFICATION_WIDTH: i32 = 280;
const NOTIFICATION_HEIGHT: i32 = 36;
const SEARCH_WIDTH: i32 = 820;
const SEARCH_HEIGHT: i32 = 620;
/// Transparent room preventing the independent launcher's shadow from clipping.
const SEARCH_SHADOW_MARGIN: i32 = 56;
const WEATHER_WIDTH: i32 = 380;
const WEATHER_HEIGHT: i32 = 390;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Compact,
    Media,
    Dashboard,
    Weather,
    Osd,
    /// Only used with `notifications.position = "pill"`.
    Notification,
    /// Only used with the integrated launcher presentation.
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Geometry {
    width: f64,
    height: f64,
    y: f64,
}

impl Geometry {
    fn for_view(view: View, metrics: Metrics, media_width: i32, compact_width: i32) -> Self {
        let (width, height) = match view {
            View::Compact => (compact_width, metrics.compact_height),
            View::Media => (media_width, metrics.media_height),
            View::Dashboard => (metrics.dashboard_width, metrics.dashboard_height),
            View::Weather => (metrics.weather_width, metrics.weather_height),
            View::Osd => (metrics.osd_width, metrics.osd_height),
            View::Notification => (metrics.notification_width, metrics.notification_height),
            View::Search => (metrics.search_width, metrics.search_height),
        };
        Self {
            width: f64::from(width),
            height: f64::from(height),
            y: if view == View::Search {
                f64::from(metrics.search_y)
            } else {
                0.0
            },
        }
    }

    fn interpolate(self, target: Self, progress: f64) -> Self {
        Self {
            width: lerp(self.width, target.width, progress),
            height: lerp(self.height, target.height, progress),
            y: lerp(self.y, target.y, progress),
        }
    }
}

pub struct IslandWindow {
    monitor_name: String,
    metrics: Metrics,
    window: ApplicationWindow,
    /// Production uses the layer window; Broadway tests need a normal focus root.
    focus_root: RefCell<gtk::Widget>,
    search_window: ApplicationWindow,
    search_fixed: Fixed,
    search_surface: gtk::ScrolledWindow,
    dismiss_window: ApplicationWindow,
    /// Full-size pick target for the mapped outside-click layer surface.
    dismiss_area: gtk::Box,
    dismiss_click: RefCell<Option<gtk::GestureClick>>,
    fixed: Fixed,
    /// Stable hover target whose allocation is independent of the animated pill.
    content: Fixed,
    /// Rounded clip/fill containing the battery background and scrolling foreground.
    surface_shell: gtk::Overlay,
    surface: gtk::ScrolledWindow,
    compact: gtk::Widget,
    media: gtk::Overlay,
    dashboard: gtk::Box,
    dashboard_sections: Vec<dashboard::DashboardSection>,
    dashboard_expansion: Cell<f64>,
    dashboard_section_opacity: Cell<f64>,
    panel_scroll: gtk::ScrolledWindow,
    notification_scroll: gtk::ScrolledWindow,
    search: gtk::Box,
    weather: gtk::Box,
    osd: gtk::Box,
    /// Only shown with `notifications.position = "pill"`.
    notification: gtk::Box,
    compact_workspaces: gtk::Box,
    compact_clock: gtk::Label,
    compact_date_day: gtk::Label,
    compact_date_rest: gtk::Label,
    compact_date: gtk::Box,
    hardware: dashboard::HardwarePanel,
    compact_battery: gtk::Label,
    battery_waves: battery_wave::BatteryWaves,
    compact_tray: gtk::Box,
    /// Natural idle width including nested spacing and CSS padding.
    compact_width: Cell<i32>,
    /// Enables the compact tray row while the pill is hovered.
    tray_hovered: Cell<bool>,
    /// Physical pointer state survives page switches that reset presentation depth.
    pointer_in_hover_region: Cell<bool>,
    tray_item_count: Cell<usize>,
    tray_circle_assigned: bool,
    /// Pins the tray while a popover's pointer grab produces a pill leave event.
    tray_menu_open: Cell<bool>,
    /// Shared by legacy and circle trays; owns window pinning across both.
    tray_menu_manager: Rc<tray::TrayMenuManager>,
    tray_menu_tracker: Rc<tray::TrayMenuTracker>,
    media_workspaces: gtk::Box,
    media_clock: gtk::Label,
    media_center: gtk::Box,
    media_icon: gtk::Image,
    media_title: gtk::Label,
    media_visualizer: gtk::DrawingArea,
    compact_visualizer: gtk::DrawingArea,
    compact_visualizer_revealer: gtk::Revealer,
    visualizer_enabled: bool,
    visualizer_active: Cell<bool>,
    visualizer_revision: Cell<u64>,
    media_levels: Rc<RefCell<VisualizerLevels>>,
    media_tray: gtk::Box,
    latest_media: RefCell<Option<MediaState>>,
    selected_media_service: RefCell<Option<String>>,
    uptime_value: gtk::Label,
    last_update_value: gtk::Label,
    active_eyebrow: gtk::Label,
    active_title: gtk::Label,
    workspace_row: gtk::FlowBox,
    volume_scale: gtk::Scale,
    volume_value: gtk::Label,
    mute_button: gtk::Button,
    notification_count: gtk::Label,
    notification_inhibit_remaining: gtk::Label,
    notification_clear_button: gtk::Button,
    notification_inhibit_button: gtk::ToggleButton,
    notification_list: gtk::Box,
    updating_notification_inhibit: Cell<bool>,
    search_entry: gtk::SearchEntry,
    search_results: gtk::ListBox,
    search_status: gtk::Label,
    search_stack: gtk::Stack,
    search_plugin_toggle: gtk::ToggleButton,
    search_plugins: gtk::ListBox,
    search_preview_stack: gtk::Stack,
    search_preview_picture: gtk::Picture,
    /// Single-child slot switching between a chrome glyph and a foreign image.
    search_preview_icon: gtk::Box,
    search_preview_title: gtk::Label,
    search_preview_description: gtk::Label,
    search_preview_file_meta: gtk::Label,
    search_preview_meta: gtk::Label,
    search_preview_text: gtk::TextView,
    search_preview_text_scroll: gtk::ScrolledWindow,
    search_preview_error: gtk::Label,
    osd_icon: gtk::Widget,
    osd_title: gtk::Label,
    osd_progress: gtk::ProgressBar,
    osd_value: gtk::Label,
    notification_icon: gtk::Image,
    notification_app: gtk::Label,
    notification_body: gtk::Label,
    weather_location: gtk::Label,
    weather_eyebrow: gtk::Label,
    weather_hero_icon: gtk::DrawingArea,
    weather_hero_temp: gtk::Label,
    weather_hero_description: gtk::Label,
    weather_status: gtk::Label,
    weather_forecast_row: gtk::Box,
    /// Retained so theme changes can redraw all forecast icons immediately.
    weather_icons: RefCell<Vec<gtk::DrawingArea>>,
    latest_weather: RefCell<Option<WeatherState>>,
    current_view: Cell<View>,
    dashboard_open: Cell<bool>,
    island_hovered: Cell<bool>,
    search_open: Cell<bool>,
    weather_open: Cell<bool>,
    search_connected: Cell<bool>,
    search_generation: Cell<u64>,
    /// Invalidates queued focus requests when a temporary page or close wins.
    search_focus_generation: Cell<u64>,
    search_focus_pending: Cell<bool>,
    preview_generation: Cell<u64>,
    search_action_generation: Cell<u64>,
    search_selection_pending: Cell<bool>,
    search_selection_keep_open: Cell<bool>,
    search_preview_key: RefCell<Option<String>>,
    /// Last dispatched query, so typing ahead does not discard in-flight results.
    search_dispatched: RefCell<Option<String>>,
    /// Timestamp for leading-edge query throttling.
    last_search_dispatch: Cell<Option<Instant>>,
    search_snapshot: RefCell<Option<TarragonSnapshot>>,
    search_backend_status: RefCell<Option<TarragonStatus>>,
    search_geometry: Cell<Geometry>,
    search_animation_generation: Cell<u64>,
    osd_active: Cell<bool>,
    media_playing: Cell<bool>,
    media_width: Cell<i32>,
    geometry: Cell<Geometry>,
    /// Independent of pill width updates so those cannot cancel a page close.
    view_animation_generation: Cell<u64>,
    pill_animation_generation: Cell<u64>,
    pill_animation_target: Cell<Option<Geometry>>,
    view_animation_target: Cell<Option<Geometry>>,
    /// Gives page transitions exclusive ownership of shared surface geometry.
    view_transition_active: Cell<bool>,
    animation_ms: Cell<u32>,
    motion: Cell<Option<crate::config::MotionConfig>>,
    motion_style: gtk::CssProvider,
    animations_enabled: Cell<bool>,
    launcher_presentation: LauncherPresentation,
    osd_generation: Cell<u64>,
    volume_generation: Cell<u64>,
    updating_controls: Cell<bool>,
    latest_hyprland: RefCell<HyprlandSnapshot>,
    notifications: NotificationConfig,
    /// Pending and current presentations for pill-position notifications.
    notification_queue: RefCell<VecDeque<PendingNotification>>,
    notification_current: RefCell<Option<CurrentNotification>>,
    notification_active: Cell<bool>,
    notification_generation: Cell<u64>,
    /// Used only by below-pill and corner positions.
    notification_toasts: Option<NotificationToasts>,
    pill_overlay: Option<PillOverlay>,
    actions: IslandActions,
    /// Optional circles share this window and the normal snapshot/action owners.
    pub(crate) circles: RefCell<Option<circle_integration::CircleIntegration>>,
}

impl IslandWindow {
    fn motion_profile(
        &self,
        transition: crate::ui::motion::Transition,
    ) -> crate::ui::motion::Profile {
        transition.resolve(
            self.motion.get(),
            self.animations_enabled.get(),
            self.animation_ms.get(),
        )
    }

    fn motion_enabled(&self) -> bool {
        self.animations_enabled.get()
            && self
                .motion
                .get()
                .map_or(self.animation_ms.get() > 0, |motion| {
                    motion.duration_scale > 0.0
                })
    }

    fn auxiliary_duration_ms(&self, default_ms: u32) -> u32 {
        crate::ui::motion::auxiliary_duration_ms(
            self.motion.get(),
            self.animations_enabled.get(),
            self.animation_ms.get(),
            default_ms,
        )
    }

    fn motion_delay(&self, duration: Duration) -> Duration {
        if !self.motion_enabled() {
            Duration::ZERO
        } else {
            crate::ui::motion::scale_duration(
                duration,
                self.motion
                    .get()
                    .map_or(1.0, |motion| motion.duration_scale),
            )
        }
    }
}

fn hover_geometry(base: Geometry, inset: f64, hovered: bool) -> Geometry {
    if hovered {
        Geometry {
            width: base.width + inset * 2.0,
            height: base.height + inset,
            y: base.y + inset / 2.0,
        }
    } else {
        base
    }
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn clear_list_box(container: &gtk::ListBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn measure_clamped<W: IsA<gtk::Widget>>(widget: &W, max_width: i32) -> i32 {
    let (_, natural, _, _) = widget.measure(Orientation::Horizontal, -1);
    natural.min(max_width)
}

fn lerp(start: f64, target: f64, progress: f64) -> f64 {
    start + (target - start) * progress
}

fn dominant_scroll_direction(dx: f64, dy: f64) -> i8 {
    let delta = if dy.abs() >= dx.abs() { dy } else { dx };
    if delta > 0.0 {
        1
    } else if delta < 0.0 {
        -1
    } else {
        0
    }
}
