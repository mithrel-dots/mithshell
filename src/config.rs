use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::weather::WeatherProvider;

pub const DEFAULT_SOURCE_COLOR: &str = "#9aa7ff";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub shell: ShellConfig,
    pub media: MediaConfig,
    pub theme: ThemeConfig,
    pub weather: WeatherConfig,
    pub lock: LockConfig,
    pub notifications: NotificationConfig,
    pub tray: TrayConfig,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(contents) => toml::from_str(&contents)
                .with_context(|| format!("failed to parse {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellConfig {
    /// Exact Wayland connector names, or a single `*` entry for every output.
    pub monitors: Vec<String>,
    pub top_margin: i32,
    pub exclusive_zone: i32,
    pub animation_ms: u32,
    /// UI scale. Values <= 0 select a scale from the monitor's logical width.
    pub scale: f64,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            monitors: vec!["*".to_owned()],
            top_margin: 6,
            exclusive_zone: 48,
            animation_ms: 280,
            scale: 0.0,
        }
    }
}

impl ShellConfig {
    pub fn shows_on(&self, connector: &str) -> bool {
        self.monitors
            .iter()
            .any(|name| name == "*" || name == connector)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
    /// Maximum media-pill width as a multiple of the compact width.
    pub max_width_factor: f64,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_width_factor: 1.8,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherConfig {
    /// Which upstream forecast service the weather tile polls.
    pub provider: WeatherProvider,
    /// City name for the forecast. When unset (or when the city cannot be
    /// resolved), the provider falls back to best-effort IP geolocation.
    pub city: Option<String>,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            provider: WeatherProvider::Wttr,
            city: None,
        }
    }
}

/// Lock screen appearance and authentication.
///
/// The blur is applied to a screenshot taken the moment before the lock
/// appears, so these values trade capture-to-visible latency against how
/// soft the backdrop looks. `downscale` is by far the strongest lever: the
/// blur cost falls with its square.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LockConfig {
    /// PAM service to authenticate against. When unset, `/etc/pam.d/mithshell`
    /// is used if it exists and `login` is inherited otherwise.
    pub pam_service: Option<String>,
    /// Box-blur radius, measured in downscaled pixels.
    pub blur_radius: u32,
    /// Integer shrink factor applied to the screenshot before blurring.
    pub blur_downscale: u32,
    /// Brightness multiplier for the backdrop, from 0 (black) to 1 (untouched).
    pub dim: f64,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            pam_service: None,
            blur_radius: 6,
            blur_downscale: 6,
            dim: 0.55,
        }
    }
}

impl LockConfig {
    /// Clamps the configured values into ranges the blur can actually
    /// honour, so a typo cannot wedge the daemon in a multi-second blur.
    pub fn blur_settings(&self) -> crate::lock::backdrop::BlurSettings {
        crate::lock::backdrop::BlurSettings {
            radius: self.blur_radius.min(64) as usize,
            downscale: self.blur_downscale.clamp(1, 32) as usize,
            dim: if self.dim.is_finite() {
                self.dim.clamp(0.0, 1.0)
            } else {
                Self::default().dim
            },
        }
    }
}

/// Desktop notification (`org.freedesktop.Notifications`) popup behavior.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationConfig {
    /// When `false`, incoming notifications are still acknowledged over
    /// D-Bus (so senders don't error out) but nothing is shown or recorded.
    pub enabled: bool,
    /// Where a notification appears.
    pub position: NotificationPosition,
    /// Fallback duration a notification stays visible for, honored when the
    /// sender does not request an explicit `expire_timeout`.
    pub timeout_ms: u64,
    /// Maximum simultaneously visible toasts. Only applies to
    /// `below-pill`/corner positions; `pill` shows one notification at a
    /// time, queueing the rest.
    pub max_visible: usize,
    /// Number of notifications kept in the dashboard's notification history.
    pub max_history: usize,
    /// Spacing between stacked toasts, and between the island and the
    /// popup in `below-pill` position.
    pub gap: i32,
    /// Distance from the screen edges for the corner positions.
    pub margin: i32,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            position: NotificationPosition::default(),
            timeout_ms: 5_000,
            max_visible: 5,
            max_history: 50,
            gap: 8,
            margin: 12,
        }
    }
}

/// Where an incoming notification is rendered.
///
/// `Pill` reuses the island's own surface exactly like the OSD does --
/// showing one notification at a time in place of the compact pill -- and
/// is the default so a fresh install behaves consistently with the
/// existing OSD without opening any extra surface. The remaining variants
/// spawn a separate small popup instead of touching the pill: `BelowPill`
/// centers it directly under the island, and the four corner variants
/// anchor it to a screen corner.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationPosition {
    #[default]
    Pill,
    BelowPill,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl NotificationPosition {
    /// `true` for the four screen-corner variants, i.e. positions that
    /// anchor to two adjacent layer-shell edges rather than following the
    /// island.
    pub fn is_corner(self) -> bool {
        matches!(
            self,
            Self::TopLeft | Self::TopRight | Self::BottomLeft | Self::BottomRight
        )
    }
}

