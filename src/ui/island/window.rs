//! The island window itself: construction, lifecycle, and the public
//! commands the controller invokes.

use super::*;

use std::rc::Rc;

use gtk::{Application, ApplicationWindow, DrawingArea, Fixed, Orientation, Overflow, gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::battery_wave::BatteryWaves;
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
        Self::new_with_layer_shell(
            application,
            monitor,
            monitor_name,
            config,
            actions,
            animations_enabled,
            true,
        )
    }

    #[cfg(test)]
    pub(super) fn new_for_test(
        application: &Application,
        monitor: &gdk::Monitor,
        monitor_name: String,
        config: &AppConfig,
        actions: IslandActions,
        animations_enabled: bool,
    ) -> Rc<Self> {
        let island = Self::new_with_layer_shell(
            application,
            monitor,
            monitor_name,
            config,
            actions,
            animations_enabled,
            false,
        );
        let focus_window = gtk::Window::new();
        // Reparenting must retain the production CSS ancestry, including the
        // runtime density tier used by fonts, padding, and control minima.
        focus_window.add_css_class("mithshell-window");
        if let Some(class) = island.metrics.css_class() {
            focus_window.add_css_class(class);
        }
        island.window.set_child(None::<&gtk::Widget>);
        focus_window.set_child(Some(&island.fixed));
        focus_window.set_default_size(island.metrics.window_width, island.metrics.window_height);
        focus_window.present();
        *island.focus_root.borrow_mut() = focus_window.upcast::<gtk::Widget>();
        island
    }

    fn new_with_layer_shell(
        application: &Application,
        monitor: &gdk::Monitor,
        monitor_name: String,
        config: &AppConfig,
        actions: IslandActions,
        animations_enabled: bool,
        layer_shell: bool,
    ) -> Rc<Self> {
        let shell = &config.shell;
        let metrics = Metrics::new(
            monitor,
            shell.scale,
            config.media.max_width_factor,
            config.icons.style,
        );

        let dismiss_window = ApplicationWindow::builder()
            .application(application)
            .title("mithshell dismiss")
            .decorated(false)
            .build();
        dismiss_window.add_css_class("mithshell-dismiss");
        if layer_shell {
            dismiss_window.init_layer_shell();
        }
        dismiss_window.set_namespace(Some("mithshell-dismiss"));
        // The catcher stays at Top. Views that need it promote the main
        // surface to Overlay before presenting the catcher, so the catcher
        // remains below every interactive main-window surface.
        dismiss_window.set_layer(Layer::Top);
        // The catcher must never acquire seat-wide keyboard focus. Pointer
        // delivery is independent of this layer-shell keyboard setting.
        dismiss_window.set_keyboard_mode(KeyboardMode::None);
        dismiss_window.set_monitor(Some(monitor));
        for edge in [Edge::Top, Edge::Right, Edge::Bottom, Edge::Left] {
            dismiss_window.set_anchor(edge, true);
        }
        dismiss_window.set_exclusive_zone(0);
        let dismiss_area = gtk::Box::new(Orientation::Vertical, 0);
        dismiss_area.set_hexpand(true);
        dismiss_area.set_vexpand(true);
        dismiss_area.set_can_target(true);
        // An all-edge layer-shell window does not derive its GTK child
        // allocation from the monitor geometry.  With only expand flags the
        // child can retain a zero/natural allocation even while the
        // compositor maps the surface over the whole monitor.  That leaves
        // holes in the GTK pick tree (the symptom is an outside click that
        // works only after it happens to land inside the main surface).
        let monitor_geometry = monitor.geometry();
        dismiss_area.set_size_request(
            monitor_geometry.width().max(1),
            monitor_geometry.height().max(1),
        );
        // GTK does not allocate a wl_buffer for an otherwise empty
        // transparent container. A bufferless layer surface can commit
        // forever without ever becoming a compositor pointer target, and
        // GDK consequently has nowhere to send its input region. Keep the
        // catcher visually transparent, but give it a real render node so
        // the Wayland surface gets an attached buffer.
        let dismiss_render = DrawingArea::new();
        dismiss_render.set_hexpand(true);
        dismiss_render.set_vexpand(true);
        dismiss_render.set_draw_func(|_, context, width, height| {
            context.set_operator(gtk::cairo::Operator::Clear);
            context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
            context.fill().expect("clear catcher render surface");
        });
        dismiss_area.append(&dismiss_render);
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
        if layer_shell {
            window.init_layer_shell();
        }
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

        let hover_region = gtk::Box::new(Orientation::Vertical, 0);
        // Motion hot-zone only; it must never win GTK picking over the pill.
        hover_region.set_can_target(false);
        hover_region.set_size_request(
            metrics.media_max_width + metrics.spacing(16),
            metrics.compact_height + metrics.spacing(16),
        );
        fixed.put(
            &hover_region,
            f64::from((metrics.window_width - metrics.media_max_width) / 2 - metrics.spacing(8)),
            0.0,
        );

        let battery_waves = BatteryWaves::new(
            config.battery,
            animations_enabled,
            shell.animation_ms,
            metrics.compact_height,
        );
        let surface_shell = gtk::Overlay::new();
        surface_shell.add_css_class("island-surface");
        surface_shell.set_overflow(Overflow::Hidden);
        surface_shell.set_child(Some(&gtk::Box::new(Orientation::Vertical, 0)));

        let surface = gtk::ScrolledWindow::new();
        surface.add_css_class("island-content-surface");
        surface.set_overflow(Overflow::Hidden);
        surface.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        surface.set_propagate_natural_width(false);
        surface.set_propagate_natural_height(false);
        surface.set_kinetic_scrolling(false);
        surface.set_has_frame(false);
        surface.set_can_target(true);
        surface.set_hexpand(true);
        surface.set_vexpand(true);
        surface_shell.add_overlay(&surface);
        fixed.put(
            &surface_shell,
            f64::from((metrics.window_width - metrics.compact_width) / 2),
            0.0,
        );
        surface_shell.set_size_request(metrics.compact_width, metrics.compact_height);
        surface.set_size_request(metrics.compact_width, metrics.compact_height);

        let content = Fixed::new();
        // The production root picker must descend through the clipped content
        // container to reach workspace/tray controls; it is not a visual
        // overlay and should not terminate picking at GtkFixed.
        content.set_can_target(true);
        content.set_size_request(metrics.window_width, metrics.window_height);
        surface.set_child(Some(&content));

        let (
            compact,
            compact_workspaces,
            compact_clock,
            compact_battery,
            compact_tray,
            compact_date_day,
            compact_date_rest,
            compact_date,
        ) = compact_view(metrics);
        compact.set_halign(gtk::Align::Center);
        compact.set_valign(gtk::Align::Start);
        compact.set_can_target(true);
        compact.set_overflow(Overflow::Hidden);
        compact.set_child(Some(&battery_waves.area));

        let dashboard_widgets = dashboard_view(metrics);
        let panel_scroll = gtk::ScrolledWindow::new();
        panel_scroll.add_css_class("island-panel-viewport");
        // Both axes clip independently of child minima while rolling shut.
        // Never on the horizontal axis forces the hardware's minimum width
        // onto the shrinking viewport and clips away its rounded right border.
        panel_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        panel_scroll.set_overflow(Overflow::Hidden);
        panel_scroll.set_child(Some(&dashboard_widgets.root));
        panel_scroll.set_visible(false);
        surface_shell.add_overlay(&panel_scroll);
        surface_shell.add_overlay(&compact);
        dashboard_widgets.root.set_opacity(0.0);
        dashboard_widgets.root.set_visible(false);

        let search_widgets = search_view(metrics);
        let search_window = ApplicationWindow::builder()
            .application(application)
            .title("mithshell search")
            .decorated(false)
            .resizable(false)
            .default_width(metrics.search_window_width)
            .default_height(metrics.search_window_height)
            .build();
        search_window.add_css_class("mithshell-window");
        if let Some(class) = metrics.css_class() {
            search_window.add_css_class(class);
        }
        if layer_shell {
            search_window.init_layer_shell();
        }
        search_window.set_namespace(Some("mithshell-search"));
        search_window.set_layer(Layer::Top);
        search_window.set_keyboard_mode(KeyboardMode::None);
        search_window.set_monitor(Some(monitor));
        search_window.set_anchor(Edge::Top, true);
        search_window.set_margin(Edge::Top, metrics.spacing(shell.top_margin));
        // Search must share the island's absolute top origin instead of being
        // displaced by the island's own reserved panel zone.
        search_window.set_exclusive_zone(-1);

        let search_fixed = Fixed::new();
        search_fixed.set_size_request(metrics.search_window_width, metrics.search_window_height);
        search_window.set_child(Some(&search_fixed));

        let search_surface = gtk::ScrolledWindow::new();
        search_surface.add_css_class("island-surface");
        search_surface.set_overflow(Overflow::Hidden);
        search_surface.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        search_surface.set_propagate_natural_width(false);
        search_surface.set_propagate_natural_height(false);
        search_surface.set_kinetic_scrolling(false);
        search_surface.set_has_frame(false);
        search_surface.set_child(Some(&search_widgets.root));
        search_fixed.put(&search_surface, 0.0, 0.0);

        let media_widgets = media_view(metrics);
        media_widgets
            .visualizer
            .set_visible(config.media.visualizer);
        let compact_visualizer =
            super::media::visualizer_widget(metrics, media_widgets.levels.clone());
        compact_visualizer.set_margin_start(metrics.spacing(10));
        let compact_visualizer_revealer = gtk::Revealer::new();
        compact_visualizer_revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
        compact_visualizer_revealer.set_transition_duration(if animations_enabled {
            shell.animation_ms
        } else {
            0
        });
        compact_visualizer_revealer.set_child(Some(&compact_visualizer));
        compact_clock
            .parent()
            .and_downcast::<gtk::Box>()
            .expect("compact clock slot")
            .append(&compact_visualizer_revealer);
        content.put(
            &media_widgets.root,
            f64::from((metrics.window_width - metrics.compact_width) / 2),
            0.0,
        );
        media_widgets.root.set_opacity(0.0);
        media_widgets.root.set_visible(false);

        let (osd, osd_icon, osd_title, osd_progress, osd_value) = osd_view(metrics);
        content.put(
            &osd,
            f64::from((metrics.window_width - metrics.osd_width) / 2),
            0.0,
        );
        osd.set_opacity(0.0);
        osd.set_visible(false);

        let (notification, notification_icon, notification_app, notification_body) =
            notification_view(metrics);
        content.put(
            &notification,
            f64::from((metrics.window_width - metrics.notification_width) / 2),
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
            f64::from((metrics.window_width - metrics.weather_width) / 2),
            0.0,
        );
        weather_widgets.root.set_opacity(0.0);
        weather_widgets.root.set_visible(false);
        draw_weather_condition(&weather_widgets.hero_icon, WeatherCondition::Unknown);
        let weather_icons = RefCell::new(vec![weather_widgets.hero_icon.clone()]);

        let tray_menu_manager = tray::TrayMenuManager::new();
        let tray_menu_tracker = tray::TrayMenuTracker::new(tray_menu_manager.clone(), |_| {});
        let island = Rc::new(Self {
            monitor_name,
            metrics,
            window: window.clone(),
            focus_root: RefCell::new(window.clone().upcast::<gtk::Widget>()),
            search_window,
            search_fixed,
            search_surface,
            dismiss_window,
            dismiss_area: dismiss_area.clone(),
            dismiss_click: RefCell::new(None),
            fixed,
            content,
            surface_shell,
            surface,
            compact: compact.upcast(),
            media: media_widgets.root,
            hardware: dashboard_widgets.hardware,
            dashboard: dashboard_widgets.root,
            dashboard_sections: dashboard_widgets.sections,
            dashboard_expansion: Cell::new(0.0),
            dashboard_section_opacity: Cell::new(0.0),
            panel_scroll,
            search: search_widgets.root,
            weather: weather_widgets.root,
            osd,
            notification,
            compact_workspaces,
            compact_clock,
            compact_date_day,
            compact_date_rest,
            compact_date,
            compact_battery,
            battery_waves,
            compact_tray,
            compact_width: Cell::new(metrics.compact_width),
            tray_hovered: Cell::new(false),
            pointer_in_hover_region: Cell::new(false),
            tray_item_count: Cell::new(0),
            tray_circle_assigned: config.circles.contains(crate::config::CircleModule::Tray),
            tray_menu_open: Cell::new(false),
            tray_menu_manager,
            tray_menu_tracker,
            media_workspaces: media_widgets.workspaces,
            media_clock: media_widgets.clock,
            media_center: media_widgets.center,
            media_icon: media_widgets.icon,
            media_title: media_widgets.title,
            media_visualizer: media_widgets.visualizer,
            compact_visualizer,
            compact_visualizer_revealer,
            visualizer_enabled: config.media.visualizer,
            visualizer_active: Cell::new(false),
            visualizer_revision: Cell::new(0),
            media_levels: media_widgets.levels,
            media_tray: media_widgets.tray,
            latest_media: RefCell::new(None),
            selected_media_service: RefCell::new(None),
            active_eyebrow: dashboard_widgets.active_eyebrow,
            active_title: dashboard_widgets.active_title,
            workspace_row: dashboard_widgets.workspace_row,
            volume_scale: dashboard_widgets.volume_scale,
            volume_value: dashboard_widgets.volume_value,
            mute_button: dashboard_widgets.mute_button,
            notification_count: dashboard_widgets.notification_count,
            notification_inhibit_remaining: dashboard_widgets.notification_inhibit_remaining,
            notification_clear_button: dashboard_widgets.notification_clear_button,
            notification_inhibit_button: dashboard_widgets.notification_inhibit_button,
            notification_list: dashboard_widgets.notification_list,
            updating_notification_inhibit: Cell::new(false),
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
            island_hovered: Cell::new(false),
            search_open: Cell::new(false),
            weather_open: Cell::new(false),
            search_connected: Cell::new(false),
            search_generation: Cell::new(0),
            search_focus_generation: Cell::new(0),
            search_focus_pending: Cell::new(false),
            preview_generation: Cell::new(0),
            search_action_generation: Cell::new(0),
            search_selection_pending: Cell::new(false),
            search_selection_keep_open: Cell::new(false),
            search_preview_key: RefCell::new(None),
            search_dispatched: RefCell::new(None),
            last_search_dispatch: Cell::new(None),
            search_snapshot: RefCell::new(None),
            search_backend_status: RefCell::new(None),
            search_geometry: Cell::new(Geometry::for_view(
                View::Compact,
                metrics,
                metrics.compact_width,
                metrics.compact_width,
            )),
            search_animation_generation: Cell::new(0),
            osd_active: Cell::new(false),
            media_playing: Cell::new(false),
            media_width: Cell::new(metrics.compact_width),
            geometry: Cell::new(Geometry::for_view(
                View::Compact,
                metrics,
                metrics.compact_width,
                metrics.compact_width,
            )),
            view_animation_generation: Cell::new(0),
            pill_animation_generation: Cell::new(0),
            pill_animation_target: Cell::new(None),
            view_animation_target: Cell::new(None),
            view_transition_active: Cell::new(false),
            animation_ms: Cell::new(shell.animation_ms),
            animations_enabled: Cell::new(animations_enabled),
            launcher_presentation: config.launcher.presentation,
            osd_generation: Cell::new(0),
            volume_generation: Cell::new(0),
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
            circles: RefCell::new(None),
        });

        let circles = circle_integration::CircleIntegration::new(
            &island,
            config.circles.left,
            config.circles.right,
            &config.tray,
            &config.notifications,
        )
        .expect("valid circle configuration");
        island.circles.replace(Some(circles));

        let weak = Rc::downgrade(&island);
        island.tray_menu_manager.set_on_change(move |open| {
            if let Some(island) = weak.upgrade() {
                island.tray_menu_open.set(open);
                island.refresh_keyboard_mode();
                island.resize_compact();
                island.resize_media();
            }
        });

        island.connect_interactions(
            OverlayButtons {
                close_button: &dashboard_widgets.close_button,
                search_button: &dashboard_widgets.search_button,
                weather_button: &dashboard_widgets.weather_button,
                mute_button: &island.mute_button,
                search_back_button: &search_widgets.back_button,
                search_reload_button: &search_widgets.reload_button,
                weather_back_button: &weather_widgets.back_button,
            },
            &dismiss_area,
        );
        island.resize_compact();
        island.reconcile_pill_geometry();
        island.relayout_circles();
        island.start_clock();
        let weak = Rc::downgrade(&island);
        island.window.connect_realize(move |_| {
            if let Some(island) = weak.upgrade() {
                island.apply_geometry(island.geometry.get());
            }
        });
        island.window.present();
        // Re-assert the catcher allocation after layer-shell has negotiated
        // the mapped surface size.  The explicit input region is important
        // for compositors that otherwise retain the pre-negotiation GTK
        // input shape.
        let weak = Rc::downgrade(&island);
        island.dismiss_window.connect_realize(move |_| {
            if let Some(island) = weak.upgrade() {
                island.refresh_dismiss_input_region();
            }
        });
        if trace_catcher() {
            let weak = Rc::downgrade(&island);
            island.dismiss_window.connect_realize(move |window| {
                if let Some(clock) = window.frame_clock() {
                    let weak = weak.clone();
                    clock.connect_after_paint(move |_| {
                        if weak.upgrade().is_some() {
                            log::info!(
                                "dismiss catcher after-paint; region request reached GDK frame"
                            );
                        }
                    });
                }
            });
        }
        // Layer-shell negotiation can complete after `realize`. Re-send the
        // region after map and once more from idle so the post-map wl_surface
        // commit cannot retain the initial empty input shape.
        let weak = Rc::downgrade(&island);
        island.dismiss_window.connect_map(move |_| {
            if let Some(island) = weak.upgrade() {
                island.refresh_dismiss_input_region();
                let weak = Rc::downgrade(&island);
                glib::idle_add_local_once(move || {
                    if let Some(island) = weak.upgrade() {
                        island.refresh_dismiss_input_region();
                    }
                });
            }
        });
        let weak = Rc::downgrade(&island);
        glib::idle_add_local_once(move || {
            if let Some(island) = weak.upgrade() {
                island.apply_geometry(island.geometry.get());
            }
        });
        let weak = Rc::downgrade(&island);
        island.search_window.connect_realize(move |_| {
            if let Some(island) = weak.upgrade() {
                island.apply_search_geometry(island.search_geometry.get());
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
            "search_visible": self.search_window.is_visible(),
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
            "pointer_in_hover_region": self.pointer_in_hover_region.get(),
            "tray_menu_open": self.tray_menu_open.get(),
            "tray_visible": self.compact_tray.is_visible(),
            "tray_visible_media": self.media_tray.is_visible(),
            "notification_history_visible": self.notification_list.is_visible(),
            "circles": self.circle_debug_state(),
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
        self.dismiss_search_window(self.geometry_for_view(self.current_view.get()));
    }

    pub fn close(self: &Rc<Self>) {
        if self.current_view.get() == View::Dashboard {
            self.island_hovered.set(self.pointer_in_hover_region.get());
        }
        self.dashboard_open.set(false);
        self.weather_open.set(false);
        self.search_action_generation
            .set(self.search_action_generation.get().wrapping_add(1));
        self.clear_osd();
        self.search_open.set(false);
        self.reconcile_view();
        self.dismiss_search_window(self.geometry_for_view(self.current_view.get()));
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
        self.dismiss_search_window(self.geometry_for_view(self.current_view.get()));
    }

    /// Redraws any active custom Cairo drawing after a theme change --
    /// swapping the CSS provider doesn't trigger that on its own.
    pub fn update_palette(&self) {
        self.battery_waves.queue_draw();
        for icon in self.weather_icons.borrow().iter() {
            icon.queue_draw();
        }
        if let Some(circles) = self.circles.borrow().as_ref() {
            circles.redraw_theme();
        }
    }

    /// Forces a fresh commit; the compositor stops compositing this surface
    /// while the session is locked, so it may not resume until we submit one.
    pub fn recomposite(&self) {
        self.window.queue_draw();
        self.search_window.queue_draw();
    }

    pub fn update_shell_config(&self, config: &ShellConfig, animations_enabled: bool) {
        self.window
            .set_margin(Edge::Top, self.metrics.spacing(config.top_margin));
        self.search_window
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
        self.battery_waves
            .set_motion(animations_enabled, config.animation_ms);
    }

    pub fn destroy(&self) {
        #[cfg(test)]
        if let Ok(window) = self.focus_root.borrow().clone().downcast::<gtk::Window>() {
            window.close();
        }
        self.dismiss_window.close();
        self.search_window.close();
        self.window.close();
        if let Some(toasts) = &self.notification_toasts {
            toasts.window.close();
        }
        if let Some(overlay) = &self.pill_overlay {
            overlay.window.close();
        }
    }

    /// Establish the layer/stack invariant needed by outside dismissal:
    /// `dismiss_window` is a full-screen Top-layer catcher and the interactive
    /// main window must be Overlay while the catcher is mapped. This is also
    /// needed for full notification circles, whose normal compact view stays
    /// on Top rather than entering the dashboard view path.
    pub(super) fn promote_main_above_dismiss_catcher(&self) {
        self.window.set_layer(Layer::Overlay);
        // Re-presenting is intentional: an already-visible catcher may have
        // been shown before the main layer changed. Presenting the main window
        // again re-establishes ordering without changing keyboard policy.
        self.window.present();
    }

    /// Keep the compositor input shape synchronized with the actual GTK
    /// allocation. The layer-shell protocol may negotiate the window size
    /// after the child has first been realized.
    #[allow(deprecated)]
    pub(super) fn refresh_dismiss_input_region(&self) {
        let Some(surface) = self.dismiss_window.surface() else {
            return;
        };
        let width = self.dismiss_area.allocated_width();
        let height = self.dismiss_area.allocated_height();
        if trace_catcher() {
            log::info!(
                "dismiss catcher input region commit mapped={} window={}x{} child={}x{}",
                self.dismiss_window.is_mapped(),
                self.dismiss_window.width(),
                self.dismiss_window.height(),
                width,
                height
            );
        }
        let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
            0,
            0,
            width.max(1),
            height.max(1),
        ));
        surface.set_input_region(Some(&region));
        // The region is transmitted with the next wl_surface commit. The
        // catcher has no normal visual changes, so request that commit.
        self.dismiss_window.queue_draw();
    }

    pub(super) fn restore_main_layer_after_full_circle(&self) {
        let layer = desired_main_layer(
            self.current_view.get(),
            self.launcher_presentation == crate::config::LauncherPresentation::Independent
                && self.search_window.is_visible(),
        );
        self.window.set_layer(layer);
        self.window.present();
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
            }
            if let Ok(day) = now.format("%a") {
                self.compact_date_day.set_label(&day);
            }
            if let Ok(rest) = now.format("%-d %b") {
                self.compact_date_rest.set_label(&rest);
            }
        }
    }
}

pub(super) fn trace_catcher() -> bool {
    std::env::var_os("MITHSHELL_TRACE_CATCHER").is_some()
}

fn desired_main_layer(view: View, independent_search_visible: bool) -> Layer {
    if independent_search_visible || matches!(view, View::Dashboard | View::Weather | View::Search)
    {
        Layer::Overlay
    } else {
        Layer::Top
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    #[test]
    fn full_circle_restores_top_only_without_another_overlay_owner() {
        assert_eq!(desired_main_layer(View::Compact, false), Layer::Top);
        assert_eq!(desired_main_layer(View::Media, false), Layer::Top);
        assert_eq!(desired_main_layer(View::Compact, true), Layer::Overlay);
        assert_eq!(desired_main_layer(View::Dashboard, false), Layer::Overlay);
        assert_eq!(desired_main_layer(View::Weather, false), Layer::Overlay);
        assert_eq!(desired_main_layer(View::Search, false), Layer::Overlay);
    }
}
