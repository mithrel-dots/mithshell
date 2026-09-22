//! The dynamic island: a layer-shell window that morphs between the
//! compact pill and the dashboard, weather, OSD, and notification views,
//! alongside an independent launcher window.

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
#[allow(dead_code)] // consumed by central circle integration
pub(crate) mod tray_circle;
mod view;
mod weather;
mod window;

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
// Fixed backing canvas for every presentation, including the 820x620
// integrated launcher.  Keeping the layer window stable avoids stale opaque
// rectangles when a launcher closes after a geometry animation.
const WINDOW_HEIGHT: i32 = 900;
const COMPACT_WIDTH: i32 = 224;
const COMPACT_HEIGHT: i32 = 32;
/// Floor for the pill's content-driven width (`resize_compact`), so it
/// never shrinks to an oddly narrow sliver when nothing but the clock is
/// showing.
const COMPACT_MIN_WIDTH: i32 = 128;
/// Per-element caps `resize_compact` clamps each compact-pill child to
/// before summing them into the pill's width. Kept separate from a single
/// shared cap so a long workspace list can't crowd out the clock, etc.
const COMPACT_WORKSPACES_MAX_WIDTH: i32 = 110;
const COMPACT_CLOCK_MAX_WIDTH: i32 = 70;
const COMPACT_BATTERY_MAX_WIDTH: i32 = 50;
const COMPACT_TRAY_MAX_WIDTH: i32 = 120;
/// Rendered size of each tray icon, independent of `COMPACT_TRAY_MAX_WIDTH`
/// (which instead bounds how many icons fit before the row stops growing
/// and clips instead).
const COMPACT_TRAY_ICON_SIZE: i32 = 16;
const COMPACT_TRAY_ICON_SCALE: f64 = 0.9;
const MEDIA_HEIGHT: i32 = 32;
const DASHBOARD_WIDTH: i32 = 448;
const DASHBOARD_HEIGHT: i32 = 400;
/// Depth of the dashboard's header band, measured from the top of the
/// view. Doubles as the click-to-close hit zone, so it stops inside the gap
/// below the header: anything past it belongs to the status strip. The
/// clock's line height grows more slowly than the surface across the
/// density tiers, so this is sized against the tightest of them.
const DASHBOARD_HEADER_HEIGHT: i32 = 44;
const OSD_WIDTH: i32 = 292;
const OSD_HEIGHT: i32 = 36;
/// `pill`-position notification geometry: wide enough for an icon, summary
/// and a one-line body preview without wrapping in the common case.
const NOTIFICATION_WIDTH: i32 = 280;
const NOTIFICATION_HEIGHT: i32 = 36;

const SEARCH_WIDTH: i32 = 820;
const SEARCH_HEIGHT: i32 = 620;
/// Transparent room around the independent search surface for its CSS shadow.
/// Without it, the layer window clips the blur into faint square corner bands.
const SEARCH_SHADOW_MARGIN: i32 = 56;
// Kept comfortably under the main island canvas dimensions.
const WEATHER_WIDTH: i32 = 380;
const WEATHER_HEIGHT: i32 = 390;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Compact,
    Media,
    Dashboard,
    Weather,
    Osd,
    /// Only reachable when `notifications.position = "pill"`; the other
    /// positions render notifications in a separate popup window instead.
    Notification,
    /// The integrated launcher replaces the island's normal page in the same
    /// fixed backing canvas. Independent launchers do not enter this state.
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
        match view {
            View::Compact => Self {
                width: f64::from(compact_width),
                height: f64::from(metrics.compact_height),
                y: 0.0,
            },
            View::Media => Self {
                width: f64::from(media_width),
                height: f64::from(metrics.media_height),
                y: 0.0,
            },
            View::Dashboard => Self {
                width: f64::from(metrics.dashboard_width),
                height: f64::from(metrics.dashboard_height),
                y: 0.0,
            },
            View::Weather => Self {
                width: f64::from(metrics.weather_width),
                height: f64::from(metrics.weather_height),
                y: 0.0,
            },
            View::Osd => Self {
                width: f64::from(metrics.osd_width),
                height: f64::from(metrics.osd_height),
                y: 0.0,
            },
            View::Notification => Self {
                width: f64::from(metrics.notification_width),
                height: f64::from(metrics.notification_height),
                y: 0.0,
            },
            View::Search => Self {
                width: f64::from(metrics.search_width),
                height: f64::from(metrics.search_height),
                y: f64::from(metrics.search_y),
            },
        }
    }

    fn interpolate(self, target: Self, progress: f64) -> Self {
        Self {
            width: self.width + (target.width - self.width) * progress,
            height: self.height + (target.height - self.height) * progress,
            y: self.y + (target.y - self.y) * progress,
        }
    }
}

