//! View reconciliation: picking the active view, animating geometry and
//! opacity between views, and applying the result to the layer surface.

use super::*;

use std::{cell::Cell, rc::Rc, time::Duration};

use gtk4_layer_shell::KeyboardMode;

use super::{Geometry, IslandWindow, View};

fn sequential_fade_progress(
    previous: View,
    target: View,
    elapsed: Duration,
    outgoing: crate::ui::motion::Profile,
    incoming: crate::ui::motion::Profile,
) -> (f64, f64) {
    if previous == target {
        (0.0, incoming.progress(elapsed))
    } else {
        (
            outgoing.progress(elapsed),
            incoming.progress(elapsed.saturating_sub(outgoing.duration)),
        )
    }
}

fn transition_duration(
    geometry: crate::ui::motion::Profile,
    outgoing: crate::ui::motion::Profile,
    incoming: crate::ui::motion::Profile,
) -> Duration {
    geometry
        .duration
        .max(outgoing.duration.saturating_add(incoming.duration))
}

impl IslandWindow {
    /// Presents the outside-click catcher below the interactive island.
    ///
    /// Presenting the catcher can raise it above the island, so a newly shown
    /// catcher is immediately followed by the main window.  Already-visible
    /// surfaces are left alone to avoid stealing focus or changing stacking
    /// order during snapshot and animation updates.
    pub(super) fn present_dismiss_catcher_behind_main(&self) {
        let catcher_was_visible = self.dismiss_window.is_visible();
        if !catcher_was_visible {
            self.dismiss_window.present();
        }
        if !catcher_was_visible || !self.window.is_visible() {
            self.window.present();
        }
    }

    fn dismiss_catcher_needed(&self) -> bool {
        self.circle_full_active()
            || matches!(
                self.current_view.get(),
                View::Dashboard | View::Weather | View::Search
            )
            || (self.launcher_presentation == crate::config::LauncherPresentation::Independent
                && self.search_window.is_visible())
    }

    /// Re-applies the keyboard mode the current state wants. Split out of
    /// `set_view` so showing/dismissing a tray menu can borrow the surface's
    /// focus without having to know what the active view expects.
    pub(super) fn refresh_keyboard_mode(&self) {
        let mode = match self.current_view.get() {
            View::Weather => KeyboardMode::Exclusive,
            View::Search => KeyboardMode::Exclusive,
            _ if self.tray_menu_open.get() => KeyboardMode::OnDemand,
            _ if self.circle_full_active() => KeyboardMode::OnDemand,
            _ => KeyboardMode::None,
        };
        self.window.set_keyboard_mode(mode);
        self.search_window.set_keyboard_mode(
            if self.search_open.get()
                && self.launcher_presentation == crate::config::LauncherPresentation::Independent
            {
                KeyboardMode::Exclusive
            } else {
                KeyboardMode::None
            },
        );
        if self.dismiss_catcher_needed() {
            self.present_dismiss_catcher_behind_main();
        } else {
            self.dismiss_window.set_visible(false);
        }
    }

    pub(super) fn reconcile_view(self: &Rc<Self>) {
        let view = if self.notification_active.get()
            && self
                .notification_current
                .borrow()
                .as_ref()
                .is_some_and(|current| !current.overlay)
        {
            View::Notification
        } else if self.osd_active.get() {
            View::Osd
        } else if self.search_open.get()
            && self.launcher_presentation == crate::config::LauncherPresentation::Integrated
        {
            View::Search
        } else if self.weather_open.get() {
            View::Weather
        } else if self.dashboard_open.get() {
            View::Dashboard
        } else if self.media_playing.get() {
            View::Media
        } else {
            View::Compact
        };
        self.set_view(view);
    }

    pub(super) fn geometry_for_view(&self, view: View) -> Geometry {
        Geometry::for_view(
            view,
            self.metrics,
            self.media_width.get(),
            self.compact_width.get(),
        )
    }

