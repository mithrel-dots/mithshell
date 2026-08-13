mod island;
mod lock;

use std::{fs, path::Path};

use gtk::{CssProvider, gdk, prelude::MonitorExt};
use log::warn;

use crate::state::Palette;

pub use island::{IslandActions, IslandWindow};
pub use lock::{LockEndedAction, LockSession, LockSubmitAction};

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

pub fn install_styles(palette: &Palette) -> CssProvider {
    let provider = CssProvider::new();
    provider.load_from_string(&format!("{}\n{BASE_CSS}", palette.css()));
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
    provider.load_from_string(&format!("{}\n{BASE_CSS}", palette.css()));
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
