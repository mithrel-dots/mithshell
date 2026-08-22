//! The hover-revealed tray row: icon buttons, DBusMenu popovers, and the
//! click/scroll routing back to `crate::tray`.

use super::*;

use std::rc::Rc;

use gtk::{
    Align, EventControllerScroll, EventControllerScrollFlags, GestureClick, Orientation, gdk, glib,
};

use super::IslandWindow;
use crate::state::{TrayIcon, TrayItem, TrayMenuItem, TrayStatus};

fn apply_tray_icon(image: &gtk::Image, icon: &TrayIcon) {
    const FALLBACK: &str = "application-x-executable-symbolic";
    match icon {
        TrayIcon::Name(name) => image.set_icon_name(Some(name)),
        TrayIcon::Pixmap {
            width,
            height,
            argb,
        } => match tray_texture_from_pixmap(*width, *height, argb) {
            Some(texture) => image.set_paintable(Some(&texture)),
            None => image.set_icon_name(Some(FALLBACK)),
        },
        TrayIcon::None => image.set_icon_name(Some(FALLBACK)),
    }
}

/// Converts a `TrayIcon::Pixmap`'s raw bytes (32-bit ARGB, network/big-endian
/// byte order, i.e. each pixel is `[A, R, G, B]`) into a paintable.
fn tray_texture_from_pixmap(width: i32, height: i32, argb: &[u8]) -> Option<gdk::Texture> {
    if width <= 0 || height <= 0 || argb.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let mut rgba = vec![0_u8; argb.len()];
    for (pixel_in, pixel_out) in argb.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        pixel_out.copy_from_slice(&[pixel_in[1], pixel_in[2], pixel_in[3], pixel_in[0]]);
    }
    let bytes = glib::Bytes::from_owned(rgba);
    let texture = gdk::MemoryTexture::new(
        width,
        height,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        (width * 4) as usize,
    );
    Some(texture.upcast())
}

impl IslandWindow {
    /// Rebuilds the tray row from a fresh snapshot, the same
    /// clear-and-rebuild approach `update_hyprland` uses for workspace
    /// dots -- tray churn is rare enough that reusing widgets isn't worth
    /// the bookkeeping.
    pub fn update_tray(self: &Rc<Self>, items: &[TrayItem]) {
        clear_box(&self.compact_tray);
        clear_box(&self.media_tray);
        for item in items {
            // A widget can only have one parent, so each pill gets its own
            // freshly built icon -- the same duplication `update_hyprland`
            // already does for `compact_workspaces`/`media_workspaces`.
            self.compact_tray.append(&self.build_tray_icon(item));
            self.media_tray.append(&self.build_tray_icon(item));
        }
        self.tray_item_count.set(items.len());
        self.resize_compact();
        self.resize_media();
    }

    fn build_tray_icon(self: &Rc<Self>, item: &TrayItem) -> gtk::Button {
        let button = gtk::Button::new();
        button.add_css_class("tray-icon");
        button.set_has_frame(false);
        if item.status == TrayStatus::NeedsAttention {
            button.add_css_class("needs-attention");
        }
        if let Some(tooltip) = item.tooltip.as_deref().filter(|text| !text.is_empty()) {
            button.set_tooltip_text(Some(tooltip));
        }

        let image = gtk::Image::new();
        image.set_pixel_size(self.metrics.tray_icon_size);
        apply_tray_icon(&image, &item.icon);
        button.set_child(Some(&image));

        // Primary click goes through `GtkButton`'s own `clicked` signal
        // rather than an extra `GestureClick`: the button already has an
        // internal click gesture that claims the primary-button sequence,
        // so a second gesture watching the same button loses the claim and
        // never fires. Middle/secondary are free for gestures below, but
        // each is restricted to exactly the button it handles -- a
        // catch-all `button(0)` gesture competes with that same internal
        // one (which is itself "any button", it just only *emits* for the
        // primary) and can swallow those clicks too.
        let weak = Rc::downgrade(self);
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        let primary_menu_path = item.menu_path.clone();
        // Items advertising `ItemIsMenu` declare they have no meaningful
        // activation at all and expect their menu on a plain left click.
        let item_is_menu = item.item_is_menu;
        button.connect_clicked(move |button| {
            let Some(island) = weak.upgrade() else {
                return;
            };
            match primary_menu_path.clone().filter(|_| item_is_menu) {
                Some(menu_path) => {
                    island.open_tray_menu(button.clone(), service.clone(), menu_path);
                }
                None => {
                    // The spec's x/y are screen coordinates used by items
                    // that position their own menu; there is no way to get
                    // those for a Wayland client, and items treat 0,0 as
                    // "unspecified".
                    (island.actions.tray_activate)(service.clone(), object_path.clone(), 0, 0);
                }
            }
        });

        let middle_click = GestureClick::new();
        middle_click.set_button(gdk::BUTTON_MIDDLE);
        let weak = Rc::downgrade(self);
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        middle_click.connect_released(move |_, _, _, _| {
            if let Some(island) = weak.upgrade() {
                (island.actions.tray_secondary_activate)(
                    service.clone(),
                    object_path.clone(),
                    0,
                    0,
                );
            }
        });
        button.add_controller(middle_click);

        let context_click = GestureClick::new();
        context_click.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(self);
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        let menu_path = item.menu_path.clone();
        let button_weak = button.downgrade();
        context_click.connect_pressed(move |gesture, _, _, _| {
            let Some(island) = weak.upgrade() else {
                return;
            };
            // Claim the sequence so the press can't also bubble up to the
            // pill's own click gesture, which would toggle the dashboard.
            gesture.set_state(gtk::EventSequenceState::Claimed);
            match (menu_path.clone(), button_weak.upgrade()) {
                (Some(menu_path), Some(button)) => {
                    island.open_tray_menu(button, service.clone(), menu_path);
                }
                _ => {
                    (island.actions.tray_context_menu)(service.clone(), object_path.clone(), 0, 0);
                }
            }
        });
        button.add_controller(context_click);

        let scroll = EventControllerScroll::new(EventControllerScrollFlags::BOTH_AXES);
        let action = self.actions.tray_scroll.clone();
        let service = item.service.clone();
        let object_path = item.object_path.clone();
        scroll.connect_scroll(move |_, dx, dy| {
            let (delta, horizontal) = if dx.abs() > dy.abs() {
                (dx, true)
            } else {
                (dy, false)
            };
            if delta != 0.0 {
                action(
                    service.clone(),
                    object_path.clone(),
                    (delta * 10.0).round() as i32,
                    horizontal,
                );
            }
            glib::Propagation::Proceed
        });
        button.add_controller(scroll);

        button
    }

