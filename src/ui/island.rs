use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::{Duration, Instant},
};

use std::thread;

use gtk::{
    Align, Application, ApplicationWindow, EventControllerMotion, EventControllerScroll,
    EventControllerScrollFlags, Fixed, GestureClick, Orientation, Overflow,
    gdk::{self, prelude::*},
    glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::{resolved_scale, scaled};
use crate::{
    config::{AppConfig, NotificationConfig, NotificationPosition, ShellConfig},
    ipc::OsdKind,
    media::{VISUALIZER_BARS, VisualizerLevels},
    preview::{HIGHLIGHT_NAMES, PreviewContent, PreviewData},
    state::{
        HyprlandSnapshot, MediaPlayer, MediaState, Notification, OsdState, PlaybackStatus,
        SystemSnapshot, TrayIcon, TrayItem, TrayMenuItem, TrayStatus, Urgency, WeatherCondition,
        WeatherDay, WeatherState,
    },
    tarragon::{
        TarragonPlugin, TarragonPluginState, TarragonSelection, TarragonSnapshot, TarragonStatus,
    },
    tray,
};

const WINDOW_WIDTH: i32 = 860;
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
const OSD_WIDTH: i32 = 292;
const OSD_HEIGHT: i32 = 36;
/// `pill`-position notification geometry: wide enough for an icon, summary
/// and a one-line body preview without wrapping in the common case.
const NOTIFICATION_WIDTH: i32 = 280;
const NOTIFICATION_HEIGHT: i32 = 36;
/// Width of a `below-pill`/corner toast, independent of `Metrics` since
/// those popups aren't part of the animated island surface.
const NOTIFICATION_TOAST_WIDTH: i32 = 320;
const SEARCH_WIDTH: i32 = 820;
const SEARCH_HEIGHT: i32 = 620;
const SEARCH_RESULTS_MIN_WIDTH: i32 = 340;
const SEARCH_PREVIEW_MIN_WIDTH: i32 = 260;
// Kept comfortably under SEARCH_HEIGHT/SEARCH_WIDTH, which size the shared
// Fixed container every view is centered inside of.
const WEATHER_WIDTH: i32 = 380;
const WEATHER_HEIGHT: i32 = 390;
/// Minimum spacing between dispatched queries.
///
/// This throttles on the leading edge: the first keystroke after an idle
/// period is sent immediately, and only a burst faster than this window (held
/// keys repeating, or a very fast typist) is coalesced. A trailing debounce
/// would instead tax every keystroke, which is pure cost given TarraGon
/// answers in well under a millisecond.
const SEARCH_THROTTLE: Duration = Duration::from_millis(16);
/// How long a selection must hold still before its file preview is loaded.
/// Previews can spawn ffprobe or ffmpegthumbnailer, so unlike a query they are
/// far too expensive to issue for every intermediate row.
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(80);

#[derive(Debug, Clone, Copy)]
struct Metrics {
    scale: f64,
    window_width: i32,
    window_height: i32,
    compact_width: i32,
    compact_height: i32,
    compact_min_width: i32,
    compact_workspaces_max_width: i32,
    compact_clock_max_width: i32,
    compact_battery_max_width: i32,
    compact_tray_max_width: i32,
    tray_icon_size: i32,
    media_max_width: i32,
    media_height: i32,
    dashboard_width: i32,
    dashboard_height: i32,
    osd_width: i32,
    osd_height: i32,
    notification_width: i32,
    notification_height: i32,
    search_width: i32,
    search_height: i32,
    search_y: i32,
    weather_width: i32,
    weather_height: i32,
}

impl Metrics {
    fn new(monitor: &gdk::Monitor, configured_scale: f64, media_width_factor: f64) -> Self {
        let scale = resolved_scale(configured_scale, super::automatic_scale(monitor));
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

    fn spacing(self, value: i32) -> i32 {
        scaled(value, self.scale)
    }

    fn css_class(self) -> Option<&'static str> {
        super::scale_class(self.scale)
    }
}

pub type WorkspaceAction = Rc<dyn Fn(&str, i64)>;
pub type ValueAction = Rc<dyn Fn(u8)>;
pub type SearchAction = Rc<dyn Fn(String)>;
pub type SelectionAction = Rc<dyn Fn(TarragonSelection)>;
pub type UnitAction = Rc<dyn Fn()>;
pub type PreviewAction = Rc<dyn Fn(u64, String)>;
/// Argument is the target MPRIS player's full D-Bus service name.
pub type MediaAction = Rc<dyn Fn(String)>;
/// Argument is a notification id. Used both for the timer-driven expiry
/// (which only needs to emit `NotificationClosed` over D-Bus) and for an
/// explicit user dismissal (which additionally drops the notification from
/// history and closes it on every island).
pub type NotificationCloseAction = Rc<dyn Fn(u32)>;
/// Arguments are a notification id and the invoked action's key.
pub type NotificationInvokeAction = Rc<dyn Fn(u32, String)>;
/// Arguments are a tray item's `service`/`object_path` and pointer
/// coordinates, for `Activate`/`SecondaryActivate`/`ContextMenu`.
pub type TrayPointAction = Rc<dyn Fn(String, String, i32, i32)>;
/// Arguments are a tray item's `service`/`object_path`, a scroll delta and
/// whether it was along the horizontal axis.
pub type TrayScrollAction = Rc<dyn Fn(String, String, i32, bool)>;
/// Arguments are a tray item's `service`, its DBusMenu object path, and the
/// clicked entry's id.
pub type TrayMenuEventAction = Rc<dyn Fn(String, String, i32)>;

#[derive(Clone)]
pub struct IslandActions {
    pub switch_workspace: WorkspaceAction,
    pub set_volume: ValueAction,
    pub set_brightness: ValueAction,
    pub search: SearchAction,
    pub select: SelectionAction,
    pub tarragon_status: UnitAction,
    pub tarragon_reload: UnitAction,
    pub load_preview: PreviewAction,
    pub media_play_pause: MediaAction,
    pub media_next: MediaAction,
    pub media_previous: MediaAction,
    pub notification_expired: NotificationCloseAction,
    pub notification_dismiss: NotificationCloseAction,
    pub notification_invoke: NotificationInvokeAction,
    pub tray_activate: TrayPointAction,
    pub tray_secondary_activate: TrayPointAction,
    pub tray_context_menu: TrayPointAction,
    pub tray_scroll: TrayScrollAction,
    pub tray_menu_event: TrayMenuEventAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Compact,
    Media,
    Dashboard,
    Search,
    Weather,
    Osd,
    /// Only reachable when `notifications.position = "pill"`; the other
    /// positions render notifications in a separate popup window instead.
    Notification,
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
            View::Search => Self {
                width: f64::from(metrics.search_width),
                height: f64::from(metrics.search_height),
                y: f64::from(metrics.search_y),
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

/// Buttons owned by `dashboard_view`/`search_view`/`weather_view` that need
/// click handlers wired up centrally in `connect_interactions`. Grouped into
/// one struct instead of separate parameters purely to keep that function's
/// signature manageable.
struct OverlayButtons<'a> {
    close_button: &'a gtk::Button,
    search_button: &'a gtk::Button,
    weather_button: &'a gtk::Button,
    search_back_button: &'a gtk::Button,
    search_reload_button: &'a gtk::Button,
    weather_back_button: &'a gtk::Button,
}

/// The separate popup used for `notifications.position` values other than
/// `pill`: a small always-on-top layer-shell surface anchored either
/// directly under the island (`below-pill`) or to a screen corner, holding
/// a vertically stacked list of toast rows.
struct NotificationToasts {
    window: ApplicationWindow,
    stack: gtk::Box,
    entries: RefCell<HashMap<u32, ToastEntry>>,
    /// Notification ids, most recent first. Tracked separately from
    /// `entries` (a `HashMap`) purely to know which toast is oldest once
    /// `max_visible` is exceeded.
    order: RefCell<Vec<u32>>,
}

struct ToastEntry {
    row: gtk::Box,
    /// Cancelled on manual dismissal so an already-removed row can't be
    /// double-removed when its timer later fires.
    timeout_source: Option<glib::SourceId>,
}

pub struct IslandWindow {
    monitor_name: String,
    metrics: Metrics,
    window: ApplicationWindow,
    dismiss_window: ApplicationWindow,
    fixed: Fixed,
    content: Fixed,
    surface: gtk::ScrolledWindow,
    compact: gtk::Box,
    media: gtk::Box,
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
    compact_tray: gtk::Box,
    /// Current animated/target width of the compact pill, recomputed by
    /// `resize_compact` from the combined width of its (individually
    /// capped) children -- the `View::Compact` analogue of `media_width`.
    compact_width: Cell<i32>,
    /// `true` while the pointer is over the compact pill; the tray row is
    /// only shown (and only then counted into `resize_compact`) while this
    /// is set and at least one tray item exists.
    tray_hovered: Cell<bool>,
    tray_item_count: Cell<usize>,
    /// `true` while a tray item's context menu popover is up. Opening a
    /// popover takes a pointer grab, which makes the pill's motion
    /// controller report a `leave` -- without pinning the tray open here,
    /// the row (and the popover's own anchor widget with it) would collapse
    /// out from under the menu the instant it appeared.
    tray_menu_open: Cell<bool>,
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
    battery_icon: gtk::Image,
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
    workspace_row: gtk::FlowBox,
    volume_scale: gtk::Scale,
    volume_value: gtk::Label,
    brightness_row: gtk::Box,
    brightness_scale: gtk::Scale,
    brightness_value: gtk::Label,
    /// Dashboard notification-history widgets: the count badge and the
    /// vertical list of recent notifications, rebuilt by
    /// `update_notification_history` from the controller's bounded history.
    notification_count: gtk::Label,
    notification_list: gtk::Box,
    search_entry: gtk::SearchEntry,
    search_results: gtk::ListBox,
    search_status: gtk::Label,
    search_stack: gtk::Stack,
    search_plugin_toggle: gtk::ToggleButton,
    search_plugins: gtk::ListBox,
    search_preview_stack: gtk::Stack,
    search_preview_picture: gtk::Picture,
    search_preview_icon: gtk::Image,
    search_preview_title: gtk::Label,
    search_preview_description: gtk::Label,
    search_preview_file_meta: gtk::Label,
    search_preview_meta: gtk::Label,
    search_preview_text: gtk::TextView,
    search_preview_text_scroll: gtk::ScrolledWindow,
    search_preview_error: gtk::Label,
    search_preview_actions: gtk::Box,
    osd_icon: gtk::Image,
    osd_title: gtk::Label,
    osd_progress: gtk::ProgressBar,
    osd_value: gtk::Label,
    notification_icon: gtk::Image,
    notification_app: gtk::Label,
    notification_body: gtk::Label,
    weather_location: gtk::Label,
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
    preview_generation: Cell<u64>,
    search_action_generation: Cell<u64>,
    search_selection_pending: Cell<bool>,
    search_preview_key: RefCell<Option<String>>,
    /// Text of the most recently dispatched query. Snapshots are matched
    /// against this rather than the live entry text, so results still land
    /// when the user has typed ahead of the query that is in flight.
    search_dispatched: RefCell<Option<String>>,
    /// When the last query was sent, for the leading-edge throttle.
    last_search_dispatch: Cell<Option<Instant>>,
    search_snapshot: RefCell<Option<TarragonSnapshot>>,
    search_backend_status: RefCell<Option<TarragonStatus>>,
    osd_active: Cell<bool>,
    media_playing: Cell<bool>,
    media_width: Cell<i32>,
    geometry: Cell<Geometry>,
    animation_generation: Cell<u64>,
    animation_ms: Cell<u32>,
    animations_enabled: Cell<bool>,
    osd_generation: Cell<u64>,
    volume_generation: Cell<u64>,
    brightness_generation: Cell<u64>,
    updating_controls: Cell<bool>,
    latest_hyprland: RefCell<HyprlandSnapshot>,
    notifications: NotificationConfig,
    /// `pill`-position only: notifications waiting to be shown once the
    /// currently displayed one advances or expires.
    notification_queue: RefCell<VecDeque<Notification>>,
    /// `pill`-position only: the notification currently occupying the pill.
    notification_current: RefCell<Option<Notification>>,
    notification_active: Cell<bool>,
    notification_generation: Cell<u64>,
    /// `below-pill`/corner positions only; `None` when `notifications` is
    /// `pill` (the default), since no extra popup window is needed then.
    notification_toasts: Option<NotificationToasts>,
    actions: IslandActions,
}

impl IslandWindow {
    pub fn new(
        application: &Application,
        monitor: &gdk::Monitor,
        monitor_name: String,
        config: &AppConfig,
        actions: IslandActions,
        animations_enabled: bool,
    ) -> Rc<Self> {
        let shell = &config.shell;
        let metrics = Metrics::new(monitor, shell.scale, config.media.max_width_factor);

        let dismiss_window = ApplicationWindow::builder()
            .application(application)
            .title("mithshell dismiss")
            .decorated(false)
            .build();
        dismiss_window.add_css_class("mithshell-dismiss");
        dismiss_window.init_layer_shell();
        dismiss_window.set_namespace(Some("mithshell-dismiss"));
        dismiss_window.set_layer(Layer::Top);
        dismiss_window.set_keyboard_mode(KeyboardMode::None);
        dismiss_window.set_monitor(Some(monitor));
        for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
            dismiss_window.set_anchor(edge, true);
        }
        dismiss_window.set_exclusive_zone(0);
        let dismiss_area = gtk::Box::new(Orientation::Vertical, 0);
        dismiss_area.set_hexpand(true);
        dismiss_area.set_vexpand(true);
        dismiss_window.set_child(Some(&dismiss_area));

        let window = ApplicationWindow::builder()
            .application(application)
            .title("mithshell")
            .decorated(false)
            .resizable(false)
            .default_width(metrics.window_width)
            .default_height(metrics.window_height)
            .build();
        window.add_css_class("mithshell-window");
        if let Some(class) = metrics.css_class() {
            window.add_css_class(class);
        }
        window.init_layer_shell();
        window.set_namespace(Some("mithshell"));
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_monitor(Some(monitor));
        window.set_anchor(Edge::Top, true);
        window.set_margin(Edge::Top, metrics.spacing(shell.top_margin));
        window.set_exclusive_zone(metrics.spacing(shell.exclusive_zone));

        let fixed = Fixed::new();
        fixed.set_size_request(metrics.window_width, metrics.window_height);
        window.set_child(Some(&fixed));

        let surface = gtk::ScrolledWindow::new();
        surface.add_css_class("island-surface");
        surface.set_overflow(Overflow::Hidden);
        surface.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        surface.set_propagate_natural_width(false);
        surface.set_propagate_natural_height(false);
        surface.set_kinetic_scrolling(false);
        surface.set_has_frame(false);
        fixed.put(
            &surface,
            f64::from((metrics.window_width - metrics.compact_width) / 2),
            0.0,
        );
        surface.set_size_request(metrics.compact_width, metrics.compact_height);

        let content = Fixed::new();
        content.set_size_request(metrics.search_width, metrics.search_height);
        surface.set_child(Some(&content));

        let (compact, compact_workspaces, compact_clock, compact_battery, compact_tray) =
            compact_view(metrics);
        content.put(
            &compact,
            f64::from((metrics.search_width - metrics.compact_width) / 2),
            0.0,
        );

        let dashboard_widgets = dashboard_view(metrics);
        content.put(
            &dashboard_widgets.root,
            f64::from((metrics.search_width - metrics.dashboard_width) / 2),
            0.0,
        );
        dashboard_widgets.root.set_opacity(0.0);
        dashboard_widgets.root.set_visible(false);

        let search_widgets = search_view(metrics);
        content.put(&search_widgets.root, 0.0, 0.0);
        search_widgets.root.set_opacity(0.0);
        search_widgets.root.set_visible(false);

        let media_widgets = media_view(metrics);
        content.put(
            &media_widgets.root,
            f64::from((metrics.search_width - metrics.compact_width) / 2),
            0.0,
        );
        media_widgets.root.set_opacity(0.0);
        media_widgets.root.set_visible(false);

        let (osd, osd_icon, osd_title, osd_progress, osd_value) = osd_view(metrics);
        content.put(
            &osd,
            f64::from((metrics.search_width - metrics.osd_width) / 2),
            0.0,
        );
        osd.set_opacity(0.0);
        osd.set_visible(false);

        let (notification, notification_icon, notification_app, notification_body) =
            notification_view(metrics);
        content.put(
            &notification,
            f64::from((metrics.search_width - metrics.notification_width) / 2),
            0.0,
        );
        notification.set_opacity(0.0);
        notification.set_visible(false);

        let notification_toasts = (config.notifications.position != NotificationPosition::Pill)
            .then(|| {
                build_notification_toasts(
                    application,
                    monitor,
                    shell,
                    &config.notifications,
                    metrics,
                )
            });

        let weather_widgets = weather_view(metrics);
        content.put(
            &weather_widgets.root,
            f64::from((metrics.search_width - metrics.weather_width) / 2),
            0.0,
        );
        weather_widgets.root.set_opacity(0.0);
        weather_widgets.root.set_visible(false);
        draw_weather_condition(&weather_widgets.hero_icon, WeatherCondition::Unknown);
        let weather_icons = RefCell::new(vec![weather_widgets.hero_icon.clone()]);

        let island = Rc::new(Self {
            monitor_name,
            metrics,
            window,
            dismiss_window,
            fixed,
            content,
            surface,
            compact,
            media: media_widgets.root,
            dashboard: dashboard_widgets.root,
            search: search_widgets.root,
            weather: weather_widgets.root,
            osd,
            notification,
            compact_workspaces,
            compact_clock,
            compact_battery,
            compact_tray,
            compact_width: Cell::new(metrics.compact_width),
            tray_hovered: Cell::new(false),
            tray_item_count: Cell::new(0),
            tray_menu_open: Cell::new(false),
            media_workspaces: media_widgets.workspaces,
            media_clock: media_widgets.clock,
            media_center: media_widgets.center,
            media_icon: media_widgets.icon,
            media_title: media_widgets.title,
            media_visualizer: media_widgets.visualizer,
            media_levels: media_widgets.levels,
            media_tray: media_widgets.tray,
            hero_time: dashboard_widgets.hero_time,
            hero_date: dashboard_widgets.hero_date,
            battery_chip: dashboard_widgets.battery_chip,
            battery_icon: dashboard_widgets.battery_icon,
            battery_label: dashboard_widgets.battery_label,
            player_card: dashboard_widgets.player_card,
            player_icon: dashboard_widgets.player_icon,
            player_title: dashboard_widgets.player_title,
            player_artist: dashboard_widgets.player_artist,
            player_progress: dashboard_widgets.player_progress,
            player_elapsed_label: dashboard_widgets.player_elapsed_label,
            player_duration_label: dashboard_widgets.player_duration_label,
            player_prev_button: dashboard_widgets.player_prev_button,
            player_play_pause_button: dashboard_widgets.player_play_pause_button,
            player_next_button: dashboard_widgets.player_next_button,
            player_switch_row: dashboard_widgets.player_switch_row,
            player_switch_label: dashboard_widgets.player_switch_label,
            player_switch_prev: dashboard_widgets.player_switch_prev,
            player_switch_next: dashboard_widgets.player_switch_next,
            player_progress_base_us: Cell::new(0),
            player_progress_started_at: Cell::new(None),
            player_length_us: Cell::new(0),
            player_active: Cell::new(false),
            latest_media: RefCell::new(None),
            selected_media_service: RefCell::new(None),
            active_eyebrow: dashboard_widgets.active_eyebrow,
            active_title: dashboard_widgets.active_title,
            workspace_row: dashboard_widgets.workspace_row,
            volume_scale: dashboard_widgets.volume_scale,
            volume_value: dashboard_widgets.volume_value,
            brightness_row: dashboard_widgets.brightness_row,
            brightness_scale: dashboard_widgets.brightness_scale,
            brightness_value: dashboard_widgets.brightness_value,
            notification_count: dashboard_widgets.notification_count,
            notification_list: dashboard_widgets.notification_list,
            search_entry: search_widgets.entry,
            search_results: search_widgets.results,
            search_status: search_widgets.status,
            search_stack: search_widgets.stack,
            search_plugin_toggle: search_widgets.plugin_toggle,
            search_plugins: search_widgets.plugins,
            search_preview_stack: search_widgets.preview_stack,
            search_preview_picture: search_widgets.preview_picture,
            search_preview_icon: search_widgets.preview_icon,
            search_preview_title: search_widgets.preview_title,
            search_preview_description: search_widgets.preview_description,
            search_preview_file_meta: search_widgets.preview_file_meta,
            search_preview_meta: search_widgets.preview_meta,
            search_preview_text: search_widgets.preview_text,
            search_preview_text_scroll: search_widgets.preview_text_scroll,
            search_preview_error: search_widgets.preview_error,
            search_preview_actions: search_widgets.preview_actions,
            osd_icon,
            osd_title,
            osd_progress,
            osd_value,
            notification_icon,
            notification_app,
            notification_body,
            weather_location: weather_widgets.location,
            weather_hero_icon: weather_widgets.hero_icon,
            weather_hero_temp: weather_widgets.hero_temp,
            weather_hero_description: weather_widgets.hero_description,
            weather_status: weather_widgets.status,
            weather_forecast_row: weather_widgets.forecast_row,
            weather_icons,
            latest_weather: RefCell::new(None),
            current_view: Cell::new(View::Compact),
            dashboard_open: Cell::new(false),
            search_open: Cell::new(false),
            weather_open: Cell::new(false),
            search_connected: Cell::new(false),
            search_generation: Cell::new(0),
            preview_generation: Cell::new(0),
            search_action_generation: Cell::new(0),
            search_selection_pending: Cell::new(false),
            search_preview_key: RefCell::new(None),
            search_dispatched: RefCell::new(None),
            last_search_dispatch: Cell::new(None),
            search_snapshot: RefCell::new(None),
            search_backend_status: RefCell::new(None),
            osd_active: Cell::new(false),
            media_playing: Cell::new(false),
            media_width: Cell::new(metrics.compact_width),
            geometry: Cell::new(Geometry::for_view(
                View::Compact,
                metrics,
                metrics.compact_width,
                metrics.compact_width,
            )),
            animation_generation: Cell::new(0),
            animation_ms: Cell::new(shell.animation_ms),
            animations_enabled: Cell::new(animations_enabled),
            osd_generation: Cell::new(0),
            volume_generation: Cell::new(0),
            brightness_generation: Cell::new(0),
            updating_controls: Cell::new(false),
            latest_hyprland: RefCell::new(HyprlandSnapshot::default()),
            notifications: config.notifications.clone(),
            notification_queue: RefCell::new(VecDeque::new()),
            notification_current: RefCell::new(None),
            notification_active: Cell::new(false),
            notification_generation: Cell::new(0),
            notification_toasts,
            actions,
        });

        island.connect_interactions(
            OverlayButtons {
                close_button: &dashboard_widgets.close_button,
                search_button: &dashboard_widgets.search_button,
                weather_button: &dashboard_widgets.weather_button,
                search_back_button: &search_widgets.back_button,
                search_reload_button: &search_widgets.reload_button,
                weather_back_button: &weather_widgets.back_button,
            },
            &dismiss_area,
        );
        island.resize_compact();
        island.start_clock();
        island.start_player_progress_timer();
        let weak = Rc::downgrade(&island);
        island.window.connect_realize(move |_| {
            if let Some(island) = weak.upgrade() {
                island.apply_geometry(island.geometry.get());
            }
        });
        island.window.present();
        let weak = Rc::downgrade(&island);
        glib::idle_add_local_once(move || {
            if let Some(island) = weak.upgrade() {
                island.apply_geometry(island.geometry.get());
            }
        });
        island
    }

    pub fn monitor_name(&self) -> &str {
        &self.monitor_name
    }

    pub fn debug_state(&self) -> serde_json::Value {
        let geometry = self.geometry.get();
        serde_json::json!({
            "view": format!("{:?}", self.current_view.get()).to_lowercase(),
            "scale": self.metrics.scale,
            "width": geometry.width.round() as i32,
            "height": geometry.height.round() as i32,
            "y": geometry.y.round() as i32,
            "compact_visible": self.compact.is_visible(),
            "compact_opacity": self.compact.opacity(),
            "media_visible": self.media.is_visible(),
            "media_opacity": self.media.opacity(),
            "media_playing": self.media_playing.get(),
            "media_title": self.media_title.label().to_string(),
            "dashboard_visible": self.dashboard.is_visible(),
            "dashboard_opacity": self.dashboard.opacity(),
            "search_visible": self.search.is_visible(),
            "search_connected": self.search_connected.get(),
            "weather_visible": self.weather.is_visible(),
            "osd_visible": self.osd.is_visible(),
            "osd_opacity": self.osd.opacity(),
            "notification_visible": self.notification.is_visible(),
            "notification_queued": self.notification_queue.borrow().len(),
            "notification_toasts": self
                .notification_toasts
                .as_ref()
                .map_or(0, |toasts| toasts.entries.borrow().len()),
            "tray_item_count": self.tray_item_count.get(),
            "tray_hovered": self.tray_hovered.get(),
            "tray_menu_open": self.tray_menu_open.get(),
            "tray_visible": self.compact_tray.is_visible(),
            "tray_visible_media": self.media_tray.is_visible(),
        })
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.dashboard_open.get() || self.search_open.get() || self.weather_open.get() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(self: &Rc<Self>) {
        self.clear_osd();
        self.search_open.set(false);
        self.weather_open.set(false);
        self.dashboard_open.set(true);
        self.reconcile_view();
    }

    pub fn close(self: &Rc<Self>) {
        self.dashboard_open.set(false);
        self.search_open.set(false);
        self.weather_open.set(false);
        self.search_action_generation
            .set(self.search_action_generation.get().wrapping_add(1));
        self.clear_osd();
        self.reconcile_view();
    }

    /// Switches the island to the weather forecast view. Mirrors
    /// `open_search`'s shape: clears any other overlay flag first so the
    /// views stay mutually exclusive, then hands off to `reconcile_view`.
    pub fn open_weather(self: &Rc<Self>) {
        self.clear_osd();
        self.dashboard_open.set(false);
        self.search_open.set(false);
        self.weather_open.set(true);
        if self.latest_weather.borrow().is_none() {
            self.weather_status.set_label("FETCHING FORECAST");
        }
        self.reconcile_view();
    }

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
        self.weather_hero_temp
            .set_label(&format!("{}°", state.current_c));
        self.weather_hero_description.set_label(&state.description);
        draw_weather_condition(&self.weather_hero_icon, state.condition);
        self.weather_status.set_label("UPDATED  //  WTTR.IN");

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

    pub fn update_tarragon_connection(&self, connected: bool, message: Option<&str>) {
        self.search_connected.set(connected);
        self.search_entry.set_sensitive(connected);
        self.search_plugin_toggle.set_sensitive(connected);
        if connected {
            self.search_status.set_label("READY  //  TYPE TO SEARCH");
        } else {
            self.search_selection_pending.set(false);
            self.search_action_generation
                .set(self.search_action_generation.get().wrapping_add(1));
            self.search_status
                .set_label(message.unwrap_or("TARRAGON OFFLINE"));
        }
    }

    pub fn update_tarragon_results(self: &Rc<Self>, snapshot: &TarragonSnapshot) {
        // Match the query this island actually asked for. Comparing against the
        // live entry text instead would drop every snapshot whenever the user
        // typed while results were in flight, pinning the UI on "SEARCHING".
        if self.search_dispatched.borrow().as_deref() != Some(snapshot.input.as_str()) {
            return;
        }
        let selected_index = self
            .search_results
            .selected_row()
            .map_or(0, |row| row.index());
        *self.search_snapshot.borrow_mut() = Some(snapshot.clone());
        while let Some(child) = self.search_results.first_child() {
            self.search_results.remove(&child);
        }

        for result in &snapshot.list {
            self.search_results
                .append(&search_result_row(result, self.metrics));
        }
        let selected_index = selected_index.min(snapshot.list.len().saturating_sub(1) as i32);
        if let Some(row) = self.search_results.row_at_index(selected_index) {
            self.search_results.select_row(Some(&row));
        } else {
            self.clear_search_preview();
        }

        let pending = snapshot
            .plugins
            .values()
            .filter(|plugin| plugin.state == "pending")
            .count();
        let errors = snapshot
            .plugins
            .values()
            .filter(|plugin| plugin.state == "error")
            .count();
        let completed = snapshot.plugins.len().saturating_sub(pending);
        let elapsed = snapshot
            .plugins
            .values()
            .map(|plugin| plugin.elapsed_ms)
            .fold(0.0, f64::max);
        let mut status = format!(
            "{} RESULTS  //  {completed}/{} PLUGINS",
            snapshot.list.len(),
            snapshot.plugins.len()
        );
        if pending > 0 {
            status.push_str(&format!("  //  {pending} PENDING"));
        }
        if errors > 0 {
            status.push_str(&format!("  //  {errors} ERRORS"));
        }
        if pending == 0 && elapsed > 0.0 {
            status.push_str(&format!("  //  {elapsed:.1} MS"));
        }
        if snapshot.list.is_empty() && pending == 0 {
            status = format!("NO RESULTS  //  {completed} PLUGINS COMPLETE");
        }
        self.search_status.set_label(&status);
        self.render_plugin_list();
        // t4: main-thread widget work for this snapshot is complete.
        crate::latency::mark_build();
    }

    pub fn update_tarragon_status(self: &Rc<Self>, status: &TarragonStatus) {
        *self.search_backend_status.borrow_mut() = Some(status.clone());
        self.render_plugin_list();
        if self.search_plugin_toggle.is_active() {
            self.show_plugin_summary();
        } else if self.search_open.get() {
            let snapshot = self.search_snapshot.borrow().clone();
            if let Some(snapshot) = snapshot {
                self.update_tarragon_results(&snapshot);
            } else {
                self.search_status.set_label("READY  //  TYPE TO SEARCH");
            }
        }
    }

    pub fn update_tarragon_reload(&self, success: bool, message: &str) {
        if self.search_open.get() {
            self.search_status.set_label(if success {
                "TARRAGON RELOADED  //  REFRESHING STATUS"
            } else {
                message
            });
        }
    }

    pub fn update_tarragon_selection(self: &Rc<Self>, success: bool, message: &str) {
        self.search_selection_pending.set(false);
        if success && self.search_open.get() {
            self.close();
        } else if !success && self.search_open.get() {
            self.search_status.set_label(message);
        }
    }

    fn render_plugin_list(&self) {
        // The plugin pane is a separate stack page. Rebuilding roughly seven
        // widgets per plugin on every streamed snapshot is invisible work when
        // the pane is not on screen, and TarraGon sends one snapshot per plugin
        // completion.
        if !self.search_plugin_toggle.is_active() {
            return;
        }
        clear_list_box(&self.search_plugins);
        let status = self.search_backend_status.borrow();
        let Some(status) = status.as_ref() else {
            return;
        };
        let snapshot = self.search_snapshot.borrow();
        for plugin in &status.plugins {
            let query_state = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.plugins.get(&plugin.name));
            self.search_plugins
                .append(&plugin_status_row(plugin, query_state, self.metrics));
        }
    }

    fn show_plugin_summary(&self) {
        let status = self.search_backend_status.borrow();
        let Some(status) = status.as_ref() else {
            self.search_status.set_label("LOADING PLUGIN STATUS");
            return;
        };
        let enabled = status
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled)
            .count();
        let on_call = status
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled && plugin.lifecycle == "on_call")
            .count();
        self.search_status.set_label(&format!(
            "{} DISCOVERED  //  {enabled} ENABLED  //  {} CONNECTED  //  {on_call} ON CALL",
            status.plugins.len(),
            status.connected.len()
        ));
    }

    fn clear_search_preview(&self) {
        self.search_preview_key.borrow_mut().take();
        self.preview_generation
            .set(self.preview_generation.get().wrapping_add(1));
        self.search_preview_stack.set_visible_child_name("icon");
        self.search_preview_icon
            .set_icon_name(Some("system-search-symbolic"));
        self.search_preview_picture
            .set_filename(None::<&std::path::Path>);
        self.search_preview_text.buffer().set_text("");
        self.search_preview_file_meta.set_label("");
        self.search_preview_error.set_label("");
        self.search_preview_title.set_label("Select a result");
        self.search_preview_description.set_label("");
        self.search_preview_meta
            .set_label("TarraGon aggregate results");
        clear_box(&self.search_preview_actions);
    }

    fn update_search_preview(self: &Rc<Self>, index: i32) {
        let Some(result) = self
            .search_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.list.get(index.max(0) as usize))
            .cloned()
        else {
            self.clear_search_preview();
            return;
        };

        let title = if result.label.is_empty() {
            &result.id
        } else {
            &result.label
        };
        self.search_preview_title.set_label(title);
        self.search_preview_description
            .set_label(&result.description);
        self.search_preview_description
            .set_visible(!result.description.is_empty());

        let preview_key = format!("{}\0{}\0{}", result.plugin, result.id, result.preview_path);
        let preview_changed = self
            .search_preview_key
            .replace(Some(preview_key))
            .as_deref()
            != self.search_preview_key.borrow().as_deref();

        let category = if result.category.is_empty() {
            "uncategorized"
        } else {
            &result.category
        };
        let mut meta = format!(
            "{}  //  {}\nSCORE {:.3}  //  FRECENCY {:.3}",
            result.plugin, category, result.score, result.frecency_score
        );
        if !result.preview_path.is_empty() {
            meta.push_str(&format!("\n{}", result.preview_path));
        }
        self.search_preview_meta.set_label(&meta);
        if preview_changed {
            self.search_preview_picture
                .set_filename(None::<&std::path::Path>);
            self.search_preview_text.buffer().set_text("");
            self.search_preview_error.set_label("");
            if result.icon.starts_with('/') {
                self.search_preview_icon.set_from_file(Some(&result.icon));
            } else {
                self.search_preview_icon
                    .set_icon_name(Some(if result.icon.is_empty() {
                        "content-loading-symbolic"
                    } else {
                        &result.icon
                    }));
            }
            self.search_preview_stack.set_visible_child_name("icon");
            let generation = self.preview_generation.get().wrapping_add(1);
            self.preview_generation.set(generation);
            if result.preview_path.is_empty() {
                self.search_preview_file_meta.set_label("NO FILE PREVIEW");
            } else {
                self.search_preview_file_meta
                    .set_label("LOADING FILE METADATA");
                // Held arrow keys walk the list faster than a preview can be
                // produced, so wait out the burst before touching the disk.
                let weak = Rc::downgrade(self);
                let path = result.preview_path.clone();
                glib::timeout_add_local_once(PREVIEW_DEBOUNCE, move || {
                    if let Some(island) = weak.upgrade()
                        && island.preview_generation.get() == generation
                    {
                        (island.actions.load_preview)(generation, path);
                    }
                });
            }
        }

        clear_box(&self.search_preview_actions);
        for action in &result.actions {
            if action.name.is_empty() {
                continue;
            }
            let label = if action.description.is_empty() {
                action.name.clone()
            } else {
                action.description.clone()
            };
            let button = gtk::Button::with_label(&label);
            button.add_css_class("search-action");
            if action.default {
                button.add_css_class("default");
            }
            let weak = Rc::downgrade(self);
            let action_name = action.name.clone();
            button.connect_clicked(move |_| {
                if let Some(island) = weak.upgrade() {
                    island.execute_search_action(index, &action_name);
                }
            });
            self.search_preview_actions.append(&button);
        }
        if result.actions.is_empty() {
            let unavailable = gtk::Label::new(Some("NO ACTIONS EXPOSED"));
            unavailable.add_css_class("search-preview-meta");
            self.search_preview_actions.append(&unavailable);
        }
    }

    pub fn apply_file_preview(&self, generation: u64, result: Result<PreviewData, String>) {
        if self.preview_generation.get() != generation {
            return;
        }
        let data = match result {
            Ok(data) => data,
            Err(error) => {
                self.search_preview_file_meta.set_label("PREVIEW ERROR");
                self.search_preview_error.set_label(&error);
                self.search_preview_stack.set_visible_child_name("error");
                return;
            }
        };
        self.search_preview_file_meta
            .set_label(&format_preview_metadata(&data.metadata));
        match data.content {
            PreviewContent::Text { text, highlights } => {
                let buffer = self.search_preview_text.buffer();
                buffer.set_text(&text);
                for span in highlights {
                    let Some(name) = HIGHLIGHT_NAMES.get(span.style) else {
                        continue;
                    };
                    let tag_name = format!("mithshell-highlight-{}", name.replace('.', "-"));
                    let table = buffer.tag_table();
                    let tag = table.lookup(&tag_name).unwrap_or_else(|| {
                        let tag = gtk::TextTag::new(Some(&tag_name));
                        tag.set_foreground(Some(highlight_color(name)));
                        table.add(&tag);
                        tag
                    });
                    let start = buffer.iter_at_offset(span.start);
                    let end = buffer.iter_at_offset(span.end);
                    buffer.apply_tag(&tag, &start, &end);
                }
                let scroller = self.search_preview_text_scroll.clone();
                glib::idle_add_local_once(move || {
                    scroller.hadjustment().set_value(0.0);
                    scroller.vadjustment().set_value(0.0);
                });
                self.search_preview_stack.set_visible_child_name("text");
            }
            PreviewContent::Image(path) | PreviewContent::VideoThumbnail(path) => {
                self.search_preview_picture.set_filename(Some(path));
                self.search_preview_stack.set_visible_child_name("picture");
            }
            PreviewContent::Generic => {
                self.search_preview_stack.set_visible_child_name("icon");
            }
        }
    }

    pub fn show_osd(self: &Rc<Self>, state: OsdState) {
        let (icon, title) = match state.kind {
            OsdKind::Volume if state.muted => ("audio-volume-muted-symbolic", "Muted"),
            OsdKind::Volume if state.value == 0 => ("audio-volume-low-symbolic", "Volume"),
            OsdKind::Volume if state.value < 50 => ("audio-volume-medium-symbolic", "Volume"),
            OsdKind::Volume => ("audio-volume-high-symbolic", "Volume"),
            OsdKind::Brightness => ("display-brightness-symbolic", "Brightness"),
            OsdKind::Workspace => ("focus-windows-symbolic", "Workspace"),
        };
        self.osd_icon.set_icon_name(Some(icon));
        self.osd_title.set_label(title);
        self.osd_progress
            .set_fraction(f64::from(state.value) / 100.0);
        self.osd_value.set_label(&format!("{}", state.value));
        self.osd_active.set(true);
        self.reconcile_view();

        let generation = self.osd_generation.get().wrapping_add(1);
        self.osd_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(state.timeout_ms), move || {
            if let Some(island) = weak.upgrade()
                && island.osd_generation.get() == generation
            {
                island.osd_active.set(false);
                island.reconcile_view();
            }
        });
    }

    /// Displays an incoming notification according to
    /// `notifications.position`. `pill` queues it into the same animated
    /// surface the OSD uses; every other position pushes a toast into the
    /// separate popup window built alongside this island.
    pub fn show_notification(self: &Rc<Self>, notification: Notification) {
        if self.notifications.position == NotificationPosition::Pill {
            self.show_notification_pill(notification);
        } else {
            self.show_notification_toast(notification);
        }
    }

    /// Removes a notification wherever it currently lives: an active or
    /// queued pill entry, or a toast in the popup window. A no-op if `id`
    /// isn't currently tracked in either place.
    pub fn close_notification(self: &Rc<Self>, id: u32) {
        self.remove_toast(id);
        if self
            .notification_current
            .borrow()
            .as_ref()
            .is_some_and(|current| current.id == id)
        {
            self.notification_generation
                .set(self.notification_generation.get().wrapping_add(1));
            self.advance_notification();
        } else {
            self.notification_queue
                .borrow_mut()
                .retain(|queued| queued.id != id);
        }
    }

    /// Rebuilds the dashboard's notification history list and count badge
    /// from the controller's bounded history, most recent first.
    pub fn update_notification_history(&self, history: &[Notification]) {
        self.notification_count
            .set_label(&history.len().to_string());
        clear_box(&self.notification_list);
        if history.is_empty() {
            let placeholder = gtk::Label::new(Some("No notifications yet"));
            placeholder.add_css_class("muted-label");
            placeholder.add_css_class("notification-empty");
            placeholder.set_halign(Align::Start);
            placeholder.set_wrap(true);
            self.notification_list.append(&placeholder);
            return;
        }
        for notification in history {
            self.notification_list
                .append(&self.notification_history_row(notification));
        }
    }

    fn notification_history_row(&self, notification: &Notification) -> gtk::Box {
        let row = gtk::Box::new(Orientation::Horizontal, self.metrics.spacing(8));
        row.add_css_class("notification-row");
        if notification.urgency == Urgency::Critical {
            row.add_css_class("urgency-critical");
        }

        let icon = gtk::Image::new();
        apply_notification_icon(
            &icon,
            notification.app_icon.as_deref(),
            "preferences-system-notifications-symbolic",
        );
        icon.add_css_class("notification-row-icon");
        icon.set_valign(Align::Start);

        let text = gtk::Box::new(Orientation::Vertical, self.metrics.spacing(2));
        text.set_hexpand(true);
        let summary = gtk::Label::new(Some(&notification.summary));
        summary.add_css_class("notification-row-summary");
        summary.set_halign(Align::Start);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&summary);
        if !notification.body.is_empty() {
            let body = gtk::Label::new(Some(&notification.body));
            body.add_css_class("notification-row-body");
            body.set_halign(Align::Start);
            body.set_wrap(true);
            body.set_lines(2);
            body.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&body);
        }
        row.append(&icon);
        row.append(&text);

        let dismiss = gtk::Button::from_icon_name("window-close-symbolic");
        dismiss.add_css_class("notification-row-dismiss");
        dismiss.set_valign(Align::Start);
        let action = self.actions.notification_dismiss.clone();
        let id = notification.id;
        dismiss.connect_clicked(move |_| action(id));
        row.append(&dismiss);

        row
    }

    fn show_notification_pill(self: &Rc<Self>, notification: Notification) {
        self.notification_queue.borrow_mut().push_back(notification);
        if !self.notification_active.get() {
            self.advance_notification();
        }
    }

    /// Pops the next queued notification into the pill, or clears the
    /// active flag and returns to whatever view `reconcile_view` picks next
    /// when the queue is empty.
    ///
    /// In `pill` position a notification always eventually times out, even
    /// one that requested `expire_timeout = 0` ("never expire") -- the pill
    /// has no interactive dismiss control, so a persistent entry would
    /// otherwise block the queue forever. `below-pill`/corner toasts do
    /// honor persistence, since those have a close button.
    fn advance_notification(self: &Rc<Self>) {
        let Some(notification) = self.notification_queue.borrow_mut().pop_front() else {
            self.notification_active.set(false);
            self.notification_current.borrow_mut().take();
            self.reconcile_view();
            return;
        };
        self.render_notification(&notification);
        let timeout_ms = notification
            .timeout
            .resolve(self.notifications.timeout_ms)
            .unwrap_or(self.notifications.timeout_ms);
        let id = notification.id;
        self.notification_active.set(true);
        *self.notification_current.borrow_mut() = Some(notification);
        self.reconcile_view();

        let generation = self.notification_generation.get().wrapping_add(1);
        self.notification_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(timeout_ms), move || {
            if let Some(island) = weak.upgrade()
                && island.notification_generation.get() == generation
            {
                (island.actions.notification_expired)(id);
                island.advance_notification();
            }
        });
    }

    fn render_notification(&self, notification: &Notification) {
        apply_notification_icon(
            &self.notification_icon,
            notification.app_icon.as_deref(),
            "preferences-system-notifications-symbolic",
        );
        self.notification_app.set_label(&notification.summary);
        self.notification_app
            .set_tooltip_text(Some(&notification.app_name));
        self.notification_body.set_label(&notification.body);
        self.notification_body
            .set_visible(!notification.body.is_empty());
    }

    /// Handles a click on the `pill`-position notification view. Invokes
    /// the current notification's default action if it declared one, or
    /// simply dismisses it otherwise -- clicking the pill should always do
    /// *something*, and most senders don't declare any actions at all.
    fn activate_current_notification(self: &Rc<Self>) {
        let Some(notification) = self.notification_current.borrow().clone() else {
            return;
        };
        if let Some(action) = notification.default_action() {
            (self.actions.notification_invoke)(notification.id, action.key.clone());
        } else {
            (self.actions.notification_dismiss)(notification.id);
        }
    }

    /// Right-clicking the pill always dismisses the current notification,
    /// regardless of whether it declared a default action.
    fn dismiss_current_notification(self: &Rc<Self>) {
        let Some(notification) = self.notification_current.borrow().clone() else {
            return;
        };
        (self.actions.notification_dismiss)(notification.id);
    }

    fn show_notification_toast(self: &Rc<Self>, notification: Notification) {
        let Some(toasts) = &self.notification_toasts else {
            return;
        };
        let id = notification.id;
        // `replaces_id` may reference a toast that's already showing.
        self.remove_toast(id);
        let row = self.build_toast_row(&notification);
        toasts.stack.prepend(&row);
        toasts.order.borrow_mut().insert(0, id);

        let timeout_source = notification
            .timeout
            .resolve(self.notifications.timeout_ms)
            .map(|timeout_ms| {
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(Duration::from_millis(timeout_ms), move || {
                    if let Some(island) = weak.upgrade() {
                        (island.actions.notification_expired)(id);
                        island.remove_toast(id);
                    }
                })
            });
        toasts.entries.borrow_mut().insert(
            id,
            ToastEntry {
                row,
                timeout_source,
            },
        );

        toasts.window.set_visible(true);
        toasts.window.present();
        self.trim_toasts();
    }

    fn build_toast_row(self: &Rc<Self>, notification: &Notification) -> gtk::Box {
        let row = gtk::Box::new(Orientation::Horizontal, self.metrics.spacing(10));
        row.add_css_class("notification-toast");
        if notification.urgency == Urgency::Critical {
            row.add_css_class("urgency-critical");
        }

        let icon = gtk::Image::new();
        apply_notification_icon(
            &icon,
            notification.app_icon.as_deref(),
            "preferences-system-notifications-symbolic",
        );
        icon.add_css_class("notification-toast-icon");
        icon.set_valign(Align::Start);

        let text = gtk::Box::new(Orientation::Vertical, self.metrics.spacing(2));
        text.set_hexpand(true);
        let summary = gtk::Label::new(Some(&notification.summary));
        summary.add_css_class("notification-toast-summary");
        summary.set_halign(Align::Start);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&summary);
        if !notification.body.is_empty() {
            let body = gtk::Label::new(Some(&notification.body));
            body.add_css_class("notification-toast-body");
            body.set_halign(Align::Start);
            body.set_wrap(true);
            body.set_lines(3);
            body.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&body);
        }

        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("notification-toast-close");
        close.set_valign(Align::Start);
        let dismiss_action = self.actions.notification_dismiss.clone();
        let id = notification.id;
        close.connect_clicked(move |_| dismiss_action(id));

        row.append(&icon);
        row.append(&text);
        row.append(&close);

        if notification.default_action().is_some() {
            row.add_css_class("notification-toast-clickable");
            let invoke_action = self.actions.notification_invoke.clone();
            let click = GestureClick::new();
            click.connect_released(move |gesture, _, _, _| {
                if gesture.current_button() == 1 {
                    invoke_action(id, "default".to_owned());
                }
            });
            row.add_controller(click);
        }

        row
    }

    fn remove_toast(&self, id: u32) {
        let Some(toasts) = &self.notification_toasts else {
            return;
        };
        if let Some(entry) = toasts.entries.borrow_mut().remove(&id) {
            if let Some(source) = entry.timeout_source {
                source.remove();
            }
            toasts.stack.remove(&entry.row);
        }
        toasts.order.borrow_mut().retain(|existing| *existing != id);
        if toasts.entries.borrow().is_empty() {
            toasts.window.set_visible(false);
        }
    }

    /// Drops the oldest toasts past `notifications.max_visible`.
    fn trim_toasts(&self) {
        if self.notification_toasts.is_none() {
            return;
        }
        let max_visible = self.notifications.max_visible.max(1);
        loop {
            let Some(toasts) = &self.notification_toasts else {
                return;
            };
            let oldest = {
                let order = toasts.order.borrow();
                if order.len() <= max_visible {
                    return;
                }
                *order.last().expect("checked non-empty above")
            };
            self.remove_toast(oldest);
        }
    }

    pub fn update_media(self: &Rc<Self>, state: Option<&MediaState>) {
        let compact_state = state.filter(|state| state.status == PlaybackStatus::Playing);
        if let Some(state) = compact_state {
            self.media_title.set_label(&state.title);
            self.media_title
                .set_tooltip_text(Some(&format!("{} ({})", state.title, state.player)));
            self.media_icon.set_icon_name(state.app_icon.as_deref());
            self.media_icon.set_visible(state.app_icon.is_some());
            self.media_icon
                .set_tooltip_text(Some(&state.player.replace('.', " ")));
            self.media_playing.set(true);
        } else {
            self.media_title.set_label("");
            self.media_title.set_tooltip_text(None);
            self.media_icon.set_visible(false);
            self.media_playing.set(false);
        }
        self.reconcile_view();
        if compact_state.is_some() {
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(island) = weak.upgrade()
                    && island.media_playing.get()
                {
                    island.resize_media();
                    island.reconcile_view();
                }
            });
        }

        let selected = state.map(|state| {
            let requested = self.selected_media_service.borrow().clone();
            let selected = media_state_for_player(state, requested.as_deref());
            *self.selected_media_service.borrow_mut() = Some(selected.service.clone());
            selected
        });
        self.update_player_card(selected.as_ref());
        *self.latest_media.borrow_mut() = selected;
    }

    /// Updates the always-visible media player card in the dashboard. Unlike
    /// the compact pill above (`update_media`'s first half, gated on
    /// `media_playing`), this card is part of the dashboard layout and shows
    /// an idle placeholder when nothing is playing instead of disappearing.
    fn update_player_card(&self, state: Option<&MediaState>) {
        match state {
            Some(state) => {
                self.player_card.remove_css_class("unavailable");
                self.player_title.set_label(&state.title);
                self.player_artist
                    .set_label(state.artist.as_deref().unwrap_or_default());
                self.player_artist.set_visible(state.artist.is_some());
                if let Some(icon) = state.app_icon.as_deref() {
                    self.player_icon.set_icon_name(Some(icon));
                    self.player_icon.set_visible(true);
                } else {
                    self.player_icon.set_visible(false);
                }
                self.player_prev_button.set_sensitive(state.can_go_previous);
                self.player_play_pause_button
                    .set_sensitive(state.can_play || state.can_pause);
                self.player_next_button.set_sensitive(state.can_go_next);
                self.player_switch_row.set_visible(state.players.len() > 1);
                let selected = state
                    .players
                    .iter()
                    .position(|player| player.service == state.service)
                    .unwrap_or(0);
                self.player_switch_label.set_label(&format!(
                    "{} / {}  //  {}",
                    selected + 1,
                    state.players.len(),
                    state.player.replace('.', " ")
                ));
                self.player_progress_base_us.set(state.position_us);
                self.player_length_us.set(state.length_us.unwrap_or(0));
                self.player_progress_started_at
                    .set((state.status == PlaybackStatus::Playing).then(Instant::now));
                self.player_active
                    .set(state.status == PlaybackStatus::Playing);
                self.player_play_pause_button.set_icon_name(
                    if state.status == PlaybackStatus::Playing {
                        "media-playback-pause-symbolic"
                    } else {
                        "media-playback-start-symbolic"
                    },
                );
                self.tick_player_progress();
            }
            None => {
                self.player_card.add_css_class("unavailable");
                self.player_title.set_label("Nothing playing");
                self.player_artist.set_label("");
                self.player_artist.set_visible(false);
                self.player_icon.set_visible(false);
                self.player_prev_button.set_sensitive(false);
                self.player_play_pause_button.set_sensitive(false);
                self.player_next_button.set_sensitive(false);
                self.player_switch_row.set_visible(false);
                self.player_progress.set_fraction(0.0);
                self.player_elapsed_label.set_label("--:--");
                self.player_duration_label.set_label("--:--");
                self.player_active.set(false);
                self.player_progress_started_at.set(None);
            }
        }
    }

    /// Advances the player card's progress bar between MPRIS updates by
    /// interpolating from the last known position using a local clock,
    /// rather than polling MPRIS for `Position` on a timer.
    fn tick_player_progress(&self) {
        let elapsed_us = self
            .player_progress_started_at
            .get()
            .filter(|_| self.player_active.get())
            .map_or(0, |started| started.elapsed().as_micros() as i64);
        let position_us = (self.player_progress_base_us.get() + elapsed_us).max(0);
        let length_us = self.player_length_us.get();
        if length_us > 0 {
            let position_us = position_us.min(length_us);
            self.player_progress
                .set_fraction((position_us as f64 / length_us as f64).clamp(0.0, 1.0));
            self.player_duration_label
                .set_label(&format_media_time(length_us));
        } else {
            self.player_progress.set_fraction(0.0);
            self.player_duration_label.set_label("--:--");
        }
        self.player_elapsed_label
            .set_label(&format_media_time(position_us));
    }

    fn start_player_progress_timer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(500), move || {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            island.tick_player_progress();
            glib::ControlFlow::Continue
        });
    }

    pub fn update_visualizer(&self, levels: VisualizerLevels) {
        *self.media_levels.borrow_mut() = levels;
        self.media_visualizer.queue_draw();
    }

    pub fn update_hyprland(self: &Rc<Self>, snapshot: &HyprlandSnapshot) {
        *self.latest_hyprland.borrow_mut() = snapshot.clone();
        let monitor = snapshot.monitor(&self.monitor_name);
        let active_workspace = monitor.map(|monitor| monitor.active_workspace.id);
        self.active_eyebrow.set_label(&format!(
            "{}  //  WORKSPACE {}",
            self.monitor_name,
            active_workspace
                .map(|id| id.to_string())
                .unwrap_or_else(|| "--".into())
        ));

        let active_window = snapshot
            .active_window
            .as_ref()
            .filter(|window| monitor.is_some_and(|monitor| window.monitor == monitor.id));
        self.active_title.set_label(
            active_window
                .map(|window| window.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or("Quiet desktop"),
        );

        clear_box(&self.compact_workspaces);
        clear_box(&self.media_workspaces);
        while let Some(child) = self.workspace_row.first_child() {
            let child = child
                .downcast::<gtk::FlowBoxChild>()
                .expect("flow box children are wrapped by GTK");
            self.workspace_row.remove(&child);
        }
        let workspaces = snapshot.workspaces_for(&self.monitor_name);
        for workspace in workspaces
            .iter()
            .filter(|workspace| workspace.windows > 0 || Some(workspace.id) == active_workspace)
            .take(7)
        {
            for container in [&self.compact_workspaces, &self.media_workspaces] {
                let dot = gtk::Button::new();
                dot.add_css_class("workspace-dot");
                dot.set_tooltip_text(Some(&format!("Workspace {}", workspace.name)));
                if workspace.windows > 0 {
                    dot.add_css_class("occupied");
                }
                if Some(workspace.id) == active_workspace {
                    dot.add_css_class("active");
                }
                let actions = self.actions.clone();
                let monitor_name = self.monitor_name.clone();
                let workspace_id = workspace.id;
                dot.connect_clicked(move |_| {
                    (actions.switch_workspace)(&monitor_name, workspace_id)
                });
                container.append(&dot);
            }
        }

        for workspace in workspaces.into_iter().take(10) {
            let button = gtk::Button::with_label(&workspace.name);
            button.add_css_class("workspace-button");
            if workspace.windows > 0 {
                button.add_css_class("occupied");
            }
            if Some(workspace.id) == active_workspace {
                button.add_css_class("active");
            }
            let actions = self.actions.clone();
            let monitor_name = self.monitor_name.clone();
            let workspace_id = workspace.id;
            button
                .connect_clicked(move |_| (actions.switch_workspace)(&monitor_name, workspace_id));
            self.workspace_row.insert(&button, -1);
        }
        self.resize_compact();
    }

    pub fn update_system(self: &Rc<Self>, snapshot: &SystemSnapshot) {
        self.updating_controls.set(true);
        if let Some(audio) = snapshot.audio {
            self.volume_scale.set_value(f64::from(audio.percent));
            self.volume_value.set_label(&format!("{}%", audio.percent));
            self.volume_scale.set_sensitive(true);
        } else {
            self.volume_value.set_label("--");
            self.volume_scale.set_sensitive(false);
        }

        if let Some(brightness) = &snapshot.brightness {
            self.brightness_scale
                .set_value(f64::from(brightness.percent));
            self.brightness_value
                .set_label(&format!("{}%", brightness.percent));
            self.brightness_row.set_visible(true);
            self.brightness_row.remove_css_class("unavailable");
            self.brightness_scale.set_sensitive(true);
        } else {
            self.brightness_value.set_label("--");
            self.brightness_row.set_visible(false);
            self.brightness_scale.set_sensitive(false);
        }

        if let Some(battery) = &snapshot.battery {
            self.battery_icon
                .set_icon_name(Some(battery_icon_name(battery.percent, &battery.status)));
            self.battery_label
                .set_label(&format!("{}%", battery.percent));
            self.compact_battery
                .set_label(&format!("{}%", battery.percent));
            self.battery_chip.set_visible(true);
            self.compact_battery.set_visible(true);
        } else {
            self.battery_chip.set_visible(false);
            self.compact_battery.set_visible(false);
        }
        self.updating_controls.set(false);
        self.resize_compact();
    }

    /// Rebuilds the tray row from a fresh snapshot, the same
    /// clear-and-rebuild approach `update_hyprland` uses for workspace
    /// dots -- tray churn is rare enough that reusing widgets isn't worth
    /// the bookkeeping.
    pub fn update_tray(self: &Rc<Self>, items: &[TrayItem]) {
        clear_box(&self.compact_tray);
        clear_box(&self.media_tray);
        for item in items {
            // A widget can only have one parent, so each pill gets its own
            // freshly built icon -- the same duplication `update_hyprland`
            // already does for `compact_workspaces`/`media_workspaces`.
            self.compact_tray.append(&self.build_tray_icon(item));
            self.media_tray.append(&self.build_tray_icon(item));
        }
        self.tray_item_count.set(items.len());
        self.resize_compact();
        self.resize_media();
    }

    fn build_tray_icon(self: &Rc<Self>, item: &TrayItem) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("tray-icon");
        button.set_has_frame(false);
        if item.status == TrayStatus::NeedsAttention {
            button.add_css_class("needs-attention");
        }
        if let Some(tooltip) = item.tooltip.as_deref().filter(|text| !text.is_empty()) {
            button.set_tooltip_text(Some(tooltip));
        }

        let image = gtk::Image::new();
        image.set_pixel_size(self.metrics.tray_icon_size);
        apply_tray_icon(&image, &item.icon);
        button.set_child(Some(&image));

        // Primary click goes through `GtkButton`'s own `clicked` signal
        // rather than an extra `GestureClick`: the button already has an
        // internal click gesture that claims the primary-button sequence,
        // so a second gesture watching the same button loses the claim and
        // never fires. Middle/secondary are free for gestures below, but
        // each is restricted to exactly the button it handles -- a
        // catch-all `button(0)` gesture competes with that same internal
        // one (which is itself "any button", it just only *emits* for the
        // primary) and can swallow those clicks too.
        let weak = Rc::downgrade(self);
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        let primary_menu_path = item.menu_path.clone();
        // Items advertising `ItemIsMenu` declare they have no meaningful
        // activation at all and expect their menu on a plain left click.
        let item_is_menu = item.item_is_menu;
        button.connect_clicked(move |button| {
            let Some(island) = weak.upgrade() else {
                return;
            };
            match primary_menu_path.clone().filter(|_| item_is_menu) {
                Some(menu_path) => {
                    island.open_tray_menu(button.clone(), service.clone(), menu_path);
                }
                None => {
                    // The spec's x/y are screen coordinates used by items
                    // that position their own menu; there is no way to get
                    // those for a Wayland client, and items treat 0,0 as
                    // "unspecified".
                    (island.actions.tray_activate)(service.clone(), object_path.clone(), 0, 0);
                }
            }
        });

        let middle_click = GestureClick::new();
        middle_click.set_button(gdk::BUTTON_MIDDLE);
        let weak = Rc::downgrade(self);
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        middle_click.connect_released(move |_, _, _, _| {
            if let Some(island) = weak.upgrade() {
                (island.actions.tray_secondary_activate)(
                    service.clone(),
                    object_path.clone(),
                    0,
                    0,
                );
            }
        });
        button.add_controller(middle_click);

        let context_click = GestureClick::new();
        context_click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        let menu_path = item.menu_path.clone();
        let button_weak = button.downgrade();
        context_click.connect_pressed(move |gesture, _, _, _| {
            let Some(island) = weak.upgrade() else {
                return;
            };
            // Claim the sequence so the press can't also bubble up to the
            // pill's own click gesture, which would toggle the dashboard.
            gesture.set_state(gtk::EventSequenceState::Claimed);
            match (menu_path.clone(), button_weak.upgrade()) {
                (Some(menu_path), Some(button)) => {
                    island.open_tray_menu(button, service.clone(), menu_path);
                }
                _ => {
                    (island.actions.tray_context_menu)(service.clone(), object_path.clone(), 0, 0);
                }
            }
        });
        button.add_controller(context_click);

        let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
        let action = self.actions.tray_scroll.clone();
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        scroll.connect_scroll(move |_, dx, dy| {
            let (delta, horizontal) = if dx.abs() > dy.abs() {
                (dx, true)
            } else {
                (dy, false)
            };
            if delta != 0.0 {
                action(
                    service.clone(),
                    object_path.clone(),
                    (delta * 10.0).round() as i32,
                    horizontal,
                );
            }
            glib::Propagation::Proceed
        });
        button.add_controller(scroll);

        button
    }

    /// Fetches a right-clicked item's DBusMenu layout on a throwaway
    /// thread (mirroring how `preview`/`theme` results are round-tripped
    /// back onto the GTK thread elsewhere) and shows it as a popover
    /// anchored to the icon that was clicked.
    fn open_tray_menu(self: &Rc<Self>, anchor: gtk::Button, service: String, menu_path: String) {
        let (sender, receiver) = async_channel::bounded(1);
        let fetch_service = service.clone();
        let fetch_menu_path = menu_path.clone();
        thread::spawn(move || {
            let result = tray::menu_layout(&fetch_service, &fetch_menu_path);
            let _ = sender.send_blocking(result.map_err(|error| error.to_string()));
        });
        let island = self.clone();
        glib::MainContext::default().spawn_local(async move {
            if let Ok(Ok(menu)) = receiver.recv().await {
                island.show_tray_menu(&anchor, &service, &menu_path, &menu);
            }
        });
    }

    fn show_tray_menu(
        self: &Rc<Self>,
        anchor: &gtk::Button,
        service: &str,
        menu_path: &str,
        menu: &TrayMenuItem,
    ) {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        popover.add_css_class("tray-menu");
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_has_arrow(false);
        let content = self.build_tray_menu_box(&popover, service, menu_path, &menu.children);
        popover.set_child(Some(&content));

        // Pin the tray open for as long as the menu is: popping up takes a
        // pointer grab, so the pill immediately sees a `leave` and would
        // otherwise collapse the row this popover is anchored to.
        self.tray_menu_open.set(true);
        // An autohide popover needs to be able to take focus to grab, which
        // a `KeyboardMode::None` layer surface never can; without this the
        // menu is dismissed the moment it appears.
        self.refresh_keyboard_mode();
        self.resize_compact();
        self.resize_media();

        let weak = Rc::downgrade(self);
        popover.connect_closed(move |popover| {
            popover.unparent();
            if let Some(island) = weak.upgrade() {
                island.tray_menu_open.set(false);
                island.refresh_keyboard_mode();
                island.resize_compact();
                island.resize_media();
            }
        });
        popover.popup();
    }

    /// Re-applies the keyboard mode the current state wants. Split out of
    /// `set_view` so showing/dismissing a tray menu can borrow the surface's
    /// focus without having to know what the active view expects.
    fn refresh_keyboard_mode(&self) {
        let mode = match self.current_view.get() {
            View::Search | View::Weather => KeyboardMode::Exclusive,
            _ if self.tray_menu_open.get() => KeyboardMode::OnDemand,
            _ => KeyboardMode::None,
        };
        self.window.set_keyboard_mode(mode);
    }

    fn build_tray_menu_box(
        self: &Rc<Self>,
        popover: &gtk::Popover,
        service: &str,
        menu_path: &str,
        items: &[TrayMenuItem],
    ) -> gtk::Box {
        let list = gtk::Box::new(Orientation::Vertical, 0);
        list.add_css_class("tray-menu-list");
        for entry in items {
            if !entry.visible {
                continue;
            }
            if entry.separator {
                list.append(&gtk::Separator::new(Orientation::Horizontal));
                continue;
            }
            if entry.children.is_empty() {
                // An explicit label child rather than `Button::with_label`:
                // a button centers its child, and menu entries read far
                // better left-aligned against a ragged-width list.
                let label = gtk::Label::new(Some(&entry.label));
                label.set_halign(Align::Start);
                label.set_xalign(0.0);
                let button = gtk::Button::new();
                button.set_child(Some(&label));
                button.add_css_class("tray-menu-item");
                button.set_has_frame(false);
                button.set_sensitive(entry.enabled);
                if entry.checked == Some(true) {
                    button.add_css_class("checked");
                }
                let action = self.actions.tray_menu_event.clone();
                let service = service.to_owned();
                let menu_path = menu_path.to_owned();
                let id = entry.id;
                let popover_weak = popover.downgrade();
                button.connect_clicked(move |_| {
                    action(service.clone(), menu_path.clone(), id);
                    if let Some(popover) = popover_weak.upgrade() {
                        popover.popdown();
                    }
                });
                list.append(&button);
            } else {
                let expander = gtk::Expander::new(Some(&entry.label));
                expander.add_css_class("tray-menu-submenu");
                let submenu =
                    self.build_tray_menu_box(popover, service, menu_path, &entry.children);
                expander.set_child(Some(&submenu));
                list.append(&expander);
            }
        }
        list
    }

    /// Redraws any active custom Cairo drawing after a theme change --
    /// swapping the CSS provider doesn't trigger that on its own.
    pub fn update_palette(&self) {
        for icon in self.weather_icons.borrow().iter() {
            icon.queue_draw();
        }
    }

    /// Forces a fresh commit; the compositor stops compositing this surface
    /// while the session is locked, so it may not resume until we submit one.
    pub fn recomposite(&self) {
        self.window.queue_draw();
    }

    pub fn update_shell_config(&self, config: &ShellConfig, animations_enabled: bool) {
        self.window
            .set_margin(Edge::Top, self.metrics.spacing(config.top_margin));
        self.window
            .set_exclusive_zone(self.metrics.spacing(config.exclusive_zone));
        self.animation_ms.set(config.animation_ms);
        self.animations_enabled.set(animations_enabled);
    }

    pub fn destroy(&self) {
        self.dismiss_window.close();
        self.window.close();
        if let Some(toasts) = &self.notification_toasts {
            toasts.window.close();
        }
    }

    fn connect_interactions(self: &Rc<Self>, buttons: OverlayButtons<'_>, dismiss_area: &gtk::Box) {
        let OverlayButtons {
            close_button,
            search_button,
            weather_button,
            search_back_button,
            search_reload_button,
            weather_back_button,
        } = buttons;
        let click = GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, _, _| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
            {
                island.toggle();
            }
        });
        self.compact.add_controller(click);

        let motion = EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_enter(move |_, _, _| {
            if let Some(island) = weak.upgrade() {
                island.set_tray_hovered(true);
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(island) = weak.upgrade() {
                island.set_tray_hovered(false);
            }
        });
        self.compact.add_controller(motion);

        let click = GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, _, _| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
            {
                island.toggle();
            }
        });
        self.media.add_controller(click);

        let motion = EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_enter(move |_, _, _| {
            if let Some(island) = weak.upgrade() {
                island.set_tray_hovered(true);
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(island) = weak.upgrade() {
                island.set_tray_hovered(false);
            }
        });
        self.media.add_controller(motion);

        let click = GestureClick::new();
        // `GestureSingle::button` defaults to the primary button only; ask
        // for every button so the right-click dismiss below actually gets
        // delivered (`current_button()` still filters which one fired).
        click.set_button(0);
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, _, _| {
            let Some(island) = weak.upgrade() else {
                return;
            };
            match gesture.current_button() {
                gdk::BUTTON_PRIMARY => island.activate_current_notification(),
                gdk::BUTTON_SECONDARY => island.dismiss_current_notification(),
                _ => {}
            }
        });
        self.notification.add_controller(click);

        let weak = Rc::downgrade(self);
        search_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.open_search();
            }
        });

        let weak = Rc::downgrade(self);
        search_back_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.search_open.set(false);
                island.dashboard_open.set(true);
                island.reconcile_view();
            }
        });

        let weak = Rc::downgrade(self);
        weather_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.open_weather();
            }
        });

        let weak = Rc::downgrade(self);
        weather_back_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.weather_open.set(false);
                island.dashboard_open.set(true);
                island.reconcile_view();
            }
        });

        let weak = Rc::downgrade(self);
        search_reload_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.search_status.set_label("RELOADING TARRAGON");
                (island.actions.tarragon_reload)();
            }
        });

        let weak = Rc::downgrade(self);
        self.search_plugin_toggle.connect_toggled(move |button| {
            if let Some(island) = weak.upgrade() {
                if button.is_active() {
                    island.search_stack.set_visible_child_name("plugins");
                    // Rendering is skipped while the pane is hidden, so build
                    // it now that it is about to be shown.
                    island.render_plugin_list();
                    island.show_plugin_summary();
                    (island.actions.tarragon_status)();
                } else {
                    island.search_stack.set_visible_child_name("results");
                    let snapshot = island.search_snapshot.borrow().clone();
                    if let Some(snapshot) = snapshot {
                        island.update_tarragon_results(&snapshot);
                    } else {
                        island.search_status.set_label("READY  //  TYPE TO SEARCH");
                    }
                    island.search_entry.grab_focus();
                }
            }
        });

        // GtkSearchEntry withholds `search-changed` for 150ms by default. That
        // sits on top of our own debounce, so disable it and let
        // `schedule_search` own the coalescing window.
        self.search_entry.set_search_delay(0);
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |entry| {
            if let Some(island) = weak.upgrade() {
                island.schedule_search(entry.text().to_string());
            }
        });

        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |_| {
            if let Some(island) = weak.upgrade() {
                let index = island
                    .search_results
                    .selected_row()
                    .map_or(0, |row| row.index());
                island.activate_search_result(index);
            }
        });

        let weak = Rc::downgrade(self);
        self.search_results.connect_row_activated(move |_, row| {
            if let Some(island) = weak.upgrade() {
                island.activate_search_result(row.index());
            }
        });

        let weak = Rc::downgrade(self);
        self.search_results.connect_row_selected(move |_, row| {
            if let (Some(island), Some(row)) = (weak.upgrade(), row) {
                island.update_search_preview(row.index());
            }
        });

        // Attached to the window itself (not a specific view) so Escape closes
        // whatever overlay currently holds keyboard focus -- the launcher today,
        // and any future keyboard-focused widget that extends `close()`.
        let overlay_keys = gtk::EventControllerKey::new();
        overlay_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        overlay_keys.connect_key_pressed(move |_, key, _, _| {
            let Some(island) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let results_active = island.search_open.get()
                && island.search_stack.visible_child_name().as_deref() == Some("results");
            // t0: capture the moment a text-mutating key arrives, before the
            // entry's own search-delay and the debounce timer run.
            if island.search_open.get()
                && (key.to_unicode().is_some()
                    || matches!(key, gdk::Key::BackSpace | gdk::Key::Delete))
            {
                crate::latency::mark_keystroke();
            }
            match key {
                gdk::Key::Escape => {
                    island.close();
                    glib::Propagation::Stop
                }
                gdk::Key::Down if results_active => {
                    island.move_search_selection(1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up if results_active => {
                    island.move_search_selection(-1);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(overlay_keys);

        // t5: the frame carrying the updated results reached the compositor.
        // Only wired when tracing is on, so normal runs pay nothing.
        if crate::latency::enabled() {
            self.window.connect_realize(|window| {
                if let Some(clock) = window.frame_clock() {
                    clock.connect_after_paint(|_| crate::latency::mark_paint());
                }
            });
        }

        for workspaces in [&self.compact_workspaces, &self.media_workspaces] {
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::BOTH_AXES
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            let weak = Rc::downgrade(self);
            scroll.connect_scroll(move |_, dx, dy| {
                if let Some(island) = weak.upgrade() {
                    island.scroll_workspace(dx, dy);
                }
                glib::Propagation::Stop
            });
            workspaces.add_controller(scroll);
        }

        let media_scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let weak = Rc::downgrade(self);
        media_scroll.connect_scroll(move |_, dx, dy| {
            if let Some(island) = weak.upgrade() {
                island.scroll_volume(dx, dy);
            }
            glib::Propagation::Stop
        });
        self.media_center.add_controller(media_scroll);

        let weak = Rc::downgrade(self);
        self.surface
            .hadjustment()
            .connect_value_changed(move |adjustment| {
                if let Some(island) = weak.upgrade() {
                    let target = f64::from(
                        (island.metrics.search_width - island.geometry.get().width.round() as i32)
                            / 2,
                    );
                    if (adjustment.value() - target).abs() > f64::EPSILON {
                        adjustment.set_value(target);
                    }
                }
            });
        self.surface
            .vadjustment()
            .connect_value_changed(|adjustment| {
                if adjustment.value().abs() > f64::EPSILON {
                    adjustment.set_value(0.0);
                }
            });

        let dismiss_click = GestureClick::new();
        let weak = Rc::downgrade(self);
        dismiss_click.connect_released(move |gesture, _, _, _| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
            {
                island.close();
            }
        });
        dismiss_area.add_controller(dismiss_click);

        let header_click = GestureClick::new();
        header_click.set_propagation_phase(gtk::PropagationPhase::Bubble);
        let weak = Rc::downgrade(self);
        header_click.connect_released(move |gesture, _, _, y| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
                && y <= f64::from(island.metrics.spacing(78))
            {
                island.close();
            }
        });
        self.dashboard.add_controller(header_click);

        let weather_header_click = GestureClick::new();
        weather_header_click.set_propagation_phase(gtk::PropagationPhase::Bubble);
        let weak = Rc::downgrade(self);
        weather_header_click.connect_released(move |gesture, _, _, y| {
            if gesture.current_button() == 1
                && y <= f64::from(
                    weak.upgrade()
                        .map_or(78, |island| island.metrics.spacing(78)),
                )
                && let Some(island) = weak.upgrade()
            {
                island.close();
            }
        });
        self.weather.add_controller(weather_header_click);

        let weak = Rc::downgrade(self);
        close_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.close();
            }
        });

        let weak = Rc::downgrade(self);
        self.volume_scale.connect_value_changed(move |scale| {
            if let Some(island) = weak.upgrade()
                && !island.updating_controls.get()
            {
                let value = scale.value().round().clamp(0.0, 100.0) as u8;
                island.volume_value.set_label(&format!("{value}%"));
                let generation = island.volume_generation.get().wrapping_add(1);
                island.volume_generation.set(generation);
                let weak = Rc::downgrade(&island);
                glib::timeout_add_local_once(Duration::from_millis(70), move || {
                    if let Some(island) = weak.upgrade()
                        && island.volume_generation.get() == generation
                    {
                        (island.actions.set_volume)(value);
                    }
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.brightness_scale.connect_value_changed(move |scale| {
            if let Some(island) = weak.upgrade()
                && !island.updating_controls.get()
            {
                let value = scale.value().round().clamp(0.0, 100.0) as u8;
                island.brightness_value.set_label(&format!("{value}%"));
                let generation = island.brightness_generation.get().wrapping_add(1);
                island.brightness_generation.set(generation);
                let weak = Rc::downgrade(&island);
                glib::timeout_add_local_once(Duration::from_millis(70), move || {
                    if let Some(island) = weak.upgrade()
                        && island.brightness_generation.get() == generation
                    {
                        (island.actions.set_brightness)(value);
                    }
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.player_prev_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade()
                && let Some(service) = island
                    .latest_media
                    .borrow()
                    .as_ref()
                    .map(|state| state.service.clone())
            {
                (island.actions.media_previous)(service);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_play_pause_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade()
                && let Some(service) = island
                    .latest_media
                    .borrow()
                    .as_ref()
                    .map(|state| state.service.clone())
            {
                (island.actions.media_play_pause)(service);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_next_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade()
                && let Some(service) = island
                    .latest_media
                    .borrow()
                    .as_ref()
                    .map(|state| state.service.clone())
            {
                (island.actions.media_next)(service);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_switch_prev.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.switch_media_player(-1);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_switch_next.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.switch_media_player(1);
            }
        });
    }

    fn switch_media_player(&self, direction: i32) {
        let Some(state) = self.latest_media.borrow().clone() else {
            return;
        };
        if state.players.len() < 2 {
            return;
        }
        let current = state
            .players
            .iter()
            .position(|player| player.service == state.service)
            .unwrap_or(0);
        let next = (current as i32 + direction).rem_euclid(state.players.len() as i32) as usize;
        let selected_state = media_state_for_player(&state, Some(&state.players[next].service));
        *self.selected_media_service.borrow_mut() = Some(selected_state.service.clone());
        self.update_player_card(Some(&selected_state));
        *self.latest_media.borrow_mut() = Some(selected_state);
    }

    fn scroll_workspace(&self, dx: f64, dy: f64) {
        let direction = dominant_scroll_direction(dx, dy);
        let Some(active) = self
            .latest_hyprland
            .borrow()
            .monitor(&self.monitor_name)
            .map(|monitor| monitor.active_workspace.id)
        else {
            return;
        };
        let target = if direction > 0 {
            active.saturating_add(1)
        } else if direction < 0 {
            (active - 1).max(1)
        } else {
            active
        };
        if target != active {
            (self.actions.switch_workspace)(&self.monitor_name, target);
        }
    }

    fn scroll_volume(&self, dx: f64, dy: f64) {
        let direction = dominant_scroll_direction(dx, dy);
        if direction == 0 || !self.volume_scale.is_sensitive() {
            return;
        }
        let delta = if direction > 0 { -5.0 } else { 5.0 };
        self.volume_scale
            .set_value((self.volume_scale.value() + delta).clamp(0.0, 100.0));
    }

    pub fn open_search(self: &Rc<Self>) {
        self.clear_osd();
        self.dashboard_open.set(false);
        self.weather_open.set(false);
        self.search_open.set(true);
        self.search_plugin_toggle.set_active(false);
        self.search_stack.set_visible_child_name("results");
        self.search_entry.set_text("");
        while let Some(child) = self.search_results.first_child() {
            self.search_results.remove(&child);
        }
        *self.search_snapshot.borrow_mut() = None;
        self.clear_search_preview();
        if self.search_connected.get() {
            self.search_status
                .set_label(if self.search_selection_pending.get() {
                    "ACTION STILL PENDING"
                } else {
                    "READY  //  TYPE TO SEARCH"
                });
            (self.actions.tarragon_status)();
        }
        self.reconcile_view();
        let entry = self.search_entry.clone();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
        });
    }

    fn schedule_search(self: &Rc<Self>, text: String) {
        let generation = self.search_generation.get().wrapping_add(1);
        self.search_generation.set(generation);
        if !self.search_connected.get() {
            return;
        }
        if text.trim().is_empty() {
            *self.search_dispatched.borrow_mut() = None;
            *self.search_snapshot.borrow_mut() = None;
            clear_list_box(&self.search_results);
            self.clear_search_preview();
            self.search_status.set_label("READY  //  TYPE TO SEARCH");
            return;
        }
        self.search_status.set_label("SEARCHING");

        // Leading edge: nothing dispatched recently, so send immediately.
        let now = Instant::now();
        let ready = self
            .last_search_dispatch
            .get()
            .is_none_or(|last| now.duration_since(last) >= SEARCH_THROTTLE);
        if ready {
            self.dispatch_search(text);
            return;
        }

        // Inside the window: coalesce until it closes. The generation check
        // means only the final keystroke of the burst is actually sent.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(SEARCH_THROTTLE, move || {
            if let Some(island) = weak.upgrade()
                && island.search_open.get()
                && island.search_generation.get() == generation
            {
                island.dispatch_search(text);
            }
        });
    }

    fn dispatch_search(self: &Rc<Self>, text: String) {
        crate::latency::mark_dispatch();
        self.last_search_dispatch.set(Some(Instant::now()));
        *self.search_dispatched.borrow_mut() = Some(text.clone());
        (self.actions.search)(text);
    }

    fn move_search_selection(&self, offset: i32) {
        let count = self
            .search_snapshot
            .borrow()
            .as_ref()
            .map_or(0, |snapshot| snapshot.list.len() as i32);
        if count == 0 {
            return;
        }
        let current = self
            .search_results
            .selected_row()
            .map_or(0, |row| row.index());
        let target = (current + offset).clamp(0, count - 1);
        if let Some(row) = self.search_results.row_at_index(target) {
            self.search_results.select_row(Some(&row));
            row.grab_focus();
            self.search_entry.grab_focus();
        }
    }

    fn activate_search_result(self: &Rc<Self>, index: i32) {
        let Some(snapshot) = self.search_snapshot.borrow().clone() else {
            return;
        };
        let Some(result) = snapshot.list.get(index.max(0) as usize) else {
            return;
        };
        let Some(action) = result.default_action() else {
            self.search_status.set_label("RESULT HAS NO ACTION");
            return;
        };
        let action_name = action.name.clone();
        self.execute_search_action(index, &action_name);
    }

    fn execute_search_action(self: &Rc<Self>, index: i32, action: &str) {
        if self.search_selection_pending.replace(true) {
            return;
        }
        let Some(snapshot) = self.search_snapshot.borrow().clone() else {
            self.search_selection_pending.set(false);
            return;
        };
        let Some(result) = snapshot.list.get(index.max(0) as usize) else {
            self.search_selection_pending.set(false);
            return;
        };
        (self.actions.select)(TarragonSelection {
            query_id: snapshot.query_id,
            plugin: result.plugin.clone(),
            result_id: result.id.clone(),
            action: action.to_owned(),
        });
        self.search_status.set_label("RUNNING ACTION");
        let generation = self.search_action_generation.get().wrapping_add(1);
        self.search_action_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            if let Some(island) = weak.upgrade()
                && island.search_action_generation.get() == generation
                && island.search_selection_pending.get()
                && island.search_open.get()
            {
                island.search_status.set_label("ACTION STILL PENDING");
            }
        });
    }

    fn start_clock(self: &Rc<Self>) {
        self.update_clock();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            island.update_clock();
            glib::ControlFlow::Continue
        });
    }

    fn update_clock(&self) {
        if let Ok(now) = glib::DateTime::now_local() {
            if let Ok(time) = now.format("%H:%M") {
                self.compact_clock.set_label(&time);
                self.media_clock.set_label(&time);
                self.hero_time.set_label(&time);
            }
            if let Ok(date) = now.format("%A, %d %B") {
                self.hero_date.set_label(&date);
            }
        }
    }

    fn clear_osd(&self) {
        self.osd_active.set(false);
        self.osd_generation
            .set(self.osd_generation.get().wrapping_add(1));
    }

    fn reconcile_view(self: &Rc<Self>) {
        let view = if self.notification_active.get() {
            View::Notification
        } else if self.osd_active.get() {
            View::Osd
        } else if self.search_open.get() {
            View::Search
        } else if self.weather_open.get() {
            View::Weather
        } else if self.dashboard_open.get() {
            View::Dashboard
        } else if self.media_playing.get() {
            View::Media
        } else {
            View::Compact
        };
        self.set_view(view);
    }

    fn resize_media(self: &Rc<Self>) {
        self.media.set_width_request(-1);
        self.media_title
            .set_ellipsize(gtk::pango::EllipsizeMode::None);
        let (_, title_width, _, _) = self.media_title.measure(Orientation::Horizontal, -1);
        self.media_title
            .set_ellipsize(gtk::pango::EllipsizeMode::End);
        let (_, workspace_width, _, _) = self.media_workspaces.measure(Orientation::Horizontal, -1);
        let (_, clock_width, _, _) = self.media_clock.measure(Orientation::Horizontal, -1);
        let (_, visualizer_width, _, _) =
            self.media_visualizer.measure(Orientation::Horizontal, -1);
        let icon_width = if self.media_icon.is_visible() {
            self.media_icon.measure(Orientation::Horizontal, -1).1
        } else {
            0
        };

        // The tray is a sibling of the `CenterBox` rather than part of its
        // `end` slot, so it only adds to the total -- it deliberately does
        // not participate in the `start`/`end` symmetry that keeps the
        // track title centered.
        let tray_visible = self.tray_visible();
        self.media_tray.set_visible(tray_visible);
        let tray_width = if tray_visible {
            self.metrics.spacing(8)
                + measure_clamped(&self.media_tray, self.metrics.compact_tray_max_width)
        } else {
            0
        };

        let center_gaps = self.metrics.spacing(if icon_width > 0 { 14 } else { 7 });
        let natural = self.metrics.spacing(36)
            + workspace_width.max(clock_width) * 2
            + icon_width
            + visualizer_width
            + center_gaps
            + title_width
            + tray_width;
        let width = natural.clamp(self.metrics.compact_width, self.metrics.media_max_width);
        self.media
            .set_size_request(width, self.metrics.media_height);
        self.content.move_(
            &self.media,
            f64::from((self.metrics.search_width - width) / 2),
            0.0,
        );
        self.media_width.set(width);

        if self.current_view.get() == View::Media {
            self.set_view(View::Media);
        }
    }

    /// Recomputes the compact pill's width from the combined (individually
    /// capped) natural width of its children, and repositions it within
    /// `content` to match, the same way `resize_media` does for the media
    /// pill. Called whenever a child's content changes (workspaces,
    /// battery, tray) or the tray's hover-visibility toggles.
    fn resize_compact(self: &Rc<Self>) {
        let workspaces_width = measure_clamped(
            &self.compact_workspaces,
            self.metrics.compact_workspaces_max_width,
        );
        let clock_width =
            measure_clamped(&self.compact_clock, self.metrics.compact_clock_max_width);
        let battery_width = if self.compact_battery.is_visible() {
            measure_clamped(
                &self.compact_battery,
                self.metrics.compact_battery_max_width,
            )
        } else {
            0
        };

        let tray_visible = self.tray_visible();
        self.compact_tray.set_visible(tray_visible);
        let tray_width = if tray_visible {
            measure_clamped(&self.compact_tray, self.metrics.compact_tray_max_width)
        } else {
            0
        };

        let segments = 2 + i32::from(battery_width > 0) + i32::from(tray_width > 0);
        let spacing = self.metrics.spacing(10) * (segments - 1).max(0);
        // Matches `.compact-content { padding: 0 15px; }` (both sides).
        let padding = self.metrics.spacing(30);
        let natural =
            workspaces_width + clock_width + battery_width + tray_width + spacing + padding;
        let width = natural.clamp(self.metrics.compact_min_width, self.metrics.media_max_width);

        self.compact
            .set_size_request(width, self.metrics.compact_height);
        self.content.move_(
            &self.compact,
            f64::from((self.metrics.search_width - width) / 2),
            0.0,
        );
        self.compact_width.set(width);

        if self.current_view.get() == View::Compact {
            self.set_view(View::Compact);
        }
    }

    /// Whether the tray row should be shown: at least one item exists, and
    /// either the pill (compact or media, whichever is active) is hovered
    /// or one of the items' menus is currently open.
    fn tray_visible(&self) -> bool {
        (self.tray_hovered.get() || self.tray_menu_open.get()) && self.tray_item_count.get() > 0
    }

    /// Hides/reveals the tray row on hover, and resizes both pills (only
    /// the currently active one animates; the other silently follows so
    /// it's already correct if the view switches while hovered).
    fn set_tray_hovered(self: &Rc<Self>, hovered: bool) {
        if self.tray_hovered.get() == hovered {
            return;
        }
        self.tray_hovered.set(hovered);
        self.resize_compact();
        self.resize_media();
    }

    fn geometry_for_view(&self, view: View) -> Geometry {
        Geometry::for_view(
            view,
            self.metrics,
            self.media_width.get(),
            self.compact_width.get(),
        )
    }

    fn set_view(self: &Rc<Self>, view: View) {
        let target = self.geometry_for_view(view);
        if self.current_view.get() == view && self.geometry.get() == target {
            return;
        }
        self.current_view.set(view);
        if matches!(view, View::Dashboard | View::Search | View::Weather) {
            self.dismiss_window.present();
            self.window.present();
        } else {
            self.dismiss_window.set_visible(false);
        }

        self.compact.set_visible(true);
        self.media.set_visible(true);
        self.dashboard.set_visible(true);
        self.search.set_visible(true);
        self.weather.set_visible(true);
        self.osd.set_visible(true);
        self.notification.set_visible(true);
        self.compact.set_can_target(view == View::Compact);
        self.media.set_can_target(view == View::Media);
        self.dashboard.set_can_target(view == View::Dashboard);
        self.search.set_can_target(view == View::Search);
        self.weather.set_can_target(view == View::Weather);
        self.osd.set_can_target(false);
        self.notification.set_can_target(view == View::Notification);
        self.refresh_keyboard_mode();
        let start = self.geometry.get();
        let generation = self.animation_generation.get().wrapping_add(1);
        self.animation_generation.set(generation);
        if !self.animations_enabled.get() || self.animation_ms.get() == 0 {
            self.apply_geometry(target);
            self.finish_view(view);
            return;
        }

        let duration_us = i64::from(self.animation_ms.get()) * 1000;
        let start_time = Cell::new(None::<i64>);
        let start_opacities = [
            self.compact.opacity(),
            self.media.opacity(),
            self.dashboard.opacity(),
            self.search.opacity(),
            self.weather.opacity(),
            self.osd.opacity(),
            self.notification.opacity(),
        ];
        let weak = Rc::downgrade(self);
        self.surface.add_tick_callback(move |_, frame_clock| {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if island.animation_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let now = frame_clock.frame_time();
            let started = if let Some(started) = start_time.get() {
                started
            } else {
                start_time.set(Some(now));
                now
            };
            let linear = ((now - started) as f64 / duration_us as f64).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - linear).powi(5);
            island.apply_geometry(start.interpolate(target, eased));
            island.apply_content_opacity(view, linear, start_opacities);
            if linear >= 1.0 {
                island.finish_view(view);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn apply_content_opacity(&self, target: View, progress: f64, start: [f64; 7]) {
        let progress = 1.0 - (1.0 - progress).powi(3);
        let compact_target = if target == View::Compact { 1.0 } else { 0.0 };
        let media_target = if target == View::Media { 1.0 } else { 0.0 };
        let dashboard_target = if target == View::Dashboard { 1.0 } else { 0.0 };
        let search_target = if target == View::Search { 1.0 } else { 0.0 };
        let weather_target = if target == View::Weather { 1.0 } else { 0.0 };
        let osd_target = if target == View::Osd { 1.0 } else { 0.0 };
        let notification_target = if target == View::Notification {
            1.0
        } else {
            0.0
        };
        self.compact
            .set_opacity(lerp(start[0], compact_target, progress));
        self.media
            .set_opacity(lerp(start[1], media_target, progress));
        self.dashboard
            .set_opacity(lerp(start[2], dashboard_target, progress));
        self.search
            .set_opacity(lerp(start[3], search_target, progress));
        self.weather
            .set_opacity(lerp(start[4], weather_target, progress));
        self.osd.set_opacity(lerp(start[5], osd_target, progress));
        self.notification
            .set_opacity(lerp(start[6], notification_target, progress));
    }

    fn finish_view(&self, view: View) {
        self.apply_geometry(self.geometry_for_view(view));
        self.compact.set_visible(view == View::Compact);
        self.media.set_visible(view == View::Media);
        self.dashboard.set_visible(view == View::Dashboard);
        self.search.set_visible(view == View::Search);
        self.weather.set_visible(view == View::Weather);
        self.osd.set_visible(view == View::Osd);
        self.notification.set_visible(view == View::Notification);
        self.compact
            .set_opacity(if view == View::Compact { 1.0 } else { 0.0 });
        self.media
            .set_opacity(if view == View::Media { 1.0 } else { 0.0 });
        self.dashboard
            .set_opacity(if view == View::Dashboard { 1.0 } else { 0.0 });
        self.search
            .set_opacity(if view == View::Search { 1.0 } else { 0.0 });
        self.weather
            .set_opacity(if view == View::Weather { 1.0 } else { 0.0 });
        self.osd
            .set_opacity(if view == View::Osd { 1.0 } else { 0.0 });
        self.notification
            .set_opacity(if view == View::Notification { 1.0 } else { 0.0 });
    }

    fn apply_geometry(&self, geometry: Geometry) {
        self.geometry.set(geometry);
        let width = geometry.width.round() as i32;
        let height = geometry.height.round() as i32;
        let x = (self.metrics.window_width - width) / 2;
        let y = geometry.y.round() as i32;
        self.surface.set_size_request(width, height);
        self.fixed.move_(&self.surface, f64::from(x), f64::from(y));
        self.surface
            .hadjustment()
            .set_value(f64::from((self.metrics.search_width - width) / 2));
        self.surface.vadjustment().set_value(0.0);
        if let Some(surface) = self.window.surface() {
            let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                x, y, width, height,
            ));
            surface.set_input_region(Some(&region));
        }
    }
}

fn compact_view(metrics: Metrics) -> (gtk::Box, gtk::Box, gtk::Label, gtk::Label, gtk::Box) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    root.set_size_request(metrics.compact_width, metrics.compact_height);
    root.add_css_class("compact-content");
    root.set_valign(Align::Start);

    let workspaces = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    workspaces.set_hexpand(false);
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);

    let clock = gtk::Label::new(Some("--:--"));
    clock.add_css_class("compact-clock");
    clock.set_hexpand(true);
    clock.set_halign(Align::Center);
    clock.set_valign(Align::Center);

    let battery = gtk::Label::new(None);
    battery.add_css_class("compact-battery");
    battery.set_hexpand(false);
    battery.set_halign(Align::End);
    battery.set_valign(Align::Center);
    battery.set_visible(false);

    // Hidden by default (`update_tray`/`set_tray_hovered`): only shown while
    // the pointer is over the pill and at least one tray item exists, per
    // `resize_compact`.
    let tray = gtk::Box::new(Orientation::Horizontal, metrics.spacing(3));
    tray.add_css_class("compact-tray");
    tray.set_hexpand(false);
    tray.set_halign(Align::End);
    tray.set_valign(Align::Center);
    tray.set_visible(false);

    root.append(&workspaces);
    root.append(&clock);
    root.append(&battery);
    root.append(&tray);
    (root, workspaces, clock, battery, tray)
}

