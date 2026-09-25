//! View reconciliation: picking the active view, animating geometry and
//! opacity between views, and applying the result to the layer surface.

use super::*;

use std::{rc::Rc, time::Duration};

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
        self.refresh_dismiss_input_region();
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
        // Legacy media-only configurations still use their separate header.
        // Keep the battery attached to the visible header, never the panel.
        let compact_overlay = self
            .compact
            .downcast_ref::<gtk::Overlay>()
            .expect("compact overlay");
        if view == View::Media
            && self.battery_waves.area.parent().as_ref() != Some(self.surface_shell.upcast_ref())
        {
            compact_overlay.set_child(None::<&gtk::Widget>);
            self.surface_shell.set_child(Some(&self.battery_waves.area));
        } else if view != View::Media
            && self.battery_waves.area.parent().as_ref() != Some(self.compact.upcast_ref())
        {
            self.surface_shell.set_child(None::<&gtk::Widget>);
            compact_overlay.set_child(Some(&self.battery_waves.area));
        }
        self.battery_waves
            .set_active(pill_view || view == View::Dashboard);
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
        if self.current_view.get() == view {
            if self.view_animation_target.get() == Some(target) {
                return;
            }
            if matches!(view, View::Compact | View::Media) && !self.view_transition_active.get() {
                self.reconcile_pill_geometry();
                self.refresh_keyboard_mode();
                return;
            }
        }
        let previous_view = self.current_view.get();
        let section_start = self.dashboard_expansion.get();
        let section_opacity_start = if view == View::Dashboard && section_start == 0.0 {
            1.0
        } else {
            self.dashboard_section_opacity.get()
        };
        let search_start_opacity = self.search.opacity();
        let date_start_opacity = if self.compact_date.is_visible() {
            self.compact_date.opacity()
        } else {
            0.0
        };
        self.current_view.set(view);
        if view == View::Dashboard {
            self.dashboard.set_visible(true);
            self.dashboard.set_opacity(1.0);
            self.apply_dashboard_sections(
                section_start,
                section_opacity_start,
                self.geometry.get().width,
            );
            self.compact_date.set_visible(true);
        } else if view == View::Compact {
            let peek = self.island_hovered.get();
            self.dashboard
                .set_visible(peek || self.dashboard.is_visible());
            self.dashboard.set_can_target(peek);
            self.compact_date
                .set_visible(peek || date_start_opacity > 0.0);
        }
        self.compact_date.set_opacity(date_start_opacity);
        self.sync_island_surface_style();
        // Map the incoming host before the first frame-clock callback. A
        // legacy media/page transition may be coming from the hidden canvas.
        self.sync_island_panel_geometry(self.geometry.get());
        // Establish the incoming pill's position before GTK maps/reallocates
        // it; the first transition frame must not inherit the old y=0 slot.
        self.sync_pill_content_geometry(self.geometry.get());
        // A page transition owns the shared surface geometry until its
        // terminal cleanup. Invalidate any older pill-only track, but keep
        // the page generation independent so a hover/width reconciliation
        // cannot strand an incoming page at opacity zero.
        self.pill_animation_generation
            .set(self.pill_animation_generation.get().wrapping_add(1));
        self.pill_animation_target.set(None);

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
            widget.set_can_target(view == View::Dashboard && widget == &self.compact);
        }
        self.refresh_keyboard_mode();
        let start = self.geometry.get();
        let generation = self.view_animation_generation.get().wrapping_add(1);
        self.view_animation_generation.set(generation);
        self.view_animation_target.set(Some(target));
        self.view_transition_active.set(true);
        if !self.animations_enabled.get() || self.animation_ms.get() == 0 {
            self.apply_geometry(target);
            self.compact_date.set_opacity(
                if view == View::Dashboard || (view == View::Compact && self.island_hovered.get()) {
                    1.0
                } else {
                    0.0
                },
            );
            self.finish_view(view);
            return;
        }

        let island_transition = view == View::Dashboard
            || previous_view == View::Dashboard
            || (view == View::Compact && section_start > 0.0);
        let profile = profile_timing(
            if island_transition {
                if view == View::Dashboard {
                    crate::ui::motion::Profile::ISLAND_EXPAND
                } else {
                    crate::ui::motion::Profile::ISLAND_COLLAPSE
                }
            } else if view == View::Compact {
                crate::ui::motion::Profile::CONTAINER_COLLAPSE
            } else {
                crate::ui::motion::Profile::CONTAINER_EXPAND
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let content_out = profile_timing(
            if island_transition {
                crate::ui::motion::Profile::ISLAND_FADE_OUT
            } else {
                crate::ui::motion::Profile::CONTENT_OUT
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let content_in = profile_timing(
            if island_transition {
                crate::ui::motion::Profile::ISLAND_FADE_IN
            } else {
                crate::ui::motion::Profile::CONTENT_IN
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let transition_duration = if island_transition {
            profile.duration
        } else {
            transition_duration(profile, content_out, content_in)
        };
        let started = Instant::now();
        let start_opacities = self.view_widgets().map(|(widget, _)| widget.opacity());
        let weak = Rc::downgrade(self);
        self.fixed.add_tick_callback(move |_, _| {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if island.view_animation_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let elapsed = started.elapsed();
            let linear = profile.progress(elapsed);
            let eased = linear;
            let geometry = start.interpolate(target, eased);
            let sections_open = view == View::Dashboard;
            let section_opacity = if sections_open {
                // New content is already opaque before expansion; only an
                // interrupted exit needs its partial opacity restored smoothly.
                eased
            } else {
                // The exiting sections fade first while their allocated space
                // contracts on the same Material geometry track as the panel.
                content_out.progress(elapsed)
            };
            island.apply_dashboard_sections(
                lerp(section_start, if sections_open { 1.0 } else { 0.0 }, eased),
                lerp(
                    section_opacity_start,
                    if sections_open { 1.0 } else { 0.0 },
                    section_opacity,
                ),
                geometry.width,
            );
            island.apply_geometry(geometry);
            island.apply_content_opacity(view, previous_view, elapsed, start_opacities);
            island.apply_search_opacity(view, previous_view, elapsed, search_start_opacity);
            island.apply_island_date_opacity(view, elapsed, date_start_opacity);
            // Opacity/visibility changes can cause GTK to reallocate an
            // incoming page after the geometry write. Re-apply the pill
            // position last so the mapped child cannot snap back to y=0.
            island.sync_pill_content_geometry(geometry);
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
        let island_transition = target == View::Dashboard
            || previous == View::Dashboard
            || (target == View::Compact && start[2] > 0.0);
        let outgoing = profile_timing(
            if island_transition {
                crate::ui::motion::Profile::ISLAND_FADE_OUT
            } else {
                crate::ui::motion::Profile::CONTENT_OUT
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let incoming = profile_timing(
            if island_transition {
                crate::ui::motion::Profile::ISLAND_FADE_IN
            } else {
                crate::ui::motion::Profile::CONTENT_IN
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        let (out_progress, in_progress) = if island_transition {
            let duration = self.island_transition_duration(target == View::Dashboard);
            (
                self.island_fade_progress(false, elapsed, duration),
                self.island_fade_progress(true, elapsed, duration),
            )
        } else {
            sequential_fade_progress(previous, target, elapsed, outgoing, incoming)
        };
        for ((widget, widget_view), start_opacity) in self.view_widgets().into_iter().zip(start) {
            if (matches!(target, View::Compact | View::Dashboard) && widget_view == View::Compact)
                || (target == View::Dashboard && widget_view == View::Dashboard)
            {
                widget.set_opacity(1.0);
            } else if widget_view == View::Dashboard && target == View::Compact {
                let peek = self.island_hovered.get();
                widget.set_opacity(lerp(
                    start_opacity,
                    if peek { 1.0 } else { 0.0 },
                    if peek { in_progress } else { out_progress },
                ));
            } else if widget_view == target {
                widget.set_opacity(lerp(start_opacity, 1.0, in_progress));
            } else if widget_view == previous && previous != target {
                widget.set_opacity(lerp(start_opacity, 0.0, out_progress));
            } else {
                widget.set_opacity(0.0);
            }
        }
    }

    fn apply_island_date_opacity(&self, target: View, elapsed: Duration, start: f64) {
        if !matches!(target, View::Dashboard | View::Compact) {
            return;
        }
        let show = target == View::Dashboard || self.island_hovered.get();
        if !show && self.current_view.get() != View::Compact {
            return;
        }
        let progress =
            self.island_fade_progress(show, elapsed, self.island_transition_duration(show));
        self.compact_date
            .set_opacity(lerp(start, if show { 1.0 } else { 0.0 }, progress));
    }

    fn island_transition_duration(&self, expanding: bool) -> Duration {
        profile_timing(
            if expanding {
                crate::ui::motion::Profile::ISLAND_EXPAND
            } else {
                crate::ui::motion::Profile::ISLAND_COLLAPSE
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        )
        .duration
    }

    /// Fade within the geometry track, even with very short duration overrides.
    /// Exits retain their content until the final part of the roll-up.
    pub(super) fn island_fade_progress(
        &self,
        show: bool,
        elapsed: Duration,
        geometry_duration: Duration,
    ) -> f64 {
        let mut profile = profile_timing(
            if show {
                crate::ui::motion::Profile::ISLAND_FADE_IN
            } else {
                crate::ui::motion::Profile::ISLAND_FADE_OUT
            },
            self.animations_enabled.get(),
            self.animation_ms.get(),
        );
        profile.duration = profile.duration.min(geometry_duration);
        let available_delay = geometry_duration.saturating_sub(profile.duration);
        let delay = if show {
            crate::ui::motion::duration::ISLAND_ENTER_FADE_DELAY.min(available_delay)
        } else {
            available_delay
        };
        profile.progress(elapsed.saturating_sub(delay))
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
        self.view_transition_active.set(false);
        self.view_animation_target.set(None);
        let geometry = self.presentation_target_geometry(view);
        let amount = if view == View::Dashboard { 1.0 } else { 0.0 };
        self.apply_dashboard_sections(amount, amount, geometry.width);
        self.apply_geometry(geometry);
        self.sync_pill_content_geometry(geometry);
        for (widget, widget_view) in self.view_widgets() {
            let active = widget_view == view
                || (view == View::Dashboard && widget_view == View::Compact)
                || (view == View::Compact
                    && self.island_hovered.get()
                    && widget_view == View::Dashboard);
            widget.set_visible(active);
            widget.set_can_target(active);
            if !(view == View::Dashboard && widget_view == View::Compact) {
                widget.set_opacity(if active { 1.0 } else { 0.0 });
            }
        }
        if view == View::Dashboard {
            self.dashboard.set_visible(true);
            self.compact_date.set_visible(true);
            self.compact_date.set_opacity(1.0);
        } else if view == View::Compact && !self.island_hovered.get() {
            self.compact_date.set_visible(false);
            self.compact_date.set_opacity(0.0);
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
        // The legacy density classes have a readable font-size floor. Keep the
        // reference's minimum content width even when the idle pill is tiny.
        let scale_width =
            |width: i32| f64::from(width) * (self.metrics.scale * 32.0 / 52.0).max(1.0) * 1.1;
        let lift = self.metrics.spacing(16);
        let bottom_clearance = self.metrics.spacing(16);
        let top_margin = if self.window.is_layer_window() {
            self.window.margin(gtk4_layer_shell::Edge::Top)
        } else {
            0
        };
        let max_height = (self.metrics.monitor_height - top_margin - lift - bottom_clearance)
            .min(self.metrics.window_height - lift - bottom_clearance)
            .max(self.metrics.compact_height + 1);
        if view == View::Dashboard || (view == View::Compact && self.island_hovered.get()) {
            // GTK reports zero for an invisible widget. Measure the incoming
            // panel before starting the track, including direct Idle -> Open.
            let was_visible = self.dashboard.is_visible();
            self.dashboard.set_visible(true);
            let width = scale_width(if view == View::Dashboard { 640 } else { 500 })
                .max(f64::from(
                    self.dashboard.measure(Orientation::Horizontal, -1).0
                        + self.metrics.spacing(44)
                        + 12,
                ))
                .min(f64::from(
                    self.metrics.window_width - self.metrics.spacing(80),
                ));
            let panel_width = width.round() as i32 - self.metrics.spacing(44);
            self.layout_dashboard_sections(if view == View::Dashboard { 1.0 } else { 0.0 }, width);
            let natural = self
                .dashboard
                .measure(Orientation::Vertical, panel_width - 12)
                .1
                + 4;
            // Target measurement must never unmount the still-rendered Open
            // sections. Restore the current presentation before GTK allocates.
            self.layout_dashboard_sections(
                self.dashboard_expansion.get(),
                self.geometry.get().width,
            );
            self.dashboard.set_visible(was_visible);
            Geometry {
                width,
                height: (f64::from(self.metrics.compact_height) * 0.55 + f64::from(natural))
                    .min(f64::from(max_height)),
                y: f64::from(lift) - f64::from(self.metrics.compact_height) * 0.05,
            }
        } else if matches!(view, View::Compact | View::Media) {
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
        self.sync_island_surface_style();
        let width = geometry.width.round() as i32;
        let height = geometry.height.round() as i32;
        let x = (self.metrics.window_width - width) / 2;
        let y = geometry.y.round() as i32;
        self.surface_shell.set_size_request(width, height);
        self.surface.set_size_request(width, height);
        self.fixed
            .move_(&self.surface_shell, f64::from(x), f64::from(y));
        self.surface
            .hadjustment()
            .set_value(f64::from((self.metrics.window_width - width) / 2));
        self.surface.vadjustment().set_value(0.0);
        self.sync_island_panel_geometry(geometry);
        if let Some(surface) = self.window.surface() {
            let region = self.island_input_region(geometry);
            surface.set_input_region(Some(&region));
        }
        self.relayout_circles();
        self.refresh_keyboard_mode();
    }

    fn sync_island_panel_geometry(&self, geometry: Geometry) {
        self.surface.set_visible(!matches!(
            self.current_view.get(),
            View::Compact | View::Dashboard
        ));
        if !matches!(self.current_view.get(), View::Compact | View::Dashboard) {
            self.panel_scroll.set_visible(false);
            return;
        }
        let expanded = self.current_view.get() == View::Dashboard || self.island_hovered.get();
        let header_height = self.persistent_header_height(geometry);
        let pill_width = geometry.width.round() as i32;
        self.compact.set_size_request(pill_width, header_height);
        self.compact.set_margin_top(0);
        self.panel_scroll
            .set_visible(expanded || geometry.height > f64::from(header_height + 1));
        self.panel_scroll.set_margin_start(self.metrics.spacing(22));
        self.panel_scroll.set_margin_end(self.metrics.spacing(22));
        self.panel_scroll.set_margin_top(header_height / 2);
        self.dashboard.set_size_request(-1, -1);
        self.layout_dashboard_sections(self.dashboard_expansion.get(), geometry.width);
    }

    fn layout_dashboard_sections(&self, amount: f64, width: f64) {
        #[allow(deprecated)]
        let padding = self.dashboard.style_context().padding();
        let inner_width = width.round() as i32
            - self.metrics.spacing(44)
            - 4
            - i32::from(padding.left())
            - i32::from(padding.right());
        let gap = (f64::from(self.metrics.spacing(8)) * amount).round() as i32;
        self.hardware.root.set_margin_bottom(gap);
        for section in &self.dashboard_sections {
            if amount <= 0.0 {
                // Pre-map incoming dashboard content behind the zero-height
                // clip; the first revealed pixels must already contain it.
                section
                    .viewport
                    .set_visible(self.current_view.get() == View::Dashboard);
                section.viewport.set_height_request(0);
                section.viewport.set_margin_bottom(0);
                continue;
            }
            let minimum = section.content.measure(Orientation::Horizontal, -1).0;
            let natural = section
                .content
                .measure(Orientation::Vertical, inner_width.max(minimum))
                .1;
            section
                .viewport
                .set_height_request((f64::from(natural) * amount).round() as i32);
            section
                .viewport
                .set_margin_bottom(if section.trailing_gap { gap } else { 0 });
            section.viewport.set_visible(amount > 0.0);
        }
    }

    fn apply_dashboard_sections(&self, amount: f64, opacity: f64, width: f64) {
        self.dashboard_expansion.set(amount);
        self.dashboard_section_opacity.set(opacity);
        self.layout_dashboard_sections(amount, width);
        for section in &self.dashboard_sections {
            section.viewport.set_opacity(opacity);
            section
                .viewport
                .set_can_target(self.current_view.get() == View::Dashboard && opacity > 0.0);
        }
    }

    pub(super) fn sync_island_surface_style(&self) {
        if matches!(self.current_view.get(), View::Compact | View::Dashboard) {
            self.surface_shell.add_css_class("island-persistent");
        } else {
            self.surface_shell.remove_css_class("island-persistent");
        }
        let expanded = self.current_view.get() == View::Dashboard
            || (self.current_view.get() == View::Compact
                && (self.island_hovered.get()
                    || self.geometry.get().height > f64::from(self.metrics.compact_height + 1)));
        if expanded {
            self.surface_shell.add_css_class("island-expanded");
            self.compact.add_css_class("island-expanded-pill");
        } else {
            self.surface_shell.remove_css_class("island-expanded");
            self.compact.remove_css_class("island-expanded-pill");
        }
    }

    pub(super) fn persistent_header_height(&self, geometry: Geometry) -> i32 {
        let expanded_y =
            f64::from(self.metrics.spacing(16)) - f64::from(self.metrics.compact_height) * 0.05;
        let amount = (geometry.y / expanded_y.max(1.0)).clamp(0.0, 1.0);
        (f64::from(self.metrics.compact_height) * (1.0 + 0.1 * amount)).round() as i32
    }

    pub(super) fn island_input_region(&self, geometry: Geometry) -> gtk::cairo::Region {
        let width = geometry.width.round() as i32;
        let height = geometry.height.round() as i32;
        let x = (self.metrics.window_width - width) / 2;
        let y = geometry.y.round() as i32;
        if !matches!(
            self.current_view.get(),
            View::Compact | View::Media | View::Dashboard
        ) {
            return gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                x,
                y,
                width.max(1),
                height.max(1),
            ));
        }
        let pill_width = if matches!(self.current_view.get(), View::Compact | View::Dashboard) {
            width
        } else {
            self.metrics.media_max_width
        };
        let pill_x = (self.metrics.window_width - pill_width) / 2;
        let pill_y = y;
        let header_height = if self.current_view.get() == View::Media {
            height
        } else {
            self.persistent_header_height(geometry)
        };
        let pill =
            gtk::cairo::RectangleInt::new(pill_x, pill_y, pill_width.max(1), header_height.max(1));
        let region = gtk::cairo::Region::create_rectangle(&pill);
        subtract_capsule_corners(&region, pill_x, pill_y, pill_width, header_height);
        if self.current_view.get() == View::Compact {
            // A short, invisible strip above the pill bridges its downward
            // hover lift without turning the whole layer window into a target.
            let bridge_h = self.metrics.spacing(20).max(1);
            let bridge_y = (pill_y - bridge_h).max(0);
            let bridge =
                gtk::cairo::RectangleInt::new(pill_x, bridge_y, pill_width, pill_y - bridge_y);
            let _ = region.union_rectangle(&bridge);
        }
        if self.current_view.get() == View::Compact && self.island_hovered.get()
            || self.current_view.get() == View::Dashboard
        {
            let inset = self.metrics.spacing(22);
            let panel_x = x + inset;
            let panel_y = y + header_height / 2;
            let panel_w = (width - inset * 2).max(1);
            let panel_h = (height - header_height / 2).max(1);
            let radius = if self.current_view.get() == View::Dashboard {
                self.metrics.spacing(30)
            } else {
                self.metrics.spacing(22)
            };
            let _ = union_rounded_panel(&region, panel_x, panel_y, panel_w, panel_h, radius);
        }
        region
    }
}

fn subtract_capsule_corners(region: &gtk::cairo::Region, x: i32, y: i32, width: i32, height: i32) {
    let radius = (height / 2).clamp(0, width / 2);
    let rows = 8;
    for row in 0..rows {
        let t = (f64::from(row) + 0.5) / f64::from(rows);
        let dy = (f64::from(radius) * (1.0 - 2.0 * t)).abs();
        let inset = (f64::from(radius) - (f64::from(radius.pow(2)) - dy * dy).max(0.0).sqrt())
            .ceil() as i32;
        let top = y + row * radius / rows;
        let bottom = y + height - (row + 1) * radius / rows;
        let strip_h = (radius / rows).max(1);
        for (strip_y, cut_h) in [(top, strip_h), (bottom, strip_h)] {
            if inset > 0 {
                let left = gtk::cairo::RectangleInt::new(x, strip_y, inset, cut_h);
                let right = gtk::cairo::RectangleInt::new(x + width - inset, strip_y, inset, cut_h);
                let _ = region.subtract_rectangle(&left);
                let _ = region.subtract_rectangle(&right);
            }
        }
    }
}

fn union_rounded_panel(
    region: &gtk::cairo::Region,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: i32,
) -> Result<(), gtk::cairo::Error> {
    let radius = radius.clamp(0, width.min(height) / 2);
    let strips = 12;
    for strip in 0..strips {
        let y0 = height * strip / strips;
        let y1 = height * (strip + 1) / strips;
        let inset = if strip >= strips - 4 {
            let from_bottom = f64::from(strip - (strips - 4) + 1) / 4.0;
            (f64::from(radius) * (1.0 - (1.0 - from_bottom * from_bottom).sqrt())).round() as i32
        } else {
            0
        };
        let rect = gtk::cairo::RectangleInt::new(
            x + inset,
            y + y0,
            (width - inset * 2).max(1),
            (y1 - y0).max(1),
        );
        region.union_rectangle(&rect)?;
    }
    Ok(())
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
    #[ignore = "requires an isolated GTK display; run scripts/run-ui-regressions-gtk.py"]
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
