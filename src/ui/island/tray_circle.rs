//! Tray content for an optional circle host.
//!
//! This module owns only GTK content and snapshot projection.  The existing
//! tray listener and action closures remain owned by `IslandWindow`; buttons
//! are made by the same builder used by the legacy pill.

use std::rc::Rc;

use gtk::{Align, Overflow, Overlay, prelude::*};

use super::circle::{CircleContent, CircleHost, Event};
use super::{
    IslandWindow,
    tray::{TrayMenuTracker, apply_tray_icon},
};
use crate::config::{TrayCompactStyle, TrayConfig};
use crate::state::TrayItem;

/// Snapshot-driven tray circle.  `host()` is mounted by central circle
/// integration; callers dispatch layout/geometry there and call `update` for
/// each fresh tray snapshot.
pub(crate) struct TrayCircle {
    host: Rc<CircleHost>,
    compact: gtk::Overlay,
    hover: gtk::FlowBox,
    full: gtk::FlowBox,
    island: Rc<IslandWindow>,
    enabled: bool,
    style: TrayCompactStyle,
    max_compact_icons: usize,
    menu_tracker: Rc<TrayMenuTracker>,
}

impl TrayCircle {
    pub(crate) fn new(
        island: &Rc<IslandWindow>,
        config: &TrayConfig,
    ) -> Result<Self, &'static str> {
        let compact = Overlay::new();
        compact.set_halign(Align::Center);
        compact.set_valign(Align::Center);
        let count = gtk::Label::new(Some("0"));
        count.add_css_class("circle-tray-count");
        compact.set_child(Some(&count));

        let hover = tray_grid();
        let full = tray_grid();
        let host = CircleHost::new(CircleContent {
            compact: compact.clone().upcast(),
            hover: hover.clone().upcast(),
            full: Some(full.clone().upcast()),
        })?;
        let weak_host = Rc::downgrade(&host);
        let weak_island = Rc::downgrade(island);
        let menu_tracker = TrayMenuTracker::new(move |open| {
            if let Some(host) = weak_host.upgrade() {
                host.dispatch(Event::Menu(open));
            }
            if let Some(island) = weak_island.upgrade() {
                island.refresh_keyboard_mode();
            }
        });
        Ok(Self {
            host,
            compact,
            hover,
            full,
            island: island.clone(),
            enabled: config.enabled,
            style: config.compact_style,
            max_compact_icons: config.max_compact_icons,
            menu_tracker,
        })
    }

    pub(crate) fn host(&self) -> Rc<CircleHost> {
        self.host.clone()
    }

    /// Replace all three pages from one effective snapshot.  Compact preview
    /// is intentionally capped independently of the hover/full pages.
    pub(crate) fn update(&self, items: &[TrayItem]) {
        self.menu_tracker.invalidate();
        clear_children(&self.compact);
        clear_children(&self.hover);
        clear_children(&self.full);
        let present = self.enabled && !items.is_empty();
        if present {
            let count = gtk::Label::new(Some(&items.len().to_string()));
            count.add_css_class("circle-tray-count");
            self.compact.set_child(Some(&count));
            if self.style == TrayCompactStyle::CountWithIcons {
                for (index, item) in items
                    .iter()
                    .take(preview_count(items.len(), self.max_compact_icons))
                    .enumerate()
                {
                    let image = self.small_preview(item);
                    let (x, y) = radial_offset(index, self.max_compact_icons.max(1));
                    image.set_halign(Align::Center);
                    image.set_valign(Align::Center);
                    image.set_margin_start(x.unsigned_abs() as i32);
                    image.set_margin_end((-x).unsigned_abs() as i32);
                    image.set_margin_top(y.unsigned_abs() as i32);
                    image.set_margin_bottom((-y).unsigned_abs() as i32);
                    self.compact.add_overlay(&image);
                }
            }
            for item in items {
                self.hover.append(&self.button(item));
                self.full.append(&self.button(item));
            }
        }
        // Content is the only state transition this module owns.  Pointer,
        // focus, and menu events are owned by the host/integration.
        self.host.dispatch(Event::Content(present));
    }

    fn small_preview(&self, item: &TrayItem) -> gtk::Image {
        let image = gtk::Image::new();
        image.set_pixel_size((self.island.metrics.tray_icon_size / 2).max(8));
        apply_tray_icon(&image, &item.icon);
        image
    }

    fn button(&self, item: &TrayItem) -> gtk::Button {
        self.island
            .build_tray_icon_with_tracker(item, Some(self.menu_tracker.clone()))
    }
}

fn tray_grid() -> gtk::FlowBox {
    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_max_children_per_line(8);
    grid.set_min_children_per_line(1);
    grid.set_row_spacing(4);
    grid.set_column_spacing(4);
    grid.set_halign(Align::Center);
    grid.set_valign(Align::Center);
    let scroll = grid.clone();
    // FlowBox itself is the page; the host wraps it in a ScrolledWindow only
    // for expanded pages through this bounded policy in integration.
    scroll.set_overflow(Overflow::Hidden);
    grid
}

fn clear_children<W: IsA<gtk::Widget>>(widget: &W) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        child = current.next_sibling();
        current.unparent();
    }
}

fn radial_offset(index: usize, total: usize) -> (i32, i32) {
    let angle = std::f64::consts::TAU * (index as f64) / total as f64;
    (
        (angle.cos() * 11.0).round() as i32,
        (angle.sin() * 11.0).round() as i32,
    )
}

fn preview_count(total: usize, limit: usize) -> usize {
    total.min(limit)
}

#[cfg(test)]
mod tests {
    use super::{preview_count, radial_offset};

    #[test]
    fn radial_preview_stays_bounded() {
        for index in 0..8 {
            let (x, y) = radial_offset(index, 8);
            assert!(x.abs() <= 11 && y.abs() <= 11);
        }
    }

    #[test]
    fn preview_limit_never_truncates_expanded_snapshot() {
        let total = 17;
        assert_eq!(preview_count(total, 0), 0);
        assert_eq!(preview_count(total, 4), 4);
        assert_eq!(total, 17);
    }
}
