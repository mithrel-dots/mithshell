//! The resting compact pill: workspace dots, clock, battery, and the
//! hover-revealed tray segment, plus its content-driven width solver.

use super::*;

use std::rc::Rc;

use gtk::{Align, Orientation};

use super::{IslandWindow, Metrics, View, measure_clamped};

pub(super) fn compact_view(
    metrics: Metrics,
) -> (gtk::Box, gtk::Box, gtk::Label, gtk::Label, gtk::Box) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    root.set_size_request(metrics.compact_width, metrics.compact_height);
    root.add_css_class("compact-content");
    root.set_valign(Align::Start);

    let workspaces = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    workspaces.set_hexpand(false);
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);

    let clock = gtk::Label::new(Some("--:--"));
    clock.add_css_class("compact-clock");
    clock.set_hexpand(true);
    clock.set_halign(Align::Center);
    clock.set_valign(Align::Center);

    let battery = gtk::Label::new(None);
    battery.add_css_class("compact-battery");
    battery.set_hexpand(false);
    battery.set_halign(Align::End);
    battery.set_valign(Align::Center);
    battery.set_visible(false);

    // Hidden by default (`update_tray`/`set_tray_hovered`): only shown while
    // the pointer is over the pill and at least one tray item exists, per
    // `resize_compact`.
    let tray = gtk::Box::new(Orientation::Horizontal, metrics.spacing(3));
    tray.add_css_class("compact-tray");
    tray.set_hexpand(false);
    tray.set_halign(Align::End);
    tray.set_valign(Align::Center);
    tray.set_visible(false);

    root.append(&workspaces);
    root.append(&clock);
    root.append(&battery);
    root.append(&tray);
    (root, workspaces, clock, battery, tray)
}

impl IslandWindow {
    /// Recomputes the compact pill's width from the combined (individually
    /// capped) natural width of its children, and repositions it within
    /// `content` to match, the same way `resize_media` does for the media
    /// pill. Called whenever a child's content changes (workspaces,
    /// battery, tray) or the tray's hover-visibility toggles.
    pub(super) fn resize_compact(self: &Rc<Self>) {
        let workspaces_width = measure_clamped(
            &self.compact_workspaces,
            self.metrics.compact_workspaces_max_width,
        );
        let clock_width =
            measure_clamped(&self.compact_clock, self.metrics.compact_clock_max_width);
        let battery_width = if self.compact_battery.is_visible() {
            measure_clamped(
                &self.compact_battery,
                self.metrics.compact_battery_max_width,
            )
        } else {
            0
        };

        let tray_visible = self.tray_visible();
        self.compact_tray.set_visible(tray_visible);
        let tray_width = if tray_visible {
            measure_clamped(&self.compact_tray, self.metrics.compact_tray_max_width)
        } else {
            0
        };

        let segments = 2 + i32::from(battery_width > 0) + i32::from(tray_width > 0);
        let spacing = self.metrics.spacing(10) * (segments - 1).max(0);
        // Matches `.compact-content { padding: 0 15px; }` (both sides).
        let padding = self.metrics.spacing(30);
        let natural =
            workspaces_width + clock_width + battery_width + tray_width + spacing + padding;
        let width = natural.clamp(self.metrics.compact_min_width, self.metrics.media_max_width);

        self.compact
            .set_size_request(width, self.metrics.compact_height);
        self.content.move_(
            &self.compact,
            f64::from((self.metrics.search_width - width) / 2),
            0.0,
        );
        self.compact_width.set(width);

        if self.current_view.get() == View::Compact {
            self.set_view(View::Compact);
        }
    }

    /// Whether the tray row should be shown: at least one item exists, and
    /// either the pill (compact or media, whichever is active) is hovered
    /// or one of the items' menus is currently open.
    pub(super) fn tray_visible(&self) -> bool {
        (self.tray_hovered.get() || self.tray_menu_open.get()) && self.tray_item_count.get() > 0
    }

    /// Hides/reveals the tray row on hover, and resizes both pills (only
    /// the currently active one animates; the other silently follows so
    /// it's already correct if the view switches while hovered).
    pub(super) fn set_tray_hovered(self: &Rc<Self>, hovered: bool) {
        if self.tray_hovered.get() == hovered {
            return;
        }
        self.tray_hovered.set(hovered);
        self.resize_compact();
        self.resize_media();
    }
}
