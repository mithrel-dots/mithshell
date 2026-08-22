use std::{collections::HashMap, fs, path::PathBuf, str::FromStr, thread};

use anyhow::{Context, Result};
use async_channel::Sender;
use gtk::prelude::*;
use log::warn;
use material_colors::{
    color::Argb,
    dynamic_color::Variant,
    image::{FilterType, ImageReader},
    scheme::Scheme,
    theme::ThemeBuilder,
};
use notify::{Event, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};

use crate::{
    config::{
        DEFAULT_SOURCE_COLOR, PaletteEngine, ThemeConfig, ThemeMode, ThemeSource, ThemeVariant,
        expand_home, gtk_user_css_path, state_dir,
    },
    state::Palette,
};

/// Assembles a `Palette` from any `lookup(names, fallback) -> hex` closure,
/// shared between the file-parsing and GTK style-context backends so the
/// role-to-name mapping (and its fallback chain) only has to be written
/// once.
fn build_gtk_palette(
    mode: ThemeMode,
    source: &str,
    lookup: impl Fn(&[&str], &str) -> String,
) -> Palette {
    let on_primary = lookup(&["accent_fg_color", "theme_selected_fg_color"], "#ffffff");
    let outline = lookup(&["borders", "unfocused_borders"], "#79747e");
    let accent = lookup(&["accent_color"], DEFAULT_SOURCE_COLOR);
    Palette {
        source: source.to_owned(),
        mode,
        primary: lookup(
            &["accent_color", "theme_selected_bg_color"],
            DEFAULT_SOURCE_COLOR,
        ),
        on_primary: on_primary.clone(),
        primary_container: lookup(&["accent_bg_color", "accent_color"], DEFAULT_SOURCE_COLOR),
        on_primary_container: on_primary,
        secondary: accent.clone(),
        tertiary: accent,
        surface: lookup(&["window_bg_color", "theme_bg_color"], "#141318"),
        surface_container_low: lookup(&["view_bg_color", "theme_base_color"], "#1d1b20"),
        surface_container: lookup(&["headerbar_bg_color", "window_bg_color"], "#211f26"),
        surface_container_high: lookup(&["popover_bg_color", "headerbar_bg_color"], "#2b2930"),
        on_surface: lookup(&["window_fg_color", "theme_fg_color"], "#e6e0e9"),
        on_surface_variant: lookup(&["insensitive_fg_color", "view_fg_color"], "#cac4d0"),
        outline: outline.clone(),
        outline_variant: outline,
        error: lookup(&["error_color"], "#ffb4ab"),
    }
}

const OVERRIDE_FILE: &str = "theme.toml";
const PALETTE_FILE: &str = "palette.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeOverride {
    pub source: ThemeSource,
    pub mode: ThemeMode,
}

/// Generates a palette per `config.engine`. The GTK engine touches GTK
/// widgets and must be called from the main thread; the Material engine does
/// not depend on GTK and is safe to call from a background thread.
pub fn generate(config: &ThemeConfig) -> Result<Palette> {
    match config.engine {
        PaletteEngine::Material => generate_material(config),
        PaletteEngine::Gtk => Ok(generate_gtk()),
    }
}

fn generate_material(config: &ThemeConfig) -> Result<Palette> {
    let source = source_color(&config.source)?;
    let theme = ThemeBuilder::with_source(source)
        .variant(variant(config.variant))
        .build();
    let scheme = match config.mode {
        ThemeMode::Dark => &theme.schemes.dark,
        ThemeMode::Light => &theme.schemes.light,
    };
    Ok(palette(source, config.mode, scheme))
}

/// Builds a palette from the active GTK theme's standard named colors (the
/// same `@accent_color`/`@window_bg_color`/... convention libadwaita and
/// most modern GTK4 themes ship in their `gtk.css`), so mithshell follows
/// the system theme instead of generating its own scheme.
///
/// Prefers directly parsing `$XDG_CONFIG_HOME/gtk-4.0/gtk.css` -- the
/// common target external palette generators (matugen, wallust, ...) write
/// to -- since GTK itself only reads that file once at process startup and
/// never re-resolves a `StyleContext` color lookup when it changes on
/// disk. Falls back to a live GTK style context lookup when that file is
/// absent or doesn't define any of the roles we need.
pub fn generate_gtk() -> Palette {
    let mode = current_gtk_mode();
    if let Some(palette) = generate_gtk_from_file(mode) {
        return palette;
    }
    generate_gtk_from_style_context(mode)
}

fn current_gtk_mode() -> ThemeMode {
    gtk::Settings::default()
        .map(|settings| {
            if settings.is_gtk_application_prefer_dark_theme() {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            }
        })
        .unwrap_or_default()
}

fn generate_gtk_from_file(mode: ThemeMode) -> Option<Palette> {
    let path = gtk_user_css_path().ok()?;
    let css = fs::read_to_string(&path).ok()?;
    let colors = parse_named_colors(&css);
    if colors.is_empty() {
        return None;
    }
    Some(build_gtk_palette(mode, "gtk-theme", |names, fallback| {
        names
            .iter()
            .find_map(|name| colors.get(*name).and_then(|value| normalize_hex(value)))
            .unwrap_or_else(|| fallback.to_owned())
    }))
}