pub struct IslandWindow {
    monitor_name: String,
    metrics: Metrics,
    window: ApplicationWindow,
    /// Focus root; production aliases the layer window, while the GTK test
    /// constructor supplies a normal window because Broadway has no layer
    /// surface activation.
    focus_root: RefCell<gtk::Widget>,
    search_window: ApplicationWindow,
    search_fixed: Fixed,
    search_surface: gtk::ScrolledWindow,
    dismiss_window: ApplicationWindow,
    fixed: Fixed,
    /// Stable, neutral hover hit target behind the moving pill. Its allocation
    /// does not change with tray width or depth motion.
    hover_region: gtk::Box,
    content: Fixed,
    surface: gtk::ScrolledWindow,
    compact: gtk::Widget,
    media: gtk::Overlay,
    dashboard: gtk::Box,
    search: gtk::Box,
    weather: gtk::Box,
    osd: gtk::Box,
    /// `pill`-position notification view; unused (never shown) for the
    /// other `notifications.position` values.
    notification: gtk::Box,
    compact_workspaces: gtk::Box,
    compact_clock: gtk::Label,
    compact_battery: gtk::Label,
    battery_waves: battery_wave::BatteryWaves,
    compact_tray: gtk::Box,
    /// Current animated/target width of the compact pill, recomputed by
    /// `resize_compact` from the combined width of its (individually
    /// capped) children -- the `View::Compact` analogue of `media_width`.
    compact_width: Cell<i32>,
    /// `true` while the pointer is over the compact pill; the tray row is
    /// only shown (and only then counted into `resize_compact`) while this
    /// is set and at least one tray item exists.
    tray_hovered: Cell<bool>,
    /// Physical pointer state from the stable neutral hover region. Kept
    /// separately so a page switch can clear presentation depth without
    /// requiring a leave event from a widget whose allocation was replaced.
    pointer_in_hover_region: Cell<bool>,
    tray_item_count: Cell<usize>,
    /// `true` while a tray item's context menu popover is up. Opening a
    /// popover takes a pointer grab, which makes the pill's motion
    /// controller report a `leave` -- without pinning the tray open here,
    /// the row (and the popover's own anchor widget with it) would collapse
    /// out from under the menu the instant it appeared.
    tray_menu_open: Cell<bool>,
    /// Persistent menu lifetime manager shared by legacy and circle tray
    /// presentations; it is the sole source of truth for window pinning.
    tray_menu_manager: Rc<tray::TrayMenuManager>,
    tray_menu_tracker: Rc<tray::TrayMenuTracker>,
    media_workspaces: gtk::Box,
    media_clock: gtk::Label,
    media_center: gtk::Box,
    media_icon: gtk::Image,
    media_title: gtk::Label,
    media_visualizer: gtk::DrawingArea,
    media_levels: Rc<RefCell<VisualizerLevels>>,
    media_tray: gtk::Box,
    hero_time: gtk::Label,
    hero_date: gtk::Label,
    battery_chip: gtk::Box,
    battery_icon: gtk::Widget,
    battery_label: gtk::Label,
    player_card: gtk::Box,
    player_icon: gtk::Image,
    player_title: gtk::Label,
    player_artist: gtk::Label,
    player_progress: gtk::ProgressBar,
    player_elapsed_label: gtk::Label,
    player_duration_label: gtk::Label,
    player_prev_button: gtk::Button,
    player_play_pause_button: gtk::Button,
    player_next_button: gtk::Button,
    player_switch_row: gtk::Box,
    player_switch_label: gtk::Label,
    player_switch_prev: gtk::Button,
    player_switch_next: gtk::Button,
    /// Position reported by the last MPRIS update, in microseconds. Since
    /// `MediaState` only ever represents a `Playing` player, the progress
    /// bar advances this locally between updates instead of polling MPRIS.
    player_progress_base_us: Cell<i64>,
    player_progress_started_at: Cell<Option<Instant>>,
    player_length_us: Cell<i64>,
    player_active: Cell<bool>,
    latest_media: RefCell<Option<MediaState>>,
    selected_media_service: RefCell<Option<String>>,
    active_eyebrow: gtk::Label,
    active_title: gtk::Label,
    status_card: gtk::Box,
    workspace_row: gtk::FlowBox,
    controls_stack: gtk::Box,
    volume_scale: gtk::Scale,
    volume_value: gtk::Label,
    brightness_row: gtk::Box,
    brightness_scale: gtk::Scale,
    brightness_value: gtk::Label,
    /// Dashboard notification-history widgets: the count badge and the
    /// vertical list of recent notifications, rebuilt by
    /// `update_notification_history` from the controller's bounded history.
    notification_count: gtk::Label,
    notification_inhibit_remaining: gtk::Label,
    notification_clear_button: gtk::Button,
    notification_inhibit_button: gtk::ToggleButton,
    notification_expand_button: gtk::ToggleButton,
    notification_list: gtk::Box,
    notifications_expanded: Cell<bool>,
    updating_notification_inhibit: Cell<bool>,
    search_entry: gtk::SearchEntry,
    search_results: gtk::ListBox,
    search_status: gtk::Label,
    search_stack: gtk::Stack,
    search_plugin_toggle: gtk::ToggleButton,
    search_plugins: gtk::ListBox,
    search_preview_stack: gtk::Stack,
    search_preview_picture: gtk::Picture,
    /// Holds exactly one child: a chrome glyph when the preview is empty or
    /// loading, or a TarraGon-supplied image once a result arrives. The two
    /// cannot be the same widget, so the slot swaps children instead.
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
    /// Every currently displayed condition icon (the hero icon plus one per
    /// forecast day), so a live theme change can redraw them all in place
    /// instead of waiting for the next scheduled forecast refresh.
    weather_icons: RefCell<Vec<gtk::DrawingArea>>,
    latest_weather: RefCell<Option<WeatherState>>,
    current_view: Cell<View>,
    dashboard_open: Cell<bool>,
    search_open: Cell<bool>,
    weather_open: Cell<bool>,
    search_connected: Cell<bool>,
    search_generation: Cell<u64>,
    /// Invalidates queued focus requests when a temporary page or close wins
    /// before the GTK idle callback runs.
    search_focus_generation: Cell<u64>,
    search_focus_pending: Cell<bool>,
    preview_generation: Cell<u64>,
    search_action_generation: Cell<u64>,
    search_selection_pending: Cell<bool>,
    /// Whether the in-flight selection should leave the launcher open after
    /// a successful response.
    search_selection_keep_open: Cell<bool>,
    search_preview_key: RefCell<Option<String>>,
    /// Text of the most recently dispatched query. Snapshots are matched
    /// against this rather than the live entry text, so results still land
    /// when the user has typed ahead of the query that is in flight.
    search_dispatched: RefCell<Option<String>>,
    /// When the last query was sent, for the leading-edge throttle.
    last_search_dispatch: Cell<Option<Instant>>,
    search_snapshot: RefCell<Option<TarragonSnapshot>>,
    search_backend_status: RefCell<Option<TarragonStatus>>,
    search_geometry: Cell<Geometry>,
    search_animation_generation: Cell<u64>,
    osd_active: Cell<bool>,
    media_playing: Cell<bool>,
    media_width: Cell<i32>,
    geometry: Cell<Geometry>,
    animation_generation: Cell<u64>,
    animation_ms: Cell<u32>,
    animations_enabled: Cell<bool>,
    launcher_presentation: LauncherPresentation,
    osd_generation: Cell<u64>,
    volume_generation: Cell<u64>,
    brightness_generation: Cell<u64>,
    updating_controls: Cell<bool>,
    latest_hyprland: RefCell<HyprlandSnapshot>,
    notifications: NotificationConfig,
    /// `pill`-position only: notifications waiting to be shown once the
    /// currently displayed one advances or expires.
    notification_queue: RefCell<VecDeque<PendingNotification>>,
    /// `pill`-position only: the notification currently occupying the pill.
    notification_current: RefCell<Option<CurrentNotification>>,
    notification_active: Cell<bool>,
    notification_generation: Cell<u64>,
    /// `below-pill`/corner positions only.
    notification_toasts: Option<NotificationToasts>,
    pill_overlay: Option<PillOverlay>,
    actions: IslandActions,
}