struct MediaWidgets {
    root: gtk::Box,
    workspaces: gtk::Box,
    clock: gtk::Label,
    center: gtk::Box,
    icon: gtk::Image,
    title: gtk::Label,
    visualizer: gtk::DrawingArea,
    levels: Rc<RefCell<VisualizerLevels>>,
    /// Same role as `compact_tray`, but for the media pill. Deliberately a
    /// sibling of the `CenterBox` rather than a child of its `end` slot:
    /// `CenterBox` keeps its center child centered by giving `start`/`end`
    /// equal width, so anything added to `end` visibly pads `start` (the
    /// workspace dots) by the same amount.
    tray: gtk::Box,
}

fn media_view(metrics: Metrics) -> MediaWidgets {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    root.set_size_request(metrics.compact_width, metrics.media_height);
    root.add_css_class("media-content");
    root.set_valign(Align::Start);

    let center_box = gtk::CenterBox::new();
    center_box.set_hexpand(true);

    let workspaces = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);

    let media = gtk::Box::new(Orientation::Horizontal, metrics.spacing(7));
    media.add_css_class("media-center");
    media.set_halign(Align::Center);
    media.set_valign(Align::Center);

    let icon = gtk::Image::new();
    icon.add_css_class("media-app-icon");
    icon.set_margin_start(metrics.spacing(2));
    icon.set_visible(false);

    let levels = Rc::new(RefCell::new([0; VISUALIZER_BARS]));
    let draw_levels = levels.clone();
    let visualizer = gtk::DrawingArea::new();
    visualizer.add_css_class("media-visualizer");
    visualizer.set_content_width(metrics.spacing(31));
    visualizer.set_content_height(metrics.spacing(18));
    visualizer.set_valign(Align::Center);
    visualizer.set_draw_func(move |area, context, width, height| {
        let color = area.color();
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        context.set_line_cap(gtk::cairo::LineCap::Round);
        let width = f64::from(width);
        let height = f64::from(height);
        let gap = width / (VISUALIZER_BARS as f64 * 2.2);
        let bar_width = gap * 0.72;
        let baseline = height * 0.5;
        context.set_line_width(bar_width);
        for (index, level) in draw_levels.borrow().iter().enumerate() {
            let x = gap + index as f64 * gap * 2.0;
            let half_height = ((height * 0.12) + (height * 0.67 * f64::from(*level) / 100.0)) / 2.0;
            context.move_to(x, baseline - half_height);
            context.line_to(x, baseline + half_height);
            let _ = context.stroke();
        }
    });

    let title = gtk::Label::new(None);
    title.add_css_class("media-title");
    title.set_hexpand(true);
    title.set_halign(Align::Fill);
    title.set_valign(Align::Center);
    title.set_xalign(0.0);
    title.set_single_line_mode(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let clock = gtk::Label::new(Some("--:--"));
    clock.add_css_class("media-clock");
    clock.set_halign(Align::End);
    clock.set_valign(Align::Center);

    // Hidden by default, same as `compact_tray`: only shown while hovering
    // the pill and at least one tray item exists.
    let tray = gtk::Box::new(Orientation::Horizontal, metrics.spacing(3));
    tray.add_css_class("compact-tray");
    tray.set_halign(Align::End);
    tray.set_valign(Align::Center);
    tray.set_visible(false);

    media.append(&icon);
    media.append(&visualizer);
    media.append(&title);
    center_box.set_start_widget(Some(&workspaces));
    center_box.set_center_widget(Some(&media));
    center_box.set_end_widget(Some(&clock));

    root.append(&center_box);
    root.append(&tray);
    MediaWidgets {
        root,
        workspaces,
        clock,
        center: media,
        icon,
        title,
        visualizer,
        levels,
        tray,
    }
}

