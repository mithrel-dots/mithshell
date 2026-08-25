//! Desktop-notification rendering: the pill-position queue, the optional
//! overlay window, and the below-pill/corner toast popup.

use super::*;

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use gtk::{Align, Application, ApplicationWindow, GestureClick, Orientation, gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::{IslandWindow, Metrics};
use crate::config::{NotificationConfig, NotificationPosition, ShellConfig};
use crate::state::{Notification, Urgency};

/// Width of a `below-pill`/corner toast, independent of `Metrics` since
/// those popups aren't part of the animated island surface.
const NOTIFICATION_TOAST_WIDTH: i32 = 320;

/// The separate popup used for `notifications.position` values other than
/// `pill`: a small always-on-top layer-shell surface anchored either
/// directly under the island (`below-pill`) or to a screen corner, holding
/// a vertically stacked list of toast rows.
pub(super) struct NotificationToasts {
    pub(super) window: ApplicationWindow,
    pub(super) stack: gtk::Box,
    pub(super) entries: RefCell<HashMap<u32, ToastEntry>>,
    /// Notification ids, most recent first. Tracked separately from
    /// `entries` (a `HashMap`) purely to know which toast is oldest once
    /// `max_visible` is exceeded.
    pub(super) order: RefCell<Vec<u32>>,
    pub(super) overlay: Cell<bool>,
}

pub(super) struct ToastEntry {
    row: gtk::Box,
    urgency: Urgency,
    /// Cancelled on manual dismissal so an already-removed row can't be
    /// double-removed when its timer later fires.
    timeout_source: Option<glib::SourceId>,
}

#[derive(Clone)]
pub(super) struct PendingNotification {
    notification: Notification,
    epoch: u64,
}

pub(super) struct CurrentNotification {
    pub(super) pending: PendingNotification,
    pub(super) overlay: bool,
}

pub(super) struct PillOverlay {
    pub(super) window: ApplicationWindow,
    pub(super) root: gtk::Box,
    pub(super) icon: gtk::Image,
    pub(super) app: gtk::Label,
    pub(super) body: gtk::Label,
}

pub(super) fn notification_view(
    metrics: Metrics,
) -> (gtk::Box, gtk::Image, gtk::Label, gtk::Label) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    root.set_size_request(metrics.notification_width, metrics.notification_height);
    root.add_css_class("notification-content");
    root.set_valign(Align::Center);

    let icon = gtk::Image::from_icon_name("preferences-system-notifications-symbolic");
    icon.add_css_class("notification-icon");
    icon.set_valign(Align::Center);

    let text = gtk::Box::new(Orientation::Vertical, metrics.spacing(2));
    text.set_valign(Align::Center);
    text.set_hexpand(true);
    let app = gtk::Label::new(None);
    app.add_css_class("notification-app");
    app.set_halign(Align::Start);
    app.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let body = gtk::Label::new(None);
    body.add_css_class("notification-body");
    body.set_halign(Align::Start);
    body.set_ellipsize(gtk::pango::EllipsizeMode::End);
    body.set_visible(false);
    text.append(&app);
    text.append(&body);

    root.append(&icon);
    root.append(&text);
    (root, icon, app, body)
}

fn apply_notification_content(
    icon: &gtk::Image,
    app: &gtk::Label,
    body: &gtk::Label,
    notification: &Notification,
) {
    apply_notification_icon(
        icon,
        notification.app_icon.as_deref(),
        "preferences-system-notifications-symbolic",
    );
    app.set_label(&notification.summary);
    app.set_tooltip_text(Some(&notification.app_name));
    body.set_label(&notification.body);
    body.set_visible(!notification.body.is_empty());
}

