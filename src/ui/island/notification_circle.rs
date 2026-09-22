//! Notification history content for the optional island circle.
//!
//! This module owns widgets only.  The notification controller remains the
//! source of truth: callers replace the retained history with [`update`], and
//! the callbacks perform the existing dismiss/action/clear operations.

#![allow(dead_code)]

use std::{cell::Cell, rc::Rc};

use gtk::{Align, GestureClick, Orientation, prelude::*};

use crate::{
    config::{IconStyle, NotificationConfig},
    state::Notification,
    ui::icon::{self, Icon},
};

use super::circle::{CircleContent, CircleHost, Event};

pub(crate) struct NotificationCircleCallbacks {
    pub invoke: Rc<dyn Fn(u32, String)>,
    pub dismiss: Rc<dyn Fn(u32)>,
    pub clear: Rc<dyn Fn()>,
    pub inhibit: Rc<dyn Fn(bool)>,
    /// Called by the explicit compact/preview affordance.  Row controls do
    /// not call this, so activating an action can never also open full view.
    pub open_full: Rc<dyn Fn()>,
}

pub(crate) struct NotificationCircle {
    pub(crate) host: Rc<CircleHost>,
    compact: gtk::Box,
    compact_count: gtk::Label,
    hover: gtk::Box,
    hover_list: gtk::Box,
    full: gtk::Box,
    full_list: gtk::Box,
    hover_count: usize,
    style: IconStyle,
    callbacks: NotificationCircleCallbacks,
    inhibit: gtk::ToggleButton,
    inhibit_remaining: gtk::Label,
    inhibit_updating: Rc<Cell<bool>>,
}

impl NotificationCircle {
    /// Builds three separate pages as required by `CircleHost`; no page is
    /// reused or parented elsewhere.
    pub(crate) fn new(
        config: &NotificationConfig,
        style: IconStyle,
        history: &[Notification],
        callbacks: NotificationCircleCallbacks,
    ) -> Result<Rc<Self>, &'static str> {
        let compact = gtk::Box::new(Orientation::Horizontal, 6);
        compact.add_css_class("notification-circle-compact");
        compact.set_halign(Align::Center);
        let bell = icon::icon_widget(Icon::Bell, style);
        compact.append(&bell);
        let count = gtk::Label::new(Some("0"));
        count.add_css_class("notification-circle-count");
        compact.append(&count);
        let open_callback = callbacks.open_full.clone();
        let click = GestureClick::new();
        click.connect_released(move |_, _, _, _| open_callback());
        compact.add_controller(click);

        let hover = gtk::Box::new(Orientation::Vertical, 6);
        hover.add_css_class("notification-circle-hover");
        let hover_header = gtk::Box::new(Orientation::Horizontal, 6);
        let hover_title = gtk::Label::new(Some("Notifications"));
        hover_title.set_hexpand(true);
        hover_title.set_xalign(0.0);
        hover_header.append(&hover_title);
        let hover_open = gtk::Button::with_label("View all");
        hover_open.add_css_class("notification-circle-open");
        let open_callback = callbacks.open_full.clone();
        hover_open.connect_clicked(move |_| open_callback());
        hover_header.append(&hover_open);
        hover.append(&hover_header);
        let hover_list = gtk::Box::new(Orientation::Vertical, 4);
        hover.append(&hover_list);

        let full = gtk::Box::new(Orientation::Vertical, 6);
        full.add_css_class("notification-circle-full");
        let full_header = gtk::Box::new(Orientation::Horizontal, 6);
        let full_title = gtk::Label::new(Some("Notification history"));
        full_title.set_hexpand(true);
        full_title.set_xalign(0.0);
        full_header.append(&full_title);
        let clear = gtk::Button::with_label("Clear all");
        clear.add_css_class("notification-circle-clear");
        let clear_callback = callbacks.clear.clone();
        clear.connect_clicked(move |_| clear_callback());
        full_header.append(&clear);
        let inhibit = gtk::ToggleButton::with_label("Inhibit");
        inhibit.add_css_class("notification-circle-inhibit");
        let inhibit_callback = callbacks.inhibit.clone();
        let inhibit_updating = Rc::new(Cell::new(false));
        let inhibit_guard = inhibit_updating.clone();
        inhibit.connect_toggled(move |button| {
            if !inhibit_guard.get() {
                inhibit_callback(button.is_active());
            }
        });
        full_header.append(&inhibit);
        let remaining = gtk::Label::new(None);
        remaining.add_css_class("notification-inhibit-remaining");
        remaining.set_visible(false);
        full_header.append(&remaining);
        full.append(&full_header);
        let full_list = gtk::Box::new(Orientation::Vertical, 4);
        full.append(&full_list);

