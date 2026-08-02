mod island;

use gtk::{CssProvider, gdk};

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
