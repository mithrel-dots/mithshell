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
    /// reused or parented elsewhere.  The host starts absent.  Integration
    /// must install `host.set_on_change(...)` before its first `update`, which
    /// dispatches `Content` and requests the initial layout.
    pub(crate) fn new(
        config: &NotificationConfig,
        style: IconStyle,
        callbacks: NotificationCircleCallbacks,
    ) -> Result<Rc<Self>, &'static str> {
        let compact = gtk::Box::new(Orientation::Horizontal, 6);
        compact.add_css_class("notification-content");
        compact.set_halign(Align::Center);
        compact.update_property(&[gtk::accessible::Property::Label(
            "Open notification history",
        )]);
        let bell = icon::icon_widget(Icon::Bell, style);
        compact.append(&bell);
        let count = gtk::Label::new(Some("0"));
        count.add_css_class("notification-count");
        compact.append(&count);
        let open_callback = callbacks.open_full.clone();
        let click = GestureClick::new();
        click.connect_released(move |_, _, _, _| open_callback());
        compact.add_controller(click);

        let hover = gtk::Box::new(Orientation::Vertical, 6);
        let hover_header = gtk::Box::new(Orientation::Horizontal, 6);
        let hover_title = gtk::Label::new(Some("Notifications"));
        hover_title.set_hexpand(true);
        hover_title.set_xalign(0.0);
        hover_header.append(&hover_title);
        let hover_open = gtk::Button::with_label("View all");
        hover_open.add_css_class("close-button");
        let open_callback = callbacks.open_full.clone();
        hover_open.connect_clicked(move |_| open_callback());
        hover_header.append(&hover_open);
        hover.append(&hover_header);
        let hover_list = gtk::Box::new(Orientation::Vertical, 4);
        hover.append(&hover_list);

        let full = gtk::Box::new(Orientation::Vertical, 6);
        let full_header = gtk::Box::new(Orientation::Horizontal, 6);
        let full_title = gtk::Label::new(Some("Notification history"));
        full_title.set_hexpand(true);
        full_title.set_xalign(0.0);
        full_header.append(&full_title);
        let clear = gtk::Button::with_label("Clear all");
        clear.add_css_class("close-button");
        let clear_callback = callbacks.clear.clone();
        clear.connect_clicked(move |_| clear_callback());
        full_header.append(&clear);
        let inhibit = gtk::ToggleButton::with_label("Inhibit");
        inhibit.add_css_class("close-button");
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
        Ok(circle)
    }

    pub(crate) fn update(&self, history: &[Notification]) {
        // The count is retained history, not unread state.
        self.compact_count.set_label(&history.len().to_string());
        rebuild(
            &self.hover_list,
            preview_history(history, self.hover_count).into_iter(),
            &self.callbacks,
            self.style,
            true,
        );
        rebuild(
            &self.full_list,
            full_history(history).into_iter(),
            &self.callbacks,
            self.style,
            false,
        );
        self.host.dispatch(Event::Content(has_content(history)));
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
        page.append(&row(notification, callbacks, style, preview));
    }
    if !any && !preview {
        let empty = gtk::Label::new(Some("No notifications yet"));
        empty.add_css_class("notification-empty");
        empty.set_halign(Align::Start);
        page.append(&empty);
    }
}

fn row(
    notification: &Notification,
    callbacks: &NotificationCircleCallbacks,
    style: IconStyle,
    preview: bool,
) -> gtk::Box {
    let row = gtk::Box::new(Orientation::Horizontal, 8);
    row.add_css_class("notification-row");
    if notification.urgency == crate::state::Urgency::Critical {
        row.add_css_class("urgency-critical");
    }
    let image = icon::foreign_image(notification.app_icon.as_deref(), Icon::Bell);
    image.add_css_class("notification-row-icon");
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
        if let Some(lines) = body_line_limit(preview) {
            body.set_lines(lines);
        }
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
        let target = action_target(notification.id, &action.key);
        button.connect_clicked(move |_| invoke(target.0, target.1.clone()));
        text.append(&button);
    }
    for action in notification.actions.iter().filter(|a| a.key != "default") {
        let button = gtk::Button::with_label(&action.label);
        let invoke = callbacks.invoke.clone();
        let target = action_target(notification.id, &action.key);
        button.connect_clicked(move |_| invoke(target.0, target.1.clone()));
        text.append(&button);
    }
    row.append(&text);
    let dismiss = icon::icon_button(Icon::Close, style);
    dismiss.add_css_class("notification-row-dismiss");
    dismiss.set_tooltip_text(Some("Dismiss notification"));
    let callback = callbacks.dismiss.clone();
    let id = notification.id;
    dismiss.connect_clicked(move |_| callback(id));
    row.append(&dismiss);
    row
}