/// `StyleContext::lookup_color` has been deprecated since GTK 4.10 with no
/// direct replacement for resolving a named color to RGBA outside of a
/// stylesheet; it remains the only way to read the current theme's palette
/// when there's no `gtk.css` to parse directly, and continues to function
/// correctly.
#[allow(deprecated)]
fn generate_gtk_from_style_context(mode: ThemeMode) -> Palette {
    let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let context = widget.style_context();
    build_gtk_palette(mode, "gtk-theme", |names, fallback| {
        names
            .iter()
            .find_map(|name| context.lookup_color(name))
            .map(|rgba| {
                hex(Argb::new(
                    255,
                    (rgba.red() * 255.0).round() as u8,
                    (rgba.green() * 255.0).round() as u8,
                    (rgba.blue() * 255.0).round() as u8,
                ))
            })
            .unwrap_or_else(|| fallback.to_owned())
    })
}

/// Minimal `@define-color <name> <value>;` extractor -- not a CSS parser.
/// Handles the flat `#rrggbb`/`#rgb` and single-level `@other-name` alias
/// forms that matugen/wallust-style gtk4 templates emit; anything else
/// (functions like `alpha()`, gradients, ...) is left unresolved so the
/// caller's fallback chain takes over instead.
fn parse_named_colors(css: &str) -> HashMap<String, String> {
    let mut raw = HashMap::new();
    for statement in css.split(';') {
        let Some(rest) = statement.trim().strip_prefix("@define-color") else {
            continue;
        };
        let Some((name, value)) = rest.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if !name.is_empty() && !value.is_empty() {
            raw.insert(name.to_owned(), value.to_owned());
        }
    }

    let mut resolved = HashMap::with_capacity(raw.len());
    for name in raw.keys() {
        if let Some(value) = resolve_alias(&raw, name, 0) {
            resolved.insert(name.clone(), value);
        }
    }
    resolved
}

fn resolve_alias(raw: &HashMap<String, String>, name: &str, depth: u8) -> Option<String> {
    const MAX_ALIAS_DEPTH: u8 = 8;
    if depth > MAX_ALIAS_DEPTH {
        return None;
    }
    let value = raw.get(name)?;
    match value.strip_prefix('@') {
        Some(alias) => resolve_alias(raw, alias, depth + 1),
        None => Some(value.clone()),
    }
}

fn normalize_hex(value: &str) -> Option<String> {
    let value = value.trim();
    if !value.starts_with('#') || !value[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    match value.len() {
        7 => Some(value.to_ascii_lowercase()),
        4 => {
            let mut expanded = String::with_capacity(7);
            expanded.push('#');
            for ch in value[1..].chars() {
                expanded.push(ch);
                expanded.push(ch);
            }
            Some(expanded.to_ascii_lowercase())
        }
        _ => None,
    }
}

/// Watches `$XDG_CONFIG_HOME/gtk-4.0/gtk.css` for changes and sends a `()`
/// signal whenever it's modified, so callers can regenerate the `gtk`
/// palette engine without restarting mithshell when an external tool (or
/// the user) rewrites it. The parent directory is watched non-recursively
/// rather than the file itself, since tools typically replace config files
/// via a temp-file-then-rename rather than an in-place write.
pub fn watch_gtk_css(sender: Sender<()>) -> Option<thread::JoinHandle<()>> {
    let path = gtk_user_css_path().ok()?;
    let watch_dir = path.parent()?.to_path_buf();
    Some(thread::spawn(move || {
        if let Err(error) = fs::create_dir_all(&watch_dir) {
            warn!("failed to create {}: {error}", watch_dir.display());
            return;
        }
        let target = path.clone();
        let mut watcher = match notify::recommended_watcher(move |event: notify::Result<Event>| {
            let Ok(event) = event else {
                return;
            };
            if event.paths.iter().any(|changed| changed == &target) {
                let _ = sender.send_blocking(());
            }
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                warn!("failed to start a watcher for {}: {error}", path.display());
                return;
            }
        };
        if let Err(error) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
            warn!("failed to watch {}: {error}", watch_dir.display());
            return;
        }
        // Keep the watcher (and this thread) alive for the process lifetime;
        // nothing ever unparks it.
        loop {
            thread::park();
        }
    }))
}