pub(super) fn build_pill_overlay(
    application: &Application,
    monitor: &gdk::Monitor,
    shell: &ShellConfig,
    metrics: Metrics,
) -> PillOverlay {
    let window = ApplicationWindow::builder()
        .application(application)
        .title("mithshell notification overlay")
        .decorated(false)
        .resizable(false)
        .default_width(metrics.notification_width)
        .default_height(metrics.notification_height)
        .build();
    window.add_css_class("mithshell-window");
    if let Some(class) = metrics.css_class() {
        window.add_css_class(class);
    }
    window.init_layer_shell();
    window.set_namespace(Some("mithshell-notification-overlay"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_monitor(Some(monitor));
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Top, metrics.spacing(shell.top_margin));
    window.set_exclusive_zone(0);

    let surface = gtk::ScrolledWindow::new();
    surface.add_css_class("island-surface");
    surface.set_overflow(Overflow::Hidden);
    surface.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
    surface.set_size_request(metrics.notification_width, metrics.notification_height);
    let (root, icon, app, body) = notification_view(metrics);
    surface.set_child(Some(&root));
    window.set_child(Some(&surface));

    PillOverlay {
        window,
        root,
        icon,
        app,
        body,
    }
}

/// Builds the separate popup window used for `below-pill`/corner
/// notification positions. Must only be called with a non-`Pill` position.
pub(super) fn build_notification_toasts(
    application: &Application,
    monitor: &gdk::Monitor,
    shell: &ShellConfig,
    notifications: &NotificationConfig,
    metrics: Metrics,
) -> NotificationToasts {
    let window = ApplicationWindow::builder()
        .application(application)
        .title("mithshell notifications")
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("mithshell-notifications");
    if let Some(class) = metrics.css_class() {
        window.add_css_class(class);
    }
    window.init_layer_shell();
    window.set_namespace(Some("mithshell-notifications"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_monitor(Some(monitor));
    window.set_exclusive_zone(0);

    match notifications.position {
        NotificationPosition::Pill => {
            unreachable!("build_notification_toasts is only called for non-pill positions")
        }
        // Anchoring only the top edge, like the island itself, is what
        // centers this window horizontally under it; the margin clears the
        // island's resting (compact) height plus the configured gap. The
        // popup does not follow the island if it grows taller (dashboard,
        // search, ...) -- those overlay views already dim/steal input via
        // the dismiss window, so brief overlap is an acceptable trade for
        // not re-anchoring a second surface on every geometry animation.
        NotificationPosition::BelowPill => {
            window.set_anchor(Edge::Top, true);
            window.set_margin(
                Edge::Top,
                metrics.spacing(shell.top_margin)
                    + metrics.compact_height
                    + metrics.spacing(notifications.gap),
            );
        }
        NotificationPosition::TopLeft => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_margin(Edge::Top, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Left, metrics.spacing(notifications.margin));
        }
        NotificationPosition::TopRight => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Top, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Right, metrics.spacing(notifications.margin));
        }
        NotificationPosition::BottomLeft => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_margin(Edge::Bottom, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Left, metrics.spacing(notifications.margin));
        }
        NotificationPosition::BottomRight => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Right, true);
            window.set_margin(Edge::Bottom, metrics.spacing(notifications.margin));
            window.set_margin(Edge::Right, metrics.spacing(notifications.margin));
        }
    }

    let stack = gtk::Box::new(Orientation::Vertical, metrics.spacing(notifications.gap));
    stack.add_css_class("notification-stack");
    stack.set_width_request(metrics.spacing(NOTIFICATION_TOAST_WIDTH));
    window.set_child(Some(&stack));

    NotificationToasts {
        window,
        stack,
        entries: RefCell::new(HashMap::new()),
        order: RefCell::new(Vec::new()),
        overlay: Cell::new(false),
    }
}

/// Resolves a notification's icon onto `image`: an absolute path is loaded
/// as a file, anything else is treated as a themed icon name, and an empty
/// hint falls back to `fallback`.
fn apply_notification_icon(image: &gtk::Image, icon: Option<&str>, fallback: &str) {
    match icon {
        Some(path) if path.starts_with('/') => image.set_from_file(Some(path)),
        Some(name) if !name.is_empty() => image.set_icon_name(Some(name)),
        _ => image.set_icon_name(Some(fallback)),
    }
}

impl IslandWindow {
    /// Displays an incoming notification according to
    /// `notifications.position`. `pill` queues it into the island or its
    /// overlay counterpart; every other position pushes a toast into the
    /// separate popup window built alongside this island.
    pub fn show_notification(self: &Rc<Self>, notification: Notification, epoch: u64) {
        let pending = PendingNotification {
            notification,
            epoch,
        };
        if self.notifications.position == NotificationPosition::Pill {
            self.show_notification_pill(pending);
        } else {
            self.show_notification_toast(pending);
        }
    }

    /// Removes a notification wherever it currently lives: an active or
    /// queued pill entry, or a toast in the popup window. A no-op if `id`
    /// isn't currently tracked in either place.
    pub fn close_notification(self: &Rc<Self>, id: u32) {
        self.remove_toast(id);
        if self
            .notification_current
            .borrow()
            .as_ref()
            .is_some_and(|current| current.pending.notification.id == id)
        {
            self.notification_generation
                .set(self.notification_generation.get().wrapping_add(1));
            self.advance_notification();
        } else {
            self.notification_queue
                .borrow_mut()
                .retain(|queued| queued.notification.id != id);
        }
    }

