//! The island window itself: construction, lifecycle, and the public
//! commands the controller invokes.

use super::*;

use std::rc::Rc;

use gtk::{Application, ApplicationWindow, Fixed, Orientation, Overflow, gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::compact::compact_view;
use super::dashboard::dashboard_view;
use super::media::media_view;
use super::notification::{build_notification_toasts, build_pill_overlay, notification_view};
use super::osd::osd_view;
use super::search::search_view;
use super::weather::{draw_weather_condition, weather_view};
use super::{Geometry, IslandWindow, Metrics, OverlayButtons, View};
use crate::config::{AppConfig, NotificationPosition, ShellConfig};
use crate::state::{HyprlandSnapshot, WeatherCondition};

impl IslandWindow {
    pub fn new(
        application: &Application,
        monitor: &gdk::Monitor,
        monitor_name: String,
        config: &AppConfig,
        actions: IslandActions,
        animations_enabled: bool,
    ) -> Rc<Self> {
        let shell = &config.shell;
        let metrics = Metrics::new(monitor, shell.scale, config.media.max_width_factor);

        let dismiss_window = ApplicationWindow::builder()
            .application(application)
            .title("mithshell dismiss")
            .decorated(false)
            .build();
        dismiss_window.add_css_class("mithshell-dismiss");
        dismiss_window.init_layer_shell();
        dismiss_window.set_namespace(Some("mithshell-dismiss"));
        dismiss_window.set_layer(Layer::Top);
        dismiss_window.set_keyboard_mode(KeyboardMode::None);
        dismiss_window.set_monitor(Some(monitor));
        for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
            dismiss_window.set_anchor(edge, true);
        }
        dismiss_window.set_exclusive_zone(0);
        let dismiss_area = gtk::Box::new(Orientation::Vertical, 0);
        dismiss_area.set_hexpand(true);
        dismiss_area.set_vexpand(true);
        dismiss_window.set_child(Some(&dismiss_area));

        let window = ApplicationWindow::builder()
            .application(application)
            .title("mithshell")
            .decorated(false)
            .resizable(false)
            .default_width(metrics.window_width)
            .default_height(metrics.window_height)
            .build();
        window.add_css_class("mithshell-window");
        if let Some(class) = metrics.css_class() {
            window.add_css_class(class);
        }
        window.init_layer_shell();
        window.set_namespace(Some("mithshell"));
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_monitor(Some(monitor));
        window.set_anchor(Edge::Top, true);
        window.set_margin(Edge::Top, metrics.spacing(shell.top_margin));
        window.set_exclusive_zone(metrics.spacing(shell.exclusive_zone));

        let fixed = Fixed::new();
        fixed.set_size_request(metrics.window_width, metrics.window_height);
        window.set_child(Some(&fixed));

        let surface = gtk::ScrolledWindow::new();
        surface.add_css_class("island-surface");
        surface.set_overflow(Overflow::Hidden);
        surface.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        surface.set_propagate_natural_width(false);
        surface.set_propagate_natural_height(false);
        surface.set_kinetic_scrolling(false);
        surface.set_has_frame(false);
        fixed.put(
            &surface,
            f64::from((metrics.window_width - metrics.compact_width) / 2),
            0.0,
        );
        surface.set_size_request(metrics.compact_width, metrics.compact_height);

        let content = Fixed::new();
        content.set_size_request(metrics.search_width, metrics.search_height);
        surface.set_child(Some(&content));

        let (compact, compact_workspaces, compact_clock, compact_battery, compact_tray) =
            compact_view(metrics);
        content.put(
            &compact,
            f64::from((metrics.search_width - metrics.compact_width) / 2),
            0.0,
        );

        let dashboard_widgets = dashboard_view(metrics);
        content.put(
            &dashboard_widgets.root,
            f64::from((metrics.search_width - metrics.dashboard_width) / 2),
            0.0,
        );
        dashboard_widgets.root.set_opacity(0.0);
        dashboard_widgets.root.set_visible(false);

        let search_widgets = search_view(metrics);
        content.put(&search_widgets.root, 0.0, 0.0);
        search_widgets.root.set_opacity(0.0);
        search_widgets.root.set_visible(false);

        let media_widgets = media_view(metrics);
        content.put(
            &media_widgets.root,
            f64::from((metrics.search_width - metrics.compact_width) / 2),
            0.0,
        );
        media_widgets.root.set_opacity(0.0);
        media_widgets.root.set_visible(false);

        let (osd, osd_icon, osd_title, osd_progress, osd_value) = osd_view(metrics);
        content.put(
            &osd,
            f64::from((metrics.search_width - metrics.osd_width) / 2),
            0.0,
        );
        osd.set_opacity(0.0);
        osd.set_visible(false);

        let (notification, notification_icon, notification_app, notification_body) =
            notification_view(metrics);
        content.put(
            &notification,
            f64::from((metrics.search_width - metrics.notification_width) / 2),
            0.0,
        );
        notification.set_opacity(0.0);
        notification.set_visible(false);

        let notification_toasts = (config.notifications.position != NotificationPosition::Pill)
            .then(|| {
                build_notification_toasts(
                    application,
                    monitor,
                    shell,
                    &config.notifications,
                    metrics,
                )
            });
        let pill_overlay = (config.notifications.position == NotificationPosition::Pill
            && config
                .notifications
                .overlay_over_fullscreen
                .threshold()
                .is_some())
        .then(|| build_pill_overlay(application, monitor, shell, metrics));

        let weather_widgets = weather_view(metrics, config.weather.provider);
        content.put(
            &weather_widgets.root,
            f64::from((metrics.search_width - metrics.weather_width) / 2),
            0.0,
        );
        weather_widgets.root.set_opacity(0.0);
        weather_widgets.root.set_visible(false);
        draw_weather_condition(&weather_widgets.hero_icon, WeatherCondition::Unknown);
        let weather_icons = RefCell::new(vec![weather_widgets.hero_icon.clone()]);

        let island = Rc::new(Self {
            monitor_name,
            metrics,
            window,
            dismiss_window,
            fixed,
            content,
            surface,
            compact,
            media: media_widgets.root,
            dashboard: dashboard_widgets.root,
            search: search_widgets.root,
            weather: weather_widgets.root,
            osd,
            notification,
            compact_workspaces,
            compact_clock,
            compact_battery,
            compact_tray,
            compact_width: Cell::new(metrics.compact_width),
            tray_hovered: Cell::new(false),
            tray_item_count: Cell::new(0),
            tray_menu_open: Cell::new(false),
            media_workspaces: media_widgets.workspaces,
            media_clock: media_widgets.clock,
            media_center: media_widgets.center,
            media_icon: media_widgets.icon,
            media_title: media_widgets.title,
            media_visualizer: media_widgets.visualizer,
            media_levels: media_widgets.levels,
            media_tray: media_widgets.tray,
            hero_time: dashboard_widgets.hero_time,
            hero_date: dashboard_widgets.hero_date,
            battery_chip: dashboard_widgets.battery_chip,
            battery_icon: dashboard_widgets.battery_icon,
            battery_label: dashboard_widgets.battery_label,
            player_card: dashboard_widgets.player_card,
            player_icon: dashboard_widgets.player_icon,
            player_title: dashboard_widgets.player_title,
            player_artist: dashboard_widgets.player_artist,
            player_progress: dashboard_widgets.player_progress,
            player_elapsed_label: dashboard_widgets.player_elapsed_label,
            player_duration_label: dashboard_widgets.player_duration_label,
            player_prev_button: dashboard_widgets.player_prev_button,
            player_play_pause_button: dashboard_widgets.player_play_pause_button,
            player_next_button: dashboard_widgets.player_next_button,
            player_switch_row: dashboard_widgets.player_switch_row,
            player_switch_label: dashboard_widgets.player_switch_label,
            player_switch_prev: dashboard_widgets.player_switch_prev,
            player_switch_next: dashboard_widgets.player_switch_next,
            player_progress_base_us: Cell::new(0),
            player_progress_started_at: Cell::new(None),
            player_length_us: Cell::new(0),
            player_active: Cell::new(false),
            latest_media: RefCell::new(None),
            selected_media_service: RefCell::new(None),
            active_eyebrow: dashboard_widgets.active_eyebrow,
            active_title: dashboard_widgets.active_title,
            status_card: dashboard_widgets.status_card,
            workspace_row: dashboard_widgets.workspace_row,
            controls_stack: dashboard_widgets.controls_stack,
            volume_scale: dashboard_widgets.volume_scale,
            volume_value: dashboard_widgets.volume_value,
            brightness_row: dashboard_widgets.brightness_row,
            brightness_scale: dashboard_widgets.brightness_scale,
            brightness_value: dashboard_widgets.brightness_value,
            notification_count: dashboard_widgets.notification_count,
            notification_expand_button: dashboard_widgets.notification_expand_button,
            notification_list: dashboard_widgets.notification_list,
            notifications_expanded: Cell::new(false),
            search_entry: search_widgets.entry,
            search_results: search_widgets.results,
            search_status: search_widgets.status,
            search_stack: search_widgets.stack,
            search_plugin_toggle: search_widgets.plugin_toggle,
            search_plugins: search_widgets.plugins,
            search_preview_stack: search_widgets.preview_stack,
            search_preview_picture: search_widgets.preview_picture,
            search_preview_icon: search_widgets.preview_icon,
            search_preview_title: search_widgets.preview_title,
            search_preview_description: search_widgets.preview_description,
            search_preview_file_meta: search_widgets.preview_file_meta,
            search_preview_meta: search_widgets.preview_meta,
            search_preview_text: search_widgets.preview_text,
            search_preview_text_scroll: search_widgets.preview_text_scroll,
            search_preview_error: search_widgets.preview_error,
            search_preview_actions: search_widgets.preview_actions,
            osd_icon,
            osd_title,
            osd_progress,
            osd_value,
            notification_icon,
            notification_app,
            notification_body,
            weather_location: weather_widgets.location,
            weather_eyebrow: weather_widgets.eyebrow,
            weather_hero_icon: weather_widgets.hero_icon,
            weather_hero_temp: weather_widgets.hero_temp,
            weather_hero_description: weather_widgets.hero_description,
            weather_status: weather_widgets.status,
            weather_forecast_row: weather_widgets.forecast_row,
            weather_icons,
            latest_weather: RefCell::new(None),
            current_view: Cell::new(View::Compact),
            dashboard_open: Cell::new(false),
            search_open: Cell::new(false),
            weather_open: Cell::new(false),
            search_connected: Cell::new(false),
            search_generation: Cell::new(0),
            preview_generation: Cell::new(0),
            search_action_generation: Cell::new(0),
            search_selection_pending: Cell::new(false),
            search_preview_key: RefCell::new(None),
            search_dispatched: RefCell::new(None),
            last_search_dispatch: Cell::new(None),
            search_snapshot: RefCell::new(None),
            search_backend_status: RefCell::new(None),
            osd_active: Cell::new(false),
            media_playing: Cell::new(false),
            media_width: Cell::new(metrics.compact_width),
            geometry: Cell::new(Geometry::for_view(
                View::Compact,
                metrics,
                metrics.compact_width,
                metrics.compact_width,
            )),
            animation_generation: Cell::new(0),
            animation_ms: Cell::new(shell.animation_ms),
            animations_enabled: Cell::new(animations_enabled),
            osd_generation: Cell::new(0),
            volume_generation: Cell::new(0),
            brightness_generation: Cell::new(0),
            updating_controls: Cell::new(false),
            latest_hyprland: RefCell::new(HyprlandSnapshot::default()),
            notifications: config.notifications.clone(),
            notification_queue: RefCell::new(VecDeque::new()),
            notification_current: RefCell::new(None),
            notification_active: Cell::new(false),
            notification_generation: Cell::new(0),
            notification_toasts,
            pill_overlay,
            actions,
        });

        island.connect_interactions(
            OverlayButtons {
                close_button: &dashboard_widgets.close_button,
                search_button: &dashboard_widgets.search_button,
                weather_button: &dashboard_widgets.weather_button,
                search_back_button: &search_widgets.back_button,
                search_reload_button: &search_widgets.reload_button,
                weather_back_button: &weather_widgets.back_button,
            },
            &dismiss_area,
        );
        island.resize_compact();
        island.start_clock();
        island.start_player_progress_timer();
        let weak = Rc::downgrade(&island);
        island.window.connect_realize(move |_| {
            if let Some(island) = weak.upgrade() {
                island.apply_geometry(island.geometry.get());
            }
        });
        island.window.present();
        let weak = Rc::downgrade(&island);
        glib::idle_add_local_once(move || {
            if let Some(island) = weak.upgrade() {
                island.apply_geometry(island.geometry.get());
            }
        });
        island
    }

    pub fn monitor_name(&self) -> &str {
        &self.monitor_name
    }

    pub fn debug_state(&self) -> serde_json::Value {
        let geometry = self.geometry.get();
        serde_json::json!({
            "view": format!("{:?}", self.current_view.get()).to_lowercase(),
            "scale": self.metrics.scale,
            "width": geometry.width.round() as i32,
            "height": geometry.height.round() as i32,
            "y": geometry.y.round() as i32,
            "compact_visible": self.compact.is_visible(),
            "compact_opacity": self.compact.opacity(),
            "media_visible": self.media.is_visible(),
            "media_opacity": self.media.opacity(),
            "media_playing": self.media_playing.get(),
            "media_title": self.media_title.label().to_string(),
            "dashboard_visible": self.dashboard.is_visible(),
            "dashboard_opacity": self.dashboard.opacity(),
            "search_visible": self.search.is_visible(),
            "search_connected": self.search_connected.get(),
            "weather_visible": self.weather.is_visible(),
            "osd_visible": self.osd.is_visible(),
            "osd_opacity": self.osd.opacity(),
            "notification_visible": self.notification.is_visible(),
            "notification_queued": self.notification_queue.borrow().len(),
            "notification_toasts": self
                .notification_toasts
                .as_ref()
                .map_or(0, |toasts| toasts.entries.borrow().len()),
            "tray_item_count": self.tray_item_count.get(),
            "tray_hovered": self.tray_hovered.get(),
            "tray_menu_open": self.tray_menu_open.get(),
            "tray_visible": self.compact_tray.is_visible(),
            "tray_visible_media": self.media_tray.is_visible(),
        })
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.dashboard_open.get() || self.search_open.get() || self.weather_open.get() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(self: &Rc<Self>) {
        self.clear_osd();
        self.search_open.set(false);
        self.weather_open.set(false);
        self.dashboard_open.set(true);
        self.reconcile_view();
    }

    pub fn close(self: &Rc<Self>) {
        self.dashboard_open.set(false);
        self.search_open.set(false);
        self.weather_open.set(false);
        self.search_action_generation
            .set(self.search_action_generation.get().wrapping_add(1));
        self.clear_osd();
        self.reconcile_view();
    }

    /// Switches the island to the weather forecast view. Mirrors
    /// `open_search`'s shape: clears any other overlay flag first so the
    /// views stay mutually exclusive, then hands off to `reconcile_view`.
    pub fn open_weather(self: &Rc<Self>) {
        self.clear_osd();
        self.dashboard_open.set(false);
        self.search_open.set(false);
        self.weather_open.set(true);
        if self.latest_weather.borrow().is_none() {
            self.weather_status.set_label("FETCHING FORECAST");
        }
        self.reconcile_view();
    }

    /// Redraws any active custom Cairo drawing after a theme change --
    /// swapping the CSS provider doesn't trigger that on its own.
    pub fn update_palette(&self) {
        for icon in self.weather_icons.borrow().iter() {
            icon.queue_draw();
        }
    }

    /// Forces a fresh commit; the compositor stops compositing this surface
    /// while the session is locked, so it may not resume until we submit one.
    pub fn recomposite(&self) {
        self.window.queue_draw();
    }

    pub fn update_shell_config(&self, config: &ShellConfig, animations_enabled: bool) {
        self.window
            .set_margin(Edge::Top, self.metrics.spacing(config.top_margin));
        if let Some(overlay) = &self.pill_overlay {
            overlay
                .window
                .set_margin(Edge::Top, self.metrics.spacing(config.top_margin));
        }
        self.window
            .set_exclusive_zone(self.metrics.spacing(config.exclusive_zone));
        self.animation_ms.set(config.animation_ms);
        self.animations_enabled.set(animations_enabled);
    }

    pub fn destroy(&self) {
        self.dismiss_window.close();
        self.window.close();
        if let Some(toasts) = &self.notification_toasts {
            toasts.window.close();
        }
        if let Some(overlay) = &self.pill_overlay {
            overlay.window.close();
        }
    }

    fn start_clock(self: &Rc<Self>) {
        self.update_clock();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_secs(1), move || {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            island.update_clock();
            glib::ControlFlow::Continue
        });
    }

    fn update_clock(&self) {
        if let Ok(now) = glib::DateTime::now_local() {
            if let Ok(time) = now.format("%H:%M") {
                self.compact_clock.set_label(&time);
                self.media_clock.set_label(&time);
                self.hero_time.set_label(&time);
            }
            if let Ok(date) = now.format("%A, %d %B") {
                self.hero_date.set_label(&date);
            }
        }
    }
}