/// System tray (`org.kde.StatusNotifierItem`) support.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TrayConfig {
    /// When `false`, mithshell neither hosts nor watches for
    /// `org.kde.StatusNotifierItem` tray icons, and the pill never grows a
    /// tray section.
    pub enabled: bool,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// Where palette colors come from: generated Material You, or inherited
    /// from the active GTK theme's named colors.
    pub engine: PaletteEngine,
    pub mode: ThemeMode,
    pub variant: ThemeVariant,
    pub source: ThemeSource,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            engine: PaletteEngine::Material,
            mode: ThemeMode::Dark,
            variant: ThemeVariant::TonalSpot,
            source: ThemeSource::Color {
                value: DEFAULT_SOURCE_COLOR.to_owned(),
            },
        }
    }
}

/// Selects how the `ms_*` role colors used by `style.css` are produced.
/// `mode`/`variant`/`source` only apply when `engine = "material"`.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaletteEngine {
    /// Generate a Material You scheme from `source` (color or image).
    #[default]
    Material,
    /// Alias the palette to the active GTK theme's standard named colors
    /// (`@accent_color`, `@window_bg_color`, `@borders`, ...).
    Gtk,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeVariant {
    #[default]
    TonalSpot,
    Content,
    Expressive,
    Fidelity,
    FruitSalad,
    Monochrome,
    Neutral,
    Rainbow,
    Vibrant,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ThemeSource {
    Color { value: String },
    Image { path: PathBuf },
}

impl Default for ThemeSource {
    fn default() -> Self {
        Self::Color {
            value: DEFAULT_SOURCE_COLOR.to_owned(),
        }
    }
}

pub fn config_path(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(expand_home(path));
    }

    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?
        .join("mithshell")
        .join("config.toml"))
}

/// Path to an optional user stylesheet living next to `config.toml`. When
/// present, its contents are loaded as a higher-priority CSS provider layered
/// on top of the generated/inherited palette, so it can override individual
/// `@ms_*` colors or arbitrary widget rules.
pub fn colors_css_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("colors.css")
}

/// The user's personal GTK4 stylesheet override
/// (`$XDG_CONFIG_HOME/gtk-4.0/gtk.css`). This is the common target external
/// palette generators (matugen, wallust, ...) write `@define-color` roles
/// to, and what `theme.engine = "gtk"` parses directly so mithshell can
/// follow palette changes without a restart.
pub fn gtk_user_css_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?
        .join("gtk-4.0")
        .join("gtk.css"))
}

pub fn state_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("mithshell"))
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CACHE_HOME", ".cache")?.join("mithshell"))
}

pub fn runtime_dir() -> Result<PathBuf> {
    let directory = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is not set")?;
    Ok(directory.join("mithshell"))
}

pub fn default_socket_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("ipc.sock"))
}

pub fn expand_home(path: PathBuf) -> PathBuf {
    let string = path.to_string_lossy();
    if string == "~" {
        return env::var_os("HOME").map(PathBuf::from).unwrap_or(path);
    }
    if let Some(rest) = string.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path
}

