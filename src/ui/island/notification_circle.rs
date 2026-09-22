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
    compact_click: GestureClick,
    compact_style: gtk::CssProvider,
    compact_count: gtk::Label,
    hover: gtk::Box,
    hover_list: gtk::Box,
    full: gtk::Box,
    full_list: gtk::Box,
    hover_open: gtk::Button,
    hover_count: usize,
    style: IconStyle,
    callbacks: NotificationCircleCallbacks,
    inhibit: gtk::ToggleButton,
    inhibit_remaining: gtk::Label,
    inhibit_updating: Rc<Cell<bool>>,
}

impl NotificationCircle {
    /// Builds three separate pages as required by `CircleHost`; no page is
    /// reused or parented elsewhere.  `ui_scale` is the resolved output scale
    /// used for the compact bell, independent of popup notification padding.
    /// The host starts absent.  Integration
    /// must install `host.set_on_change(...)` before its first `update`, which
    /// dispatches `Content` and requests the initial layout.
    pub(crate) fn new(
        config: &NotificationConfig,
        style: IconStyle,
        ui_scale: f64,
        callbacks: NotificationCircleCallbacks,
    ) -> Result<Rc<Self>, &'static str> {
        let compact = gtk::Box::new(Orientation::Horizontal, 6);
        compact.add_css_class("notification-circle-compact");
        compact.set_halign(Align::Center);
        compact.update_property(&[gtk::accessible::Property::Label(
            "Open notification history",
        )]);
        let bell = icon::icon_widget(Icon::Bell, style);
        bell.add_css_class("notification-circle-bell");
        let icon_size = (16.0 * ui_scale.max(0.5)).round() as i32;
        if let Some(image) = bell.downcast_ref::<gtk::Image>() {
            image.set_pixel_size(icon_size);
        }
        compact.append(&bell);
        let count = gtk::Label::new(Some("0"));
        count.add_css_class("notification-count");
        compact.append(&count);
        let open_callback = callbacks.open_full.clone();
        let click = GestureClick::new();
        click.connect_released(move |_, _, _, _| open_callback());
        compact.add_controller(click.clone());

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
        let compact_style = gtk::CssProvider::new();
        compact_style.load_from_string(&format!(
            ".notification-circle-compact {{ padding: 0; min-height: 0; }}\
             .notification-circle-compact .notification-circle-bell {{\
                 color: @ms_primary; font-size: {icon_size}px;\
                 -gtk-icon-size: {icon_size}px;\
             }}\
             .notification-circle-bell {{ color: @ms_primary; }}\
             .notification-circle-compact .notification-count {{\
                 min-width: 0; min-height: 0; padding: 0; border-radius: 0;\
             }}"
        ));
        #[allow(deprecated)]
        compact
            .style_context()
            .add_provider(&compact_style, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        #[allow(deprecated)]
        bell.style_context()
            .add_provider(&compact_style, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        let circle = Rc::new(Self {
            host,
            compact,
            compact_click: click,
            compact_style,
            compact_count: count,
            hover,
            hover_list,
            full,
            full_list,
            hover_open,
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

    #[cfg(test)]
    pub(crate) fn test_click_compact(&self) {
        self.compact_click
            .emit_by_name::<()>("pressed", &[&1_i32, &0.0_f64, &0.0_f64]);
        self.compact_click
            .emit_by_name::<()>("released", &[&1_i32, &0.0_f64, &0.0_f64]);
    }

    #[cfg(test)]
    pub(crate) fn test_inhibition(&self) -> (bool, String, bool) {
        (
            self.inhibit.is_active(),
            self.inhibit_remaining.label().to_string(),
            self.inhibit_remaining.is_visible(),
        )
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

    /// Full production-widget coverage. Run through the project-local
    /// Broadway runner (`target/run-notification-circle-gtk.py`).
    #[test]
    #[ignore = "requires an isolated GTK display"]
    #[allow(deprecated)]
    fn gtk_notification_circle_integration() {
        gtk::init().expect("initialize GTK");
        fn children(widget: &gtk::Widget) -> Vec<gtk::Widget> {
            let mut result = Vec::new();
            let mut child = widget.first_child();
            while let Some(current) = child {
                child = current.next_sibling();
                result.push(current);
            }
            result
        }
        fn test_circle(
            config: &NotificationConfig,
            events: Rc<std::cell::RefCell<Vec<String>>>,
        ) -> (Rc<NotificationCircle>, Rc<Cell<u32>>) {
            let callbacks = NotificationCircleCallbacks {
                invoke: {
                    let events = events.clone();
                    Rc::new(move |id, key| events.borrow_mut().push(format!("invoke:{id}:{key}")))
                },
                dismiss: {
                    let events = events.clone();
                    Rc::new(move |id| events.borrow_mut().push(format!("dismiss:{id}")))
                },
                clear: {
                    let events = events.clone();
                    Rc::new(move || events.borrow_mut().push("clear".to_owned()))
                },
                inhibit: {
                    let events = events.clone();
                    Rc::new(move |active| events.borrow_mut().push(format!("inhibit:{active}")))
                },
                open_full: Rc::new(|| {}),
            };
            let circle = NotificationCircle::new(config, IconStyle::default(), 1.0, callbacks)
                .expect("distinct unparented pages");
            let hook_calls = Rc::new(Cell::new(0));
            let hook_counter = hook_calls.clone();
            circle.host.set_on_change(move |_| {
                hook_counter.set(hook_counter.get() + 1);
            });
            (circle, hook_calls)
        }

        let config = NotificationConfig {
            hover_preview_count: 1,
            ..NotificationConfig::default()
        };
        let events = Rc::new(std::cell::RefCell::new(Vec::new()));
        let (circle, hook_calls) = test_circle(&config, events.clone());
        assert_eq!(circle.host.mode(), super::super::circle::Mode::Absent);
        assert!(circle.host.frame().is_none());
        assert!(circle.compact.parent().is_some());
        assert!(circle.hover.parent().is_some());
        assert!(circle.full.parent().is_some());
        assert_ne!(circle.compact.as_ptr(), circle.hover.as_ptr());
        assert_ne!(circle.hover.as_ptr(), circle.full.as_ptr());
        assert!(!circle.compact.has_css_class("notification-content"));
        assert!(
            circle
                .compact
                .first_child()
                .is_some_and(|widget| widget.has_css_class("notification-circle-bell"))
        );
        for (style, scale) in [
            (IconStyle::Glyph, 0.75),
            (IconStyle::Glyph, 1.5),
            (IconStyle::Symbolic, 0.75),
            (IconStyle::Symbolic, 1.5),
        ] {
            let variant = NotificationCircle::new(
                &config,
                style,
                scale,
                NotificationCircleCallbacks {
                    invoke: Rc::new(|_, _| {}),
                    dismiss: Rc::new(|_| {}),
                    clear: Rc::new(|| {}),
                    inhibit: Rc::new(|_| {}),
                    open_full: Rc::new(|| {}),
                },
            )
            .expect("scaled icon variant");
            let palette = gtk::CssProvider::new();
            palette.load_from_string("@define-color ms_primary rgb(17, 34, 51);");
            #[allow(deprecated)]
            variant
                .compact
                .style_context()
                .add_provider(&palette, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
            let bell = variant.compact.first_child().unwrap();
            #[allow(deprecated)]
            bell.style_context()
                .add_provider(&palette, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
            assert!(
                variant
                    .compact
                    .first_child()
                    .is_some_and(|widget| widget.has_css_class("notification-circle-bell"))
            );
            assert_eq!(children(&variant.compact.clone().upcast()).len(), 2);
            assert_eq!(
                bell.style_context().color(),
                gtk::gdk::RGBA::new(17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 1.0)
            );
            let expected_size = (16.0 * scale.max(0.5)).round() as i32;
            match style {
                IconStyle::Glyph => {
                    if let Ok(label) = bell.clone().downcast::<gtk::Label>() {
                        assert_eq!(
                            label.pango_context().font_description().unwrap().size(),
                            expected_size * gtk::pango::SCALE
                        );
                    } else {
                        let image = bell.downcast::<gtk::Image>().expect("glyph fallback image");
                        assert_eq!(image.pixel_size(), expected_size);
                    }
                }
                IconStyle::Symbolic => {
                    let image = bell.downcast::<gtk::Image>().expect("symbolic bell");
                    assert_eq!(image.pixel_size(), expected_size);
                }
            }
        }

        let mut first = notification(10, "first body");
        first.actions = vec![NotificationAction {
            key: "default".to_owned(),
            label: "Open first".to_owned(),
        }];
        let mut second = notification(20, "second body that remains complete in full history");
        second.actions = vec![NotificationAction {
            key: "default".to_owned(),
            label: "Open second".to_owned(),
        }];
        circle.update(&[first.clone(), second.clone()]);
        assert_eq!(
            hook_calls.get(),
            1,
            "first update must request initial layout"
        );
        assert_eq!(circle.host.mode(), super::super::circle::Mode::Compact);
        let compact_children = children(&circle.compact.clone().upcast());
        assert_eq!(
            compact_children[1]
                .downcast_ref::<gtk::Label>()
                .unwrap()
                .label(),
            "2"
        );
        let hover_rows = children(&circle.hover_list.clone().upcast());
        assert_eq!(hover_rows.len(), 1);
        let hover_text = children(&hover_rows[0])[1].clone();
        assert!(
            children(&hover_text)[0]
                .downcast_ref::<gtk::Label>()
                .unwrap()
                .label()
                .contains("summary-10")
        );
        let full_rows = children(&circle.full_list.clone().upcast());
        assert_eq!(full_rows.len(), 2);
        for (row, expected) in full_rows.iter().zip([
            "first body",
            "second body that remains complete in full history",
        ]) {
            let text = children(row)[1].clone();
            let body = children(&text)
                .into_iter()
                .find(|widget| widget.has_css_class("notification-row-body"))
                .unwrap()
                .downcast::<gtk::Label>()
                .unwrap();
            assert_eq!(body.label(), expected);
            assert!(body.lines() <= 0, "full body unexpectedly has a line cap");
        }

        // Replace the snapshot, then activate the actual rebuilt row button.
        circle.update(&[second]);
        let row = children(&circle.full_list.clone().upcast())[0].clone();
        let text = children(&row)[1].clone();
        let action = children(&text)
            .into_iter()
            .last()
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        action.emit_clicked();
        let dismiss = children(&row)
            .into_iter()
            .last()
            .unwrap()
            .downcast::<gtk::Button>()
            .unwrap();
        dismiss.emit_clicked();
        assert_eq!(&*events.borrow(), &["invoke:20:default", "dismiss:20"]);

        let header = children(&circle.full.clone().upcast())[0].clone();
        children(&header)[1]
            .clone()
            .downcast::<gtk::Button>()
            .unwrap()
            .emit_clicked();
        children(&header)[2]
            .clone()
            .downcast::<gtk::ToggleButton>()
            .unwrap()
            .set_active(true);
        assert_eq!(
            &*events.borrow(),
            &["invoke:20:default", "dismiss:20", "clear", "inhibit:true"]
        );

        // A zero-preview circle keeps only its header affordance and still
        // exposes the full history page.
        let zero_config = NotificationConfig {
            hover_preview_count: 0,
            ..NotificationConfig::default()
        };
        let (zero, zero_hook_calls) =
            test_circle(&zero_config, Rc::new(std::cell::RefCell::new(Vec::new())));
        zero.update(&[first]);
        assert_eq!(
            zero_hook_calls.get(),
            1,
            "zero-preview content still binds host layout"
        );
        assert!(children(&zero.hover_list.clone().upcast()).is_empty());
        assert_eq!(children(&zero.hover.clone().upcast()).len(), 2);
        assert_eq!(children(&zero.full_list.clone().upcast()).len(), 1);

        // Render a real frame, then ensure empty history removes content and
        // clears the host geometry rather than leaving a stale surface.
        use super::super::circle::{CircleRequest, CircleSpec, Rect, Size, layout};
        let spec = CircleSpec {
            diameter: 32.0,
            hover: Size {
                width: 160.0,
                height: 80.0,
            },
            full: Some(Size {
                width: 240.0,
                height: 180.0,
            }),
        };
        let visual = spec.visual(super::super::circle::Mode::Compact).unwrap();
        let frame = layout(
            Rect {
                x: 100.0,
                y: 10.0,
                width: 100.0,
                height: 30.0,
            },
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
            1.0,
            [Some(CircleRequest { spec, visual }), None],
        )[0]
        .unwrap();
        assert!(circle.host.render(circle.host.revision(), Some(frame)));
        assert!(circle.host.frame().is_some());
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        let callbacks_before_empty = hook_calls.get();
        circle.update(&[]);
        assert_eq!(
            hook_calls.get(),
            callbacks_before_empty + 1,
            "empty update must notify host disappearance exactly once"
        );
        assert_eq!(circle.host.mode(), super::super::circle::Mode::Absent);
        assert!(circle.host.frame().is_none());
    }
}
