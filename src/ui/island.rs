use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gtk::{
    Align, Application, ApplicationWindow, Fixed, GestureClick, Orientation, Overflow,
    gdk::{self, prelude::*},
    glib,
    prelude::*,
};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::{
    config::{AppConfig, ShellConfig},
    ipc::OsdKind,
    media::{VISUALIZER_BARS, VisualizerLevels},
    state::{HyprlandSnapshot, MediaState, OsdState, Palette, SystemSnapshot},
};

const WINDOW_WIDTH: i32 = 480;
const WINDOW_HEIGHT: i32 = 420;
const COMPACT_WIDTH: i32 = 224;
const COMPACT_HEIGHT: i32 = 40;
const MEDIA_HEIGHT: i32 = 40;
const DASHBOARD_WIDTH: i32 = 440;
const DASHBOARD_HEIGHT: i32 = 370;
const OSD_WIDTH: i32 = 292;
const OSD_HEIGHT: i32 = 44;

#[derive(Debug, Clone, Copy)]
struct Metrics {
    scale: f64,
    window_width: i32,
    window_height: i32,
    compact_width: i32,
    compact_height: i32,
    media_max_width: i32,
    media_height: i32,
    dashboard_width: i32,
    dashboard_height: i32,
    osd_width: i32,
    osd_height: i32,
}

impl Metrics {
    fn new(monitor: &gdk::Monitor, configured_scale: f64, media_width_factor: f64) -> Self {
        let automatic = (f64::from(monitor.geometry().width()) / 2560.0).clamp(1.0, 1.45);
        let scale = if configured_scale > 0.0 {
            configured_scale.clamp(0.8, 1.5)
        } else {
            automatic
        };
        let media_width_factor =
            media_width_factor.clamp(1.0, f64::from(DASHBOARD_WIDTH) / f64::from(COMPACT_WIDTH));
        Self {
            scale,
            window_width: scaled(WINDOW_WIDTH, scale),
            window_height: scaled(WINDOW_HEIGHT, scale),
            compact_width: scaled(COMPACT_WIDTH, scale),
            compact_height: scaled(COMPACT_HEIGHT, scale),
            media_max_width: (f64::from(COMPACT_WIDTH) * media_width_factor * scale).round() as i32,
            media_height: scaled(MEDIA_HEIGHT, scale),
            dashboard_width: scaled(DASHBOARD_WIDTH, scale),
            dashboard_height: scaled(DASHBOARD_HEIGHT, scale),
            osd_width: scaled(OSD_WIDTH, scale),
            osd_height: scaled(OSD_HEIGHT, scale),
        }
    }

    fn spacing(self, value: i32) -> i32 {
        scaled(value, self.scale)
    }

    fn css_class(self) -> Option<&'static str> {
        if self.scale >= 1.35 {
            Some("scale-large")
        } else if self.scale >= 1.12 {
            Some("scale-medium")
        } else {
            None
        }
    }
}

pub type WorkspaceAction = Rc<dyn Fn(&str, i64)>;
pub type ValueAction = Rc<dyn Fn(u8)>;

#[derive(Clone)]
pub struct IslandActions {
    pub switch_workspace: WorkspaceAction,
    pub set_volume: ValueAction,
    pub set_brightness: ValueAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Compact,
    Media,
    Dashboard,
    Osd,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Geometry {
    width: f64,
    height: f64,
}

impl Geometry {
    fn for_view(view: View, metrics: Metrics, media_width: i32) -> Self {
        match view {
            View::Compact => Self {
                width: f64::from(metrics.compact_width),
                height: f64::from(metrics.compact_height),
            },
            View::Media => Self {
                width: f64::from(media_width),
                height: f64::from(metrics.media_height),
            },
            View::Dashboard => Self {
                width: f64::from(metrics.dashboard_width),
                height: f64::from(metrics.dashboard_height),
            },
            View::Osd => Self {
                width: f64::from(metrics.osd_width),
                height: f64::from(metrics.osd_height),
            },
        }
    }

