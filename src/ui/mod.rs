mod island;

use std::{fs, path::Path};

use gtk::{CssProvider, gdk};
use log::warn;

use crate::state::Palette;

pub use island::{IslandActions, IslandWindow};

const BASE_CSS: &str = include_str!("style.css");

pub fn install_styles(palette: &Palette) -> CssProvider {
    let provider = CssProvider::new();
    provider.load_from_string(&format!("{}\n{BASE_CSS}", palette.css()));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
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
            gtk::STYLE_PROVIDER_PRIORITY_USER,
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
