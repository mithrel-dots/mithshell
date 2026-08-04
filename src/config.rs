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

fn xdg_dir(variable: &str, fallback: &str) -> Result<PathBuf> {
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
}