    fn interpolate(self, target: Self, progress: f64) -> Self {
        Self {
            width: self.width + (target.width - self.width) * progress,
            height: self.height + (target.height - self.height) * progress,
        }
    }
}

pub struct IslandWindow {
    monitor_name: String,
    metrics: Metrics,
    window: ApplicationWindow,
    dismiss_window: ApplicationWindow,
    fixed: Fixed,
    content: Fixed,
    surface: gtk::ScrolledWindow,
    compact: gtk::Box,
    media: gtk::CenterBox,
    dashboard: gtk::Box,
    osd: gtk::Box,
    compact_workspaces: gtk::Box,
    compact_clock: gtk::Label,
    compact_battery: gtk::Label,
    media_workspaces: gtk::Box,
    media_clock: gtk::Label,
    media_icon: gtk::Image,
    media_title: gtk::Label,
    media_visualizer: gtk::DrawingArea,
    media_levels: Rc<RefCell<VisualizerLevels>>,
    hero_time: gtk::Label,
    hero_date: gtk::Label,
    battery_chip: gtk::Box,
    battery_label: gtk::Label,
    active_eyebrow: gtk::Label,
    active_title: gtk::Label,
    workspace_row: gtk::Box,
    volume_scale: gtk::Scale,
    volume_value: gtk::Label,
    brightness_row: gtk::Box,
    brightness_scale: gtk::Scale,
    brightness_value: gtk::Label,
    theme_source: gtk::Label,
    osd_icon: gtk::Image,
    osd_title: gtk::Label,
    osd_progress: gtk::ProgressBar,
    osd_value: gtk::Label,
    current_view: Cell<View>,
    dashboard_open: Cell<bool>,
    osd_active: Cell<bool>,
    media_playing: Cell<bool>,
    media_width: Cell<i32>,
    geometry: Cell<Geometry>,
    animation_generation: Cell<u64>,
    animation_ms: Cell<u32>,
    animations_enabled: Cell<bool>,
    osd_generation: Cell<u64>,
    volume_generation: Cell<u64>,
    brightness_generation: Cell<u64>,
    updating_controls: Cell<bool>,
    latest_hyprland: RefCell<HyprlandSnapshot>,
    actions: IslandActions,
}

impl IslandWindow {
    pub fn new(
        application: &Application,
        monitor: &gdk::Monitor,
        monitor_name: String,
        config: &AppConfig,
        palette: &Palette,
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
        window.set_layer(Layer::Bottom);
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
        content.set_size_request(metrics.dashboard_width, metrics.dashboard_height);
        surface.set_child(Some(&content));

        let (compact, compact_workspaces, compact_clock, compact_battery) = compact_view(metrics);
        content.put(
            &compact,
            f64::from((metrics.dashboard_width - metrics.compact_width) / 2),
            0.0,
        );

        let dashboard_widgets = dashboard_view(palette, metrics);
        content.put(&dashboard_widgets.root, 0.0, 0.0);
        dashboard_widgets.root.set_opacity(0.0);
        dashboard_widgets.root.set_visible(false);

        let media_widgets = media_view(metrics);
        content.put(
            &media_widgets.root,
            f64::from((metrics.dashboard_width - metrics.compact_width) / 2),
            0.0,
        );
        media_widgets.root.set_opacity(0.0);
        media_widgets.root.set_visible(false);

        let (osd, osd_icon, osd_title, osd_progress, osd_value) = osd_view(metrics);
        content.put(
            &osd,
            f64::from((metrics.dashboard_width - metrics.osd_width) / 2),
            0.0,
        );
        osd.set_opacity(0.0);
        osd.set_visible(false);

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
            osd,
            compact_workspaces,
            compact_clock,
            compact_battery,
            media_workspaces: media_widgets.workspaces,
            media_clock: media_widgets.clock,
            media_icon: media_widgets.icon,
            media_title: media_widgets.title,
            media_visualizer: media_widgets.visualizer,
            media_levels: media_widgets.levels,
            hero_time: dashboard_widgets.hero_time,
            hero_date: dashboard_widgets.hero_date,
            battery_chip: dashboard_widgets.battery_chip,
            battery_label: dashboard_widgets.battery_label,
            active_eyebrow: dashboard_widgets.active_eyebrow,
            active_title: dashboard_widgets.active_title,
            workspace_row: dashboard_widgets.workspace_row,
            volume_scale: dashboard_widgets.volume_scale,
            volume_value: dashboard_widgets.volume_value,
            brightness_row: dashboard_widgets.brightness_row,
            brightness_scale: dashboard_widgets.brightness_scale,
            brightness_value: dashboard_widgets.brightness_value,
            theme_source: dashboard_widgets.theme_source,
            osd_icon,
            osd_title,
            osd_progress,
            osd_value,
            current_view: Cell::new(View::Compact),
            dashboard_open: Cell::new(false),
            osd_active: Cell::new(false),
            media_playing: Cell::new(false),
            media_width: Cell::new(metrics.compact_width),
            geometry: Cell::new(Geometry::for_view(
                View::Compact,
                metrics,
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
            actions,
        });

        island.connect_interactions(&dashboard_widgets.close_button, &dismiss_area);
        island.start_clock();
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
            "compact_visible": self.compact.is_visible(),
            "compact_opacity": self.compact.opacity(),
            "media_visible": self.media.is_visible(),
            "media_opacity": self.media.opacity(),
            "media_playing": self.media_playing.get(),
            "media_title": self.media_title.label().to_string(),
            "dashboard_visible": self.dashboard.is_visible(),
            "dashboard_opacity": self.dashboard.opacity(),
            "osd_visible": self.osd.is_visible(),
            "osd_opacity": self.osd.opacity(),
        })
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.dashboard_open.get() {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn open(self: &Rc<Self>) {
        self.clear_osd();
        self.dashboard_open.set(true);
        self.reconcile_view();
    }

    pub fn close(self: &Rc<Self>) {
        self.dashboard_open.set(false);
        self.clear_osd();
        self.reconcile_view();
    }

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

    pub fn update_media(self: &Rc<Self>, state: Option<&MediaState>) {
        if let Some(state) = state {
            self.media_title.set_label(&state.title);
            self.media_title
                .set_tooltip_text(Some(&format!("{} ({})", state.title, state.player)));
            self.media_icon.set_icon_name(state.app_icon.as_deref());
            self.media_icon.set_visible(state.app_icon.is_some());
            self.media_icon
                .set_tooltip_text(Some(&state.player.replace('.', " ")));
            self.media_playing.set(true);
        } else {
            self.media_title.set_label("");
            self.media_title.set_tooltip_text(None);
            self.media_icon.set_visible(false);
            self.media_playing.set(false);
        }
        self.reconcile_view();
        if state.is_some() {
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(island) = weak.upgrade()
                    && island.media_playing.get()
                {
                    island.resize_media();
                    island.reconcile_view();
                }
            });
        }
    }

    pub fn update_visualizer(&self, levels: VisualizerLevels) {
        *self.media_levels.borrow_mut() = levels;
        self.media_visualizer.queue_draw();
    }

    pub fn update_hyprland(self: &Rc<Self>, snapshot: &HyprlandSnapshot) {
        *self.latest_hyprland.borrow_mut() = snapshot.clone();
        let monitor = snapshot.monitor(&self.monitor_name);
        let active_workspace = monitor.map(|monitor| monitor.active_workspace.id);
        self.active_eyebrow.set_label(&format!(
            "{}  //  WORKSPACE {}",
            self.monitor_name,
            active_workspace
                .map(|id| id.to_string())
                .unwrap_or_else(|| "--".into())
        ));

        let active_window = snapshot
            .active_window
            .as_ref()
            .filter(|window| monitor.is_some_and(|monitor| window.monitor == monitor.id));
        self.active_title.set_label(
            active_window
                .map(|window| window.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or("Quiet desktop"),
        );

        clear_box(&self.compact_workspaces);
        clear_box(&self.media_workspaces);
        clear_box(&self.workspace_row);
        let workspaces = snapshot.workspaces_for(&self.monitor_name);
        for workspace in workspaces
            .iter()
            .filter(|workspace| workspace.windows > 0 || Some(workspace.id) == active_workspace)
            .take(7)
        {
            for container in [&self.compact_workspaces, &self.media_workspaces] {
                let dot = gtk::Button::new();
                dot.add_css_class("workspace-dot");
                dot.set_tooltip_text(Some(&format!("Workspace {}", workspace.name)));
                if workspace.windows > 0 {
                    dot.add_css_class("occupied");
                }
                if Some(workspace.id) == active_workspace {
                    dot.add_css_class("active");
                }
                let actions = self.actions.clone();
                let monitor_name = self.monitor_name.clone();
                let workspace_id = workspace.id;
                dot.connect_clicked(move |_| {
                    (actions.switch_workspace)(&monitor_name, workspace_id)
                });
                container.append(&dot);
            }
        }

        for workspace in workspaces.into_iter().take(10) {
            let button = gtk::Button::with_label(&workspace.name);
            button.add_css_class("workspace-button");
            if workspace.windows > 0 {
                button.add_css_class("occupied");
            }
            if Some(workspace.id) == active_workspace {
                button.add_css_class("active");
            }
            let actions = self.actions.clone();
            let monitor_name = self.monitor_name.clone();
            let workspace_id = workspace.id;
            button
                .connect_clicked(move |_| (actions.switch_workspace)(&monitor_name, workspace_id));
            self.workspace_row.append(&button);
        }
    }

    pub fn update_system(&self, snapshot: &SystemSnapshot) {
        self.updating_controls.set(true);
        if let Some(audio) = snapshot.audio {
            self.volume_scale.set_value(f64::from(audio.percent));
            self.volume_value.set_label(&format!("{}%", audio.percent));
            self.volume_scale.set_sensitive(true);
        } else {
            self.volume_value.set_label("--");
            self.volume_scale.set_sensitive(false);
        }

        if let Some(brightness) = &snapshot.brightness {
            self.brightness_scale
                .set_value(f64::from(brightness.percent));
            self.brightness_value
                .set_label(&format!("{}%", brightness.percent));
            self.brightness_row.remove_css_class("unavailable");
            self.brightness_scale.set_sensitive(true);
        } else {
            self.brightness_value.set_label("--");
            self.brightness_row.add_css_class("unavailable");
            self.brightness_scale.set_sensitive(false);
        }

        if let Some(battery) = &snapshot.battery {
            self.battery_label
                .set_label(&format!("{}%", battery.percent));
            self.compact_battery
                .set_label(&format!("{}%", battery.percent));
            self.battery_chip.set_visible(true);
            self.compact_battery.set_visible(true);
        } else {
            self.battery_chip.set_visible(false);
            self.compact_battery.set_visible(false);
        }
        self.updating_controls.set(false);
    }

    pub fn update_palette(&self, source: &str) {
        self.theme_source.set_label(source);
    }

    pub fn update_shell_config(&self, config: &ShellConfig, animations_enabled: bool) {
        self.window
            .set_margin(Edge::Top, self.metrics.spacing(config.top_margin));
        self.window
            .set_exclusive_zone(self.metrics.spacing(config.exclusive_zone));
        self.animation_ms.set(config.animation_ms);
        self.animations_enabled.set(animations_enabled);
    }

    pub fn destroy(&self) {
        self.dismiss_window.close();
        self.window.close();
    }

    fn connect_interactions(self: &Rc<Self>, close_button: &gtk::Button, dismiss_area: &gtk::Box) {
        let click = GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, _, _| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
            {
                island.toggle();
            }
        });
        self.compact.add_controller(click);

        let click = GestureClick::new();
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, _, _| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
            {
                island.toggle();
            }
        });
        self.media.add_controller(click);

        let dismiss_click = GestureClick::new();
        let weak = Rc::downgrade(self);
        dismiss_click.connect_released(move |gesture, _, _, _| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
            {
                island.close();
            }
        });
        dismiss_area.add_controller(dismiss_click);

        let header_click = GestureClick::new();
        header_click.set_propagation_phase(gtk::PropagationPhase::Bubble);
        let weak = Rc::downgrade(self);
        header_click.connect_released(move |gesture, _, _, y| {
            if gesture.current_button() == 1
                && let Some(island) = weak.upgrade()
                && y <= f64::from(island.metrics.spacing(78))
            {
                island.close();
            }
        });
        self.dashboard.add_controller(header_click);

        let weak = Rc::downgrade(self);
        close_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.close();
            }
        });

        let weak = Rc::downgrade(self);
        self.volume_scale.connect_value_changed(move |scale| {
            if let Some(island) = weak.upgrade()
                && !island.updating_controls.get()
            {
                let value = scale.value().round().clamp(0.0, 100.0) as u8;
                island.volume_value.set_label(&format!("{value}%"));
                let generation = island.volume_generation.get().wrapping_add(1);
                island.volume_generation.set(generation);
                let weak = Rc::downgrade(&island);
                glib::timeout_add_local_once(Duration::from_millis(70), move || {
                    if let Some(island) = weak.upgrade()
                        && island.volume_generation.get() == generation
                    {
                        (island.actions.set_volume)(value);
                    }
                });
            }
        });

        let weak = Rc::downgrade(self);
        self.brightness_scale.connect_value_changed(move |scale| {
            if let Some(island) = weak.upgrade()
                && !island.updating_controls.get()
            {
                let value = scale.value().round().clamp(0.0, 100.0) as u8;
                island.brightness_value.set_label(&format!("{value}%"));
                let generation = island.brightness_generation.get().wrapping_add(1);
                island.brightness_generation.set(generation);
                let weak = Rc::downgrade(&island);
                glib::timeout_add_local_once(Duration::from_millis(70), move || {
                    if let Some(island) = weak.upgrade()
                        && island.brightness_generation.get() == generation
                    {
                        (island.actions.set_brightness)(value);
                    }
                });
            }
        });
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

    fn clear_osd(&self) {
        self.osd_active.set(false);
        self.osd_generation
            .set(self.osd_generation.get().wrapping_add(1));
    }

    fn reconcile_view(self: &Rc<Self>) {
        let view = if self.osd_active.get() {
            View::Osd
        } else if self.dashboard_open.get() {
            View::Dashboard
        } else if self.media_playing.get() {
            View::Media
        } else {
            View::Compact
        };
        self.set_view(view);
    }

    fn resize_media(&self) {
        self.media.set_width_request(-1);
        self.media_title
            .set_ellipsize(gtk::pango::EllipsizeMode::None);
        let (_, title_width, _, _) = self.media_title.measure(Orientation::Horizontal, -1);
        self.media_title
            .set_ellipsize(gtk::pango::EllipsizeMode::End);
        let (_, workspace_width, _, _) = self.media_workspaces.measure(Orientation::Horizontal, -1);
        let (_, clock_width, _, _) = self.media_clock.measure(Orientation::Horizontal, -1);
        let (_, visualizer_width, _, _) =
            self.media_visualizer.measure(Orientation::Horizontal, -1);
        let icon_width = if self.media_icon.is_visible() {
            self.media_icon.measure(Orientation::Horizontal, -1).1
        } else {
            0
        };
        let center_gaps = self.metrics.spacing(if icon_width > 0 { 14 } else { 7 });
        let natural = self.metrics.spacing(36)
            + workspace_width.max(clock_width) * 2
            + icon_width
            + visualizer_width
            + center_gaps
            + title_width;
        let width = natural.clamp(self.metrics.compact_width, self.metrics.media_max_width);
        self.media
            .set_size_request(width, self.metrics.media_height);
        self.content.move_(
            &self.media,
            f64::from((self.metrics.dashboard_width - width) / 2),
            0.0,
        );
        self.media_width.set(width);
    }

    fn geometry_for_view(&self, view: View) -> Geometry {
        Geometry::for_view(view, self.metrics, self.media_width.get())
    }

    fn set_view(self: &Rc<Self>, view: View) {
        let target = self.geometry_for_view(view);
        if self.current_view.get() == view && self.geometry.get() == target {
            return;
        }
        self.current_view.set(view);
        if view == View::Dashboard {
            self.window.set_layer(Layer::Top);
            self.dismiss_window.present();
            self.window.present();
        } else {
            self.window.set_layer(Layer::Bottom);
            self.dismiss_window.set_visible(false);
        }

        self.compact.set_visible(true);
        self.media.set_visible(true);
        self.dashboard.set_visible(true);
        self.osd.set_visible(true);
        self.compact.set_can_target(view == View::Compact);
        self.media.set_can_target(view == View::Media);
        self.dashboard.set_can_target(view == View::Dashboard);
        self.osd.set_can_target(false);
        let start = self.geometry.get();
        let generation = self.animation_generation.get().wrapping_add(1);
        self.animation_generation.set(generation);
        if !self.animations_enabled.get() || self.animation_ms.get() == 0 {
            self.apply_geometry(target);
            self.finish_view(view);
            return;
        }

        let duration_us = i64::from(self.animation_ms.get()) * 1000;
        let start_time = Cell::new(None::<i64>);
        let start_compact_opacity = self.compact.opacity();
        let start_media_opacity = self.media.opacity();
        let start_dashboard_opacity = self.dashboard.opacity();
        let start_osd_opacity = self.osd.opacity();
        let weak = Rc::downgrade(self);
        self.surface.add_tick_callback(move |_, frame_clock| {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if island.animation_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let now = frame_clock.frame_time();
            let started = if let Some(started) = start_time.get() {
                started
            } else {
                start_time.set(Some(now));
                now
            };
            let linear = ((now - started) as f64 / duration_us as f64).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - linear).powi(5);
            island.apply_geometry(start.interpolate(target, eased));
            island.apply_content_opacity(
                view,
                linear,
                start_compact_opacity,
                start_media_opacity,
                start_dashboard_opacity,
                start_osd_opacity,
            );
            if linear >= 1.0 {
                island.finish_view(view);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn apply_content_opacity(
        &self,
        target: View,
        progress: f64,
        compact_start: f64,
        media_start: f64,
        dashboard_start: f64,
        osd_start: f64,
    ) {
        let progress = 1.0 - (1.0 - progress).powi(3);
        let compact_target = if target == View::Compact { 1.0 } else { 0.0 };
        let media_target = if target == View::Media { 1.0 } else { 0.0 };
        let dashboard_target = if target == View::Dashboard { 1.0 } else { 0.0 };
        let osd_target = if target == View::Osd { 1.0 } else { 0.0 };
        self.compact
            .set_opacity(lerp(compact_start, compact_target, progress));
        self.media
            .set_opacity(lerp(media_start, media_target, progress));
        self.dashboard
            .set_opacity(lerp(dashboard_start, dashboard_target, progress));
        self.osd.set_opacity(lerp(osd_start, osd_target, progress));
    }

    fn finish_view(&self, view: View) {
        self.apply_geometry(self.geometry_for_view(view));
        self.compact.set_visible(view == View::Compact);
        self.media.set_visible(view == View::Media);
        self.dashboard.set_visible(view == View::Dashboard);
        self.osd.set_visible(view == View::Osd);
        self.compact
            .set_opacity(if view == View::Compact { 1.0 } else { 0.0 });
        self.media
            .set_opacity(if view == View::Media { 1.0 } else { 0.0 });
        self.dashboard
            .set_opacity(if view == View::Dashboard { 1.0 } else { 0.0 });
        self.osd
            .set_opacity(if view == View::Osd { 1.0 } else { 0.0 });
    }

    fn apply_geometry(&self, geometry: Geometry) {
        self.geometry.set(geometry);
        let width = geometry.width.round() as i32;
        let height = geometry.height.round() as i32;
        let x = (self.metrics.window_width - width) / 2;
        self.surface.set_size_request(width, height);
        self.fixed.move_(&self.surface, f64::from(x), 0.0);
        self.surface
            .hadjustment()
            .set_value(f64::from((self.metrics.dashboard_width - width) / 2));
        self.surface.vadjustment().set_value(0.0);
        if let Some(surface) = self.window.surface() {
            let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                x, 0, width, height,
            ));
            surface.set_input_region(Some(&region));
        }
    }
}