/// Resolves an XDG base directory, falling back to `$HOME/{fallback}` when
/// the variable is unset. `pub(crate)` so other modules resolving their own
/// paths under a standard XDG directory (e.g. `setup::install_tarragon`
/// under `XDG_CONFIG_HOME`) don't have to duplicate this fallback.
pub(crate) fn xdg_dir(variable: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(path) = env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(fallback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_show_on_every_monitor() {
        assert!(ShellConfig::default().shows_on("DP-7"));
    }

    #[test]
    fn exact_monitor_selection_does_not_fall_back() {
        let shell = ShellConfig {
            monitors: vec!["DP-2".into()],
            ..ShellConfig::default()
        };
        assert!(shell.shows_on("DP-2"));
        assert!(!shell.shows_on("DP-1"));
    }

    #[test]
    fn parses_tagged_image_source() {
        let config: AppConfig = toml::from_str(
            r#"
            [shell]
            monitors = ["DP-1"]

            [theme]
            mode = "dark"

            [theme.source]
            kind = "image"
            path = "~/wallpaper.png"
            "#,
        )
        .unwrap();

        assert_eq!(config.shell.monitors, ["DP-1"]);
        assert!(matches!(config.theme.source, ThemeSource::Image { .. }));
        assert_eq!(config.media.max_width_factor, 1.8);
    }

    #[test]
    fn defaults_to_material_engine() {
        assert_eq!(ThemeConfig::default().engine, PaletteEngine::Material);
    }

    #[test]
    fn parses_gtk_engine() {
        let config: AppConfig = toml::from_str(
            r#"
            [theme]
            engine = "gtk"
            "#,
        )
        .unwrap();

        assert_eq!(config.theme.engine, PaletteEngine::Gtk);
    }

    #[test]
    fn derives_colors_css_path_next_to_config() {
        let config_path = PathBuf::from("/home/user/.config/mithshell/config.toml");
        assert_eq!(
            colors_css_path(&config_path),
            PathBuf::from("/home/user/.config/mithshell/colors.css")
        );
    }

    #[test]
    fn parses_media_width_factor() {
        let config: AppConfig = toml::from_str(
            r#"
            [media]
            max_width_factor = 1.6
            "#,
        )
        .unwrap();

        assert_eq!(config.media.max_width_factor, 1.6);
    }

    #[test]
    fn lock_defaults_to_inheriting_the_pam_service() {
        let config = LockConfig::default();
        assert_eq!(config.pam_service, None);
        assert!(config.dim > 0.0 && config.dim < 1.0);
    }

    #[test]
    fn parses_a_lock_section() {
        let config: AppConfig = toml::from_str(
            r#"
            [lock]
            pam_service = "mithshell"
            blur_radius = 12
            blur_downscale = 4
            dim = 0.3
            "#,
        )
        .unwrap();

        assert_eq!(config.lock.pam_service.as_deref(), Some("mithshell"));
        let settings = config.lock.blur_settings();
        assert_eq!(settings.radius, 12);
        assert_eq!(settings.downscale, 4);
        assert!((settings.dim - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn blur_settings_clamp_hostile_values() {
        // A typo here would otherwise wedge the daemon in a multi-second
        // blur, or divide by zero in the downscaler.
        let config = LockConfig {
            pam_service: None,
            blur_radius: 100_000,
            blur_downscale: 0,
            dim: f64::NAN,
        };
        let settings = config.blur_settings();
        assert_eq!(settings.radius, 64);
        assert_eq!(settings.downscale, 1);
        assert!(settings.dim.is_finite());

        let settings = LockConfig {
            blur_downscale: 1_000,
            dim: 4.0,
            ..LockConfig::default()
        }
        .blur_settings();
        assert_eq!(settings.downscale, 32);
        assert!((settings.dim - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn defaults_to_wttr_weather_provider() {
        assert_eq!(WeatherConfig::default().provider, WeatherProvider::Wttr);
        assert_eq!(WeatherConfig::default().city, None);
    }

    #[test]
    fn parses_weather_provider() {
        let config: AppConfig = toml::from_str(
            r#"
            [weather]
            provider = "open-meteo"
            "#,
        )
        .unwrap();

        assert_eq!(config.weather.provider, WeatherProvider::OpenMeteo);
        assert_eq!(config.weather.city, None);
    }

    #[test]
    fn parses_weather_city() {
        let config: AppConfig = toml::from_str(
            r#"
            [weather]
            city = "Athens"
            "#,
        )
        .unwrap();

        assert_eq!(config.weather.city.as_deref(), Some("Athens"));
    }

    #[test]
    fn notifications_default_to_the_pill_position() {
        let config = NotificationConfig::default();
        assert_eq!(config.position, NotificationPosition::Pill);
        assert!(config.enabled);
        assert!(!config.position.is_corner());
    }

    #[test]
    fn parses_a_notification_corner_position() {
        let config: AppConfig = toml::from_str(
            r#"
            [notifications]
            position = "top-right"
            timeout_ms = 4000
            max_visible = 3
            "#,
        )
        .unwrap();

        assert_eq!(
            config.notifications.position,
            NotificationPosition::TopRight
        );
        assert!(config.notifications.position.is_corner());
        assert_eq!(config.notifications.timeout_ms, 4000);
        assert_eq!(config.notifications.max_visible, 3);
    }

    #[test]
    fn tray_defaults_to_enabled() {
        assert!(TrayConfig::default().enabled);
    }

    #[test]
    fn parses_a_disabled_tray_section() {
        let config: AppConfig = toml::from_str(
            r#"
            [tray]
            enabled = false
            "#,
        )
        .unwrap();

        assert!(!config.tray.enabled);
    }

    #[test]
    fn parses_the_below_pill_notification_position() {
        let config: AppConfig = toml::from_str(
            r#"
            [notifications]
            position = "below-pill"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.notifications.position,
            NotificationPosition::BelowPill
        );
        assert!(!config.notifications.position.is_corner());
    }
}
