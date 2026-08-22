//! View reconciliation: picking the active view, animating geometry and
//! opacity between views, and applying the result to the layer surface.

use super::*;

use std::{cell::Cell, rc::Rc};

use gtk4_layer_shell::KeyboardMode;

use super::{Geometry, IslandWindow, View};

impl IslandWindow {
    /// Re-applies the keyboard mode the current state wants. Split out of
    /// `set_view` so showing/dismissing a tray menu can borrow the surface's
    /// focus without having to know what the active view expects.
    pub(super) fn refresh_keyboard_mode(&self) {
        let mode = match self.current_view.get() {
            View::Search | View::Weather => KeyboardMode::Exclusive,
            _ if self.tray_menu_open.get() => KeyboardMode::OnDemand,
            _ => KeyboardMode::None,
        };
        self.window.set_keyboard_mode(mode);
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
        } else if self.search_open.get() {
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
        let target = self.geometry_for_view(view);
        if self.current_view.get() == view && self.geometry.get() == target {
            return;
        }
        self.current_view.set(view);
        if matches!(view, View::Dashboard | View::Search | View::Weather) {
            self.dismiss_window.present();
            self.window.present();
        } else {
            self.dismiss_window.set_visible(false);
        }

        self.compact.set_visible(true);
        self.media.set_visible(true);
        self.dashboard.set_visible(true);
        self.search.set_visible(true);
        self.weather.set_visible(true);
        self.osd.set_visible(true);
        self.notification.set_visible(true);
        self.compact.set_can_target(view == View::Compact);
        self.media.set_can_target(view == View::Media);
        self.dashboard.set_can_target(view == View::Dashboard);
        self.search.set_can_target(view == View::Search);
        self.weather.set_can_target(view == View::Weather);
        self.osd.set_can_target(false);
        self.notification.set_can_target(view == View::Notification);
        self.refresh_keyboard_mode();
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
        let start_opacities = [
            self.compact.opacity(),
            self.media.opacity(),
            self.dashboard.opacity(),
            self.search.opacity(),
            self.weather.opacity(),
            self.osd.opacity(),
            self.notification.opacity(),
        ];
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
            island.apply_content_opacity(view, linear, start_opacities);
            if linear >= 1.0 {
                island.finish_view(view);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    pub(super) fn apply_content_opacity(&self, target: View, progress: f64, start: [f64; 7]) {
        let progress = 1.0 - (1.0 - progress).powi(3);
        let compact_target = if target == View::Compact { 1.0 } else { 0.0 };
        let media_target = if target == View::Media { 1.0 } else { 0.0 };
        let dashboard_target = if target == View::Dashboard { 1.0 } else { 0.0 };
        let search_target = if target == View::Search { 1.0 } else { 0.0 };
        let weather_target = if target == View::Weather { 1.0 } else { 0.0 };
        let osd_target = if target == View::Osd { 1.0 } else { 0.0 };
        let notification_target = if target == View::Notification {
            1.0
        } else {
            0.0
        };
        self.compact
            .set_opacity(lerp(start[0], compact_target, progress));
        self.media
            .set_opacity(lerp(start[1], media_target, progress));
        self.dashboard
            .set_opacity(lerp(start[2], dashboard_target, progress));
        self.search
            .set_opacity(lerp(start[3], search_target, progress));
        self.weather
            .set_opacity(lerp(start[4], weather_target, progress));
        self.osd.set_opacity(lerp(start[5], osd_target, progress));
        self.notification
            .set_opacity(lerp(start[6], notification_target, progress));
    }

    pub(super) fn finish_view(&self, view: View) {
        self.apply_geometry(self.geometry_for_view(view));
        self.compact.set_visible(view == View::Compact);
        self.media.set_visible(view == View::Media);
        self.dashboard.set_visible(view == View::Dashboard);
        self.search.set_visible(view == View::Search);
        self.weather.set_visible(view == View::Weather);
        self.osd.set_visible(view == View::Osd);
        self.notification.set_visible(view == View::Notification);
        self.compact
            .set_opacity(if view == View::Compact { 1.0 } else { 0.0 });
        self.media
            .set_opacity(if view == View::Media { 1.0 } else { 0.0 });
        self.dashboard
            .set_opacity(if view == View::Dashboard { 1.0 } else { 0.0 });
        self.search
            .set_opacity(if view == View::Search { 1.0 } else { 0.0 });
        self.weather
            .set_opacity(if view == View::Weather { 1.0 } else { 0.0 });
        self.osd
            .set_opacity(if view == View::Osd { 1.0 } else { 0.0 });
        self.notification
            .set_opacity(if view == View::Notification { 1.0 } else { 0.0 });
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
            .set_value(f64::from((self.metrics.search_width - width) / 2));
        self.surface.vadjustment().set_value(0.0);
        if let Some(surface) = self.window.surface() {
            let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(
                x, y, width, height,
            ));
            surface.set_input_region(Some(&region));
        }
    }
}
