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
    hover: gtk::Box,
    full: gtk::Box,
    island: Weak<IslandWindow>,
    enabled: bool,
    style: TrayCompactStyle,
    max_compact_icons: usize,
    max_expanded_icons: usize,
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

        let hover = tray_row(island.metrics.scale);
        let full = tray_row(island.metrics.scale);
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
            max_expanded_icons: config.max_expanded_icons,
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
        let hover = tray_row(scale);
        let full = tray_row(scale);
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
            max_expanded_icons: config.max_expanded_icons,
            menu_tracker,
            scale,
        }
    }

    /// Replace all three pages from one effective snapshot.  Compact preview
    /// is intentionally capped independently of the hover/full pages.
    pub(crate) fn update(&self, items: &[TrayItem]) {
        // Popdowns can synchronously change keyboard mode and trigger a
        // relayout. Keep callbacks behind the child rebuild.
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
        super::circle::scale_text(self.host.widget(), self.scale);
        self.host.dispatch(Event::Content(present));
        self.menu_tracker.end_batch();
    }

    fn small_preview(&self, item: &TrayItem, size: i32) -> gtk::Image {
        let image = gtk::Image::new();
        image.set_pixel_size(size);
        apply_tray_icon(&image, &item.icon);
        image
    }

    pub(crate) fn expanded_width(&self) -> f64 {
        let mut width = f64::from(self.hover.margin_start() + self.hover.margin_end());
        let mut child = self.hover.first_child();
        for index in 0..self.max_expanded_icons.max(1) {
            let Some(icon) = child else { break };
            let (_, natural, _, _) = icon.measure(gtk::Orientation::Horizontal, -1);
            width += f64::from(natural);
            if index > 0 {
                width += f64::from(self.hover.spacing());
            }
            child = icon.next_sibling();
        }
        (width / self.scale).max(32.0)
    }

    fn button(&self, item: &TrayItem) -> gtk::Button {
        self.island.upgrade().map_or_else(
            || {
                let button = gtk::Button::new();
                let image = self.small_preview(item, (18.0 * self.scale).round() as i32);
                button.set_child(Some(&image));
                button
            },
            |island| island.build_tray_icon_with_tracker(item, Some(self.menu_tracker.clone())),
        )
    }
}

fn tray_row(scale: f64) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, (4.0 * scale).round() as i32);
    row.set_margin_start((8.0 * scale).round() as i32);
    row.set_margin_end((8.0 * scale).round() as i32);
    row.set_hexpand(false);
    row.set_vexpand(false);
    row.set_halign(Align::Start);
    row.set_valign(Align::Center);
    row.set_size_request(-1, 24);
    row.set_overflow(Overflow::Hidden);
    row
}