    /// Rebuilds the dashboard's notification history list and count badge
    /// from the controller's bounded history, most recent first.
    pub fn update_notification_history(&self, history: &[Notification]) {
        self.notification_count
            .set_label(&history.len().to_string());
        clear_box(&self.notification_list);
        if history.is_empty() {
            let placeholder = gtk::Label::new(Some("No notifications yet"));
            placeholder.add_css_class("muted-label");
            placeholder.add_css_class("notification-empty");
            placeholder.set_halign(Align::Start);
            placeholder.set_wrap(true);
            self.notification_list.append(&placeholder);
            return;
        }
        for notification in history {
            self.notification_list
                .append(&self.notification_history_row(notification));
        }
    }

    fn notification_history_row(&self, notification: &Notification) -> gtk::Box {
        let row = gtk::Box::new(Orientation::Horizontal, self.metrics.spacing(8));
        row.add_css_class("notification-row");
        if notification.urgency == Urgency::Critical {
            row.add_css_class("urgency-critical");
        }

        let icon = gtk::Image::new();
        apply_notification_icon(
            &icon,
            notification.app_icon.as_deref(),
            "preferences-system-notifications-symbolic",
        );
        icon.add_css_class("notification-row-icon");
        icon.set_valign(Align::Start);

        let text = gtk::Box::new(Orientation::Vertical, self.metrics.spacing(2));
        text.set_hexpand(true);
        let summary = gtk::Label::new(Some(&notification.summary));
        summary.add_css_class("notification-row-summary");
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        // Filled to the row rather than sized to the text: the dashboard is
        // laid out at its natural width inside a fixed-width surface, so an
        // unbounded row would push its own dismiss button outside the panel
        // instead of wrapping and ellipsizing within it.
        summary.set_xalign(0.0);
        summary.set_max_width_chars(20);
        text.append(&summary);
        if !notification.body.is_empty() {
            let body = gtk::Label::new(Some(&notification.body));
            body.add_css_class("notification-row-body");
            body.set_wrap(true);
            body.set_lines(2);
            body.set_ellipsize(gtk::pango::EllipsizeMode::End);
            body.set_xalign(0.0);
            body.set_max_width_chars(24);
            text.append(&body);
        }

        // Keep the gesture on the expanding content column rather than the
        // whole row, so clicking the child dismiss button cannot also invoke
        // the notification's default action.
        let default_action = notification.default_action().cloned();
        let tooltip = default_action.as_ref().map_or_else(
            || "Right-click to dismiss".to_owned(),
            |action| {
                if action.label.is_empty() {
                    "Left-click to activate  //  right-click to dismiss".to_owned()
                } else {
                    format!("{}  //  right-click to dismiss", action.label)
                }
            },
        );
        row.set_tooltip_text(Some(&tooltip));

        let click = GestureClick::new();
        click.set_button(0);
        let invoke = self.actions.notification_invoke.clone();
        let dismiss_from_history = self.actions.notification_dismiss.clone();
        let id = notification.id;
        click.connect_released(move |gesture, _, _, _| match gesture.current_button() {
            gdk::BUTTON_PRIMARY => {
                if let Some(action) = &default_action {
                    invoke(id, action.key.clone());
                }
            }
            gdk::BUTTON_SECONDARY => dismiss_from_history(id),
            _ => {}
        });
        text.add_controller(click);

        row.append(&icon);
        row.append(&text);

        let dismiss = gtk::Button::from_icon_name("window-close-symbolic");
        dismiss.add_css_class("notification-row-dismiss");
        dismiss.set_valign(Align::Start);
        let action = self.actions.notification_dismiss.clone();
        dismiss.connect_clicked(move |_| action(id));
        row.append(&dismiss);

        row
    }

    fn show_notification_pill(self: &Rc<Self>, pending: PendingNotification) {
        if self
            .notification_current
            .borrow()
            .as_ref()
            .is_some_and(|current| current.pending.notification.id == pending.notification.id)
        {
            self.present_notification_pill(pending);
            return;
        }

        let mut queue = self.notification_queue.borrow_mut();
        if let Some(queued) = queue
            .iter_mut()
            .find(|queued| queued.notification.id == pending.notification.id)
        {
            *queued = pending;
            return;
        }
        queue.push_back(pending);
        drop(queue);
        if !self.notification_active.get() {
            self.advance_notification();
        }
    }