struct SearchWidgets {
    root: gtk::Box,
    entry: gtk::SearchEntry,
    results: gtk::ListBox,
    status: gtk::Label,
    back_button: gtk::Button,
    reload_button: gtk::Button,
    plugin_toggle: gtk::ToggleButton,
    stack: gtk::Stack,
    plugins: gtk::ListBox,
    preview_stack: gtk::Stack,
    preview_picture: gtk::Picture,
    preview_icon: gtk::Image,
    preview_title: gtk::Label,
    preview_description: gtk::Label,
    preview_file_meta: gtk::Label,
    preview_meta: gtk::Label,
    preview_text: gtk::TextView,
    preview_text_scroll: gtk::ScrolledWindow,
    preview_error: gtk::Label,
    preview_actions: gtk::Box,
}

fn search_view(metrics: Metrics) -> SearchWidgets {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(10));
    root.set_size_request(metrics.search_width, metrics.search_height);
    root.add_css_class("search-content");
    root.set_valign(Align::Start);

    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(9));
    let back_button = gtk::Button::from_icon_name("go-previous-symbolic");
    back_button.add_css_class("close-button");
    back_button.set_tooltip_text(Some("Back to dashboard"));
    let entry = gtk::SearchEntry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("Search apps, files, commands, and plugins"));
    entry.add_css_class("tarragon-search");
    let plugin_toggle = gtk::ToggleButton::with_label("PLUGINS");
    plugin_toggle.add_css_class("search-header-button");
    plugin_toggle.set_tooltip_text(Some("Show loaded TarraGon plugins"));
    let reload_button = gtk::Button::from_icon_name("view-refresh-symbolic");
    reload_button.add_css_class("close-button");
    reload_button.set_tooltip_text(Some("Reload TarraGon configuration and plugins"));
    header.append(&back_button);
    header.append(&entry);
    header.append(&plugin_toggle);
    header.append(&reload_button);
    root.append(&header);

    let status = gtk::Label::new(Some("TARRAGON OFFLINE"));
    status.add_css_class("search-status");
    status.set_halign(Align::Start);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    root.append(&status);

    let results = gtk::ListBox::new();
    results.add_css_class("search-results");
    results.set_selection_mode(gtk::SelectionMode::Single);
    results.set_activate_on_single_click(true);
    results.set_vexpand(true);
    let results_scroller = gtk::ScrolledWindow::new();
    results_scroller.add_css_class("search-results-scroll");
    results_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    results_scroller.set_vexpand(true);
    results_scroller.set_size_request(metrics.spacing(SEARCH_RESULTS_MIN_WIDTH), -1);
    results_scroller.set_child(Some(&results));

    let preview = gtk::Box::new(Orientation::Vertical, metrics.spacing(9));
    preview.add_css_class("search-preview");
    preview.set_size_request(metrics.spacing(SEARCH_PREVIEW_MIN_WIDTH), -1);
    let preview_title = gtk::Label::new(Some("Select a result"));
    preview_title.add_css_class("search-preview-title");
    preview_title.set_halign(Align::Start);
    preview_title.set_wrap(true);
    preview_title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    let preview_description = gtk::Label::new(None);
    preview_description.add_css_class("search-preview-description");
    preview_description.set_halign(Align::Start);
    preview_description.set_wrap(true);
    preview_description.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    preview_description.set_lines(3);
    let preview_file_meta = gtk::Label::new(None);
    preview_file_meta.add_css_class("search-preview-file-meta");
    preview_file_meta.set_halign(Align::Start);
    preview_file_meta.set_wrap(true);
    let preview_meta = gtk::Label::new(Some("TarraGon aggregate results"));
    preview_meta.add_css_class("search-preview-meta");
    preview_meta.set_halign(Align::Start);
    preview_meta.set_wrap(true);
    preview.append(&preview_title);
    preview.append(&preview_description);
    preview.append(&preview_file_meta);
    preview.append(&preview_meta);

    let preview_stack = gtk::Stack::new();
    preview_stack.set_vhomogeneous(false);
    preview_stack.set_vexpand(true);
    preview_stack.set_size_request(-1, metrics.spacing(190));
    let preview_picture = gtk::Picture::new();
    preview_picture.set_content_fit(gtk::ContentFit::Contain);
    preview_picture.add_css_class("search-preview-picture");
    let preview_icon = gtk::Image::from_icon_name("system-search-symbolic");
    preview_icon.add_css_class("search-preview-icon");
    let preview_text = gtk::TextView::new();
    preview_text.add_css_class("search-preview-text");
    preview_text.set_editable(false);
    preview_text.set_cursor_visible(false);
    preview_text.set_monospace(true);
    preview_text.set_wrap_mode(gtk::WrapMode::None);
    preview_text.set_left_margin(metrics.spacing(9));
    preview_text.set_right_margin(metrics.spacing(9));
    preview_text.set_top_margin(metrics.spacing(8));
    preview_text.set_bottom_margin(metrics.spacing(8));
    let preview_text_scroll = gtk::ScrolledWindow::new();
    preview_text_scroll.add_css_class("search-preview-text-scroll");
    preview_text_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    preview_text_scroll.set_child(Some(&preview_text));
    let preview_error = gtk::Label::new(None);
    preview_error.add_css_class("search-preview-error");
    preview_error.set_wrap(true);
    preview_error.set_halign(Align::Center);
    preview_error.set_valign(Align::Center);
    preview_stack.add_named(&preview_picture, Some("picture"));
    preview_stack.add_named(&preview_icon, Some("icon"));
    preview_stack.add_named(&preview_text_scroll, Some("text"));
    preview_stack.add_named(&preview_error, Some("error"));
    preview_stack.set_visible_child_name("icon");
    preview.append(&preview_stack);

    let preview_actions = gtk::Box::new(Orientation::Vertical, metrics.spacing(5));
    preview_actions.set_valign(Align::End);
    preview.append(&preview_actions);

    let result_page = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    results_scroller.set_hexpand(true);
    result_page.append(&results_scroller);
    result_page.append(&preview);

    let plugins = gtk::ListBox::new();
    plugins.add_css_class("plugin-list");
    plugins.set_selection_mode(gtk::SelectionMode::None);
    let plugin_scroller = gtk::ScrolledWindow::new();
    plugin_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    plugin_scroller.set_vexpand(true);
    plugin_scroller.set_child(Some(&plugins));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(160);
    stack.set_vexpand(true);
    stack.add_named(&result_page, Some("results"));
    stack.add_named(&plugin_scroller, Some("plugins"));
    stack.set_visible_child_name("results");
    root.append(&stack);

    SearchWidgets {
        root,
        entry,
        results,
        status,
        back_button,
        reload_button,
        plugin_toggle,
        stack,
        plugins,
        preview_stack,
        preview_picture,
        preview_icon,
        preview_title,
        preview_description,
        preview_file_meta,
        preview_meta,
        preview_text,
        preview_text_scroll,
        preview_error,
        preview_actions,
    }
}

