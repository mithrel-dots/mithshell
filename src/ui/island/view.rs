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

        for (widget, widget_view) in self.view_widgets() {
            widget.set_visible(true);
            widget.set_can_target(widget_view == view && widget_view != View::Osd);
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

        let duration_us = i64::from(self.animation_ms.get()) * 1000;
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

    /// Every animatable view surface paired with its view, in the fixed
    /// order the `start` opacity array is indexed by.
    fn view_widgets(&self) -> [(&gtk::Box, View); 7] {
        [
            (&self.compact, View::Compact),
            (&self.media, View::Media),
            (&self.dashboard, View::Dashboard),
            (&self.search, View::Search),
            (&self.weather, View::Weather),
            (&self.osd, View::Osd),
            (&self.notification, View::Notification),
        ]
    }

    pub(super) fn apply_content_opacity(&self, target: View, progress: f64, start: [f64; 7]) {
        let progress = 1.0 - (1.0 - progress).powi(3);
        for ((widget, widget_view), start_opacity) in self.view_widgets().into_iter().zip(start) {
            let end = if widget_view == target { 1.0 } else { 0.0 };
            widget.set_opacity(lerp(start_opacity, end, progress));
        }
    }

    pub(super) fn finish_view(&self, view: View) {
        self.apply_geometry(self.geometry_for_view(view));
        for (widget, widget_view) in self.view_widgets() {
            let active = widget_view == view;
            widget.set_visible(active);
            widget.set_opacity(if active { 1.0 } else { 0.0 });
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
