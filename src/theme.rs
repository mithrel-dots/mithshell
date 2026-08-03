use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result};
use gtk::prelude::*;
use material_colors::{
    color::Argb,
    dynamic_color::Variant,
    image::{FilterType, ImageReader},
    scheme::Scheme,
    theme::ThemeBuilder,
};
use serde::{Deserialize, Serialize};

use crate::{
    config::{
        DEFAULT_SOURCE_COLOR, PaletteEngine, ThemeConfig, ThemeMode, ThemeSource, ThemeVariant,
        expand_home, state_dir,
    },
    state::Palette,
};

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

/// Builds a palette by resolving the active GTK theme's standard named
/// colors (the same `@accent_color`/`@window_bg_color`/... convention
/// libadwaita and most modern GTK4 themes ship in their `gtk.css`), so
/// mithshell follows the system theme instead of generating its own scheme.
/// Falls back to the default Material You source color for any name the
/// active theme doesn't define.
///
/// `StyleContext::lookup_color` has been deprecated since GTK 4.10 with no
/// direct replacement for resolving a named color to RGBA outside of a
/// stylesheet; it remains the only way to read the current theme's palette
/// and continues to function correctly.
#[allow(deprecated)]
pub fn generate_gtk() -> Palette {
    let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let context = widget.style_context();
    let lookup = |names: &[&str], fallback: &str| -> String {
        for name in names {
            if let Some(rgba) = context.lookup_color(name) {
                return hex(Argb::new(
                    255,
                    (rgba.red() * 255.0).round() as u8,
                    (rgba.green() * 255.0).round() as u8,
                    (rgba.blue() * 255.0).round() as u8,
                ));
            }
        }
        fallback.to_owned()
    };
    let mode = gtk::Settings::default()
        .map(|settings| {
            if settings.is_gtk_application_prefer_dark_theme() {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            }
        })
        .unwrap_or_default();

    let on_primary = lookup(&["accent_fg_color", "theme_selected_fg_color"], "#ffffff");
    let outline = lookup(&["borders", "unfocused_borders"], "#79747e");
    Palette {
        source: "gtk-theme".to_owned(),
        mode,
        primary: lookup(
            &["accent_color", "theme_selected_bg_color"],
            DEFAULT_SOURCE_COLOR,
        ),
        on_primary: on_primary.clone(),
        primary_container: lookup(&["accent_bg_color", "accent_color"], DEFAULT_SOURCE_COLOR),
        on_primary_container: on_primary,
        secondary: lookup(&["accent_color"], DEFAULT_SOURCE_COLOR),
        tertiary: lookup(&["accent_color"], DEFAULT_SOURCE_COLOR),
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

pub fn override_path() -> Result<PathBuf> {
    Ok(state_dir()?.join(OVERRIDE_FILE))
}

pub fn palette_path() -> Result<PathBuf> {
    Ok(state_dir()?.join(PALETTE_FILE))
}

pub fn source_label(source: &ThemeSource) -> String {
    match source {
        ThemeSource::Color { value } => value.clone(),
        ThemeSource::Image { path } => Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image")
            .to_owned(),
    }
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
}