fn search_result_row(
    result: &crate::tarragon::TarragonResult,
    metrics: Metrics,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("search-result");
    let content = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));

    let icon = if result.icon.starts_with('/') {
        gtk::Image::from_file(&result.icon)
    } else if result.icon.is_empty() {
        gtk::Image::from_icon_name("system-search-symbolic")
    } else {
        gtk::Image::from_icon_name(&result.icon)
    };
    icon.add_css_class("search-result-icon");
    icon.set_valign(Align::Center);
    content.append(&icon);

    let text = gtk::Box::new(Orientation::Vertical, 0);
    text.set_hexpand(true);
    let label = gtk::Label::new(Some(if result.label.is_empty() {
        &result.id
    } else {
        &result.label
    }));
    label.add_css_class("search-result-title");
    label.set_halign(Align::Start);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let description = gtk::Label::new(Some(&result.description));
    description.add_css_class("search-result-description");
    description.set_halign(Align::Start);
    description.set_ellipsize(gtk::pango::EllipsizeMode::End);
    description.set_visible(!result.description.is_empty());
    text.append(&label);
    text.append(&description);
    content.append(&text);

    let source = if result.category.is_empty() {
        result.plugin.as_str()
    } else {
        result.category.as_str()
    };
    let plugin = gtk::Label::new(Some(source));
    plugin.add_css_class("search-result-plugin");
    plugin.set_valign(Align::Center);
    content.append(&plugin);
    row.set_child(Some(&content));
    row
}

