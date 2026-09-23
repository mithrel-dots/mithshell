//! The resting compact pill: workspace dots, clock, battery, and the
//! hover-revealed tray segment, plus its content-driven width solver.

use super::*;

use std::rc::Rc;

use gtk::{Align, Orientation};

use super::{IslandWindow, Metrics, View, measure_clamped};

pub(super) fn compact_view(
    metrics: Metrics,
    wave: &gtk::DrawingArea,
) -> (gtk::Overlay, gtk::Box, gtk::Label, gtk::Label, gtk::Box) {
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
    content.append(&clock);
    content.append(&battery);
    content.append(&tray);
    root.set_child(Some(wave));
    root.add_overlay(&content);
    (root, workspaces, clock, battery, tray)
}

impl IslandWindow {
    /// Recomputes the compact pill's width from the combined (individually
    /// capped) natural width of its children, and repositions it within
    /// `content` to match, the same way `resize_media` does for the media
    /// pill. Called whenever a child's content changes (workspaces,
    /// battery, tray) or the tray's hover-visibility toggles.
    pub(super) fn resize_compact(self: &Rc<Self>) {
        let workspaces_width = measure_clamped(
            &self.compact_workspaces,
            self.metrics.compact_workspaces_max_width,
        );
        let clock_width =
            measure_clamped(&self.compact_clock, self.metrics.compact_clock_max_width);
        let battery_width = if self.compact_battery.is_visible() {
            measure_clamped(
                &self.compact_battery,
                self.metrics.compact_battery_max_width,
            )
        } else {
            0
        };

        let tray_visible = self.tray_visible();
        self.compact_tray.set_visible(tray_visible);
        let tray_width = if tray_visible {
            measure_clamped(&self.compact_tray, self.metrics.compact_tray_max_width)
        } else {
            0
        };

        let segments = 2 + i32::from(battery_width > 0) + i32::from(tray_width > 0);
        let spacing = self.metrics.spacing(10) * (segments - 1).max(0);
        // Matches the compact content row's horizontal CSS padding.
        let padding = self.metrics.spacing(30);
        let natural =
            workspaces_width + clock_width + battery_width + tray_width + spacing + padding;
        let width = natural.clamp(self.metrics.compact_min_width, self.metrics.media_max_width);

        self.compact
            .set_size_request(width, self.metrics.compact_height);
        self.content.move_(
            &self.compact,
            f64::from((self.metrics.window_width - width) / 2),
            0.0,
        );
        self.compact_width.set(width);
        // Rebuilds can happen while hover is already rendered.  Repositioning
        // the root above must not erase its offset against the current
        // backdrop, even when the new width produces no new animation.
        if self.current_view.get() == View::Compact {
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
        if start == target {
            return;
        }
        let profile = profile_timing(
            if self.tray_hovered.get() {
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
        if profile.duration.is_zero() {
            self.apply_geometry(target);
            self.sync_pill_content_geometry(target);
            return;
        }
        let started = Cell::new(None::<i64>);
        let weak = Rc::downgrade(self);
        self.surface.add_tick_callback(move |_, clock| {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if island.pill_animation_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time();
            let origin = started.get().unwrap_or_else(|| {
                started.set(Some(now));
                now
            });
            let progress = profile.progress(Duration::from_micros((now - origin).max(0) as u64));
            let geometry = start.interpolate(target, progress);
            island.apply_geometry(geometry);
            island.sync_pill_content_geometry(geometry);
            if progress >= 1.0 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Keeps the fixed-size foreground centered in the animated pill surface.
    ///
    /// `surface` is the animated backdrop and grows symmetrically for hover,
    /// while the compact/media roots deliberately retain their natural size so
    /// labels and icons do not scale with the chrome.  Without this correction
    /// the roots stay at the surface's old top edge: the backdrop moves down
    /// and grows, but its contents do not.  Positioning the unchanged content
    /// in the animated surface preserves both visual alignment and GTK's real
    /// descendant pick coordinates.
    pub(super) fn sync_pill_content_geometry(&self, geometry: Geometry) {
        let (widget, base_width, base_height) = match self.current_view.get() {
            View::Compact => (
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
        let (x, y) =
            pill_content_offset(geometry, self.metrics.window_width, base_width, base_height);
        self.content.move_(widget, x, y);
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
            set_brightness: Rc::new(|_| {}),
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
        let base_height = f64::from(island.metrics.compact_height);
        let content = island.compact.clone();
        let samples: Samples = Rc::new(RefCell::new(Vec::new()));
        let sample_tick = |island: &Rc<super::super::IslandWindow>, samples: &Samples| {
            let geometry = island.geometry.get();
            let allocation = island.compact.allocation();
            let bounds = island.compact.compute_bounds(&island.surface).unwrap();
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
            let expected = ((height - base_height) / 2.0).max(0.0);
            assert!(
                (local_y - expected).abs() <= 2.0,
                "content y={local_y} expected {expected}"
            );
            assert!(
                *surface_y >= -1.0 && *surface_y <= 80.0,
                "content escaped backdrop: {surface_y}"
            );
        }
        drop(sampled);
        assert!(content.is_mapped() && content.width() > 0 && content.height() > 0);
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
        settle(&|| {
            island.current_view.get() == super::super::View::Media
                && !island.view_transition_active.get()
        });
        drain(Duration::from_millis(30));
        let media_y_before = f64::from(island.media.allocation().y());
        island.update_media(Some(&media));
        drain(Duration::from_millis(20));
        let media_y_after_update = f64::from(island.media.allocation().y());
        assert!((media_y_after_update - media_y_before).abs() <= 2.0);
        assert!(
            island
                .media
                .pick(
                    f64::from(island.media.width()) / 2.0,
                    f64::from(island.media.height()) / 2.0,
                    gtk::PickFlags::DEFAULT,
                )
                .is_some()
        );
        island.update_media(None);
        settle(&|| {
            island.current_view.get() == super::super::View::Compact
                && !island.view_transition_active.get()
        });

        // Reverse before the 420 ms enter track settles; no stale-frame jump
        // may place the content outside its current backdrop.
        drain(Duration::from_millis(55));
        island.set_pointer_in_hover_region(false);
        for _ in 0..4 {
            drain(Duration::from_millis(35));
            let geometry = island.geometry.get();
            let local_y = f64::from(island.compact.allocation().y());
            let expected = ((geometry.height - base_height) / 2.0).max(0.0);
            assert!((local_y - expected).abs() <= 2.0);
        }
        settle(&|| {
            island.geometry.get()
                == island.presentation_target_geometry(crate::ui::island::View::Compact)
        });

        island.open();
        settle(&|| {
            island.current_view.get() == crate::ui::island::View::Dashboard
                && !island.view_transition_active.get()
        });
        island.close();
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
