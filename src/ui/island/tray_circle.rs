//! Tray content for an optional circle host.
//!
//! This module owns only GTK content and snapshot projection.  The existing
//! tray listener and action closures remain owned by `IslandWindow`; buttons
//! are made by the same builder used by the legacy pill.

use std::rc::{Rc, Weak};

use gtk::{Align, Overflow, Overlay, prelude::*};

use super::circle::{CircleContent, CircleHost, Event};
#[cfg(test)]
use super::tray::TrayMenuManager;
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
    island: Weak<IslandWindow>,
    enabled: bool,
    style: TrayCompactStyle,
    max_compact_icons: usize,
    menu_tracker: Rc<TrayMenuTracker>,
    scale: f64,
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
        ensure_page_measurement(&hover);
        ensure_page_measurement(&full);
        let weak_host = Rc::downgrade(&host);
        let weak_island = Rc::downgrade(island);
        let menu_tracker = TrayMenuTracker::new(island.tray_menu_manager.clone(), move |open| {
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
            island: Rc::downgrade(island),
            enabled: config.enabled,
            style: config.compact_style,
            max_compact_icons: config.max_compact_icons,
            menu_tracker,
            scale: island.metrics.scale,
        })
    }

    pub(crate) fn host(&self) -> Rc<CircleHost> {
        self.host.clone()
    }

    #[cfg(test)]
    fn synthetic(config: &TrayConfig, scale: f64) -> Self {
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
        })
        .expect("synthetic tray pages");
        ensure_page_measurement(&hover);
        ensure_page_measurement(&full);
        let manager = TrayMenuManager::new();
        let menu_tracker = TrayMenuTracker::new(manager, |_| {});
        Self {
            host,
            compact,
            hover,
            full,
            island: Weak::new(),
            enabled: config.enabled,
            style: config.compact_style,
            max_compact_icons: config.max_compact_icons,
            menu_tracker,
            scale,
        }
    }

    /// Replace all three pages from one effective snapshot.  Compact preview
    /// is intentionally capped independently of the hover/full pages.
    pub(crate) fn update(&self, items: &[TrayItem]) {
        // Popdowns can synchronously change keyboard mode and trigger a
        // relayout.  Keep those callbacks behind the child rebuild: GTK must
        // never allocate a FlowBox while its old children are being removed.
        self.menu_tracker.begin_batch();
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
                    let Some((x, y, size)) = compact_preview_layout(
                        self.scale,
                        preview_count(items.len(), self.max_compact_icons),
                    )
                    .get(index)
                    .copied() else {
                        continue;
                    };
                    let image = self.small_preview(item, size);
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
        self.menu_tracker.end_batch();
    }

    fn small_preview(&self, item: &TrayItem, size: i32) -> gtk::Image {
        let image = gtk::Image::new();
        image.set_pixel_size(size);
        apply_tray_icon(&image, &item.icon);
        image
    }

    fn button(&self, item: &TrayItem) -> gtk::Button {
        self.island
            .upgrade()
            .map_or_else(gtk::Button::new, |island| {
                island.build_tray_icon_with_tracker(item, Some(self.menu_tracker.clone()))
            })
    }
}

fn tray_grid() -> gtk::FlowBox {
    let grid = gtk::FlowBox::new();
    // Tray indicators are a single horizontal strip.  FlowBox's default
    // vertical orientation wraps after the configured children-per-line,
    // turning a compact tray into a tall panel as items accumulate.
    grid.set_orientation(gtk::Orientation::Horizontal);
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_max_children_per_line(i32::MAX as u32);
    grid.set_min_children_per_line(1);
    grid.set_row_spacing(4);
    grid.set_column_spacing(4);
    grid.set_hexpand(true);
    grid.set_vexpand(false);
    // The shared circle scroller intentionally suppresses natural-size
    // propagation.  Give FlowBox a real minimum so its viewport does not
    // collapse to 0x0 before the first allocation.
    grid.set_size_request(24, 24);
    grid.set_halign(Align::Fill);
    grid.set_valign(Align::Fill);
    let scroll = grid.clone();
    // FlowBox itself is the page; the host wraps it in a ScrolledWindow only
    // for expanded pages through this bounded policy in integration.
    scroll.set_overflow(Overflow::Hidden);
    grid
}