fn plugin_status_row(
    plugin: &TarragonPlugin,
    query: Option<&TarragonPluginState>,
    metrics: Metrics,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("plugin-row");
    let content = gtk::Box::new(Orientation::Horizontal, metrics.spacing(11));
    let icon = if plugin.icon.starts_with('/') {
        gtk::Image::from_file(&plugin.icon)
    } else if plugin.icon.is_empty() {
        gtk::Image::from_icon_name("application-x-addon-symbolic")
    } else {
        gtk::Image::from_icon_name(&plugin.icon)
    };
    icon.add_css_class("plugin-icon");
    content.append(&icon);

    let details = gtk::Box::new(Orientation::Vertical, metrics.spacing(2));
    details.set_hexpand(true);
    let name = gtk::Label::new(Some(&plugin.name));
    name.add_css_class("plugin-name");
    name.set_halign(Align::Start);
    let description = gtk::Label::new(Some(&plugin.description));
    description.add_css_class("plugin-description");
    description.set_halign(Align::Start);
    description.set_ellipsize(gtk::pango::EllipsizeMode::End);
    description.set_visible(!plugin.description.is_empty());
    let mut metadata = vec![plugin.lifecycle.clone()];
    if !plugin.prefix.is_empty() {
        metadata.push(format!("prefix {}", plugin.prefix));
    }
    if plugin.require_prefix {
        metadata.push("prefix required".into());
    }
    if plugin.provides_general_suggestions {
        metadata.push("general".into());
    }
    if !plugin.source.is_empty() {
        metadata.push(plugin.source.clone());
    }
    if !plugin.capabilities.is_empty() {
        metadata.push(plugin.capabilities.join(", "));
    }
    let metadata = gtk::Label::new(Some(&metadata.join("  //  ")));
    metadata.add_css_class("plugin-metadata");
    metadata.set_halign(Align::Start);
    metadata.set_ellipsize(gtk::pango::EllipsizeMode::End);
    details.append(&name);
    details.append(&description);
    details.append(&metadata);
    content.append(&details);

    let availability = if !plugin.enabled {
        "DISABLED".to_owned()
    } else if let Some(query) = query {
        match query.state.as_str() {
            "pending" => "PENDING".into(),
            "done" => format!("{} RESULTS  //  {:.1} MS", query.count, query.elapsed_ms),
            "empty" => format!("EMPTY  //  {:.1} MS", query.elapsed_ms),
            "error" => {
                if query.error.is_empty() {
                    "ERROR".into()
                } else {
                    format!("ERROR  //  {}", query.error)
                }
            }
            state => state.to_uppercase(),
        }
    } else if plugin.lifecycle == "on_call" {
        "ON CALL".into()
    } else if plugin.connected {
        "CONNECTED".into()
    } else if plugin.lifecycle == "on_demand_persistent" {
        "IDLE".into()
    } else {
        "DISCONNECTED".into()
    };
    let state = gtk::Label::new(Some(&availability));
    state.add_css_class("plugin-state");
    if query.is_some_and(|query| query.state == "error") || !plugin.enabled {
        state.add_css_class("error");
    } else if plugin.connected || plugin.lifecycle == "on_call" {
        state.add_css_class("available");
    }
    state.set_valign(Align::Center);
    state.set_max_width_chars(30);
    state.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&state);
    row.set_child(Some(&content));
    row
}

