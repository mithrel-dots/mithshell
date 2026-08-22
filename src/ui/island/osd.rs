//! The volume/brightness/workspace OSD overlay.

use super::*;

use std::rc::Rc;

use gtk::{Align, Orientation, glib};

use super::{IslandWindow, Metrics};
use crate::ipc::OsdKind;
use crate::state::OsdState;

pub(super) fn osd_view(
    metrics: Metrics,
) -> (
    gtk::Box,
    gtk::Image,
    gtk::Label,
    gtk::ProgressBar,
    gtk::Label,
) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    root.set_size_request(metrics.osd_width, metrics.osd_height);
    root.add_css_class("osd-content");
    root.set_valign(Align::Start);
    let icon = gtk::Image::from_icon_name("audio-volume-high-symbolic");
    icon.add_css_class("osd-icon");
    icon.set_valign(Align::Center);
    let title = gtk::Label::new(Some("Volume"));
    title.add_css_class("osd-title");
    title.set_halign(Align::Start);
    title.set_valign(Align::Center);
    let progress = gtk::ProgressBar::new();
    progress.set_hexpand(true);
    progress.set_valign(Align::Center);
    progress.add_css_class("osd-progress");
    let value = gtk::Label::new(Some("0"));
    value.add_css_class("osd-value");
    value.set_xalign(1.0);
    value.set_valign(Align::Center);
    root.append(&icon);
    root.append(&title);
    root.append(&progress);
    root.append(&value);
    (root, icon, title, progress, value)
}

impl IslandWindow {
    pub fn show_osd(self: &Rc<Self>, state: OsdState) {
        let (icon, title) = match state.kind {
            OsdKind::Volume if state.muted => ("audio-volume-muted-symbolic", "Muted"),
            OsdKind::Volume if state.value == 0 => ("audio-volume-low-symbolic", "Volume"),
            OsdKind::Volume if state.value < 50 => ("audio-volume-medium-symbolic", "Volume"),
            OsdKind::Volume => ("audio-volume-high-symbolic", "Volume"),
            OsdKind::Brightness => ("display-brightness-symbolic", "Brightness"),
            OsdKind::Workspace => ("focus-windows-symbolic", "Workspace"),
        };
        self.osd_icon.set_icon_name(Some(icon));
        self.osd_title.set_label(title);
        self.osd_progress
            .set_fraction(f64::from(state.value) / 100.0);
        self.osd_value.set_label(&format!("{}", state.value));
        self.osd_active.set(true);
        self.reconcile_view();

        let generation = self.osd_generation.get().wrapping_add(1);
        self.osd_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(state.timeout_ms), move || {
            if let Some(island) = weak.upgrade()
                && island.osd_generation.get() == generation
            {
                island.osd_active.set(false);
                island.reconcile_view();
            }
        });
    }

    pub(super) fn clear_osd(&self) {
        self.osd_active.set(false);
        self.osd_generation
            .set(self.osd_generation.get().wrapping_add(1));
    }
}
