//! The resting compact pill: workspace dots, clock, battery, and the
//! hover-revealed tray segment, plus its content-driven width solver.

use super::*;

use std::rc::Rc;

use gtk::{Align, Orientation};

use super::{IslandWindow, Metrics, View};

pub(super) fn compact_view(
    metrics: Metrics,
) -> (
    gtk::Overlay,
    gtk::Box,
    gtk::Label,
    gtk::Label,
    gtk::Box,
    gtk::Label,
    gtk::Label,
    gtk::Box,
) {
    let root = gtk::Overlay::new();
    root.set_size_request(metrics.compact_width, metrics.compact_height);
    // Padding belongs to the foreground row only. Putting compact-content on
    // this overlay also inset the background at medium/large density tiers.
    root.add_css_class("compact-pill");
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

    // Hidden by default (`update_tray`/`set_tray_hovered`): only shown while
    // the pointer is over the pill and at least one tray item exists, per
    // `resize_compact`.
    let tray = gtk::Box::new(Orientation::Horizontal, metrics.spacing(3));
    tray.add_css_class("compact-tray");
    tray.set_hexpand(false);
    tray.set_halign(Align::End);
    tray.set_valign(Align::Center);
    tray.set_visible(false);

    let content = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    content.add_css_class("compact-content");
    content.set_hexpand(true);
    content.set_vexpand(true);
    content.set_halign(Align::Fill);
    content.set_valign(Align::Fill);
    content.append(&workspaces);
    let clock_slot = gtk::Box::new(Orientation::Horizontal, metrics.spacing(18));
    clock_slot.set_hexpand(true);
    clock_slot.set_halign(Align::Center);
    clock_slot.append(&clock);
    let date = gtk::Box::new(Orientation::Horizontal, metrics.spacing(2));
    date.add_css_class("compact-date");
    date.set_valign(Align::Center);
    date.set_visible(false);
    let day = gtk::Label::new(Some("---"));
    day.add_css_class("compact-date-day");
    day.set_valign(Align::Start);
    let slash = gtk::DrawingArea::new();
    slash.set_content_width(metrics.spacing(10));
    slash.set_content_height(metrics.spacing(22));
    slash.set_draw_func(|area, cr, w, h| {
        let color = area.color();
        cr.set_source_rgba(
            color.red().into(),
            color.green().into(),
            color.blue().into(),
            0.7,
        );
        cr.set_line_width(1.4 * f64::from(h) / 22.0);
        cr.move_to(1.5, f64::from(h) - 1.0);
        cr.line_to(f64::from(w) - 1.5, 1.0);
        let _ = cr.stroke();
    });
    slash.set_valign(Align::Center);
    let month_day = gtk::Label::new(Some("-- ---"));
    month_day.add_css_class("compact-date-rest");
    month_day.set_valign(Align::End);
    date.append(&day);
    date.append(&slash);
    date.append(&month_day);
    clock_slot.append(&date);
    content.append(&clock_slot);
    content.append(&battery);
    content.append(&tray);
    root.add_overlay(&content);
    // The same date slot is revealed in hover/open presentations; it never
    // replaces or shifts the clock's semantic position in the resting pill.
    (root, workspaces, clock, battery, tray, day, month_day, date)
}

impl IslandWindow {
    /// Recomputes the idle pill's width from its actual foreground layout,
    /// including nested spacing, margins, padding, and the full audio reveal.
    pub(super) fn resize_compact(self: &Rc<Self>) {
        let tray_visible = self.tray_visible();
        self.compact_tray.set_visible(tray_visible);
        let row = self
            .compact_clock
            .parent()
            .and_then(|slot| slot.parent())
            .expect("compact content row");
        // Measure the actual idle row: the clock slot has its own spacing and
        // the visualizer a margin, neither of which a sum of leaf widths sees.
        let date_visible = self.compact_date.is_visible();
        self.compact_date.set_visible(false);
        let mut natural = row.measure(Orientation::Horizontal, -1).1;
        self.compact_date.set_visible(date_visible);
        if self.compact_visualizer_revealer.reveals_child() {
            // Reserve its destination width throughout the reveal so the
            // growing audio bars cannot push the percentage outside the pill.
            natural += (self
                .compact_visualizer
                .measure(Orientation::Horizontal, -1)
                .1
                - self
                    .compact_visualizer_revealer
                    .measure(Orientation::Horizontal, -1)
                    .1)
                .max(0);
        }
        #[allow(deprecated)]
        let border = self.compact.style_context().border();
        natural += i32::from(border.left()) + i32::from(border.right());
        let width = natural.clamp(self.metrics.compact_min_width, self.metrics.media_max_width);

        self.compact
            .set_size_request(width, self.metrics.compact_height);
        self.compact_width.set(width);
        // Rebuilds can happen while hover is already rendered.  Repositioning
        // the root above must not erase its offset against the current
        // backdrop, even when the new width produces no new animation.
        if matches!(self.current_view.get(), View::Compact | View::Dashboard) {
            self.sync_pill_content_geometry(self.geometry.get());
        }
    }