/// `280` is the historical default.  Treating that value as the compatibility
/// default lets the named profiles select their Material timings, while any
/// other positive value remains an explicit user override.  This is necessarily
/// a convention because the TOML scalar cannot distinguish an omitted value
/// from an explicitly written `280`.
fn profile_timing(
    profile: crate::ui::motion::Profile,
    enabled: bool,
    animation_ms: u32,
) -> crate::ui::motion::Profile {
    profile.with_timing(enabled, (animation_ms != 280).then_some(animation_ms))
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

/// A widget's natural horizontal size, capped at `max_width` -- the "max
/// width" half of `resize_compact`'s per-element clamping (the widget
/// itself is left free to report whatever it wants; only the width fed
/// into the pill's total is bounded).
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

#[cfg(test)]
mod tests {
    use super::dashboard::battery_icon;
    use super::media::{format_media_time, media_state_for_player};
    use super::weather::weather_provider_label;
    use super::{
        Geometry, Icon, IslandActions, IslandWindow, View, hover_geometry, profile_timing,
    };
    use crate::config::{AppConfig, LauncherPresentation};
    use crate::state::{MediaPlayer, MediaState, PlaybackStatus};
    use crate::tarragon::TarragonSelection;
    use crate::ui::resolved_scale;
    use crate::weather::WeatherProvider;
    use gtk::prelude::*;
    use std::rc::Rc;

    #[test]
    fn hover_geometry_is_forward_and_reversible() {
        let resting = Geometry {
            width: 200.0,
            height: 32.0,
            y: 0.0,
        };
        let raised = hover_geometry(resting, 4.0, true);
        assert!(raised.width > resting.width);
        assert!(raised.height > resting.height);
        assert!(raised.y > resting.y);
        assert_eq!(hover_geometry(resting, 4.0, false), resting);
    }

    #[test]
    fn default_animation_value_selects_named_profile() {
        let profile = profile_timing(crate::ui::motion::Profile::HOVER_ENTER, true, 280);
        assert_eq!(
            profile.duration,
            crate::ui::motion::Profile::HOVER_ENTER.duration
        );
        let override_profile = profile_timing(crate::ui::motion::Profile::HOVER_ENTER, true, 333);
        assert_eq!(
            override_profile.duration,
            std::time::Duration::from_millis(333)
        );
    }

    #[test]
    #[ignore = "requires an isolated GTK display; run scripts/run-island-presentation-gtk.py"]
    fn integrated_search_return_uses_real_finish_and_scheduler_path() {
        gtk::init().expect("GTK display");
        let application = gtk::Application::new(
            Some("org.mithshell.presentation-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        application.connect_activate(|_| {});
        application
            .register(None::<&gtk::gio::Cancellable>)
            .expect("register GTK application");
        let display = gtk::gdk::Display::default().expect("GTK display");
        let monitor = display
            .monitors()
            .item(0)
            .and_downcast::<gtk::gdk::Monitor>()
            .expect("Broadway monitor");
        let actions = IslandActions {
            switch_workspace: Rc::new(|_, _| {}),
            set_volume: Rc::new(|_| {}),
            set_brightness: Rc::new(|_| {}),
            search: Rc::new(|_| {}),
            select: Rc::new(|_: TarragonSelection| {}),
            tarragon_status: Rc::new(|| {}),
            tarragon_reload: Rc::new(|| {}),
            load_preview: Rc::new(|_, _| {}),
            media_play_pause: Rc::new(|_| {}),
            media_next: Rc::new(|_| {}),
            media_previous: Rc::new(|_| {}),
            notification_expired: Rc::new(|_, _| {}),
            notification_dismiss: Rc::new(|_| {}),
            notification_invoke: Rc::new(|_, _| {}),
            notification_clear_all: Rc::new(|| {}),
            notification_inhibit: Rc::new(|_| {}),
            tray_activate: Rc::new(|_, _, _, _| {}),
            tray_secondary_activate: Rc::new(|_, _, _, _| {}),
            tray_context_menu: Rc::new(|_, _, _, _| {}),
            tray_scroll: Rc::new(|_, _, _, _| {}),
            tray_menu_event: Rc::new(|_, _, _| {}),
        };
        let mut config = AppConfig::default();
        config.launcher.presentation = LauncherPresentation::Integrated;
        config.shell.animation_ms = 0;
        let island = IslandWindow::new_for_test(
            &application,
            &monitor,
            "broadway-test".to_owned(),
            &config,
            actions,
            true,
        );
        application.activate();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        island.open_search();
        // Seed the normal GTK focus before exercising the same production
        // return scheduler below; Broadway cannot activate a layer surface.
        island.search_entry.set_can_focus(true);
        gtk::prelude::RootExt::set_focus(&island.window, Some(&island.search_entry));
        island.search_entry.grab_focus();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        assert_eq!(island.current_view.get(), View::Search);
        island.ensure_integrated_search_host();
        island.ensure_integrated_search_host();
        assert!(
            island
                .search
                .parent()
                .is_some_and(|parent| { parent == island.content.clone().upcast::<gtk::Widget>() })
        );
        let focus_window = island
            .focus_root
            .borrow()
            .clone()
            .downcast::<gtk::Window>()
            .expect("test focus window");
        let temporary_focus = gtk::Button::with_label("temporary focus");
        island.content.put(&temporary_focus, 0.0, 0.0);
        temporary_focus.set_can_focus(true);
        temporary_focus.set_can_target(true);
        focus_window.present();
        temporary_focus.grab_focus();
        gtk::prelude::RootExt::set_focus(&focus_window, Some(&temporary_focus));

        island.osd_active.set(true);
        island.reconcile_view();
        assert_eq!(island.current_view.get(), View::Osd);
        assert!(island.search_focus_pending.get());
        island.osd_active.set(false);
        island.reconcile_view();
        assert_eq!(island.current_view.get(), View::Search);
        // finish_view consumed the pending ownership handoff and queued the
        // guarded production scheduler.
        assert!(!island.search_focus_pending.get());
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        assert!(!island.search_focus_pending.get());
        assert!(island.search_entry.has_focus());

        island.schedule_search_entry_focus();
        let queued_generation = island.search_focus_generation.get();
        island.close();
        assert!(island.search_focus_generation.get() > queued_generation);
        // Make any direct refocus observable without relying on compositor
        // focus activation: closing the launcher removes the entry's focus
        // eligibility before the stale idle callback is drained.
        island.search_entry.set_can_focus(false);
        let button = gtk::Button::with_label("deliberate focus");
        island.content.put(&button, 0.0, 0.0);
        button.set_can_focus(true);
        button.set_can_target(true);
        focus_window.present();
        button.grab_focus();
        gtk::prelude::RootExt::set_focus(&focus_window, Some(&button));
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        assert!(button.has_focus());
        island.destroy();
    }

    fn player(service: &str, status: PlaybackStatus) -> MediaPlayer {
        MediaPlayer {
            player: service.to_owned(),
            service: service.to_owned(),
            title: format!("track from {service}"),
            artist: None,
            album: None,
            app_icon: None,
            position_us: 0,
            length_us: None,
            can_play: true,
            can_pause: true,
            can_go_next: true,
            can_go_previous: true,
            status,
        }
    }

    #[test]
    fn explicit_scale_is_not_capped() {
        assert_eq!(resolved_scale(2.4, 1.45), 2.4);
    }

    #[test]
    fn formats_media_time_below_and_above_an_hour() {
        assert_eq!(format_media_time(0), "0:00");
        assert_eq!(format_media_time(65_000_000), "1:05");
        assert_eq!(format_media_time(3_661_000_000), "1:01:01");
        assert_eq!(format_media_time(-5_000_000), "0:00");
    }

    #[test]
    fn picks_battery_icons_for_level_and_charge_state() {
        assert_eq!(
            battery_icon(12, "Discharging"),
            Icon::Battery {
                percent: 12,
                charging: false,
            }
        );
        assert_eq!(
            battery_icon(87, "Charging"),
            Icon::Battery {
                percent: 87,
                charging: true,
            }
        );
        // upower reports capitalised status strings; matching is case-insensitive.
        assert_eq!(
            battery_icon(50, "charging"),
            Icon::Battery {
                percent: 50,
                charging: true,
            }
        );
    }

    #[test]
    fn battery_icons_clamp_impossible_percentages() {
        assert_eq!(
            battery_icon(200, "Full"),
            Icon::Battery {
                percent: 100,
                charging: false,
            }
        );
    }

    #[test]
    fn labels_the_selected_weather_provider() {
        assert_eq!(
            weather_provider_label("WEATHER", WeatherProvider::Wttr),
            "WEATHER  //  WTTR.IN"
        );
        assert_eq!(
            weather_provider_label("UPDATED", WeatherProvider::OpenMeteo),
            "UPDATED  //  OPEN-METEO.COM"
        );
    }

    #[test]
    fn media_selection_promotes_requested_player_without_changing_status() {
        let playing = player("playing", PlaybackStatus::Playing);
        let paused = player("paused", PlaybackStatus::Paused);
        let state = MediaState {
            player: playing.player.clone(),
            service: playing.service.clone(),
            title: playing.title.clone(),
            artist: None,
            album: None,
            app_icon: None,
            position_us: 0,
            length_us: None,
            can_play: true,
            can_pause: true,
            can_go_next: true,
            can_go_previous: true,
            status: PlaybackStatus::Playing,
            players: vec![playing, paused],
        };

        let selected = media_state_for_player(&state, Some("paused"));
        assert_eq!(selected.service, "paused");
        assert_eq!(selected.status, PlaybackStatus::Paused);
        assert_eq!(selected.players.len(), 2);
    }
}