pub fn load_override() -> Result<Option<ThemeOverride>> {
    let path = state_dir()?.join(OVERRIDE_FILE);
    match fs::read_to_string(&path) {
        Ok(contents) => toml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))
            .map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn persist(theme: &ThemeOverride) -> Result<()> {
    let directory = state_dir()?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let path = directory.join(OVERRIDE_FILE);
    fs::write(&path, toml::to_string_pretty(theme)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

pub fn clear_override() -> Result<()> {
    let path = state_dir()?.join(OVERRIDE_FILE);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

pub fn export_palette(palette: &Palette) -> Result<PathBuf> {
    let directory = state_dir()?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let destination = directory.join(PALETTE_FILE);
    let temporary = directory.join(format!("{PALETTE_FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(palette)?)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    fs::rename(&temporary, &destination)
        .with_context(|| format!("failed to publish {}", destination.display()))?;
    Ok(destination)
}

fn source_color(source: &ThemeSource) -> Result<Argb> {
    match source {
        ThemeSource::Color { value } => Argb::from_str(value)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("invalid source color `{value}`")),
        ThemeSource::Image { path } => {
            let path = expand_home(path.clone());
            let mut image = ImageReader::open(&path)
                .with_context(|| format!("failed to read theme image {}", path.display()))?;
            image.resize(128, 128, FilterType::Lanczos3);
            Ok(ImageReader::extract_color(&image))
        }
    }
}

fn variant(value: ThemeVariant) -> Variant {
    match value {
        ThemeVariant::TonalSpot => Variant::TonalSpot,
        ThemeVariant::Content => Variant::Content,
        ThemeVariant::Expressive => Variant::Expressive,
        ThemeVariant::Fidelity => Variant::Fidelity,
        ThemeVariant::FruitSalad => Variant::FruitSalad,
        ThemeVariant::Monochrome => Variant::Monochrome,
        ThemeVariant::Neutral => Variant::Neutral,
        ThemeVariant::Rainbow => Variant::Rainbow,
        ThemeVariant::Vibrant => Variant::Vibrant,
    }
}

fn palette(source: Argb, mode: ThemeMode, scheme: &Scheme) -> Palette {
    Palette {
        source: hex(source),
        mode,
        primary: hex(scheme.primary),
        on_primary: hex(scheme.on_primary),
        primary_container: hex(scheme.primary_container),
        on_primary_container: hex(scheme.on_primary_container),
        secondary: hex(scheme.secondary),
        tertiary: hex(scheme.tertiary),
        surface: hex(scheme.surface),
        surface_container_low: hex(scheme.surface_container_low),
        surface_container: hex(scheme.surface_container),
        surface_container_high: hex(scheme.surface_container_high),
        on_surface: hex(scheme.on_surface),
        on_surface_variant: hex(scheme.on_surface_variant),
        outline: hex(scheme.outline),
        outline_variant: hex(scheme.outline_variant),
        error: hex(scheme.error),
    }
}

fn hex(color: Argb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue)
}

pub fn apply_override(config: &mut ThemeConfig, override_theme: ThemeOverride) {
    config.source = override_theme.source;
    config.mode = override_theme.mode;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_expected_material_roles() {
        let palette = generate(&ThemeConfig::default()).unwrap();
        assert!(palette.primary.starts_with('#'));
        assert_ne!(palette.primary, palette.on_primary);
        assert_eq!(palette.mode, ThemeMode::Dark);
    }

    #[test]
    fn formats_argb_as_css_rgb() {
        assert_eq!(hex(Argb::new(255, 10, 132, 255)), "#0a84ff");
    }

    #[test]
    fn parses_define_color_statements_across_lines_and_semicolons() {
        let css = "\
@define-color accent_color #d9b9ff;
@define-color window_bg_color #16111b; @define-color window_fg_color #e9dfee;
";
        let colors = parse_named_colors(css);
        assert_eq!(
            colors.get("accent_color").map(String::as_str),
            Some("#d9b9ff")
        );
        assert_eq!(
            colors.get("window_bg_color").map(String::as_str),
            Some("#16111b")
        );
        assert_eq!(
            colors.get("window_fg_color").map(String::as_str),
            Some("#e9dfee")
        );
    }

    #[test]
    fn resolves_simple_aliases() {
        let css = "\
@define-color accent_bg_color #d9b9ff;
@define-color theme_selected_bg_color @accent_bg_color;
";
        let colors = parse_named_colors(css);
        assert_eq!(
            colors.get("theme_selected_bg_color").map(String::as_str),
            Some("#d9b9ff")
        );
    }

    #[test]
    fn normalize_hex_rejects_functional_values() {
        // parse_named_colors only resolves plain hex/alias forms; functions
        // like alpha() are still stored as their literal text, but
        // normalize_hex (used at lookup time) rejects them so the caller's
        // fallback chain takes over instead of a garbage color.
        assert_eq!(normalize_hex("alpha(@accent_bg_color, 0.5)"), None);
    }

    #[test]
    fn ignores_alias_cycles_instead_of_recursing_forever() {
        let css = "\
@define-color a @b;
@define-color b @a;
";
        let colors = parse_named_colors(css);
        assert_eq!(colors.get("a"), None);
        assert_eq!(colors.get("b"), None);
    }

    #[test]
    fn normalizes_short_and_long_hex_forms() {
        assert_eq!(normalize_hex("#ABC"), Some("#aabbcc".to_owned()));
        assert_eq!(normalize_hex("#D9B9FF"), Some("#d9b9ff".to_owned()));
        assert_eq!(normalize_hex("rgba(1, 2, 3, 0.5)"), None);
        assert_eq!(normalize_hex("#zzzzzz"), None);
    }
}