    /// Pops the next queued notification into the pill, or clears the
    /// active flag and returns to whatever view `reconcile_view` picks next
    /// when the queue is empty.
    ///
    /// In `pill` position a notification always eventually times out, even
    /// one that requested `expire_timeout = 0` ("never expire") -- the pill
    /// has no interactive dismiss control, so a persistent entry would
    /// otherwise block the queue forever. `below-pill`/corner toasts do
    /// honor persistence, since those have a close button.
    fn advance_notification(self: &Rc<Self>) {
        let Some(pending) = self.notification_queue.borrow_mut().pop_front() else {
            self.notification_active.set(false);
            self.notification_current.borrow_mut().take();
            if let Some(overlay) = &self.pill_overlay {
                overlay.window.set_visible(false);
            }
            self.reconcile_view();
            return;
        };
        self.present_notification_pill(pending);
    }

    fn present_notification_pill(self: &Rc<Self>, pending: PendingNotification) {
        let fullscreen = self
            .latest_hyprland
            .borrow()
            .monitor(&self.monitor_name)
            .is_some_and(|monitor| monitor.fullscreen);
        let overlay_active = self
            .notifications
            .overlay_applies(pending.notification.urgency, fullscreen)
            && self.pill_overlay.is_some();
        if overlay_active {
            let overlay = self.pill_overlay.as_ref().expect("checked above");
            apply_notification_content(
                &overlay.icon,
                &overlay.app,
                &overlay.body,
                &pending.notification,
            );
            overlay.window.set_visible(true);
            overlay.window.present();
        } else {
            apply_notification_content(
                &self.notification_icon,
                &self.notification_app,
                &self.notification_body,
                &pending.notification,
            );
            if let Some(overlay) = &self.pill_overlay {
                overlay.window.set_visible(false);
            }
        }

        let timeout_ms = pending
            .notification
            .timeout
            .resolve(self.notifications.timeout_ms)
            .unwrap_or(self.notifications.timeout_ms);
        let id = pending.notification.id;
        let epoch = pending.epoch;
        self.notification_active.set(true);
        *self.notification_current.borrow_mut() = Some(CurrentNotification {
            pending,
            overlay: overlay_active,
        });
        self.reconcile_view();

        let generation = self.notification_generation.get().wrapping_add(1);
        self.notification_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(timeout_ms), move || {
            if let Some(island) = weak.upgrade()
                && island.notification_generation.get() == generation
            {
                (island.actions.notification_expired)(id, epoch);
            }
        });
    }

    /// Handles a click on the `pill`-position notification view. Invokes
    /// the current notification's default action if it declared one, or
    /// simply dismisses it otherwise -- clicking the pill should always do
    /// *something*, and most senders don't declare any actions at all.
    pub(super) fn activate_current_notification(self: &Rc<Self>) {
        let Some(notification) = self
            .notification_current
            .borrow()
            .as_ref()
            .map(|current| current.pending.notification.clone())
        else {
            return;
        };
        if let Some(action) = notification.default_action() {
            (self.actions.notification_invoke)(notification.id, action.key.clone());
        } else {
            (self.actions.notification_dismiss)(notification.id);
        }
    }

    /// Right-clicking the pill always dismisses the current notification,
    /// regardless of whether it declared a default action.
    pub(super) fn dismiss_current_notification(self: &Rc<Self>) {
        let Some(notification) = self
            .notification_current
            .borrow()
            .as_ref()
            .map(|current| current.pending.notification.clone())
        else {
            return;
        };
        (self.actions.notification_dismiss)(notification.id);
    }

    fn show_notification_toast(self: &Rc<Self>, pending: PendingNotification) {
        let Some(toasts) = &self.notification_toasts else {
            return;
        };
        let notification = &pending.notification;
        let id = notification.id;
        let epoch = pending.epoch;
        // `replaces_id` may reference a toast that's already showing.
        self.remove_toast(id);
        let row = self.build_toast_row(notification);
        toasts.stack.prepend(&row);
        toasts.order.borrow_mut().insert(0, id);

        let timeout_source = notification
            .timeout
            .resolve(self.notifications.timeout_ms)
            .map(|timeout_ms| {
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(Duration::from_millis(timeout_ms), move || {
                    if let Some(island) = weak.upgrade() {
                        (island.actions.notification_expired)(id, epoch);
                    }
                })
            });
        toasts.entries.borrow_mut().insert(
            id,
            ToastEntry {
                row,
                urgency: notification.urgency,
                timeout_source,
            },
        );

        self.trim_toasts();
        self.reconcile_notification_toasts();
        toasts.window.present();
    }

    fn build_toast_row(self: &Rc<Self>, notification: &Notification) -> gtk::Box {
        let row = gtk::Box::new(Orientation::Horizontal, self.metrics.spacing(10));
        row.add_css_class("notification-toast");
        if notification.urgency == Urgency::Critical {
            row.add_css_class("urgency-critical");
        }

        let icon = gtk::Image::new();
        apply_notification_icon(
            &icon,
            notification.app_icon.as_deref(),
            "preferences-system-notifications-symbolic",
        );
        icon.add_css_class("notification-toast-icon");
        icon.set_valign(Align::Start);

        let text = gtk::Box::new(Orientation::Vertical, self.metrics.spacing(2));
        text.set_hexpand(true);
        let summary = gtk::Label::new(Some(&notification.summary));
        summary.add_css_class("notification-toast-summary");
        summary.set_halign(Align::Start);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&summary);
        if !notification.body.is_empty() {
            let body = gtk::Label::new(Some(&notification.body));
            body.add_css_class("notification-toast-body");
            body.set_halign(Align::Start);
            body.set_wrap(true);
            body.set_lines(3);
            body.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&body);
        }

        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("notification-toast-close");
        close.set_valign(Align::Start);
        let dismiss_action = self.actions.notification_dismiss.clone();
        let id = notification.id;
        close.connect_clicked(move |_| dismiss_action(id));

        row.append(&icon);
        row.append(&text);
        row.append(&close);

        if notification.default_action().is_some() {
            row.add_css_class("notification-toast-clickable");
            let invoke_action = self.actions.notification_invoke.clone();
            let click = GestureClick::new();
            click.connect_released(move |gesture, _, _, _| {
                if gesture.current_button() == 1 {
                    invoke_action(id, "default".to_owned());
                }
            });
            row.add_controller(click);
        }

        row
    }

    fn remove_toast(&self, id: u32) {
        let Some(toasts) = &self.notification_toasts else {
            return;
        };
        if let Some(entry) = toasts.entries.borrow_mut().remove(&id) {
            if let Some(source) = entry.timeout_source {
                source.remove();
            }
            toasts.stack.remove(&entry.row);
        }
        toasts.order.borrow_mut().retain(|existing| *existing != id);
        self.reconcile_notification_toasts();
    }

    pub(super) fn reconcile_notification_toasts(&self) {
        let Some(toasts) = &self.notification_toasts else {
            return;
        };
        let fullscreen = self
            .latest_hyprland
            .borrow()
            .monitor(&self.monitor_name)
            .is_some_and(|monitor| monitor.fullscreen);
        let overlay = fullscreen
            && toasts.entries.borrow().values().any(|entry| {
                self.notifications
                    .overlay_applies(entry.urgency, fullscreen)
            });
        let update_rows = || {
            for entry in toasts.entries.borrow().values() {
                entry.row.set_visible(
                    !overlay
                        || self
                            .notifications
                            .overlay_applies(entry.urgency, fullscreen),
                );
            }
        };
        let layer_changed = toasts.overlay.replace(overlay) != overlay;
        // Hide before promotion and reveal after demotion so Top-only rows
        // never flash above a fullscreen window during the layer transition.
        if overlay {
            update_rows();
            if layer_changed {
                toasts.window.set_layer(Layer::Overlay);
            }
        } else {
            if layer_changed {
                toasts.window.set_layer(Layer::Top);
            }
            update_rows();
        }
        toasts
            .window
            .set_visible(!toasts.entries.borrow().is_empty());
    }

    /// Drops the oldest toasts past `notifications.max_visible`.
    fn trim_toasts(&self) {
        if self.notification_toasts.is_none() {
            return;
        }
        let max_visible = self.notifications.max_visible.max(1);
        loop {
            let Some(toasts) = &self.notification_toasts else {
                return;
            };
            let oldest = {
                let order = toasts.order.borrow();
                if order.len() <= max_visible {
                    return;
                }
                *order.last().expect("checked non-empty above")
            };
            self.remove_toast(oldest);
        }
    }
}