    /// Fetches a right-clicked item's DBusMenu layout on a throwaway
    /// thread (mirroring how `preview`/`theme` results are round-tripped
    /// back onto the GTK thread elsewhere) and shows it as a popover
    /// anchored to the icon that was clicked.
    fn open_tray_menu(self: &Rc<Self>, anchor: gtk::Button, service: String, menu_path: String) {
        let (sender, receiver) = async_channel::bounded(1);
        let fetch_service = service.clone();
        let fetch_menu_path = menu_path.clone();
        thread::spawn(move || {
            let result = crate::tray::menu_layout(&fetch_service, &fetch_menu_path);
            let _ = sender.send_blocking(result.map_err(|error| error.to_string()));
        });
        let island = self.clone();
        glib::MainContext::default().spawn_local(async move {
            if let Ok(Ok(menu)) = receiver.recv().await {
                island.show_tray_menu(&anchor, &service, &menu_path, &menu);
            }
        });
    }

    fn show_tray_menu(
        self: &Rc<Self>,
        anchor: &gtk::Button,
        service: &str,
        menu_path: &str,
        menu: &TrayMenuItem,
    ) {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        popover.add_css_class("tray-menu");
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_has_arrow(false);
        let content = self.build_tray_menu_box(&popover, service, menu_path, &menu.children);
        popover.set_child(Some(&content));

        // Pin the tray open for as long as the menu is: popping up takes a
        // pointer grab, so the pill immediately sees a `leave` and would
        // otherwise collapse the row this popover is anchored to.
        self.tray_menu_open.set(true);
        // An autohide popover needs to be able to take focus to grab, which
        // a `KeyboardMode::None` layer surface never can; without this the
        // menu is dismissed the moment it appears.
        self.refresh_keyboard_mode();
        self.resize_compact();
        self.resize_media();

        let weak = Rc::downgrade(self);
        popover.connect_closed(move |popover| {
            popover.unparent();
            if let Some(island) = weak.upgrade() {
                island.tray_menu_open.set(false);
                island.refresh_keyboard_mode();
                island.resize_compact();
                island.resize_media();
            }
        });
        popover.popup();
    }

    fn build_tray_menu_box(
        self: &Rc<Self>,
        popover: &gtk::Popover,
        service: &str,
        menu_path: &str,
        items: &[TrayMenuItem],
    ) -> gtk::Box {
        let list = gtk::Box::new(Orientation::Vertical, 0);
        list.add_css_class("tray-menu-list");
        for entry in items {
            if !entry.visible {
                continue;
            }
            if entry.separator {
                list.append(&gtk::Separator::new(Orientation::Horizontal));
                continue;
            }
            if entry.children.is_empty() {
                // An explicit label child rather than `Button::with_label`:
                // a button centers its child, and menu entries read far
                // better left-aligned against a ragged-width list.
                let label = gtk::Label::new(Some(&entry.label));
                label.set_halign(Align::Start);
                label.set_xalign(0.0);
                let button = gtk::Button::new();
                button.set_child(Some(&label));
                button.add_css_class("tray-menu-item");
                button.set_has_frame(false);
                button.set_sensitive(entry.enabled);
                if entry.checked == Some(true) {
                    button.add_css_class("checked");
                }
                let action = self.actions.tray_menu_event.clone();
                let service = service.to_owned();
                let menu_path = menu_path.to_owned();
                let id = entry.id;
                let popover_weak = popover.downgrade();
                button.connect_clicked(move |_| {
                    action(service.clone(), menu_path.clone(), id);
                    if let Some(popover) = popover_weak.upgrade() {
                        popover.popdown();
                    }
                });
                list.append(&button);
            } else {
                let expander = gtk::Expander::new(Some(&entry.label));
                expander.add_css_class("tray-menu-submenu");
                let submenu =
                    self.build_tray_menu_box(popover, service, menu_path, &entry.children);
                expander.set_child(Some(&submenu));
                list.append(&expander);
            }
        }
        list
    }
}
