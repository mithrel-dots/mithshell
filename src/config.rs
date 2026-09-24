use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{state::Urgency, weather::WeatherProvider};

pub const DEFAULT_SOURCE_COLOR: &str = "#9aa7ff";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub shell: ShellConfig,
    pub circles: CirclesConfig,
    pub launcher: LauncherConfig,
    pub media: MediaConfig,
    pub battery: BatteryConfig,
    pub theme: ThemeConfig,
    pub weather: WeatherConfig,
    pub lock: LockConfig,
    pub notifications: NotificationConfig,
    pub tray: TrayConfig,
    pub icons: IconConfig,
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

/// Optional module placement beside the island. Assignment is independent of
/// content availability: an empty assigned module stays out of its legacy view.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct CirclesConfig {
    pub left: CircleModule,
    pub right: CircleModule,
}

impl CirclesConfig {
    /// Whether a real module belongs to a circle instead of its legacy view.
    /// `None` is an empty slot, never a placed module.
    pub fn contains(&self, module: CircleModule) -> bool {
        module != CircleModule::None && (self.left == module || self.right == module)
    }
}

impl<'de> Deserialize<'de> for CirclesConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Default, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        struct Slots {
            left: CircleModule,
            right: CircleModule,
        }

        let slots = Slots::deserialize(deserializer)?;
        let config = Self {
            left: slots.left,
            right: slots.right,
        };
        // Both empty slots are valid, but a real module must have one owner.
        // Validate during parsing so AppConfig::load fails before reload teardown.
        if config.left != CircleModule::None && config.left == config.right {
            return Err(serde::de::Error::custom(
                "circles.left and circles.right must not assign the same non-none module",
            ));
        }
        Ok(config)
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CircleModule {
    #[default]
    None,
    Tray,
    Media,
    Notifications,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherConfig {
    pub presentation: LauncherPresentation,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherPresentation {
    #[default]
    Independent,
    Integrated,
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BatteryConfig {
    /// Draw the compact pill's battery level as a moving wave.
    pub wave: bool,
    /// Blend the battery hue into the active surface instead of using it raw.
    pub tint: bool,
    /// Direction of the divider: horizontal fills upward, vertical fills rightward.
    pub orientation: BatteryOrientation,
}

impl Default for BatteryConfig {
    fn default() -> Self {
        Self {
            wave: false,
            tint: true,
            orientation: BatteryOrientation::Horizontal,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BatteryOrientation {
    #[default]
    Horizontal,
    Vertical,
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
    /// Number of notifications kept in history, in the dashboard or a circle.
    pub max_history: usize,
    /// Latest retained notifications to preview on circle hover. Zero keeps
    /// only the minimal header and full-history affordance; it does not clear history.
    pub hover_preview_count: usize,
    /// Spacing between stacked toasts, and between the island and the
    /// popup in `below-pill` position.
    pub gap: i32,
    /// Distance from the screen edges for the corner positions.
    pub margin: i32,
    /// How notifications are routed when the focused monitor is fullscreen.
    pub fullscreen_strategy: FullscreenStrategy,
    /// Preferred fallback monitor connectors, in priority order.
    pub fallback_monitors: Vec<String>,
    /// Minimum urgency rendered above fullscreen windows, or `off`.
    pub overlay_over_fullscreen: NotificationOverlay,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            position: NotificationPosition::default(),
            timeout_ms: 5_000,
            max_visible: 5,
            max_history: 50,
            hover_preview_count: 3,
            gap: 8,
            margin: 12,
            fullscreen_strategy: FullscreenStrategy::default(),
            fallback_monitors: Vec::new(),
            overlay_over_fullscreen: NotificationOverlay::default(),
        }
    }
}

impl NotificationConfig {
    pub fn overlay_applies(&self, urgency: Urgency, fullscreen: bool) -> bool {
        fullscreen
            && self
                .overlay_over_fullscreen
                .threshold()
                .is_some_and(|threshold| urgency.at_least(threshold))
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FullscreenStrategy {
    Ignore,
    #[default]
    Fallback,
    AllNonFullscreen,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationOverlay {
    #[default]
    Off,
    Low,
    Normal,
    Critical,
}

impl NotificationOverlay {
    pub fn threshold(self) -> Option<Urgency> {
        match self {
            Self::Off => None,
            Self::Low => Some(Urgency::Low),
            Self::Normal => Some(Urgency::Normal),
            Self::Critical => Some(Urgency::Critical),
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
    /// Compact circle appearance; the legacy pill-hover tray is unaffected.
    pub compact_style: TrayCompactStyle,
    /// Maximum icons around the circle's count in `count-with-icons` mode.
    /// Zero is count-only. Expanded access and the total count are unaffected.
    pub max_compact_icons: usize,
    /// Maximum visible icons in the expanded circle. Values below one use one.
    /// Additional icons remain available by scrolling.
    pub max_expanded_icons: usize,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            compact_style: TrayCompactStyle::Count,
            max_compact_icons: 4,
            max_expanded_icons: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TrayCompactStyle {
    #[default]
    Count,
    CountWithIcons,
}

/// How mithshell draws its own chrome icons.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct IconConfig {
    pub style: IconStyle,
}

/// Selects the representation used for mithshell's own controls.
///
/// Icons supplied by other programs (tray items, notification `app_icon`,
/// MPRIS players, TarraGon results) always come from the icon theme and are
/// unaffected by this setting.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IconStyle {
    /// Draw chrome icons as Nerd Font glyphs. Any glyph the resolved font
    /// cannot render falls back to its themed icon individually, so this is
    /// safe even without a patched font installed.
    #[default]
    Glyph,
    /// Always use themed icons from the active GTK icon theme.
    Symbolic,
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
    fn optional_presentation_defaults_preserve_legacy_placement() {
        for source in ["", "[circles]\n[launcher]\n[tray]\n[notifications]"] {
            let config: AppConfig = toml::from_str(source).unwrap();
            assert_eq!(config.circles, CirclesConfig::default());
            assert_eq!(config.circles.left, CircleModule::None);
            assert_eq!(config.circles.right, CircleModule::None);
            assert!(!config.circles.contains(CircleModule::None));
            assert!(!config.circles.contains(CircleModule::Tray));
            assert_eq!(
                config.launcher.presentation,
                LauncherPresentation::Independent
            );
            assert_eq!(config.tray.compact_style, TrayCompactStyle::Count);
            assert_eq!(config.tray.max_compact_icons, 4);
            assert_eq!(config.notifications.hover_preview_count, 3);
        }
        let config: AppConfig = toml::from_str("[circles]\nright = 'media'").unwrap();
        assert_eq!(config.circles.left, CircleModule::None);
        assert!(config.circles.contains(CircleModule::Media));
    }

    #[test]
    fn circle_placements_allow_each_pair_except_duplicate_modules() {
        let modules = [
            ("none", CircleModule::None),
            ("tray", CircleModule::Tray),
            ("media", CircleModule::Media),
            ("notifications", CircleModule::Notifications),
        ];
        for (left_name, left) in modules {
            for (right_name, right) in modules {
                let source = format!("[circles]\nleft = '{left_name}'\nright = '{right_name}'");
                let parsed = toml::from_str::<AppConfig>(&source);
                if left == right && left != CircleModule::None {
                    let error = parsed.unwrap_err().to_string();
                    assert!(error.contains("same non-none module"), "{error}");
                    continue;
                }
                let config = parsed.unwrap();
                assert_eq!(config.circles, CirclesConfig { left, right });
                for (_, module) in modules {
                    assert_eq!(
                        config.circles.contains(module),
                        module != CircleModule::None && (left == module || right == module)
                    );
                }
            }
        }
    }

    #[test]
    fn parses_and_round_trips_presentation_options() {
        let source = r#"
            [circles]
            left = "tray"
            right = "notifications"
            [launcher]
            presentation = "integrated"
            [tray]
            compact_style = "count-with-icons"
            max_compact_icons = 7
            [notifications]
            hover_preview_count = 6
        "#;
        let config: AppConfig = toml::from_str(source).unwrap();
        assert_eq!(
            config.launcher.presentation,
            LauncherPresentation::Integrated
        );
        assert_eq!(config.tray.compact_style, TrayCompactStyle::CountWithIcons);
        assert_eq!(config.tray.max_compact_icons, 7);
        assert_eq!(config.notifications.hover_preview_count, 6);
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("compact_style = \"count-with-icons\""));
        let restored: AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.circles, config.circles);
        assert_eq!(restored.launcher, config.launcher);
        assert_eq!(restored.tray, config.tray);
        assert_eq!(restored.notifications, config.notifications);
    }

    #[test]
    fn zero_preview_limits_preserve_modules_and_history_capacity() {
        let config: AppConfig = toml::from_str(
            "[tray]\ncompact_style = 'count-with-icons'\nmax_compact_icons = 0\n\
             [notifications]\nhover_preview_count = 0",
        )
        .unwrap();
        assert!(config.tray.enabled);
        assert_eq!(config.tray.max_compact_icons, 0);
        assert!(config.notifications.enabled);
        assert_eq!(config.notifications.hover_preview_count, 0);
        assert_eq!(config.notifications.max_history, 50);
    }

    #[test]
    fn presentation_options_reject_unknown_fields_values_and_negative_counts() {
        for source in [
            "[circles]\nleft = 'clock'",
            "[circles]\nright = 'Media'",
            "[circles]\ncenter = 'tray'",
            "[launcher]\npresentation = 'floating'",
            "[launcher]\nmode = 'integrated'",
            "[tray]\ncompact_style = 'count_with_icons'",
            "[tray]\nmax_compact_icons = -1",
            "[tray]\nmax_icons = 4",
            "[notifications]\nhover_preview_count = -1",
            "[notifications]\nhover_previews = 3",
        ] {
            assert!(toml::from_str::<AppConfig>(source).is_err(), "{source}");
        }
    }

    #[test]
    fn load_rejects_duplicate_circles_at_the_reload_boundary() {
        // Keep test files in the project rather than the system temporary directory.
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("config-load-test-{}.toml", std::process::id()));
        fs::write(&path, "[circles]\nleft = 'media'\nright = 'media'").unwrap();
        let result = AppConfig::load(&path);
        fs::remove_file(&path).unwrap();
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains("same non-none module"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert_eq!(
            AppConfig::load(&path).unwrap().circles,
            CirclesConfig::default()
        );
    }

    #[test]
    fn legacy_example_without_presentation_options_keeps_its_defaults() {
        // The shipped example's existing sections remain a supported legacy config
        // when all newly introduced presentation keys are omitted.
        let mut legacy: toml::Table =
            toml::from_str(include_str!("../config/mithshell.example.toml")).unwrap();
        legacy.remove("circles");
        legacy.remove("launcher");
        let tray = legacy["tray"].as_table_mut().unwrap();
        tray.remove("compact_style");
        tray.remove("max_compact_icons");
        legacy["notifications"]
            .as_table_mut()
            .unwrap()
            .remove("hover_preview_count");
        let config: AppConfig = legacy.try_into().unwrap();
        assert_eq!(config.circles, CirclesConfig::default());
        assert_eq!(config.launcher, LauncherConfig::default());
        assert_eq!(config.tray, TrayConfig::default());
        assert_eq!(config.notifications, NotificationConfig::default());
        assert_eq!(config.shell.monitors, ["*"]);
        assert_eq!(config.media.max_width_factor, 1.8);
        assert_eq!(config.theme.source, ThemeSource::default());
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
    fn battery_wave_defaults_to_disabled_tinted_mode() {
        let config = BatteryConfig::default();
        assert!(!config.wave);
        assert!(config.tint);
        assert_eq!(config.orientation, BatteryOrientation::Horizontal);
        assert_eq!(AppConfig::default().battery, config);
    }

    #[test]
    fn parses_battery_wave_and_flat_color_mode() {
        let config: AppConfig = toml::from_str(
            r#"
            [battery]
            wave = true
            tint = false
            orientation = "vertical"
            "#,
        )
        .unwrap();

        assert!(config.battery.wave);
        assert!(!config.battery.tint);
        assert_eq!(config.battery.orientation, BatteryOrientation::Vertical);
    }

    #[test]
    fn battery_orientation_is_optional_and_rejects_typos() {
        let config: AppConfig = toml::from_str("[battery]\nwave = true\ntint = true").unwrap();
        assert_eq!(config.battery.orientation, BatteryOrientation::Horizontal);
        assert!(toml::from_str::<AppConfig>("[battery]\norientation = 'diagonal'").is_err());
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
        assert_eq!(config.fullscreen_strategy, FullscreenStrategy::Fallback);
        assert!(config.fallback_monitors.is_empty());
        assert_eq!(config.overlay_over_fullscreen, NotificationOverlay::Off);
    }

    #[test]
    fn parses_a_notification_corner_position() {
        let config: AppConfig = toml::from_str(
            r#"
            [notifications]
            position = "top-right"
            timeout_ms = 4000
            max_visible = 3
            fullscreen_strategy = "all-non-fullscreen"
            fallback_monitors = ["DP-2", "HDMI-A-1"]
            overlay_over_fullscreen = "critical"
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
        assert_eq!(
            config.notifications.fullscreen_strategy,
            FullscreenStrategy::AllNonFullscreen
        );
        assert_eq!(config.notifications.fallback_monitors, ["DP-2", "HDMI-A-1"]);
        assert_eq!(
            config.notifications.overlay_over_fullscreen,
            NotificationOverlay::Critical
        );
        assert!(
            config
                .notifications
                .overlay_applies(Urgency::Critical, true)
        );
        assert!(!config.notifications.overlay_applies(Urgency::Normal, true));
        assert!(
            !config
                .notifications
                .overlay_applies(Urgency::Critical, false)
        );
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
    fn the_shipped_example_config_parses() {
        // Every section carries `deny_unknown_fields`, so a stale key or a
        // renamed option in the example makes it a hard parse error for anyone
        // who copies it -- which `just install-config` does by default.
        let example = include_str!("../config/mithshell.example.toml");
        toml::from_str::<AppConfig>(example).expect("the example config should parse");
    }

    #[test]
    fn icons_default_to_glyphs() {
        assert_eq!(IconConfig::default().style, IconStyle::Glyph);
        let config = AppConfig::default();
        assert_eq!(config.icons.style, IconStyle::Glyph);
    }

    #[test]
    fn parses_a_symbolic_icon_section() {
        let config: AppConfig = toml::from_str(
            r#"
            [icons]
            style = "symbolic"
            "#,
        )
        .unwrap();

        assert_eq!(config.icons.style, IconStyle::Symbolic);
    }

    #[test]
    fn rejects_an_unknown_icon_style() {
        let error = toml::from_str::<AppConfig>(
            r#"
            [icons]
            style = "emoji"
            "#,
        );

        assert!(error.is_err());
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

    #[test]
    fn parses_disabled_fullscreen_overlay() {
        let config: AppConfig = toml::from_str(
            r#"
            [notifications]
            overlay_over_fullscreen = "off"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.notifications.overlay_over_fullscreen,
            NotificationOverlay::Off
        );
    }
}