fn ensure_page_measurement(page: &gtk::FlowBox) {
    // CircleHost deliberately disables natural propagation on its shared
    // scroller.  Keep the tray page measurable so Stack does not allocate
    // its FlowBox viewport at 0x0 before the expanded frame arrives.
    if let Some(parent) = page.parent() {
        parent.set_size_request(1, 1);
        parent.set_hexpand(true);
        parent.set_vexpand(false);
        parent.set_halign(Align::Fill);
        parent.set_valign(Align::Fill);
        if let Some(scroll) = parent.downcast_ref::<gtk::ScrolledWindow>() {
            scroll.set_min_content_width(1);
            scroll.set_min_content_height(24);
        }
    }
    page.set_size_request(1, 24);
}

fn clear_children<W: IsA<gtk::Widget>>(widget: &W) {
    if let Some(overlay) = widget.as_ref().downcast_ref::<gtk::Overlay>() {
        overlay.set_child(None::<&gtk::Widget>);
        while let Some(child) = overlay.first_child() {
            overlay.remove_overlay(&child);
        }
        return;
    }
    if let Some(flow) = widget.as_ref().downcast_ref::<gtk::FlowBox>() {
        while let Some(child) = flow.first_child() {
            flow.remove(&child);
        }
        return;
    }
    // Do not retain sibling links across unparenting.  GTK containers are
    // allowed to update their child list synchronously, and a stale sibling
    // obtained before removal can become an invalid object during allocation.
    while let Some(current) = widget.first_child() {
        current.unparent();
    }
}

fn compact_preview_layout(scale: f64, total: usize) -> Vec<(i32, i32, i32)> {
    if total == 0 {
        return Vec::new();
    }
    let diameter = (32.0 * scale).round().max(16.0);
    let mut size = (6.0 * scale).round().clamp(4.0, 8.0);
    let diagonal = size * std::f64::consts::SQRT_2 / 2.0;
    let outer = diameter / 2.0 - diagonal - 1.0;
    let inner = (10.0 * scale) / 2.0 + diagonal + 1.0;
    if inner > outer {
        size = 4.0;
    }
    let diagonal = size * std::f64::consts::SQRT_2 / 2.0;
    let radius = (diameter / 2.0 - diagonal - 1.0).max(0.0);
    if radius < (10.0 * scale) / 2.0 + diagonal + 1.0 {
        return Vec::new();
    }
    (0..total)
        .map(|index| {
            let angle = std::f64::consts::TAU * index as f64 / total as f64;
            (
                (angle.cos() * radius).round() as i32,
                (angle.sin() * radius).round() as i32,
                size.round() as i32,
            )
        })
        .collect()
}

fn preview_count(total: usize, limit: usize) -> usize {
    total.min(limit)
}

#[cfg(test)]
mod tests {
    use super::{TrayCircle, compact_preview_layout, preview_count};
    use crate::config::{TrayCompactStyle, TrayConfig};
    use crate::state::{TrayIcon, TrayItem, TrayStatus};
    use gtk::prelude::*;

    #[test]
    fn radial_preview_stays_inside_scaled_circle() {
        for scale in [0.75, 1.0, 1.5, 2.0] {
            let diameter = (32.0_f64 * scale).round().max(16.0);
            for (x, y, size) in compact_preview_layout(scale, 4) {
                let distance = f64::from(x * x + y * y).sqrt();
                let diagonal = f64::from(size) * std::f64::consts::SQRT_2 / 2.0;
                assert!(distance + diagonal <= diameter / 2.0 + 1.0);
            }
        }
    }

    #[test]
    fn preview_limit_never_truncates_expanded_snapshot() {
        let total = 17;
        assert_eq!(preview_count(total, 0), 0);
        assert_eq!(preview_count(total, 4), 4);
        assert_eq!(total, 17);
    }