        let content = CircleContent {
            compact: compact.clone().upcast(),
            hover: hover.clone().upcast(),
            full: Some(full.clone().upcast()),
        };
        let host = CircleHost::new(content)?;
        let circle = Rc::new(Self {
            host,
            compact,
            compact_count: count,
            hover,
            hover_list,
            full,
            full_list,
            hover_count: config.hover_preview_count,
            style,
            callbacks,
            inhibit,
            inhibit_remaining: remaining,
            inhibit_updating,
        });
        circle.update(history);
        Ok(circle)
    }

    pub(crate) fn update(&self, history: &[Notification]) {
        // The count is retained history, not unread state.
        self.compact_count.set_label(&history.len().to_string());
        rebuild(
            &self.hover_list,
            history.iter().take(self.hover_count),
            &self.callbacks,
            self.style,
            true,
        );
        rebuild(
            &self.full_list,
            history.iter(),
            &self.callbacks,
            self.style,
            false,
        );
        self.host.dispatch(Event::Content(!history.is_empty()));
    }

    pub(crate) fn update_inhibition(&self, active: bool, remaining: Option<&str>) {
        self.inhibit_updating.set(true);
        self.inhibit.set_active(active);
        self.inhibit_updating.set(false);
        self.inhibit_remaining
            .set_label(remaining.unwrap_or_default());
        self.inhibit_remaining.set_visible(remaining.is_some());
    }
}

fn rebuild<'a>(
    page: &gtk::Box,
    history: impl Iterator<Item = &'a Notification>,
    callbacks: &NotificationCircleCallbacks,
    style: IconStyle,
    preview: bool,
) {
    while let Some(child) = page.last_child() {
        page.remove(&child);
    }
    let mut any = false;
    for notification in history {
        any = true;
        page.append(&row(notification, callbacks, style));
    }
    if !any {
        let empty = gtk::Label::new(Some(if preview {
            "No recent notifications"
        } else {
            "No notifications yet"
        }));
        empty.add_css_class("notification-empty");
        empty.set_halign(Align::Start);
        page.append(&empty);
    }
}

fn row(
    notification: &Notification,
    callbacks: &NotificationCircleCallbacks,
    style: IconStyle,
) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 8);
    row.add_css_class("notification-row");
    if notification.urgency == crate::state::Urgency::Critical {
        row.add_css_class("urgency-critical");
    }
    let image = icon::foreign_image(notification.app_icon.as_deref(), Icon::Bell);
    image.set_valign(Align::Start);
    row.append(&image);
    let text = gtk::Box::new(Orientation::Vertical, 2);
    text.set_hexpand(true);
    let summary = gtk::Label::new(Some(&notification.summary));
    summary.set_xalign(0.0);
    summary.set_wrap(true);
    summary.add_css_class("notification-row-summary");
    text.append(&summary);
    if !notification.body.is_empty() {
        let body = gtk::Label::new(Some(&notification.body));
        body.set_xalign(0.0);
        body.set_wrap(true);
        body.set_lines(3);
        body.add_css_class("notification-row-body");
        text.append(&body);
    }
    if let Some(action) = notification.default_action() {
        let button = gtk::Button::with_label(if action.label.is_empty() {
            "Open"
        } else {
            &action.label
        });
        let invoke = callbacks.invoke.clone();
        let id = notification.id;
        let key = action.key.clone();
        button.connect_clicked(move |_| invoke(id, key.clone()));
        text.append(&button);
    }
    for action in notification.actions.iter().filter(|a| a.key != "default") {
        let button = gtk::Button::with_label(&action.label);
        let invoke = callbacks.invoke.clone();
        let id = notification.id;
        let key = action.key.clone();
        button.connect_clicked(move |_| invoke(id, key.clone()));
        text.append(&button);
    }
    row.append(&text);
    let dismiss = icon::icon_button(Icon::Close, style);
    dismiss.set_tooltip_text(Some("Dismiss notification"));
    let callback = callbacks.dismiss.clone();
    let id = notification.id;
    dismiss.connect_clicked(move |_| callback(id));
    row.append(&dismiss);
    row
}