struct DashboardWidgets {
    root: gtk::Box,
    hero_time: gtk::Label,
    hero_date: gtk::Label,
    battery_chip: gtk::Box,
    battery_icon: gtk::Image,
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
    active_eyebrow: gtk::Label,
    active_title: gtk::Label,
    workspace_row: gtk::FlowBox,
    volume_scale: gtk::Scale,
    volume_value: gtk::Label,
    brightness_row: gtk::Box,
    brightness_scale: gtk::Scale,
    brightness_value: gtk::Label,
    notification_count: gtk::Label,
    notification_list: gtk::Box,
    weather_button: gtk::Button,
    search_button: gtk::Button,
    close_button: gtk::Button,
}

fn dashboard_view(metrics: Metrics) -> DashboardWidgets {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(13));
    root.set_size_request(metrics.dashboard_width, metrics.dashboard_height);
    root.add_css_class("dashboard-content");
    root.set_valign(Align::Start);

    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    let heading = gtk::Box::new(Orientation::Vertical, 0);
    heading.set_hexpand(true);
    let eyebrow = gtk::Label::new(Some("MITHSHELL  //  LOCAL"));
    eyebrow.add_css_class("eyebrow");
    eyebrow.set_halign(Align::Start);
    let time = gtk::Label::new(Some("--:--"));
    time.add_css_class("hero-time");
    time.set_halign(Align::Start);
    let date = gtk::Label::new(None);
    date.add_css_class("hero-date");
    date.set_halign(Align::Start);
    heading.append(&eyebrow);
    heading.append(&time);
    heading.append(&date);

    let battery_chip = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    battery_chip.add_css_class("battery-chip");
    battery_chip.set_valign(Align::Center);
    battery_chip.set_visible(false);
    let battery_icon = gtk::Image::from_icon_name("xsi-battery-symbolic");
    battery_chip.append(&battery_icon);
    let battery_label = gtk::Label::new(None);
    battery_chip.append(&battery_label);

    let close_button = gtk::Button::from_icon_name("window-close-symbolic");
    close_button.add_css_class("close-button");
    close_button.set_valign(Align::Center);
    let search_button = gtk::Button::from_icon_name("system-search-symbolic");
    search_button.add_css_class("close-button");
    search_button.set_tooltip_text(Some("Search with TarraGon"));
    search_button.set_valign(Align::Center);
    let weather_button = gtk::Button::from_icon_name("weather-clear-symbolic");
    weather_button.add_css_class("close-button");
    weather_button.set_tooltip_text(Some("Weather forecast"));
    weather_button.set_valign(Align::Center);
    header.append(&heading);
    header.append(&battery_chip);
    header.append(&weather_button);
    header.append(&search_button);
    header.append(&close_button);
    root.append(&header);

    let player_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(8));
    player_card.add_css_class("player-card");
    player_card.add_css_class("unavailable");

    let player_top = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    let player_icon = gtk::Image::new();
    player_icon.add_css_class("player-icon");
    player_icon.set_valign(Align::Center);
    player_icon.set_visible(false);

    let player_text = gtk::Box::new(Orientation::Vertical, 0);
    player_text.set_hexpand(true);
    player_text.set_valign(Align::Center);
    let player_title = gtk::Label::new(Some("Nothing playing"));
    player_title.add_css_class("player-title");
    player_title.set_halign(Align::Start);
    player_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let player_artist = gtk::Label::new(None);
    player_artist.add_css_class("player-artist");
    player_artist.set_halign(Align::Start);
    player_artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    player_artist.set_visible(false);
    player_text.append(&player_title);
    player_text.append(&player_artist);

    let player_prev_button = gtk::Button::from_icon_name("media-skip-backward-symbolic");
    player_prev_button.add_css_class("player-button");
    player_prev_button.set_tooltip_text(Some("Previous track"));
    player_prev_button.set_valign(Align::Center);
    player_prev_button.set_sensitive(false);
    let player_play_pause_button = gtk::Button::from_icon_name("media-playback-pause-symbolic");
    player_play_pause_button.add_css_class("player-button");
    player_play_pause_button.add_css_class("player-play");
    player_play_pause_button.set_tooltip_text(Some("Play/Pause"));
    player_play_pause_button.set_valign(Align::Center);
    player_play_pause_button.set_sensitive(false);
    let player_next_button = gtk::Button::from_icon_name("media-skip-forward-symbolic");
    player_next_button.add_css_class("player-button");
    player_next_button.set_tooltip_text(Some("Next track"));
    player_next_button.set_valign(Align::Center);
    player_next_button.set_sensitive(false);

    player_top.append(&player_icon);
    player_top.append(&player_text);
    player_top.append(&player_prev_button);
    player_top.append(&player_play_pause_button);
    player_top.append(&player_next_button);
    player_card.append(&player_top);

    let player_progress = gtk::ProgressBar::new();
    player_progress.set_hexpand(true);
    player_progress.add_css_class("player-progress");
    player_card.append(&player_progress);

    let player_time_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    let player_elapsed_label = gtk::Label::new(Some("--:--"));
    player_elapsed_label.add_css_class("player-time");
    player_elapsed_label.set_halign(Align::Start);
    let player_duration_label = gtk::Label::new(Some("--:--"));
    player_duration_label.add_css_class("player-time");
    player_duration_label.set_halign(Align::End);
    player_duration_label.set_hexpand(true);
    player_time_row.append(&player_elapsed_label);
    player_time_row.append(&player_duration_label);
    player_card.append(&player_time_row);
    let player_switch_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(4));
    player_switch_row.add_css_class("player-switch-row");
    let player_switch_prev = gtk::Button::from_icon_name("go-previous-symbolic");
    player_switch_prev.add_css_class("player-switch-button");
    let player_switch_label = gtk::Label::new(Some("1 player"));
    player_switch_label.add_css_class("player-switch-label");
    player_switch_label.set_hexpand(true);
    player_switch_label.set_xalign(0.5);
    let player_switch_next = gtk::Button::from_icon_name("go-next-symbolic");
    player_switch_next.add_css_class("player-switch-button");
    player_switch_row.append(&player_switch_prev);
    player_switch_row.append(&player_switch_label);
    player_switch_row.append(&player_switch_next);
    player_card.append(&player_switch_row);
    root.append(&player_card);

    let status_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    status_row.add_css_class("status-row");

    let active_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(2));
    active_card.add_css_class("active-card");
    active_card.set_hexpand(true);
    let active_eyebrow = gtk::Label::new(Some("OUTPUT  //  WORKSPACE --"));
    active_eyebrow.add_css_class("eyebrow");
    active_eyebrow.set_halign(Align::Start);
    let active_title = gtk::Label::new(Some("Quiet desktop"));
    active_title.add_css_class("active-title");
    active_title.set_halign(Align::Start);
    active_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    active_title.set_max_width_chars(48);
    active_card.append(&active_eyebrow);
    active_card.append(&active_title);

    let workspace_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(2));
    workspace_card.add_css_class("active-card");
    let workspace_label = gtk::Label::new(Some("WORKSPACES"));
    workspace_label.add_css_class("eyebrow");
    workspace_label.set_halign(Align::Start);
    let workspace_row = gtk::FlowBox::new();
    workspace_row.set_column_spacing(metrics.spacing(4) as u32);
    workspace_row.set_row_spacing(metrics.spacing(4) as u32);
    workspace_row.set_max_children_per_line(5);
    workspace_row.set_min_children_per_line(5);
    workspace_row.set_selection_mode(gtk::SelectionMode::None);
    workspace_row.set_halign(Align::Start);
    workspace_card.append(&workspace_label);
    workspace_card.append(&workspace_row);

    status_row.append(&active_card);
    status_row.append(&workspace_card);
    root.append(&status_row);

    let controls = gtk::Box::new(Orientation::Vertical, 0);
    controls.add_css_class("controls-card");
    let (volume_row, volume_scale, volume_value) =
        control_row("audio-volume-high-symbolic", "Volume", metrics);
    let (brightness_row, brightness_scale, brightness_value) =
        control_row("display-brightness-symbolic", "Brightness", metrics);
    controls.append(&volume_row);
    controls.append(&brightness_row);
    root.append(&controls);

    let notification_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(8));
    notification_card.add_css_class("notification-card");

    let notification_header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    let notification_title = gtk::Label::new(Some("NOTIFICATIONS"));
    notification_title.add_css_class("eyebrow");
    notification_title.set_halign(Align::Start);
    notification_title.set_hexpand(true);
    let notification_count = gtk::Label::new(Some("0"));
    notification_count.add_css_class("notification-count");
    notification_header.append(&notification_title);
    notification_header.append(&notification_count);
    notification_card.append(&notification_header);

    let notification_list = gtk::Box::new(Orientation::Vertical, metrics.spacing(4));
    notification_list.add_css_class("notification-list");
    // Replaced by `update_notification_history` as soon as the controller
    // pushes its first (possibly empty) history snapshot.
    let notification_placeholder = gtk::Label::new(Some("No notifications yet"));
    notification_placeholder.add_css_class("muted-label");
    notification_placeholder.add_css_class("notification-empty");
    notification_placeholder.set_halign(Align::Start);
    notification_placeholder.set_wrap(true);
    notification_list.append(&notification_placeholder);
    notification_card.append(&notification_list);
    root.append(&notification_card);

    DashboardWidgets {
        root,
        hero_time: time,
        hero_date: date,
        battery_chip,
        battery_icon,
        battery_label,
        player_card,
        player_icon,
        player_title,
        player_artist,
        player_progress,
        player_elapsed_label,
        player_duration_label,
        player_prev_button,
        player_play_pause_button,
        player_next_button,
        player_switch_row,
        player_switch_label,
        player_switch_prev,
        player_switch_next,
        active_eyebrow,
        active_title,
        workspace_row,
        volume_scale,
        volume_value,
        brightness_row,
        brightness_scale,
        brightness_value,
        notification_count,
        notification_list,
        weather_button,
        search_button,
        close_button,
    }
}