fn compact_view(metrics: Metrics) -> (gtk::Box, gtk::Box, gtk::Label, gtk::Label) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    root.set_size_request(metrics.compact_width, metrics.compact_height);
    root.add_css_class("compact-content");
    root.set_valign(Align::Start);

    let workspaces = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    workspaces.set_hexpand(false);
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);

    let clock = gtk::Label::new(Some("--:--"));
    clock.add_css_class("compact-clock");
    clock.set_hexpand(true);
    clock.set_halign(Align::Center);
    clock.set_valign(Align::Center);

    let battery = gtk::Label::new(None);
    battery.add_css_class("compact-battery");
    battery.set_hexpand(false);
    battery.set_halign(Align::End);
    battery.set_valign(Align::Center);
    battery.set_visible(false);

    root.append(&workspaces);
    root.append(&clock);
    root.append(&battery);
    (root, workspaces, clock, battery)
}

struct MediaWidgets {
    root: gtk::CenterBox,
    workspaces: gtk::Box,
    clock: gtk::Label,
    icon: gtk::Image,
    title: gtk::Label,
    visualizer: gtk::DrawingArea,
    levels: Rc<RefCell<VisualizerLevels>>,
}

fn media_view(metrics: Metrics) -> MediaWidgets {
    let root = gtk::CenterBox::new();
    root.set_size_request(metrics.compact_width, metrics.media_height);
    root.add_css_class("media-content");
    root.set_valign(Align::Start);

    let workspaces = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);

    let media = gtk::Box::new(Orientation::Horizontal, metrics.spacing(7));
    media.add_css_class("media-center");
    media.set_halign(Align::Center);
    media.set_valign(Align::Center);

    let icon = gtk::Image::new();
    icon.add_css_class("media-app-icon");
    icon.set_visible(false);

    let levels = Rc::new(RefCell::new([0; VISUALIZER_BARS]));
    let draw_levels = levels.clone();
    let visualizer = gtk::DrawingArea::new();
    visualizer.add_css_class("media-visualizer");
    visualizer.set_content_width(metrics.spacing(31));
    visualizer.set_content_height(metrics.spacing(18));
    visualizer.set_valign(Align::Center);
    visualizer.set_draw_func(move |area, context, width, height| {
        let color = area.color();
        context.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        context.set_line_cap(gtk::cairo::LineCap::Round);
        let width = f64::from(width);
        let height = f64::from(height);
        let gap = width / (VISUALIZER_BARS as f64 * 2.2);
        let bar_width = gap * 0.72;
        let baseline = height * 0.82;
        context.set_line_width(bar_width);
        for (index, level) in draw_levels.borrow().iter().enumerate() {
            let x = gap + index as f64 * gap * 2.0;
            let bar_height = (height * 0.12) + (height * 0.67 * f64::from(*level) / 100.0);
            context.move_to(x, baseline);
            context.line_to(x, baseline - bar_height);
            let _ = context.stroke();
        }
    });

    let title = gtk::Label::new(None);
    title.add_css_class("media-title");
    title.set_hexpand(true);
    title.set_halign(Align::Fill);
    title.set_valign(Align::Center);
    title.set_xalign(0.0);
    title.set_single_line_mode(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let clock = gtk::Label::new(Some("--:--"));
    clock.add_css_class("media-clock");
    clock.set_halign(Align::End);
    clock.set_valign(Align::Center);

    media.append(&icon);
    media.append(&visualizer);
    media.append(&title);
    root.set_start_widget(Some(&workspaces));
    root.set_center_widget(Some(&media));
    root.set_end_widget(Some(&clock));
    MediaWidgets {
        root,
        workspaces,
        clock,
        icon,
        title,
        visualizer,
        levels,
    }
}

