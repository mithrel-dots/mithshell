//! The volume/brightness/workspace OSD overlay.

use super::*;

use std::rc::Rc;

use gtk::{Align, Application, ApplicationWindow, Orientation, gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::{IslandWindow, Metrics};
use crate::{
    config::ShellConfig,
    ipc::OsdKind,
    state::{OsdState, Urgency},
};

/// Keep fullscreen feedback separate so it cannot promote circles or an open
/// dashboard above the fullscreen client along with the OSD.
pub(super) struct OsdOverlay {
    pub(super) window: ApplicationWindow,
    icon: gtk::Widget,
    title: gtk::Label,
    progress: gtk::ProgressBar,
    value: gtk::Label,
}

pub(super) fn build_osd_overlay(
    application: &Application,
    monitor: &gdk::Monitor,
    shell: &ShellConfig,
    metrics: Metrics,
    layer_shell: bool,
) -> OsdOverlay {
    let window = ApplicationWindow::builder()
        .application(application)
        .title("mithshell OSD overlay")
        .decorated(false)
        .resizable(false)
        .default_width(metrics.osd_width)
        .default_height(metrics.osd_height)
        .build();
    window.add_css_class("mithshell-window");
    if let Some(class) = metrics.css_class() {
        window.add_css_class(class);
    }
    if layer_shell {
        window.init_layer_shell();
    }
    window.set_namespace(Some("mithshell-osd-overlay"));
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
    surface.set_size_request(metrics.osd_width, metrics.osd_height);
    let (root, icon, title, progress, value) = osd_view(metrics);
    surface.set_child(Some(&root));
    window.set_child(Some(&surface));

    OsdOverlay {
        window,
        icon,
        title,
        progress,
        value,
    }
}

pub(super) fn osd_view(
    metrics: Metrics,
) -> (
    gtk::Box,
    gtk::Widget,
    gtk::Label,
    gtk::ProgressBar,
    gtk::Label,
) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    root.set_size_request(metrics.osd_width, metrics.osd_height);
    root.add_css_class("osd-content");
    root.set_valign(Align::Start);
    let icon = icon::icon_widget(Icon::VolumeHigh, metrics.icons);
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

fn apply_osd_content(
    icon_widget: &gtk::Widget,
    title_label: &gtk::Label,
    progress: &gtk::ProgressBar,
    value: &gtk::Label,
    state: &OsdState,
    metrics: Metrics,
) {
    let (icon, title) = match state.kind {
        OsdKind::Volume if state.muted => (Icon::VolumeMuted, "Muted"),
        OsdKind::Volume if state.value == 0 => (Icon::VolumeLow, "Volume"),
        OsdKind::Volume if state.value < 50 => (Icon::VolumeMedium, "Volume"),
        OsdKind::Volume => (Icon::VolumeHigh, "Volume"),
        OsdKind::Brightness => (Icon::Brightness, "Brightness"),
        OsdKind::Workspace => (Icon::Workspace, "Workspace"),
    };
    icon::set_icon(icon_widget, icon, metrics.icons);
    title_label.set_label(title);
    progress.set_fraction(f64::from(state.value) / 100.0);
    value.set_label(&state.value.to_string());
}

impl IslandWindow {
    pub fn show_osd(self: &Rc<Self>, state: OsdState) {
        apply_osd_content(
            &self.osd_icon,
            &self.osd_title,
            &self.osd_progress,
            &self.osd_value,
            &state,
            self.metrics,
        );
        let fullscreen = self
            .latest_hyprland
            .borrow()
            .monitor(&self.monitor_name)
            .is_some_and(|monitor| monitor.fullscreen);
        // Like pill notifications, choose a surface for each new presentation;
        // fullscreen changes alone do not migrate an already-visible OSD.
        self.osd_in_overlay.set(
            self.osd_overlay.is_some()
                && self
                    .notifications
                    .overlay_applies(Urgency::Normal, fullscreen)
                && !self.search_open.get(),
        );
        if let Some(overlay) = &self.osd_overlay {
            apply_osd_content(
                &overlay.icon,
                &overlay.title,
                &overlay.progress,
                &overlay.value,
                &state,
                self.metrics,
            );
        }
        self.osd_active.set(true);
        self.reconcile_view();

        let generation = self.osd_generation.get().wrapping_add(1);
        self.osd_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(state.timeout_ms), move || {
            if let Some(island) = weak.upgrade()
                && island.osd_generation.get() == generation
            {
                island.clear_osd();
                island.reconcile_view();
            }
        });
    }

    pub(super) fn clear_osd(&self) {
        self.osd_active.set(false);
        self.osd_in_overlay.set(false);
        self.reconcile_osd_overlay();
        self.osd_generation
            .set(self.osd_generation.get().wrapping_add(1));
    }

    pub(super) fn reconcile_osd_overlay(&self) {
        if let Some(overlay) = &self.osd_overlay {
            // Pill notifications keep their existing priority over OSDs.
            let visible = self.osd_active.get()
                && self.osd_in_overlay.get()
                && !self.notification_active.get();
            if visible && !overlay.window.is_visible() {
                overlay.window.present();
            } else if !visible {
                overlay.window.set_visible(false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{AppConfig, NotificationOverlay},
        state::{HyprlandMonitor, Notification, NotificationTimeout, WorkspaceRef},
    };

    #[test]
    #[ignore = "requires an isolated GTK display; run scripts/run-ui-regressions-gtk.py"]
    fn fullscreen_osd_overlay_respects_priority_replacement_expiry_and_opt_out() {
        gtk::init().unwrap();
        let application = Application::new(None::<&str>, gtk::gio::ApplicationFlags::NON_UNIQUE);
        application.connect_activate(|_| {});
        application
            .register(None::<&gtk::gio::Cancellable>)
            .unwrap();
        let monitor = gdk::Display::default()
            .unwrap()
            .monitors()
            .item(0)
            .and_downcast::<gdk::Monitor>()
            .unwrap();
        let mut config = AppConfig::default();
        config.notifications.overlay_over_fullscreen = NotificationOverlay::Normal;
        let make_island = |config: &AppConfig| {
            IslandWindow::test_notification_window(
                &application,
                &monitor,
                "osd-test",
                config,
                Rc::new(|_, _| {}),
                Rc::new(|_| {}),
                Rc::new(|_, _| {}),
            )
        };
        let island = make_island(&config);
        let mut snapshot = HyprlandSnapshot {
            monitors: vec![HyprlandMonitor {
                id: 1,
                name: "osd-test".into(),
                focused: true,
                fullscreen: true,
                active_workspace: WorkspaceRef::default(),
                special_workspace: WorkspaceRef::default(),
            }],
            ..HyprlandSnapshot::default()
        };
        island.update_hyprland(&snapshot);
        let pump = |duration| {
            let until = Instant::now() + duration;
            while Instant::now() < until {
                while glib::MainContext::default().iteration(false) {}
                std::thread::sleep(Duration::from_millis(2));
            }
        };
        let mut state = OsdState {
            kind: OsdKind::Volume,
            value: 37,
            muted: true,
            timeout_ms: 25,
        };
        island.show_osd(state.clone());
        let overlay = island.osd_overlay.as_ref().unwrap();
        assert!(overlay.window.is_visible());
        assert_eq!(overlay.title.label(), "Muted");
        assert_eq!(overlay.value.label(), "37");
        assert_eq!(overlay.progress.fraction(), 0.37);
        assert_eq!(island.current_view.get(), View::Compact);

        state = OsdState {
            kind: OsdKind::Workspace,
            value: 4,
            muted: false,
            timeout_ms: 60_000,
        };
        island.show_osd(state.clone());
        pump(Duration::from_millis(80));
        assert!(
            overlay.window.is_visible(),
            "replaced timer must not hide the latest OSD"
        );
        assert_eq!(overlay.title.label(), "Workspace");
        assert_eq!(overlay.value.label(), "4");
        island.show_notification(
            Notification {
                id: 1,
                revision: 1,
                app_name: "test".into(),
                app_icon: None,
                summary: "Higher priority".into(),
                body: String::new(),
                urgency: Urgency::Normal,
                actions: Vec::new(),
                timeout: NotificationTimeout::Default,
                resident: false,
                transient: false,
                received_at_unix_seconds: 0,
            },
            1,
        );
        assert!(!overlay.window.is_visible());
        assert!(island.pill_overlay.as_ref().unwrap().window.is_visible());
        island.close_notification(1);
        assert!(
            overlay.window.is_visible(),
            "unexpired feedback resumes after the notification"
        );

        snapshot.monitors[0].fullscreen = false;
        island.update_hyprland(&snapshot);
        assert!(
            overlay.window.is_visible(),
            "existing presentations keep their selected surface"
        );
        state.kind = OsdKind::Brightness;
        island.show_osd(state.clone());
        assert!(!overlay.window.is_visible());
        assert_eq!(island.current_view.get(), View::Osd);
        snapshot.monitors[0].fullscreen = true;
        island.update_hyprland(&snapshot);
        state.timeout_ms = 25;
        island.show_osd(state.clone());
        pump(Duration::from_millis(80));
        assert!(!overlay.window.is_visible());
        assert!(!island.osd_active.get());
        assert_eq!(island.current_view.get(), View::Compact);
        state.timeout_ms = 60_000;
        island.show_osd(state.clone());
        island.close();
        assert!(!overlay.window.is_visible());
        island.destroy();

        for (follow, threshold) in [
            (false, NotificationOverlay::Normal),
            (true, NotificationOverlay::Critical),
        ] {
            config.osd.follow_notifications = follow;
            config.notifications.overlay_over_fullscreen = threshold;
            let island = make_island(&config);
            island.update_hyprland(&snapshot);
            island.show_osd(state.clone());
            assert!(island.osd_overlay.is_none());
            assert_eq!(island.current_view.get(), View::Osd);
            island.destroy();
        }
    }
}