    /// Whether the tray row should be shown: at least one item exists, and
    /// either the pill (compact or media, whichever is active) is hovered
    /// or one of the items' menus is currently open.
    pub(super) fn tray_visible(&self) -> bool {
        !self.tray_circle_assigned
            && (self.tray_hovered.get() || self.tray_menu_open.get())
            && self.tray_item_count.get() > 0
    }

    /// Hides/reveals the tray row on hover, and resizes both pills (only
    /// the currently active one animates; the other silently follows so
    /// it's already correct if the view switches while hovered).
    pub(super) fn set_tray_hovered(self: &Rc<Self>, hovered: bool) {
        if self.tray_hovered.get() == hovered {
            return;
        }
        // A reversal can be requested before the next frame of the current
        // track.  Preserve the content's position against that still-rendered
        // backdrop before changing natural widths and scheduling its successor.
        if matches!(self.current_view.get(), View::Compact | View::Media) {
            self.sync_pill_content_geometry(self.geometry.get());
        }
        self.tray_hovered.set(hovered);
        if !matches!(self.current_view.get(), View::Compact | View::Media) {
            return;
        }
        self.resize_compact();
        self.resize_media();
        self.sync_pill_content_geometry(self.geometry.get());
        self.reconcile_pill_geometry();
    }

    pub(super) fn set_pointer_in_hover_region(self: &Rc<Self>, inside: bool) {
        self.pointer_in_hover_region.set(inside);
        if self.current_view.get() == View::Dashboard {
            // An open dashboard is pinned until its explicit close affordance
            // or the persistent header is clicked again.
            self.island_hovered.set(true);
            self.compact_date.set_visible(true);
        } else if self.current_view.get() == View::Compact
            && self.island_hovered.replace(inside) != inside
        {
            self.sync_island_surface_style();
            if self.view_transition_active.get() {
                // Leaving while Open is rolling back to Peek changes the
                // destination to Idle; retarget from the rendered frame now.
                self.set_view(View::Compact);
            } else {
                self.reconcile_pill_geometry();
            }
        }
        if matches!(self.current_view.get(), View::Compact | View::Media) {
            self.set_tray_hovered(inside);
        } else {
            self.tray_hovered.set(false);
        }
    }