    #[test]
    #[ignore = "requires an isolated GTK display; run the tray circle GTK harness"]
    fn gtk_tray_pages_have_visible_allocated_children_for_mixed_icons() {
        gtk::init().expect("GTK display");
        let window = gtk::Window::new();
        window.set_default_size(435, 215);
        let config = TrayConfig {
            compact_style: TrayCompactStyle::CountWithIcons,
            max_compact_icons: 4,
            ..TrayConfig::default()
        };
        let circle = TrayCircle::synthetic(&config, 1.0);
        window.set_child(Some(circle.host.widget()));
        window.present();
        let items = vec![
            tray_item("named", TrayIcon::Name("application-x-executable".into())),
            tray_item("unknown", TrayIcon::Name("not-a-real-icon".into())),
            tray_item(
                "pixmap",
                TrayIcon::Pixmap {
                    width: 1,
                    height: 1,
                    argb: vec![255, 0, 200, 40],
                },
            ),
            tray_item("none", TrayIcon::None),
            tray_item("fifth", TrayIcon::Name("application-x-executable".into())),
        ];
        circle.update(&items);
        let count = circle
            .compact
            .first_child()
            .expect("compact count widget")
            .downcast::<gtk::Label>()
            .expect("compact count label");
        assert_eq!(count.label(), "5");
        let frame = super::super::circle::Frame {
            rect: super::super::circle::Rect {
                x: 0.0,
                y: 0.0,
                width: 435.0,
                height: 215.0,
            },
            radius: 28.0,
        };
        assert!(circle.host.render(circle.host.revision(), Some(frame)));
        assert!(circle.host.commit_page(circle.host.revision()));
        window.queue_resize();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        // Exercise the real FlowBox allocator independently of the shared
        // CircleSurface stack; the latter's zero allocation is reported as a
        // host integration dependency rather than hidden by this test.
        let children = |widget: &gtk::Widget| {
            let mut result = Vec::new();
            let mut child = widget.first_child();
            while let Some(current) = child {
                child = current.next_sibling();
                result.push(current);
            }
            result
        };
        circle
            .host
            .dispatch(super::super::circle::Event::Pointer(true));
        assert!(circle.host.render(circle.host.revision(), Some(frame)));
        assert!(circle.host.commit_page(circle.host.revision()));
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        circle.hover.allocate(435, 36, -1, None);
        assert_eq!(children(&circle.hover.clone().upcast()).len(), items.len());
        for child in children(&circle.hover.clone().upcast()) {
            assert!(child.is_visible() && child.is_mapped());
            assert!(child.width() > 0 && child.height() > 0);
        }
        circle.host.dispatch(super::super::circle::Event::OpenFull);
        assert!(circle.host.render(circle.host.revision(), Some(frame)));
        assert!(circle.host.commit_page(circle.host.revision()));
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        circle.full.allocate(435, 215, -1, None);
        assert_eq!(children(&circle.full.clone().upcast()).len(), items.len());
        for child in children(&circle.full.clone().upcast()) {
            assert!(child.is_visible() && child.is_mapped());
            assert!(child.width() > 0 && child.height() > 0);
        }
        for count in [1, 4, 9] {
            let sample = items
                .iter()
                .cloned()
                .chain((items.len()..count).map(|index| {
                    tray_item(
                        &format!("extra-{index}"),
                        TrayIcon::Name("application-x-executable".into()),
                    )
                }))
                .take(count)
                .collect::<Vec<_>>();
            circle.update(&sample);
            circle
                .host
                .dispatch(super::super::circle::Event::Pointer(true));
            circle.hover.allocate(435, 36, -1, None);
            while gtk::glib::MainContext::default().pending() {
                gtk::glib::MainContext::default().iteration(false);
            }
            let allocated = children(&circle.hover.clone().upcast());
            assert_eq!(allocated.len(), count);
            let row_y = allocated[0].allocation().y();
            assert!(
                allocated.iter().all(|child| {
                    child.width() > 0 && child.height() > 0 && child.allocation().y() == row_y
                }),
                "count={count}, allocations={:?}",
                allocated
                    .iter()
                    .map(|child| (
                        child.is_mapped(),
                        child.width(),
                        child.height(),
                        child.allocation().x(),
                        child.allocation().y()
                    ))
                    .collect::<Vec<_>>()
            );
        }
        assert!(circle.host.widget().width() > 0);
        window.close();
    }

    fn tray_item(id: &str, icon: TrayIcon) -> TrayItem {
        TrayItem {
            key: id.into(),
            service: "org.test".into(),
            object_path: format!("/{id}"),
            id: id.into(),
            title: id.into(),
            tooltip: Some(id.into()),
            icon,
            status: TrayStatus::Active,
            item_is_menu: false,
            menu_path: None,
        }
    }
}
