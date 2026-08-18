use serde::{Deserialize, Serialize};

use crate::{config::ThemeMode, ipc::OsdKind};

#[derive(Debug, Clone, Default, Serialize)]
pub struct HyprlandSnapshot {
    pub monitors: Vec<HyprlandMonitor>,
    pub workspaces: Vec<Workspace>,
    pub active_window: Option<ActiveWindow>,
}

impl HyprlandSnapshot {
    pub fn focused_monitor(&self) -> Option<&HyprlandMonitor> {
        self.monitors.iter().find(|monitor| monitor.focused)
    }

    pub fn monitor(&self, name: &str) -> Option<&HyprlandMonitor> {
        self.monitors.iter().find(|monitor| monitor.name == name)
    }

    pub fn workspaces_for(&self, monitor: &str) -> Vec<&Workspace> {
        let mut workspaces: Vec<_> = self
            .workspaces
            .iter()
            .filter(|workspace| workspace.monitor == monitor && workspace.id > 0)
            .collect();
        workspaces.sort_by_key(|workspace| workspace.id);
        workspaces
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HyprlandMonitor {
    pub id: i64,
    pub name: String,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceRef,
    #[serde(rename = "specialWorkspace", default)]
    pub special_workspace: WorkspaceRef,
    #[serde(default, skip_deserializing)]
    pub fullscreen: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WorkspaceRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HyprlandClient {
    pub monitor: i64,
    pub workspace: WorkspaceRef,
    pub fullscreen: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub monitor: String,
    pub windows: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActiveWindow {
    pub class: String,
    pub title: String,
    pub monitor: i64,
    pub workspace: WorkspaceRef,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SystemSnapshot {
    pub audio: Option<AudioState>,
    pub brightness: Option<BrightnessState>,
    pub battery: Option<BatteryState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AudioState {
    pub percent: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaState {
    pub player: String,
    /// Full MPRIS D-Bus service name (e.g. `org.mpris.MediaPlayer2.spotify`),
    /// kept around so play/pause/next/previous calls can target the right
    /// player. Not part of the public IPC surface.
    #[serde(skip_serializing)]
    pub service: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub app_icon: Option<String>,
    /// Track position at the moment this state was captured, in
    /// microseconds. Since `MediaState` is only ever produced for a
    /// `Playing` player, a progress bar can interpolate forward from this
    /// baseline using a local clock instead of polling MPRIS continuously.
    pub position_us: i64,
    pub length_us: Option<i64>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub status: PlaybackStatus,
    pub players: Vec<MediaPlayer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaPlayer {
    pub player: String,
    #[serde(skip_serializing)]
    pub service: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub app_icon: Option<String>,
    pub position_us: i64,
    pub length_us: Option<i64>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub status: PlaybackStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrightnessState {
    pub percent: u8,
    #[serde(skip_serializing)]
    pub device: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryState {
    pub percent: u8,
    pub status: String,
}

/// A coarse weather condition, used to pick which placeholder pixel-art
/// illustration a forecast entry is drawn with. Deliberately broader than
/// the providers' underlying condition codes -- the icon set is illustrative
/// placeholder art, not a precise per-code library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WeatherCondition {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Sleet,
    Snow,
    Thunder,
    Wind,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeatherDay {
    /// ISO `YYYY-MM-DD`.
    pub date: String,
    pub weekday: String,
    /// `None` for placeholder days padded by `weather::pad_forecast` when a
    /// provider reports fewer than seven days.
    pub max_c: Option<i32>,
    pub min_c: Option<i32>,
    pub condition: WeatherCondition,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WeatherState {
    pub location: String,
    pub current_c: i32,
    pub condition: WeatherCondition,
    pub description: String,
    pub days: Vec<WeatherDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Palette {
    pub source: String,
    pub mode: ThemeMode,
    pub primary: String,
    pub on_primary: String,
    pub primary_container: String,
    pub on_primary_container: String,
    pub secondary: String,
    pub tertiary: String,
    pub surface: String,
    pub surface_container_low: String,
    pub surface_container: String,
    pub surface_container_high: String,
    pub on_surface: String,
    pub on_surface_variant: String,
    pub outline: String,
    pub outline_variant: String,
    pub error: String,
}

impl Palette {
    pub fn css(&self) -> String {
        format!(
            r#"
@define-color ms_primary {primary};
@define-color ms_on_primary {on_primary};
@define-color ms_primary_container {primary_container};
@define-color ms_on_primary_container {on_primary_container};
@define-color ms_secondary {secondary};
@define-color ms_tertiary {tertiary};
@define-color ms_surface {surface};
@define-color ms_surface_low {surface_low};
@define-color ms_surface_container {surface_container};
@define-color ms_surface_high {surface_high};
@define-color ms_on_surface {on_surface};
@define-color ms_on_surface_variant {on_surface_variant};
@define-color ms_outline {outline};
@define-color ms_outline_variant {outline_variant};
@define-color ms_error {error};
"#,
            primary = self.primary,
            on_primary = self.on_primary,
            primary_container = self.primary_container,
            on_primary_container = self.on_primary_container,
            secondary = self.secondary,
            tertiary = self.tertiary,
            surface = self.surface,
            surface_low = self.surface_container_low,
            surface_container = self.surface_container,
            surface_high = self.surface_container_high,
            on_surface = self.on_surface,
            on_surface_variant = self.on_surface_variant,
            outline = self.outline,
            outline_variant = self.outline_variant,
            error = self.error,
        )
    }
}

#[derive(Debug, Clone)]
pub struct OsdState {
    pub kind: OsdKind,
    pub value: u8,
    pub muted: bool,
    pub timeout_ms: u64,
}

/// Urgency hint (`org.freedesktop.Notifications`) carried by a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    /// Maps the raw `urgency` hint byte (0/1/2); anything else falls back
    /// to `Normal`, matching well-behaved senders that omit the hint.
    pub fn from_hint_byte(value: u8) -> Self {
        match value {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }

}

/// One `key`/`label` pair from a `Notify` call's `actions` array. A pair
/// whose key is `"default"` represents the action invoked by activating
/// the notification itself, rather than a labeled button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
}

/// How long a notification stays visible before it is treated as expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum NotificationTimeout {
    /// The sender did not request a specific duration; fall back to
    /// `notifications.timeout_ms`.
    Default,
    /// The sender explicitly asked for the notification to persist
    /// (`expire_timeout == 0`) until closed or dismissed.
    Never,
    Millis(u64),
}

impl NotificationTimeout {
    /// Resolves this into a concrete duration, or `None` when the
    /// notification should persist until explicitly closed.
    pub fn resolve(self, default_ms: u64) -> Option<u64> {
        match self {
            Self::Default => Some(default_ms),
            Self::Never => None,
            Self::Millis(ms) => Some(ms),
        }
    }
}

/// A `org.kde.StatusNotifierItem` tray icon, as tracked by `tray::start_listener`.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayItem {
    /// Stable UI identity (`service` D-Bus name + object path joined), used
    /// to key widgets/popovers across snapshots and to route click actions
    /// back to the right item.
    pub key: String,
    /// The item's D-Bus service (bus) name, addressed for `Activate`/
    /// `Scroll`/... method calls.
    pub service: String,
    /// The item's D-Bus object path, on `service`.
    pub object_path: String,
    pub id: String,
    pub title: String,
    pub tooltip: Option<String>,
    pub icon: TrayIcon,
    pub status: TrayStatus,
    pub item_is_menu: bool,
    /// Object path of a `com.canonical.dbusmenu` menu on `service`, when the
    /// item advertises one. Absent items fall back to the `ContextMenu`
    /// method on right click instead.
    pub menu_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    Passive,
    Active,
    NeedsAttention,
}

/// A tray item's icon, as reported over D-Bus. Kept as plain data (rather
/// than a `gdk::Texture`) so it can cross the thread boundary from the
/// listener thread; the UI converts it to a paintable when applying a
/// snapshot.
#[derive(Debug, Clone, PartialEq)]
pub enum TrayIcon {
    /// A themed icon name (`IconName`), resolved through the regular icon
    /// theme -- most well-behaved items report one of these.
    Name(String),
    /// Raw pixel data (`IconPixmap`), for items that ship their own bitmap.
    /// `argb` is 32-bit ARGB, network (big-endian) byte order, i.e. each
    /// pixel is the four bytes `[A, R, G, B]`, per the `org.kde.StatusNotifierItem`
    /// spec -- the largest reported size wins.
    Pixmap {
        width: i32,
        height: i32,
        argb: Vec<u8>,
    },
    None,
}

/// One entry of a `com.canonical.dbusmenu` layout, fetched on demand when a
/// tray item's context menu is opened.
#[derive(Debug, Clone, PartialEq)]
pub struct TrayMenuItem {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub separator: bool,
    /// `Some(true/false)` for a checkmark/radio entry; `None` for a plain one.
    pub checked: Option<bool>,
    pub children: Vec<TrayMenuItem>,
}

/// A single desktop notification, as received through
/// `org.freedesktop.Notifications`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    /// Either a themed icon name, or an absolute path to an image file.
    pub app_icon: Option<String>,
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    pub actions: Vec<NotificationAction>,
    pub timeout: NotificationTimeout,
}

impl Notification {
    /// The action invoked when the notification itself (rather than one of
    /// its labeled buttons) is activated, if the sender declared one.
    pub fn default_action(&self) -> Option<&NotificationAction> {
        self.actions.iter().find(|action| action.key == "default")
    }
}