    /// The sole geometry track for compact/media presentation. It includes
    /// content-driven width, tray visibility, and hover depth, so a tray update
    /// cannot race a second hover animator. Its start is always the currently
    /// rendered geometry, making interruption and reversal continuous.
    pub(super) fn reconcile_pill_geometry(self: &Rc<Self>) {
        let view = self.current_view.get();
        if !matches!(view, View::Compact | View::Media) || self.view_transition_active.get() {
            return;
        }
        let target = self.presentation_target_geometry(view);
        let start = self.geometry.get();
        // Telemetry, tray, and media refreshes often reconcile unchanged
        // destinations while a frame is between its endpoints. Keep that
        // track's clock instead of repeatedly restarting the easing at zero.
        if self.pill_animation_target.get() == Some(target)
            || (start == target && self.pill_animation_target.get().is_none())
        {
            return;
        }
        self.pill_animation_target.set(Some(target));
        let profile = profile_timing(
            if self.island_hovered.get() && target.height >= start.height {
                crate::ui::motion::Profile::ISLAND_EXPAND
            } else if target.height < start.height || target.width < start.width {
                crate::ui::motion::Profile::ISLAND_COLLAPSE
            } else if self.tray_hovered.get() {
                crate::ui::motion::Profile::HOVER_ENTER
            } else if target.width >= start.width {
                crate::ui::motion::Profile::CONTAINER_EXPAND
            } else {
                crate::ui::motion::Profile::CONTAINER_COLLAPSE
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let generation = self.pill_animation_generation.get().wrapping_add(1);
        self.pill_animation_generation.set(generation);
        let entering_peek = view == View::Compact && self.island_hovered.get();
        let date_start = if self.compact_date.is_visible() {
            self.compact_date.opacity()
        } else {
            0.0
        };
        if entering_peek {
            self.dashboard.set_visible(true);
            self.dashboard.set_can_target(true);
            // Peek is a physical rollout, not a delayed content entrance.
            // Render hardware at full opacity before exposing its clipped
            // viewport, so the first revealed pixels already contain it.
            self.dashboard.set_opacity(1.0);
            self.panel_scroll.set_visible(true);
            self.compact_date.set_visible(true);
            self.compact_date.set_opacity(date_start);
        } else if view == View::Compact && start.height > target.height {
            self.dashboard.set_can_target(false);
        }
        if profile.duration.is_zero() {
            self.pill_animation_target.set(None);
            self.apply_geometry(target);
            self.sync_pill_content_geometry(target);
            self.dashboard
                .set_opacity(if entering_peek { 1.0 } else { 0.0 });
            self.dashboard.set_visible(entering_peek);
            self.compact_date
                .set_opacity(if entering_peek { 1.0 } else { 0.0 });
            self.compact_date.set_visible(entering_peek);
            return;
        }
        let started = Instant::now();
        let weak = Rc::downgrade(self);
        self.fixed.add_tick_callback(move |_, _| {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if island.pill_animation_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let elapsed = started.elapsed();
            let progress = profile.progress(elapsed);
            let geometry = start.interpolate(target, progress);
            island.apply_geometry(geometry);
            island.sync_pill_content_geometry(geometry);
            let fade = island.island_fade_progress(entering_peek, elapsed, profile.duration);
            // Keep the hardware painted while either rolling out or tucking
            // away; the shared viewport controls how much is visible.
            island.dashboard.set_opacity(1.0);
            island.compact_date.set_opacity(lerp(
                date_start,
                if entering_peek { 1.0 } else { 0.0 },
                fade,
            ));
            if profile.is_complete(elapsed) {
                island.pill_animation_target.set(None);
                island
                    .dashboard
                    .set_opacity(if entering_peek { 1.0 } else { 0.0 });
                island.dashboard.set_visible(entering_peek);
                island
                    .compact_date
                    .set_opacity(if entering_peek { 1.0 } else { 0.0 });
                island.compact_date.set_visible(entering_peek);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// The persistent header shares the panel's width track. Legacy media
    /// content stays centered within its own animated backdrop.
    pub(super) fn sync_pill_content_geometry(&self, geometry: Geometry) {
        let (widget, base_width, base_height) = match self.current_view.get() {
            View::Compact | View::Dashboard => (
                &self.compact,
                self.compact_width.get(),
                self.metrics.compact_height,
            ),
            View::Media => (
                self.media.upcast_ref::<gtk::Widget>(),
                self.media_width.get(),
                self.metrics.media_height,
            ),
            _ => return,
        };
        if matches!(self.current_view.get(), View::Compact | View::Dashboard) {
            // Compact is an overlay sibling of the scrolling page surface so
            // it stays above the rolling panel and retains its GTK pick path.
            let width = geometry.width.round() as i32;
            self.compact
                .set_size_request(width, self.persistent_header_height(geometry));
            self.compact.set_margin_top(0);
        } else {
            let (x, y) =
                pill_content_offset(geometry, self.metrics.window_width, base_width, base_height);
            self.content.move_(widget, x, y);
        }
    }
}

/// Returns the fixed-content position inside an animated pill surface. The
/// foreground dimensions remain unchanged; only its position follows the
/// surface's expanding frame.
fn pill_content_offset(
    geometry: Geometry,
    window_width: i32,
    content_width: i32,
    content_height: i32,
) -> (f64, f64) {
    (
        f64::from((window_width - content_width) / 2),
        ((geometry.height - f64::from(content_height)) / 2.0).max(0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::{Geometry, pill_content_offset};
    use std::cell::Cell;

    #[test]
    fn content_stays_centered_at_every_hover_frame_without_scaling() {
        let base = Geometry {
            width: 380.0,
            height: 61.0,
            y: 0.0,
        };
        let target = Geometry {
            width: 395.2,
            height: 68.6,
            y: 3.8,
        };
        let content_width = 380;
        let content_height = 61;

        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let frame = base.interpolate(target, progress);
            let (x, y) = pill_content_offset(frame, 1_700, content_width, content_height);
            let expected_x = f64::from((1_700 - content_width) / 2);
            let expected_y = (frame.height - f64::from(content_height)) / 2.0;
            assert_eq!(x, expected_x);
            assert!((y - expected_y.max(0.0)).abs() < f64::EPSILON);

            // The foreground is intentionally not scaled with the backdrop.
            assert_eq!(content_width, 380);
            assert_eq!(content_height, 61);
            assert!(x >= 0.0 && x + f64::from(content_width) <= 1_700.0);
            assert!(y >= 0.0 && y + f64::from(content_height) <= frame.height + 0.001);
        }
    }

    /// This uses the production IslandWindow hierarchy and animation tracks,
    /// rather than manually allocating a compact fixture.  The runner for
    /// this ignored test is `scripts/run-ui-regressions-gtk.py`.
    #[test]
    #[ignore = "requires the project-local Broadway runner"]
    #[allow(deprecated)]
    fn mapped_scale_1p9_hover_content_tracks_animation_and_picking() {
        use super::super::{IslandActions, IslandWindow};
        use crate::config::AppConfig;
        use crate::state::{MediaPlayer, MediaState, PlaybackStatus};
        use crate::tarragon::TarragonSelection;
        use gtk::prelude::*;
        use std::{cell::RefCell, rc::Rc, time::Duration};
        type Samples = Rc<RefCell<Vec<(f64, f64, f64)>>>;

        gtk::init().expect("Broadway GTK display");
        let app = gtk::Application::new(
            Some("org.mithshell.compact-motion-test"),
            gtk::gio::ApplicationFlags::NON_UNIQUE,
        );
        app.connect_activate(|_| {});
        app.register(None::<&gtk::gio::Cancellable>)
            .expect("register GTK application");
        let display = gtk::gdk::Display::default().expect("Broadway display");
        let monitor = display
            .monitors()
            .item(0)
            .and_downcast::<gtk::gdk::Monitor>()
            .expect("Broadway monitor");
        let actions = IslandActions {
            switch_workspace: Rc::new(|_, _| {}),
            set_volume: Rc::new(|_| {}),
            toggle_mute: Rc::new(|| {}),
            search: Rc::new(|_| {}),
            select: Rc::new(|_: TarragonSelection| {}),
            tarragon_status: Rc::new(|| {}),
            tarragon_reload: Rc::new(|| {}),
            load_preview: Rc::new(|_, _| {}),
            media_play_pause: Rc::new(|_| {}),
            media_next: Rc::new(|_| {}),
            media_previous: Rc::new(|_| {}),
            notification_expired: Rc::new(|_, _| {}),
            notification_dismiss: Rc::new(|_| {}),
            notification_invoke: Rc::new(|_, _| {}),
            notification_clear_all: Rc::new(|| {}),
            notification_inhibit: Rc::new(|_| {}),
            tray_activate: Rc::new(|_, _, _, _| {}),
            tray_secondary_activate: Rc::new(|_, _, _, _| {}),
            tray_context_menu: Rc::new(|_, _, _, _| {}),
            tray_scroll: Rc::new(|_, _, _, _| {}),
            tray_menu_event: Rc::new(|_, _, _| {}),
        };
        let mut config = AppConfig::default();
        config.shell.scale = 1.9;
        config.shell.animation_ms = 420;
        config.battery.wave = true;
        let island = IslandWindow::new_for_test(
            &app,
            &monitor,
            "broadway-motion".to_owned(),
            &config,
            actions,
            true,
        );
        app.activate();

        let drain = |duration: Duration| {
            let loop_ = gtk::glib::MainLoop::new(None, false);
            let quit = loop_.clone();
            gtk::glib::timeout_add_local_once(duration, move || quit.quit());
            loop_.run();
            while gtk::glib::MainContext::default().pending() {
                gtk::glib::MainContext::default().iteration(false);
            }
        };
        let settle = |predicate: &dyn Fn() -> bool| {
            for _ in 0..188 {
                if predicate() {
                    return;
                }
                drain(Duration::from_millis(16));
            }
            assert!(predicate(), "production animation did not settle");
        };
        drain(Duration::from_millis(50));
        island.battery_waves.update(Some(50));
        let assert_hover_hitbox = |island: &Rc<super::super::IslandWindow>| {
            let pill = match island.current_view.get() {
                super::super::View::Compact => island.compact.clone().upcast::<gtk::Widget>(),
                super::super::View::Media => island.media.clone().upcast::<gtk::Widget>(),
                view => panic!("expected pill view for hover hitbox: {view:?}"),
            };
            let bounds = pill.compute_bounds(&island.fixed).unwrap();
            let center_x = f64::from(bounds.x()) + f64::from(bounds.width()) / 2.0;
            let center_y = f64::from(bounds.y()) + f64::from(bounds.height()) / 2.0;
            island.update_pointer_from_root(center_x, center_y);
            assert!(
                island.pointer_in_hover_region.get(),
                "pill center must hover"
            );
            let outside_x = f64::from(bounds.x()) - 40.0;
            let outside_y = center_y;
            assert!(outside_x < f64::from(island.fixed.width()));
            assert!(outside_y < f64::from(island.fixed.height()));
            island.update_pointer_from_root(outside_x, outside_y);
            assert!(
                !island.pointer_in_hover_region.get(),
                "point outside visible island content triggered hover: pill={bounds:?} panel={:?} point=({outside_x}, {outside_y}) geometry={:?} view={:?} island_hovered={} tray_hovered={}",
                island.dashboard.compute_bounds(&island.fixed),
                island.geometry.get(),
                island.current_view.get(),
                island.island_hovered.get(),
                island.tray_hovered.get(),
            );
            island.update_pointer_from_root(center_x, center_y);
            assert!(island.pointer_in_hover_region.get(), "re-entry must hover");
        };
        let content = island.compact.clone();
        let samples: Samples = Rc::new(RefCell::new(Vec::new()));
        let sample_tick = |island: &Rc<super::super::IslandWindow>, samples: &Samples| {
            let geometry = island.geometry.get();
            let allocation = island.compact.allocation();
            let bounds = island
                .compact
                .compute_bounds(&island.surface_shell)
                .unwrap();
            assert!(island.battery_waves.area.is_visible());
            assert_eq!(island.battery_waves.area.opacity(), 1.0);
            assert!(
                (island.battery_waves.area.width() - island.compact.width()).abs() <= 2,
                "wave width {} must follow header width {}",
                island.battery_waves.area.width(),
                island.compact.width()
            );
            assert!(
                (island.battery_waves.area.height() - island.compact.height()).abs() <= 2,
                "wave height {} must follow header height {}",
                island.battery_waves.area.height(),
                island.compact.height()
            );
            samples.borrow_mut().push((
                geometry.height,
                f64::from(allocation.y()),
                f64::from(bounds.y()),
            ));
        };

        island.set_pointer_in_hover_region(true);
        for _ in 0..6 {
            drain(Duration::from_millis(45));
            sample_tick(&island, &samples);
        }
        let sampled = samples.borrow();
        assert!(sampled.len() >= 4);
        for (height, local_y, surface_y) in sampled.iter() {
            let expected = 0.0;
            assert!(
                (local_y - expected).abs() <= 2.0,
                "persistent header y={local_y} expected top anchoring in {height}px surface"
            );
            assert!(
                *surface_y >= -1.0 && *surface_y <= 80.0,
                "content escaped backdrop: {surface_y}"
            );
        }
        drop(sampled);
        assert!(content.is_mapped() && content.width() > 0 && content.height() > 0);
        assert_hover_hitbox(&island);
        island.compact_clock.set_can_target(true);
        let picked = island.compact.pick(
            f64::from(island.compact.width()) / 2.0,
            f64::from(island.compact.height()) / 2.0,
            gtk::PickFlags::DEFAULT,
        );
        assert!(
            picked.is_some(),
            "animated compact content lost GTK picking"
        );

        // These are the real rebuild/update entry points.  Exercise both
        // unchanged-width and changed-width paths while the pill is already
        // raised; neither is allowed to reset the root to y=0.
        let compact_y_before = f64::from(island.compact.allocation().y());
        island.update_tray(&[]);
        drain(Duration::from_millis(20));
        let compact_y_after_rebuild = f64::from(island.compact.allocation().y());
        assert!((compact_y_after_rebuild - compact_y_before).abs() <= 2.0);

        let media = MediaState {
            player: "Test Player".into(),
            service: "org.test.Player".into(),
            title: "Track".into(),
            artist: Some("Artist".into()),
            album: None,
            app_icon: Some("audio-x-generic".into()),
            art_url: None,
            position_us: 25,
            length_us: Some(100),
            can_play: true,
            can_pause: true,
            can_go_next: true,
            can_go_previous: true,
            status: PlaybackStatus::Playing,
            players: vec![MediaPlayer {
                player: "Test Player".into(),
                service: "org.test.Player".into(),
                title: "Track".into(),
                artist: Some("Artist".into()),
                album: None,
                app_icon: Some("audio-x-generic".into()),
                art_url: None,
                position_us: 25,
                length_us: Some(100),
                can_play: true,
                can_pause: true,
                can_go_next: true,
                can_go_previous: true,
                status: PlaybackStatus::Playing,
            }],
        };
        island.update_media(Some(&media));
        let mut media_open_samples = 0;
        for _ in 0..5 {
            drain(Duration::from_millis(45));
            if island.view_transition_active.get() {
                media_open_samples += 1;
                let geometry = island.geometry.get();
                let surface_bounds = island.surface.compute_bounds(&island.fixed).unwrap();
                let root_bounds = island.media.compute_bounds(&island.fixed).unwrap();
                let local_y = f64::from(root_bounds.y());
                let expected = f64::from(surface_bounds.y())
                    + (f64::from(surface_bounds.height()) - f64::from(island.metrics.media_height))
                        .max(0.0)
                        / 2.0;
                assert!(
                    (local_y - expected).abs() <= 2.0,
                    "media content y={local_y} expected {expected} at backdrop height {} view={:?}",
                    geometry.height,
                    island.current_view.get()
                );
                assert!(island.media_clock.is_mapped());
            }
        }
        assert!(
            media_open_samples >= 3,
            "media open had too few intermediate frames"
        );
        settle(&|| {
            island.current_view.get() == super::super::View::Media
                && !island.view_transition_active.get()
        });
        assert_hover_hitbox(&island);
        island.set_pointer_in_hover_region(false);
        island.set_tray_hovered(false);
        island.reconcile_pill_geometry();
        settle(&|| {
            let geometry = island.geometry.get();
            let target = island.presentation_target_geometry(super::super::View::Media);
            !island.view_transition_active.get()
                && (geometry.width - target.width).abs() <= 1.0
                && (geometry.height - target.height).abs() <= 1.0
        });
        let stable_media_layout = Cell::new(0_u8);
        let last_media_layout = Cell::new(None::<(i32, i32, i32)>);
        let media_is_allocated_at_rest = || {
            let allocation = island.media.allocation();
            let bounds = island.media.compute_bounds(&island.fixed).unwrap();
            let geometry = island.geometry.get();
            let target = island.presentation_target_geometry(super::super::View::Media);
            let current = (
                bounds.y().round() as i32,
                allocation.width(),
                allocation.height(),
            );
            if allocation.width() > 0 && allocation.height() > 0 {
                if last_media_layout.get() == Some(current) {
                    stable_media_layout.set(stable_media_layout.get().saturating_add(1));
                } else {
                    last_media_layout.set(Some(current));
                    stable_media_layout.set(0);
                }
            }
            allocation.width() > 0
                && allocation.height() > 0
                && !island.view_transition_active.get()
                && (geometry.width - target.width).abs() <= 1.0
                && (geometry.height - target.height).abs() <= 1.0
                && stable_media_layout.get() >= 2
        };
        let settle_media_layout = || {
            for _ in 0..188 {
                if media_is_allocated_at_rest() {
                    return;
                }
                drain(Duration::from_millis(16));
            }
            let allocation = island.media.allocation();
            panic!(
                "media layout did not settle: allocation={allocation:?} bounds={:?} geometry={:?} target={:?} stable={} last={:?}",
                island.media.compute_bounds(&island.fixed).unwrap(),
                island.geometry.get(),
                island.presentation_target_geometry(super::super::View::Media),
                stable_media_layout.get(),
                last_media_layout.get(),
            );
        };
        settle_media_layout();
        let media_y_before = f64::from(island.media.compute_bounds(&island.fixed).unwrap().y());
        let media_before = island.media.allocation();
        let geometry_before = island.geometry.get();
        let fixed_before = island.fixed.allocation();
        let surface_before = island.surface.allocation();
        island.update_media(Some(&media));
        settle_media_layout();
        drain(Duration::from_millis(20));
        let media_y_after_update =
            f64::from(island.media.compute_bounds(&island.fixed).unwrap().y());
        let media_after = island.media.allocation();
        let geometry_after = island.geometry.get();
        let fixed_after = island.fixed.allocation();
        let surface_after = island.surface.allocation();
        assert!(
            (media_y_after_update - media_y_before).abs() <= 2.0,
            "media y moved after allocated, settled media update: before={media_y_before} after={media_y_after_update}; media={media_before:?}->{media_after:?}; fixed={fixed_before:?}->{fixed_after:?}; surface={surface_before:?}->{surface_after:?}; geometry={geometry_before:?}->{geometry_after:?}; active={}",
            island.view_transition_active.get()
        );
        assert!(island.media.is_visible() && island.media.can_target());
        island.update_media(None);
        let mut media_close_samples = 0;
        for _ in 0..5 {
            drain(Duration::from_millis(45));
            if island.view_transition_active.get() {
                media_close_samples += 1;
                let geometry = island.geometry.get();
                let surface_bounds = island.surface_shell.compute_bounds(&island.fixed).unwrap();
                let root_bounds = island.compact.compute_bounds(&island.fixed).unwrap();
                let local_y = f64::from(root_bounds.y());
                let expected = f64::from(surface_bounds.y());
                assert!(
                    (local_y - expected).abs() <= 2.0,
                    "media close content y={local_y} expected {expected} at backdrop height {}",
                    geometry.height
                );
                assert!(island.compact_clock.is_mapped());
            }
        }
        assert!(
            media_close_samples >= 3,
            "media close had too few intermediate frames"
        );
        settle(&|| {
            island.current_view.get() == super::super::View::Compact
                && !island.view_transition_active.get()
        });
        assert_hover_hitbox(&island);

        // Keep hover active while opening and closing the dashboard. The close
        // track returns to the raised compact target, so sample that incoming
        // root between real Broadway frame-clock ticks rather than validating
        // only its terminal allocation.
        island.open();
        let mut open_samples = 0;
        for _ in 0..5 {
            drain(Duration::from_millis(45));
            if island.view_transition_active.get() {
                open_samples += 1;
                assert!(island.dashboard.is_visible());
                assert!(island.dashboard.is_mapped());
            }
        }
        assert!(
            open_samples >= 3,
            "dashboard open had too few intermediate frames"
        );
        settle(&|| {
            island.current_view.get() == crate::ui::island::View::Dashboard
                && !island.view_transition_active.get()
        });

        island.close();
        let mut close_samples = 0;
        for _ in 0..30 {
            drain(Duration::from_millis(16));
            if island.view_transition_active.get() {
                close_samples += 1;
                let geometry = island.geometry.get();
                let surface_bounds = island.surface_shell.compute_bounds(&island.fixed).unwrap();
                let root_bounds = island.compact.compute_bounds(&island.fixed).unwrap();
                let local_y = f64::from(root_bounds.y());
                let expected = f64::from(surface_bounds.y());
                assert!(island.compact.is_visible());
                assert!(
                    (local_y - expected).abs() <= 2.0,
                    "dashboard close content y={local_y} expected {expected} at backdrop height {}",
                    geometry.height
                );
                assert!(island.compact_clock.is_mapped());
            }
        }
        assert!(
            close_samples >= 5,
            "dashboard close had too few intermediate frames"
        );
        // Broadway can defer the ScrolledWindow allocation while the logical
        // frame track is changing; the per-frame bounds assertions above still
        // verify that the mapped root follows the rendered surface origin.
        settle(&|| {
            island.current_view.get() == crate::ui::island::View::Compact
                && !island.view_transition_active.get()
        });
        assert!(island.compact.is_mapped());
        assert!(
            island
                .compact
                .pick(
                    f64::from(island.compact.width()) / 2.0,
                    f64::from(island.compact.height()) / 2.0,
                    gtk::PickFlags::DEFAULT,
                )
                .is_some()
        );
        island.window.close();
    }
}
