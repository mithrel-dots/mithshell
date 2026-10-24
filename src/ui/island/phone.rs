//! Shared phone rows inside the hardware panel, visible in Peek and Open.

use std::{cell::RefCell, rc::Rc, thread};

use gtk::{Orientation, prelude::*};

use super::{IslandWindow, Metrics, View, clear_box};
use crate::{
    state::{CellularSignal, PhoneState},
    ui::icon::{self, Icon},
};

pub(super) struct PhonePanel {
    pub(super) root: gtk::Box,
    rows: RefCell<Vec<PhoneRow>>,
    metrics: Metrics,
}

struct PhoneRow {
    id: String,
    root: gtk::Box,
    name: gtk::Label,
    status: gtk::Label,
    signal: SignalBars,
    ring: gtk::Button,
}

impl PhonePanel {
    pub(super) fn new(metrics: Metrics) -> Self {
        let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(8));
        root.set_visible(false);
        Self {
            root,
            rows: RefCell::new(Vec::new()),
            metrics,
        }
    }

    pub(super) fn update(&self, phones: &[PhoneState]) {
        let mut rows = self.rows.borrow_mut();
        if !rows
            .iter()
            .map(|row| &row.id)
            .eq(phones.iter().map(|phone| &phone.id))
        {
            clear_box(&self.root);
            *rows = phones
                .iter()
                .map(|phone| {
                    let row = PhoneRow::new(phone, self.metrics);
                    self.root.append(&row.root);
                    row
                })
                .collect();
        }
        for (row, phone) in rows.iter().zip(phones) {
            row.name.set_label(&phone.name);
            row.name.set_tooltip_text(Some(&phone.name));
            let status = if !phone.reachable {
                "Offline".into()
            } else {
                match (phone.percent, phone.charging) {
                    (Some(percent), true) => format!("{percent}% · Charging"),
                    (Some(percent), false) => format!("{percent}%"),
                    (None, _) => "Battery unavailable".into(),
                }
            };
            row.status.set_label(&status);
            row.status.set_tooltip_text(Some(&status));
            row.signal
                .update(phone.cellular.as_ref().filter(|_| phone.reachable));
            row.ring.set_sensitive(phone.reachable && phone.can_ring);
            row.ring.set_tooltip_text(Some(if !phone.reachable {
                "Phone is offline"
            } else if !phone.can_ring {
                "Enable Find My Phone in KDE Connect to ring this device"
            } else {
                "Ring this phone"
            }));
            if phone.reachable {
                row.root.remove_css_class("phone-offline");
            } else {
                row.root.add_css_class("phone-offline");
            }
        }
        self.root.set_visible(!phones.is_empty());
    }
}

impl PhoneRow {
    fn new(phone: &PhoneState, metrics: Metrics) -> Self {
        let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
        root.add_css_class("hardware-network");
        root.add_css_class("phone-row");
        let glyph = icon::icon_widget(Icon::Phone, metrics.icons);
        glyph.add_css_class("hardware-icon");
        root.append(&glyph);
        let name = gtk::Label::new(None);
        name.add_css_class("hardware-name");
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(18);
        let status = gtk::Label::new(None);
        status.add_css_class("hardware-value");
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let signal = SignalBars::new(metrics);
        let ring = gtk::Button::with_label("Ring");
        ring.add_css_class("phone-ring");
        ring.set_valign(gtk::Align::Center);
        let id = phone.id.clone();
        ring.connect_clicked(move |_| {
            let id = id.clone();
            thread::spawn(move || {
                if let Err(error) = crate::kdeconnect::ring(&id) {
                    log::warn!("failed to ring KDE Connect phone: {error:#}");
                }
            });
        });
        root.append(&name);
        root.append(&signal.root);
        root.append(&status);
        root.append(&ring);
        Self {
            id: phone.id.clone(),
            root,
            name,
            status,
            signal,
            ring,
        }
    }
}

struct SignalBars {
    root: gtk::Box,
    bars: [gtk::Box; 4],
}

impl SignalBars {
    fn new(metrics: Metrics) -> Self {
        let root = gtk::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(metrics.spacing(2))
            .valign(gtk::Align::Center)
            .accessible_role(gtk::AccessibleRole::Img)
            .build();
        root.add_css_class("phone-signal");
        root.set_visible(false);
        let bars = std::array::from_fn(|index| {
            let bar = gtk::Box::new(Orientation::Vertical, 0);
            bar.add_css_class("phone-signal-bar");
            bar.set_size_request(metrics.spacing(3), metrics.spacing(5 + index as i32 * 4));
            bar.set_valign(gtk::Align::End);
            root.append(&bar);
            bar
        });
        Self { root, bars }
    }

    fn update(&self, signal: Option<&CellularSignal>) {
        self.root.set_visible(signal.is_some());
        let Some(signal) = signal else { return };
        for (index, bar) in self.bars.iter().enumerate() {
            if (index as i32) < signal.strength {
                bar.add_css_class("active");
            } else {
                bar.remove_css_class("active");
            }
        }
        let description = if signal.strength < 0 {
            "Cellular: no service".into()
        } else {
            let network = signal
                .network_type
                .as_deref()
                .map(|network| format!(" · {network}"))
                .unwrap_or_default();
            format!("Cellular signal: {}/4{network}", signal.strength)
        };
        self.root.set_tooltip_text(Some(&description));
        self.root
            .update_property(&[gtk::accessible::Property::Label(&description)]);
    }
}

impl IslandWindow {
    pub fn update_phones(self: &Rc<Self>, phones: &[PhoneState]) {
        self.hardware.phones.update(phones);
        if self.current_view.get() == View::Dashboard {
            self.set_view(View::Dashboard);
        } else {
            self.reconcile_pill_geometry();
        }
    }
}