fn preview_history(history: &[Notification], limit: usize) -> Vec<&Notification> {
    history.iter().take(limit).collect()
}

fn action_target(id: u32, key: &str) -> (u32, String) {
    (id, key.to_owned())
}

fn full_history(history: &[Notification]) -> Vec<&Notification> {
    history.iter().collect()
}

fn has_content(history: &[Notification]) -> bool {
    !history.is_empty()
}

fn body_line_limit(preview: bool) -> Option<i32> {
    preview.then_some(3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{NotificationAction, NotificationTimeout, Urgency};

    fn notification(id: u32, body: &str) -> Notification {
        Notification {
            id,
            app_name: format!("app-{id}"),
            app_icon: None,
            summary: format!("summary-{id}"),
            body: body.to_owned(),
            urgency: Urgency::Normal,
            actions: vec![NotificationAction {
                key: format!("action-{id}"),
                label: "Open".to_owned(),
            }],
            timeout: NotificationTimeout::Default,
        }
    }

    #[test]
    fn preview_is_most_recent_first_and_respects_zero_and_nonzero_caps() {
        let history = vec![
            notification(3, "new"),
            notification(2, "middle"),
            notification(1, "old"),
        ];
        assert!(preview_history(&history, 0).is_empty());
        let ids: Vec<_> = preview_history(&history, 2)
            .into_iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(ids, [3, 2]);
        assert_eq!(preview_history(&history, 99).len(), 3);
    }

    #[test]
    fn full_history_has_no_body_line_cap_while_preview_is_bounded() {
        assert_eq!(body_line_limit(true), Some(3));
        assert_eq!(body_line_limit(false), None);
        let history = vec![
            notification(3, "new"),
            notification(2, "middle"),
            notification(1, "old"),
        ];
        assert_eq!(full_history(&history).len(), history.len());
    }

    #[test]
    fn action_targets_keep_current_notification_ids_after_rebuild_inputs_change() {
        let first = notification(7, "first");
        let second = notification(42, "replacement");
        let first_action = first.actions[0].key.clone();
        let second_action = second.actions[0].key.clone();
        assert_eq!(
            action_target(first.id, &first_action),
            (7, "action-7".to_owned())
        );
        assert_eq!(
            action_target(second.id, &second_action),
            (42, "action-42".to_owned())
        );
    }

    #[test]
    fn empty_preview_has_only_the_existing_header_affordance() {
        // A zero preview count is intentionally content-free; the hover
        // header's View all button remains the only affordance.
        assert!(preview_history(&[notification(1, "x")], 0).is_empty());
        assert!(!has_content(&[]));
    }

    /// Requires an isolated GTK display.  This exercises the production host
    /// ownership guard rather than a duplicate test-only widget model.
    #[test]
    #[ignore = "requires an isolated GTK display"]
    fn pages_are_distinct_and_parented_once_by_the_host() {
        gtk::init().expect("initialize GTK");
        let callbacks = NotificationCircleCallbacks {
            invoke: Rc::new(|_, _| {}),
            dismiss: Rc::new(|_| {}),
            clear: Rc::new(|| {}),
            inhibit: Rc::new(|_| {}),
            open_full: Rc::new(|| {}),
        };
        let circle = NotificationCircle::new(
            &NotificationConfig::default(),
            IconStyle::default(),
            callbacks,
        )
        .expect("distinct unparented pages");
        assert!(circle.host.widget().parent().is_none());
    }
}