fn control_row(icon: &str, label: &str, metrics: Metrics) -> (gtk::Box, gtk::Scale, gtk::Label) {
    let row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    row.add_css_class("control-row");
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("control-icon");
    image.set_tooltip_text(Some(label));
    let scale = gtk::Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.add_css_class("control-scale");
    let value = gtk::Label::new(Some("--"));
    value.add_css_class("control-value");
    value.set_xalign(1.0);
    row.append(&image);
    row.append(&scale);
    row.append(&value);
    (row, scale, value)
}

fn battery_icon_name(percent: u8, status: &str) -> &'static str {
    let level = match percent {
        0..=4 => 0,
        5..=14 => 10,
        15..=24 => 20,
        25..=34 => 30,
        35..=44 => 40,
        45..=54 => 50,
        55..=64 => 60,
        65..=74 => 70,
        75..=84 => 80,
        85..=94 => 90,
        _ => 100,
    };
    let charging = status.eq_ignore_ascii_case("charging");
    match (level, charging) {
        (0, false) => "xsi-battery-level-0-symbolic",
        (10, false) => "xsi-battery-level-10-symbolic",
        (20, false) => "xsi-battery-level-20-symbolic",
        (30, false) => "xsi-battery-level-30-symbolic",
        (40, false) => "xsi-battery-level-40-symbolic",
        (50, false) => "xsi-battery-level-50-symbolic",
        (60, false) => "xsi-battery-level-60-symbolic",
        (70, false) => "xsi-battery-level-70-symbolic",
        (80, false) => "xsi-battery-level-80-symbolic",
        (90, false) => "xsi-battery-level-90-symbolic",
        (100, false) => "xsi-battery-level-100-symbolic",
        (0, true) => "xsi-battery-level-0-charging-symbolic",
        (10, true) => "xsi-battery-level-10-charging-symbolic",
        (20, true) => "xsi-battery-level-20-charging-symbolic",
        (30, true) => "xsi-battery-level-30-charging-symbolic",
        (40, true) => "xsi-battery-level-40-charging-symbolic",
        (50, true) => "xsi-battery-level-50-charging-symbolic",
        (60, true) => "xsi-battery-level-60-charging-symbolic",
        (70, true) => "xsi-battery-level-70-charging-symbolic",
        (80, true) => "xsi-battery-level-80-charging-symbolic",
        (90, true) => "xsi-battery-level-90-charging-symbolic",
        (100, true) => "xsi-battery-level-100-charging-symbolic",
        _ => "xsi-battery-symbolic",
    }
}

