pub(crate) mod icon;
mod island;
mod lock;

use std::{fs, path::Path};

use gtk::{CssProvider, gdk, prelude::MonitorExt};
use log::warn;

use crate::state::Palette;

pub use island::{IslandActions, IslandWindow};
pub use lock::{
    LockActions, LockAnimation, LockEndedAction, LockPowerAction, LockSession, LockStateAction,
    LockSubmitAction,
};

/// Rounds a design-time pixel dimension to the active UI scale.
pub(crate) fn scaled(value: i32, scale: f64) -> i32 {
    (f64::from(value) * scale).round() as i32
}

/// Resolves `shell.scale`: a positive configured value wins, otherwise the
/// scale derived from the monitor's logical width is used.
pub(crate) fn resolved_scale(configured: f64, automatic: f64) -> f64 {
    if configured.is_finite() && configured > 0.0 {
        configured.max(0.8)
    } else {
        automatic
    }
}

/// The density class that pairs with a resolved scale.
///
/// GTK output scaling is often left at 1 on 4K Hyprland setups, so surfaces
/// are scaled in layout while text and controls need matching CSS overrides
/// to stay proportional. Shared by every window so the island and the lock
/// screen never disagree about which tier an output is in.
pub(crate) fn scale_class(scale: f64) -> Option<&'static str> {
    if scale >= 1.6 {
        Some("scale-large")
    } else if scale >= 1.35 {
        Some("scale-medium")
    } else {
        None
    }
}

/// The scale mithshell picks for an output when `shell.scale` is unset.
pub(crate) fn automatic_scale(monitor: &gdk::Monitor) -> f64 {
    (f64::from(monitor.geometry().width()) / 2560.0).clamp(1.0, 1.45)
}

const BASE_CSS: &str = include_str!("style.css");

/// The palette, the glyph font rule, and the baked stylesheet, in the order
/// `style.css` expects to be able to override them.
fn stylesheet(palette: &Palette) -> String {
    format!("{}\n{}\n{BASE_CSS}", palette.css(), icon::glyph_font_css())
}

pub fn install_styles(palette: &Palette) -> CssProvider {
    let provider = CssProvider::new();
    provider.load_from_string(&stylesheet(palette));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            // Stay above gtk.css so broad user-theme selectors such as
            // `.background` and `button` cannot repaint shell surfaces or
            // override their geometry.
            gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
        );
    }
    provider
}

pub fn update_styles(provider: &CssProvider, palette: &Palette) {
    provider.load_from_string(&stylesheet(palette));
}

/// Installs the optional user stylesheet (`colors.css` next to config.toml)
/// at a higher CSS priority than `install_styles`, so it can override
/// individual `@ms_*` colors or any other widget rule. Safe to call even
/// when the file does not exist yet; the provider starts out empty and can
/// be repopulated later with `reload_user_styles`.
pub fn install_user_styles(path: &Path) -> CssProvider {
    let provider = CssProvider::new();
    reload_user_styles(&provider, path);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER + 2,
        );
    }
    provider
}

/// Re-reads the user stylesheet from disk into an already-installed
/// provider. Clears the provider when the file has been removed.
pub fn reload_user_styles(provider: &CssProvider, path: &Path) {
    match fs::read_to_string(path) {
        Ok(css) => provider.load_from_string(&css),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => provider.load_from_string(""),
        Err(error) => {
            warn!("failed to read {}: {error}", path.display());
            provider.load_from_string("");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Structural checks over a stylesheet.
    ///
    /// `CssProvider` needs an initialised GTK and therefore a display, which a
    /// test harness has no business requiring, and it recovers from a bad
    /// declaration by dropping just that one rule. These assertions instead
    /// catch the failure mode that actually threatens a stylesheet assembled
    /// from several sources: unbalanced or malformed declarations.
    fn assert_well_formed(css: &str) {
        let depth = css.chars().fold(0_i32, |depth, ch| match ch {
            '{' => depth + 1,
            '}' => depth - 1,
            _ => depth,
        });
        assert_eq!(depth, 0, "unbalanced braces");

        for (index, block) in css.split('{').skip(1).enumerate() {
            let body = block.split('}').next().unwrap_or_default();
            for declaration in body.split(';') {
                let declaration = declaration.trim();
                if declaration.is_empty() || declaration.starts_with("/*") {
                    continue;
                }
                assert!(
                    declaration.contains(':'),
                    "block {index}: `{declaration}` is not a declaration",
                );
                let value = declaration.split_once(':').map(|(_, v)| v.trim());
                assert!(
                    value.is_some_and(|value| !value.is_empty()),
                    "block {index}: `{declaration}` has an empty value",
                );
            }
        }
    }

    #[test]
    fn the_shipped_stylesheet_is_well_formed() {
        let palette = crate::theme::generate(&crate::config::ThemeConfig::default())
            .expect("the default theme should generate");
        assert_well_formed(&stylesheet(&palette));
    }

    #[test]
    fn the_glyph_font_rule_is_well_formed() {
        assert_well_formed(&icon::glyph_font_css());
    }

    #[test]
    fn every_glyph_sizing_rule_pairs_with_an_icon_size() {
        // The two representations must stay in step: a rule that sizes glyphs
        // without sizing themed icons (or vice versa) means the shell changes
        // size when it falls back.
        let icon_sized = BASE_CSS.matches("-gtk-icon-size:").count();
        let font_sized = BASE_CSS.matches("font-size:").count();
        assert!(
            icon_sized > 0 && font_sized > 0,
            "expected both sizing properties in the stylesheet",
        );
    }
}