    pub(super) fn set_view(self: &Rc<Self>, view: View) {
        if view != View::Search {
            self.search_focus_generation
                .set(self.search_focus_generation.get().wrapping_add(1));
        }
        let pill_view = matches!(view, View::Compact | View::Media);
        let desired_hover = pill_view && self.pointer_in_hover_region.get();
        let hover_changed = self.tray_hovered.get() != desired_hover;
        self.tray_hovered.set(desired_hover);
        if hover_changed && pill_view {
            self.resize_compact();
            self.resize_media();
        }
        let target = self.presentation_target_geometry(view);
        if self.launcher_presentation == crate::config::LauncherPresentation::Integrated
            && view == View::Search
        {
            self.ensure_integrated_search_host();
        }
        if matches!(view, View::Dashboard | View::Weather | View::Search) {
            self.window.set_layer(gtk4_layer_shell::Layer::Overlay);
            self.present_dismiss_catcher_behind_main();
        } else if self.search_open.get()
            && self.launcher_presentation == crate::config::LauncherPresentation::Independent
        {
            if self.search_window.is_visible() {
                self.search_window.present();
            }
            // Search promotes the island to Overlay while mapped, keeping its
            // animated origin behind the persistent pill.
            self.window.present();
        } else if !self.dismiss_catcher_needed() {
            self.window.set_layer(gtk4_layer_shell::Layer::Top);
            self.dismiss_window.set_visible(false);
        }
        if self.current_view.get() == view && self.geometry.get() == target {
            self.refresh_keyboard_mode();
            return;
        }
        let previous_view = self.current_view.get();
        let search_start_opacity = self.search.opacity();
        self.current_view.set(view);

        if self.launcher_presentation == crate::config::LauncherPresentation::Integrated
            && view == View::Search
        {
            self.search.set_visible(true);
            self.search.set_can_target(false);
            if previous_view != View::Search {
                self.search.set_opacity(0.0);
            }
        } else if self.launcher_presentation == crate::config::LauncherPresentation::Integrated {
            self.search.set_can_target(false);
        }

        for (widget, _widget_view) in self.view_widgets() {
            widget.set_visible(true);
            // Input follows the incoming CONTENT_IN track; never expose a
            // page while the outgoing page is still fading out.
            widget.set_can_target(false);
        }
        self.refresh_keyboard_mode();
        let start = self.geometry.get();
        let generation = self.animation_generation.get().wrapping_add(1);
        self.animation_generation.set(generation);
        if !self.animations_enabled.get() || self.animation_ms.get() == 0 {
            self.apply_geometry(target);
            self.finish_view(view);
            return;
        }

        let profile = profile_timing(
            if view == View::Compact {
                crate::ui::motion::Profile::CONTAINER_COLLAPSE
            } else {
                crate::ui::motion::Profile::CONTAINER_EXPAND
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let content_out = profile_timing(
            crate::ui::motion::Profile::CONTENT_OUT,
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let content_in = profile_timing(
            crate::ui::motion::Profile::CONTENT_IN,
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let transition_duration = transition_duration(profile, content_out, content_in);
        let start_time = Cell::new(None::<i64>);
        let start_opacities = self.view_widgets().map(|(widget, _)| widget.opacity());
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
            let elapsed = Duration::from_micros((now - started).max(0) as u64);
            let linear = profile.progress(elapsed);
            let eased = linear;
            island.apply_geometry(start.interpolate(target, eased));
            island.apply_content_opacity(view, previous_view, elapsed, start_opacities);
            island.apply_search_opacity(view, previous_view, elapsed, search_start_opacity);
            if elapsed >= transition_duration {
                island.finish_view(view);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Every animatable view surface paired with its view, in the fixed
    /// order the `start` opacity array is indexed by.
    fn view_widgets(&self) -> [(&gtk::Widget, View); 6] {
        [
            (&self.compact, View::Compact),
            (self.media.upcast_ref(), View::Media),
            (self.dashboard.upcast_ref(), View::Dashboard),
            (self.weather.upcast_ref(), View::Weather),
            (self.osd.upcast_ref(), View::Osd),
            (self.notification.upcast_ref(), View::Notification),
        ]
    }

    pub(super) fn apply_content_opacity(
        &self,
        target: View,
        previous: View,
        elapsed: Duration,
        start: [f64; 6],
    ) {
        let outgoing = profile_timing(
            crate::ui::motion::Profile::CONTENT_OUT,
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let incoming = profile_timing(
            crate::ui::motion::Profile::CONTENT_IN,
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let (out_progress, in_progress) =
            sequential_fade_progress(previous, target, elapsed, outgoing, incoming);
        for ((widget, widget_view), start_opacity) in self.view_widgets().into_iter().zip(start) {
            if widget_view == target {
                widget.set_opacity(lerp(start_opacity, 1.0, in_progress));
            } else if widget_view == previous && previous != target {
                widget.set_opacity(lerp(start_opacity, 0.0, out_progress));
            } else {
                widget.set_opacity(0.0);
            }
        }
    }

    fn apply_search_opacity(
        &self,
        target: View,
        previous: View,
        elapsed: Duration,
        start_opacity: f64,
    ) {
        if self.launcher_presentation != crate::config::LauncherPresentation::Integrated {
            return;
        }
        let outgoing = profile_timing(
            crate::ui::motion::Profile::CONTENT_OUT,
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let incoming = profile_timing(
            crate::ui::motion::Profile::CONTENT_IN,
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let (_, in_progress) =
            sequential_fade_progress(previous, target, elapsed, outgoing, incoming);
        if target == View::Search {
            self.search.set_can_target(in_progress > 0.0);
            let start = if previous == View::Search {
                start_opacity
            } else {
                0.0
            };
            self.search.set_opacity(lerp(start, 1.0, in_progress));
        } else if previous == View::Search {
            let (out_progress, _) =
                sequential_fade_progress(previous, target, elapsed, outgoing, incoming);
            self.search
                .set_opacity(lerp(start_opacity, 0.0, out_progress));
        }
    }

    pub(super) fn finish_view(self: &Rc<Self>, view: View) {
        self.apply_geometry(self.presentation_target_geometry(view));
        for (widget, widget_view) in self.view_widgets() {
            let active = widget_view == view;
            widget.set_visible(active);
            widget.set_opacity(if active { 1.0 } else { 0.0 });
        }
        if self.launcher_presentation == crate::config::LauncherPresentation::Integrated {
            if view == View::Search {
                finalize_integrated_search(&self.search, true);
                if self.search_focus_pending.take() {
                    self.schedule_integrated_search_focus();
                }
            } else {
                finalize_integrated_search(&self.search, false);
                self.restore_integrated_search_host();
            }
        }
    }

    pub(super) fn presentation_target_geometry(&self, view: View) -> Geometry {
        let base = self.geometry_for_view(view);
        if matches!(view, View::Compact | View::Media) {
            hover_geometry(
                base,
                f64::from(self.metrics.spacing(4)),
                self.tray_hovered.get(),
            )
        } else {
            base
        }
    }

    pub(super) fn apply_geometry(&self, geometry: Geometry) {
        self.geometry.set(geometry);
        let width = geometry.width.round() as i32;
        let height = geometry.height.round() as i32;
        let x = (self.metrics.window_width - width) / 2;
        let y = geometry.y.round() as i32;
        self.surface.set_size_request(width, height);
        self.fixed.move_(&self.surface, f64::from(x), f64::from(y));
        self.surface
            .hadjustment()
            .set_value(f64::from((self.metrics.window_width - width) / 2));
        self.surface.vadjustment().set_value(0.0);
        if let Some(surface) = self.window.surface() {
            let pill_region = !self.search_open.get()
                && matches!(self.current_view.get(), View::Compact | View::Media);
            let region_x = if pill_region {
                (self.metrics.window_width - self.metrics.media_max_width) / 2
                    - self.metrics.spacing(8)
            } else {
                x
            };
            let region_y = if pill_region { 0 } else { y };
            let region_width = if pill_region {
                self.metrics.media_max_width + self.metrics.spacing(16)
            } else {
                width
            };
            let region_height = if pill_region {
                self.metrics.compact_height + self.metrics.spacing(16)
            } else {
                height
            };
            let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                region_x,
                region_y,
                region_width,
                region_height,
            ));
            surface.set_input_region(Some(&region));
        }
        self.relayout_circles();
        self.refresh_keyboard_mode();
    }
}

/// Commits the terminal state of the integrated launcher page. Keeping this
/// in the production finish path prevents zero-duration and short-override
/// transitions from leaving a visible-but-untargetable or transparent page.
fn finalize_integrated_search(search: &gtk::Box, active: bool) {
    search.set_visible(active);
    search.set_can_target(active);
    search.set_opacity(if active { 1.0 } else { 0.0 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_fades_out_before_fading_in() {
        let outgoing = crate::ui::motion::Profile::CONTENT_OUT;
        let incoming = crate::ui::motion::Profile::CONTENT_IN;
        let (out_start, in_start) = sequential_fade_progress(
            View::Compact,
            View::Search,
            Duration::ZERO,
            outgoing,
            incoming,
        );
        assert_eq!(out_start, 0.0);
        assert_eq!(in_start, 0.0);

        let (_, in_during_out) = sequential_fade_progress(
            View::Compact,
            View::Search,
            Duration::from_millis(50),
            outgoing,
            incoming,
        );
        assert_eq!(in_during_out, 0.0);

        let (_, in_after_out) = sequential_fade_progress(
            View::Compact,
            View::Search,
            Duration::from_millis(125),
            outgoing,
            incoming,
        );
        assert!(in_after_out > 0.0);
    }

    #[test]
    fn master_transition_waits_for_all_tracks_and_honors_overrides() {
        let geometry = crate::ui::motion::Profile::CONTAINER_EXPAND;
        let outgoing = crate::ui::motion::Profile::CONTENT_OUT;
        let incoming = crate::ui::motion::Profile::CONTENT_IN;
        assert_eq!(
            transition_duration(geometry, outgoing, incoming),
            Duration::from_millis(500)
        );

        let short_geometry = geometry.with_timing(true, Some(1));
        let short_outgoing = outgoing.with_timing(true, Some(1));
        let short_incoming = incoming.with_timing(true, Some(1));
        assert_eq!(
            transition_duration(short_geometry, short_outgoing, short_incoming),
            Duration::from_millis(2)
        );

        let disabled = geometry.with_timing(false, Some(1));
        let disabled_outgoing = outgoing.with_timing(false, Some(1));
        let disabled_incoming = incoming.with_timing(false, Some(1));
        assert_eq!(
            transition_duration(disabled, disabled_outgoing, disabled_incoming),
            Duration::ZERO
        );
    }

    #[test]
    #[ignore = "requires an isolated GTK display; run with run-island-presentation-gtk.py"]
    fn finished_integrated_search_is_visible_and_targetable() {
        gtk::init().expect("GTK display");
        let search = gtk::Box::new(gtk::Orientation::Vertical, 0);
        finalize_integrated_search(&search, true);
        assert!(search.is_visible());
        assert!(search.can_target());
        assert_eq!(search.opacity(), 1.0);
        finalize_integrated_search(&search, false);
        assert!(!search.is_visible());
        assert!(!search.can_target());
        assert_eq!(search.opacity(), 0.0);
    }
}