struct WeatherWidgets {
    root: gtk::Box,
    back_button: gtk::Button,
    location: gtk::Label,
    hero_icon: gtk::DrawingArea,
    hero_temp: gtk::Label,
    hero_description: gtk::Label,
    status: gtk::Label,
    forecast_row: gtk::Box,
}

fn weather_view(metrics: Metrics) -> WeatherWidgets {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(7));
    root.set_size_request(metrics.weather_width, metrics.weather_height);
    root.add_css_class("weather-content");
    root.set_valign(Align::Start);

    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    let back_button = gtk::Button::from_icon_name("go-previous-symbolic");
    back_button.add_css_class("close-button");
    back_button.set_tooltip_text(Some("Back to dashboard"));
    back_button.set_valign(Align::Center);

    let heading = gtk::Box::new(Orientation::Vertical, 0);
    heading.set_hexpand(true);
    heading.set_valign(Align::Center);
    let eyebrow = gtk::Label::new(Some("WEATHER  //  WTTR.IN"));
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
    card.set_halign(Align::Center);

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
fn draw_weather_condition(area: &gtk::DrawingArea, condition: WeatherCondition) {
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

fn osd_view(
    metrics: Metrics,
) -> (
    gtk::Box,
    gtk::Image,
    gtk::Label,
    gtk::ProgressBar,
    gtk::Label,
) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    root.set_size_request(metrics.osd_width, metrics.osd_height);
    root.add_css_class("osd-content");
    root.set_valign(Align::Start);
    let icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");
    icon.add_css_class("osd-icon");
    icon.set_valign(Align::Center);
    let title = gtk::Label::new(Some("Volume"));
    title.add_css_class("osd-title");
    title.set_halign(Align::Start);
    title.set_valign(Align::Center);
    let progress = gtk::ProgressBar::new();
    progress.set_hexpand(true);
    progress.set_valign(Align::Center);
    progress.add_css_class("osd-progress");
    let value = gtk::Label::new(Some("0"));
    value.add_css_class("osd-value");
    value.set_xalign(1.0);
    value.set_valign(Align::Center);
    root.append(&icon);
    root.append(&title);
    root.append(&progress);
    root.append(&value);
    (root, icon, title, progress, value)
}

fn notification_view(metrics: Metrics) -> (gtk::Box, gtk::Image, gtk::Label, gtk::Label) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    root.set_size_request(metrics.notification_width, metrics.notification_height);
    root.add_css_class("notification-content");
    root.set_valign(Align::Center);

    let icon = gtk::Image::from_icon_name("preferences-system-notifications-symbolic");
    icon.add_css_class("notification-icon");
    icon.set_valign(Align::Center);

    let text = gtk::Box::new(Orientation::Vertical, metrics.spacing(2));
    text.set_valign(Align::Center);
    text.set_hexpand(true);
    let app = gtk::Label::new(None);
    app.add_css_class("notification-app");
    app.set_halign(Align::Start);
    app.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let body = gtk::Label::new(None);
    body.add_css_class("notification-body");
    body.set_halign(Align::Start);
    body.set_ellipsize(gtk::pango::EllipsizeMode::End);
    body.set_visible(false);
    text.append(&app);
    text.append(&body);

    root.append(&icon);
    root.append(&text);
    (root, icon, app, body)
}

/// Builds the separate popup window used for `below-pill`/corner
/// notification positions. Must only be called with a non-`Pill` position.
fn build_notification_toasts(
    application: &Application,
    monitor: &gdk::Monitor,
    shell: &ShellConfig,
    notifications: &NotificationConfig,
    metrics: Metrics,
) -> NotificationToasts {
    let window = ApplicationWindow::builder()
        .application(application)
        .title("mithshell notifications")
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("mithshell-notifications");
    if let Some(class) = metrics.css_class() {
        window.add_css_class(class);
    }
    window.init_layer_shell();
    window.set_namespace(Some("mithshell-notifications"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_monitor(Some(monitor));
    window.set_exclusive_zone(0);

    match notifications.position {
        NotificationPosition::Pill => {
            unreachable!("build_notification_toasts is only called for non-pill positions")
        }
        // Anchoring only the top edge, like the island itself, is what
        // centers this window horizontally under it; the margin clears the
        // island's resting (compact) height plus the configured gap. The
        // popup does not follow the island if it grows taller (dashboard,
        // search, ...) -- those overlay views already dim/steal input via
        // the dismiss window, so brief overlap is an acceptable trade for
        // not re-anchoring a second surface on every geometry animation.
        NotificationPosition::BelowPill => {
            window.set_anchor(Edge::Top, true);
            window.set_margin(
                Edge::Top,
                metrics.spacing(shell.top_margin)
                    + metrics.compact_height
                    + metrics.spacing(notifications.gap),
            );
        }
        NotificationPosition::TopLeft => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_margin(Edge::Top, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Left, metrics.spacing(notifications.margin));
        }
        NotificationPosition::TopRight => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Right, metrics.spacing(notifications.margin));
        }
        NotificationPosition::BottomLeft => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_margin(Edge::Bottom, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Left, metrics.spacing(notifications.margin));
        }
        NotificationPosition::BottomRight => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Bottom, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Right, metrics.spacing(notifications.margin));
        }
    }

    let stack = gtk::Box::new(Orientation::Vertical, metrics.spacing(notifications.gap));
    stack.add_css_class("notification-stack");
    stack.set_width_request(metrics.spacing(NOTIFICATION_TOAST_WIDTH));
    window.set_child(Some(&stack));

    NotificationToasts {
        window,
        stack,
        entries: RefCell::new(HashMap::new()),
        order: RefCell::new(Vec::new()),
    }
}

/// Resolves a notification's icon onto `image`: an absolute path is loaded
/// as a file, anything else is treated as a themed icon name, and an empty
/// hint falls back to `fallback`.
fn apply_notification_icon(image: &gtk::Image, icon: Option<&str>, fallback: &str) {
    match icon {
        Some(path) if path.starts_with('/') => image.set_from_file(Some(path)),
        Some(name) if !name.is_empty() => image.set_icon_name(Some(name)),
        _ => image.set_icon_name(Some(fallback)),
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
fn measure_clamped<W: IsA<gtk::Widget>>(widget: &W, max_width: i32) -> i32 {
    let (_, natural, _, _) = widget.measure(Orientation::Horizontal, -1);
    natural.min(max_width)
}

fn apply_tray_icon(image: &gtk::Image, icon: &TrayIcon) {
    const FALLBACK: &str = "application-x-executable-symbolic";
    match icon {
        TrayIcon::Name(name) => image.set_icon_name(Some(name)),
        TrayIcon::Pixmap {
            width,
            height,
            argb,
        } => match tray_texture_from_pixmap(*width, *height, argb) {
            Some(texture) => image.set_paintable(Some(&texture)),
            None => image.set_icon_name(Some(FALLBACK)),
        },
        TrayIcon::None => image.set_icon_name(Some(FALLBACK)),
    }
}

/// Converts a `TrayIcon::Pixmap`'s raw bytes (32-bit ARGB, network/big-endian
/// byte order, i.e. each pixel is `[A, R, G, B]`) into a paintable.
fn tray_texture_from_pixmap(width: i32, height: i32, argb: &[u8]) -> Option<gdk::Texture> {
    if width <= 0 || height <= 0 || argb.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let mut rgba = vec![0_u8; argb.len()];
    for (pixel_in, pixel_out) in argb.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        pixel_out.copy_from_slice(&[pixel_in[1], pixel_in[2], pixel_in[3], pixel_in[0]]);
    }
    let bytes = glib::Bytes::from_owned(rgba);
    let texture = gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        (width * 4) as usize,
    );
    Some(texture.upcast())
}

fn clear_list_box(container: &gtk::ListBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn format_preview_metadata(metadata: &[(String, String)]) -> String {
    metadata
        .chunks(2)
        .map(|fields| {
            fields
                .iter()
                .map(|(name, value)| format!("{} {}", name.to_uppercase(), value))
                .collect::<Vec<_>>()
                .join("  //  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn highlight_color(name: &str) -> &'static str {
    match name.split('.').next().unwrap_or(name) {
        "comment" => "#7f849c",
        "keyword" | "conditional" | "exception" => "#cba6f7",
        "string" => "#a6e3a1",
        "number" | "boolean" | "constant" => "#fab387",
        "function" | "method" | "constructor" => "#89b4fa",
        "type" | "module" | "namespace" => "#f9e2af",
        "property" | "attribute" => "#94e2d5",
        "tag" | "label" => "#f38ba8",
        "operator" | "punctuation" => "#bac2de",
        _ => "#cdd6f4",
    }
}

fn lerp(start: f64, target: f64, progress: f64) -> f64 {
    start + (target - start) * progress
}

/// Formats a microsecond duration as `M:SS`, or `H:MM:SS` past one hour.
fn format_media_time(microseconds: i64) -> String {
    let total_seconds = (microseconds.max(0) / 1_000_000) as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Returns the same discovery snapshot with one player promoted into the
/// top-level fields consumed by the dashboard controls. Selection is purely
/// presentational: it never invokes Play/PlayPause and therefore cannot
/// disturb another player's playback.
fn media_state_for_player(state: &MediaState, service: Option<&str>) -> MediaState {
    let player: &MediaPlayer = service
        .and_then(|service| {
            state
                .players
                .iter()
                .find(|player| player.service == service)
        })
        .or_else(|| {
            state
                .players
                .iter()
                .find(|player| player.service == state.service)
        })
        .unwrap_or_else(|| {
            // Every MediaState is built from at least one discovered player.
            state.players.first().expect("media state without players")
        });
    MediaState {
        player: player.player.clone(),
        service: player.service.clone(),
        title: player.title.clone(),
        artist: player.artist.clone(),
        album: player.album.clone(),
        app_icon: player.app_icon.clone(),
        position_us: player.position_us,
        length_us: player.length_us,
        can_play: player.can_play,
        can_pause: player.can_pause,
        can_go_next: player.can_go_next,
        can_go_previous: player.can_go_previous,
        status: player.status,
        players: state.players.clone(),
    }
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
    use super::{battery_icon_name, format_media_time, media_state_for_player, resolved_scale};
    use crate::state::{MediaPlayer, MediaState, PlaybackStatus};

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
            battery_icon_name(12, "Discharging"),
            "xsi-battery-level-10-symbolic"
        );
        assert_eq!(
            battery_icon_name(87, "Charging"),
            "xsi-battery-level-90-charging-symbolic"
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