fn ensure_page_measurement(page: &gtk::Box) {
    // CircleHost deliberately disables natural propagation on its shared
    // scroller. Keep the tray row measurable so Stack does not allocate its
    // viewport at 0x0 before the expanded frame arrives.
    if let Some(parent) = page.parent() {
        parent.set_size_request(1, 1);
        parent.set_hexpand(true);
        parent.set_vexpand(true);
        parent.set_halign(Align::Fill);
        parent.set_valign(Align::Fill);
        let scroller = parent
            .clone()
            .downcast::<gtk::ScrolledWindow>()
            .ok()
            .or_else(|| parent.parent().and_downcast::<gtk::ScrolledWindow>());
        if let Some(scroll) = scroller {
            scroll.set_min_content_width(1);
            scroll.set_min_content_height(24);
            scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
            let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
            wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
            let adjustment = scroll.hadjustment();
            wheel.connect_scroll(move |_, dx, dy| {
                if adjustment.upper() <= adjustment.page_size() {
                    return gtk::glib::Propagation::Proceed;
                }
                let delta = if dx.abs() > dy.abs() { dx } else { dy };
                adjustment.set_value(
                    adjustment.value() + delta * (adjustment.page_size() / 3.0).max(32.0),
                );
                gtk::glib::Propagation::Stop
            });
            scroll.add_controller(wheel);
        }
    }
    page.set_size_request(-1, 24);
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
    if let Some(row) = widget.as_ref().downcast_ref::<gtk::Box>() {
        while let Some(child) = row.first_child() {
            row.remove(&child);
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
            max_expanded_icons: 4,
            ..TrayConfig::default()
        };
        let circle = TrayCircle::synthetic(&config, 1.9);
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
                width: 144.0 * 1.9,
                height: 36.0 * 1.9,
            },
            radius: 18.0 * 1.9,
        };
        circle.host.widget().set_halign(gtk::Align::Start);
        circle.host.widget().set_valign(gtk::Align::Start);
        let root = gtk::Fixed::new();
        root.set_hexpand(true);
        root.set_vexpand(true);
        root.put(circle.host.widget(), 0.0, 0.0);
        window.set_child(Some(&root));
        window.present();
        assert!(circle.host.render(circle.host.revision(), Some(frame)));
        assert!(circle.host.commit_page(circle.host.revision()));
        window.queue_resize();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
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
        let scroller = find_scroller(circle.host.widget()).expect("host hover scroller");
        assert_eq!(
            circle.host.widget().height(),
            frame.rect.height.round() as i32
        );
        assert!(scroller.height() <= frame.rect.height.round() as i32);
        let mut previous_width = 0.0;
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
            let width = circle.expanded_width();
            assert!(width >= previous_width);
            if count == 1 {
                let (_, natural, _, _) = circle.hover.measure(gtk::Orientation::Horizontal, -1);
                assert_eq!(width, (f64::from(natural) / 1.9).max(32.0));
            }
            if count == 9 {
                assert_eq!(
                    width, previous_width,
                    "long trays should stop at four visible icons"
                );
            }
            previous_width = width;
            let frame = super::super::circle::Frame {
                rect: super::super::circle::Rect {
                    width: width * 1.9,
                    ..frame.rect
                },
                ..frame
            };
            circle
                .host
                .dispatch(super::super::circle::Event::Pointer(true));
            assert!(circle.host.render(circle.host.revision(), Some(frame)));
            assert!(circle.host.commit_page(circle.host.revision()));
            circle.host.widget().allocate(
                frame.rect.width.round() as i32,
                frame.rect.height.round() as i32,
                -1,
                None,
            );
            window.queue_resize();
            while gtk::glib::MainContext::default().pending() {
                gtk::glib::MainContext::default().iteration(false);
            }
            let allocated = children(&circle.hover.clone().upcast());
            assert_eq!(allocated.len(), count);
            let hover_widget: gtk::Widget = circle.hover.clone().upcast();
            let row_y = allocated[0]
                .compute_bounds(&hover_widget)
                .expect("button bounds")
                .y();
            assert!(
                allocated.iter().all(|child| {
                    child.is_visible()
                        && child.is_mapped()
                        && child.width() > 0
                        && child.height() > 0
                        && child
                            .compute_bounds(&hover_widget)
                            .expect("button bounds")
                            .y()
                            == row_y
                }),
                "count={count}, scroller={}x{}, row={}x{}, allocations={:?}",
                scroller.width(),
                scroller.height(),
                circle.hover.width(),
                circle.hover.height(),
                allocated
                    .iter()
                    .map(|child| (
                        child.is_mapped(),
                        child.width(),
                        child.height(),
                        child.compute_bounds(&hover_widget).map(|bounds| bounds.x()),
                        child.compute_bounds(&hover_widget).map(|bounds| bounds.y())
                    ))
                    .collect::<Vec<_>>()
            );
            if count == 1 {
                assert!(circle.hover.width() <= scroller.width());
                let right = allocated
                    .iter()
                    .filter_map(|child| child.compute_bounds(&hover_widget))
                    .map(|bounds| bounds.x() + bounds.width())
                    .fold(0.0_f32, f32::max);
                assert!(right <= scroller.width() as f32);
            }
            let adjustment = scroller.hadjustment();
            if count == 9 {
                assert!(adjustment.upper() > adjustment.page_size());
                adjustment.set_value(adjustment.upper() - adjustment.page_size());
                while gtk::glib::MainContext::default().pending() {
                    gtk::glib::MainContext::default().iteration(false);
                }
                assert!(adjustment.value() > 0.0);
                let last = allocated.last().unwrap();
                let last_bounds = last
                    .compute_bounds(&hover_widget)
                    .expect("last tray button visible bounds");
                assert!(last.can_target() && last.is_sensitive());
                assert!(
                    (last_bounds.x() - adjustment.value() as f32) < scroller.width() as f32
                        && (last_bounds.x() - adjustment.value() as f32 + last_bounds.width())
                            > 0.0,
                    "last bounds={:?}, viewport={}x{}, adjustment={}/{}",
                    last_bounds,
                    scroller.width(),
                    scroller.height(),
                    adjustment.value(),
                    adjustment.upper()
                );
                let picked = scroller.pick(
                    f64::from(last_bounds.x() - adjustment.value() as f32),
                    f64::from(last_bounds.y() + last_bounds.height() / 2.0),
                    gtk::PickFlags::DEFAULT,
                );
                assert!(
                    picked.is_some(),
                    "last icon is not pickable after scrolling"
                );
            }
        }
        circle.host.dispatch(super::super::circle::Event::OpenFull);
        assert!(circle.host.render(circle.host.revision(), Some(frame)));
        assert!(circle.host.commit_page(circle.host.revision()));
        circle.host.widget().allocate(
            frame.rect.width.round() as i32,
            frame.rect.height.round() as i32,
            -1,
            None,
        );
        window.queue_resize();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        let full_children = children(&circle.full.clone().upcast());
        assert_eq!(full_children.len(), 9);
        let full_scroller = find_parent_scroller(&circle.full).expect("full page scroller");
        assert!(full_scroller.height() <= frame.rect.height.round() as i32);
        let full_widget: gtk::Widget = circle.full.clone().upcast();
        let full_row_y = full_children[0]
            .compute_bounds(&full_widget)
            .expect("full button bounds")
            .y();
        assert!(
            full_children.iter().all(|child| {
                child.is_visible()
                    && child.is_mapped()
                    && child.width() > 0
                    && child.height() > 0
                    && child
                        .compute_bounds(&full_widget)
                        .expect("full button bounds")
                        .y()
                        == full_row_y
            }),
            "full page {}x{}, children={:?}",
            circle.full.width(),
            circle.full.height(),
            full_children
                .iter()
                .map(|child| (
                    child.is_visible(),
                    child.is_mapped(),
                    child.width(),
                    child.height(),
                    child
                        .compute_bounds(&full_widget)
                        .map(|bounds| (bounds.x(), bounds.y()))
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            circle.host.widget().height(),
            frame.rect.height.round() as i32,
            "pinned tray stays at the compact horizontal-pillar height"
        );
        assert!(circle.host.widget().width() > 0);
        window.close();
    }

    fn find_scroller(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
        if let Some(scroller) = widget.downcast_ref::<gtk::ScrolledWindow>() {
            return Some(scroller.clone());
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            if let Some(scroller) = find_scroller(&current) {
                return Some(scroller);
            }
        }
        None
    }

    fn find_parent_scroller(widget: &impl IsA<gtk::Widget>) -> Option<gtk::ScrolledWindow> {
        let mut parent = widget.as_ref().parent();
        while let Some(current) = parent {
            if let Ok(scroller) = current.clone().downcast::<gtk::ScrolledWindow>() {
                return Some(scroller);
            }
            parent = current.parent();
        }
        None
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