struct DashboardWidgets {
    root: gtk::Box,
    hero_time: gtk::Label,
    hero_date: gtk::Label,
    battery_chip: gtk::Box,
    battery_label: gtk::Label,
    active_eyebrow: gtk::Label,
    active_title: gtk::Label,
    workspace_row: gtk::Box,
    volume_scale: gtk::Scale,
    volume_value: gtk::Label,
    brightness_row: gtk::Box,
    brightness_scale: gtk::Scale,
    brightness_value: gtk::Label,
    theme_source: gtk::Label,
    close_button: gtk::Button,
}

fn dashboard_view(palette: &Palette, metrics: Metrics) -> DashboardWidgets {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(13));
    root.set_size_request(metrics.dashboard_width, metrics.dashboard_height);
    root.add_css_class("dashboard-content");
    root.set_valign(Align::Start);

    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    let heading = gtk::Box::new(Orientation::Vertical, 0);
    heading.set_hexpand(true);
    let eyebrow = gtk::Label::new(Some("MITHSHELL  //  LOCAL"));
    eyebrow.add_css_class("eyebrow");
    eyebrow.set_halign(Align::Start);
    let time = gtk::Label::new(Some("--:--"));
    time.add_css_class("hero-time");
    time.set_halign(Align::Start);
    let date = gtk::Label::new(None);
    date.add_css_class("hero-date");
    date.set_halign(Align::Start);
    heading.append(&eyebrow);
    heading.append(&time);
    heading.append(&date);

    let battery_chip = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    battery_chip.add_css_class("battery-chip");
    battery_chip.set_valign(Align::Center);
    battery_chip.set_visible(false);
    battery_chip.append(&gtk::Image::from_icon_name("battery-level-100-symbolic"));
    let battery_label = gtk::Label::new(None);
    battery_chip.append(&battery_label);

    let close_button = gtk::Button::from_icon_name("window-close-symbolic");
    close_button.add_css_class("close-button");
    close_button.set_valign(Align::Center);
    header.append(&heading);
    header.append(&battery_chip);
    header.append(&close_button);
    root.append(&header);

    let active_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(2));
    active_card.add_css_class("active-card");
    let active_eyebrow = gtk::Label::new(Some("OUTPUT  //  WORKSPACE --"));
    active_eyebrow.add_css_class("eyebrow");
    active_eyebrow.set_halign(Align::Start);
    let active_title = gtk::Label::new(Some("Quiet desktop"));
    active_title.add_css_class("active-title");
    active_title.set_halign(Align::Start);
    active_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    active_title.set_max_width_chars(48);
    active_card.append(&active_eyebrow);
    active_card.append(&active_title);
    root.append(&active_card);

    let workspace_line = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    let workspace_label = gtk::Label::new(Some("WORKSPACES"));
    workspace_label.add_css_class("eyebrow");
    workspace_label.set_hexpand(true);
    workspace_label.set_halign(Align::Start);
    let workspace_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(4));
    workspace_row.set_halign(Align::End);
    workspace_line.append(&workspace_label);
    workspace_line.append(&workspace_row);
    root.append(&workspace_line);

    let controls = gtk::Box::new(Orientation::Vertical, 0);
    controls.add_css_class("controls-card");
    let (volume_row, volume_scale, volume_value) =
        control_row("audio-volume-high-symbolic", "Volume", metrics);
    let (brightness_row, brightness_scale, brightness_value) =
        control_row("display-brightness-symbolic", "Brightness", metrics);
    controls.append(&volume_row);
    controls.append(&brightness_row);
    root.append(&controls);

    let footer = gtk::Box::new(Orientation::Horizontal, metrics.spacing(7));
    let palette_label = gtk::Label::new(Some("MATERIAL"));
    palette_label.add_css_class("eyebrow");
    footer.append(&palette_label);
    for class in ["primary", "secondary", "tertiary"] {
        let chip = gtk::Box::new(Orientation::Horizontal, 0);
        chip.add_css_class("palette-chip");
        chip.add_css_class(class);
        footer.append(&chip);
    }
    let theme_source = gtk::Label::new(Some(&palette.source));
    theme_source.add_css_class("muted-label");
    theme_source.set_hexpand(true);
    theme_source.set_halign(Align::End);
    footer.append(&theme_source);
    root.append(&footer);

    DashboardWidgets {
        root,
        hero_time: time,
        hero_date: date,
        battery_chip,
        battery_label,
        active_eyebrow,
        active_title,
        workspace_row,
        volume_scale,
        volume_value,
        brightness_row,
        brightness_scale,
        brightness_value,
        theme_source,
        close_button,
    }
}

fn control_row(icon: &str, label: &str, metrics: Metrics) -> (gtk::Box, gtk::Scale, gtk::Label) {
    let row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    row.add_css_class("control-row");
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("control-icon");
    image.set_tooltip_text(Some(label));
    let scale = gtk::Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.add_css_class("control-scale");
    let value = gtk::Label::new(Some("--"));
    value.add_css_class("control-value");
    value.set_xalign(1.0);
    row.append(&image);
    row.append(&scale);
    row.append(&value);
    (row, scale, value)
}

fn osd_view(
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

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn lerp(start: f64, target: f64, progress: f64) -> f64 {
    start + (target - start) * progress
}

fn scaled(value: i32, scale: f64) -> i32 {
    (f64::from(value) * scale).round() as i32
}
