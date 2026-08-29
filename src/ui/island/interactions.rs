//! Input wiring: gestures, motion controllers, keyboard capture, and the
//! widget callbacks that translate them into `IslandActions`.

use super::*;

use std::{rc::Rc, time::Duration};

use gtk::{EventControllerMotion, GestureClick, gdk, glib};

use super::{IslandWindow, OverlayButtons, dominant_scroll_direction};

impl IslandWindow {
    pub(super) fn connect_interactions(
        self: &Rc<Self>,
        buttons: OverlayButtons<'_>,
        dismiss_area: &gtk::Box,
    ) {
        let OverlayButtons {
            close_button,
            search_button,
            weather_button,
            search_back_button,
            search_reload_button,
            weather_back_button,
        } = buttons;
        for pill in [&self.compact, &self.media] {
            let click = GestureClick::new();
            let weak = Rc::downgrade(self);
            click.connect_released(move |gesture, _, _, _| {
                if gesture.current_button() == 1
                    && let Some(island) = weak.upgrade()
                {
                    island.toggle();
                }
            });
            pill.add_controller(click);

            let motion = EventControllerMotion::new();
            let weak = Rc::downgrade(self);
            motion.connect_enter(move |_, _, _| {
                if let Some(island) = weak.upgrade() {
                    island.set_tray_hovered(true);
                }
            });
            let weak = Rc::downgrade(self);
            motion.connect_leave(move |_| {
                if let Some(island) = weak.upgrade() {
                    island.set_tray_hovered(false);
                }
            });
            pill.add_controller(motion);
        }

        let mut notification_views = vec![self.notification.clone()];
        if let Some(overlay) = &self.pill_overlay {
            notification_views.push(overlay.root.clone());
        }
        for notification in notification_views {
            let click = GestureClick::new();
            click.set_button(0);
            let weak = Rc::downgrade(self);
            click.connect_released(move |gesture, _, _, _| {
                let Some(island) = weak.upgrade() else {
                    return;
                };
                match gesture.current_button() {
                    gdk::BUTTON_PRIMARY => island.activate_current_notification(),
                    gdk::BUTTON_SECONDARY => island.dismiss_current_notification(),
                    _ => {}
                }
            });
            notification.add_controller(click);
        }

        let weak = Rc::downgrade(self);
        search_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.open_search();
            }
        });

        let weak = Rc::downgrade(self);
        search_back_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.open();
            }
        });

        let weak = Rc::downgrade(self);
        weather_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.open_weather();
            }
        });

        let weak = Rc::downgrade(self);
        weather_back_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.weather_open.set(false);
                island.dashboard_open.set(true);
                island.reconcile_view();
            }
        });

        let weak = Rc::downgrade(self);
        search_reload_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.search_status.set_label("RELOADING TARRAGON");
                (island.actions.tarragon_reload)();
            }
        });

        let weak = Rc::downgrade(self);
        self.search_plugin_toggle.connect_toggled(move |button| {
            if let Some(island) = weak.upgrade() {
                if button.is_active() {
                    island.search_stack.set_visible_child_name("plugins");
                    // Rendering is skipped while the pane is hidden, so build
                    // it now that it is about to be shown.
                    island.render_plugin_list();
                    island.show_plugin_summary();
                    (island.actions.tarragon_status)();
                } else {
                    island.search_stack.set_visible_child_name("results");
                    let snapshot = island.search_snapshot.borrow().clone();
                    if let Some(snapshot) = snapshot {
                        island.update_tarragon_results(&snapshot);
                    } else {
                        island.search_status.set_label("READY  //  TYPE TO SEARCH");
                    }
                    island.search_entry.grab_focus();
                }
            }
        });

        // GtkSearchEntry withholds `search-changed` for 150ms by default. That
        // sits on top of our own debounce, so disable it and let
        // `schedule_search` own the coalescing window.
        self.search_entry.set_search_delay(0);
        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |entry| {
            if let Some(island) = weak.upgrade() {
                island.schedule_search(entry.text().to_string());
            }
        });

        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |_| {
            if let Some(island) = weak.upgrade() {
                let index = island
                    .search_results
                    .selected_row()
                    .map_or(0, |row| row.index());
                island.activate_search_result(index);
            }
        });

        let weak = Rc::downgrade(self);
        self.search_results.connect_row_activated(move |_, row| {
            if let Some(island) = weak.upgrade() {
                island.activate_search_result(row.index());
            }
        });

        let weak = Rc::downgrade(self);
        self.search_results.connect_row_selected(move |_, row| {
            if let (Some(island), Some(row)) = (weak.upgrade(), row) {
                island.update_search_preview(row.index());
            }
        });

        let search_keys = gtk::EventControllerKey::new();
        search_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        search_keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(island) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let results_active = island.search_open.get()
                && island.search_stack.visible_child_name().as_deref() == Some("results");
            // t0: capture the moment a text-mutating key arrives, before the
            // entry's own search-delay and the debounce timer run.
            if island.search_open.get()
                && (key.to_unicode().is_some()
                    || matches!(key, gdk::Key::BackSpace | gdk::Key::Delete))
            {
                crate::latency::mark_keystroke();
            }
            match key {
                gdk::Key::Escape => {
                    island.close();
                    glib::Propagation::Stop
                }
                gdk::Key::Return | gdk::Key::KP_Enter
                    if results_active && modifiers.contains(gdk::ModifierType::SHIFT_MASK) =>
                {
                    let index = island
                        .search_results
                        .selected_row()
                        .map_or(0, |row| row.index());
                    if let Some(row) = island.search_results.row_at_index(index) {
                        island.open_search_actions(index, &row);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Down if results_active => {
                    island.move_search_selection(1);
                    glib::Propagation::Stop
                }
                gdk::Key::Up if results_active => {
                    island.move_search_selection(-1);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.search_window.add_controller(search_keys);

        let overlay_keys = gtk::EventControllerKey::new();
        overlay_keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        overlay_keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape
                && let Some(island) = weak.upgrade()
            {
                island.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.window.add_controller(overlay_keys);

        // t5: the frame carrying the updated results reached the compositor.
        // Only wired when tracing is on, so normal runs pay nothing.
        if crate::latency::enabled() {
            self.search_window.connect_realize(|window| {
                if let Some(clock) = window.frame_clock() {
                    clock.connect_after_paint(|_| crate::latency::mark_paint());
                }
            });
        }

        for workspaces in [&self.compact_workspaces, &self.media_workspaces] {
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::BOTH_AXES
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            let weak = Rc::downgrade(self);
            scroll.connect_scroll(move |_, dx, dy| {
                if let Some(island) = weak.upgrade() {
                    island.scroll_workspace(dx, dy);
                }
                glib::Propagation::Stop
            });
            workspaces.add_controller(scroll);
        }

        let media_scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let weak = Rc::downgrade(self);
        media_scroll.connect_scroll(move |_, dx, dy| {
            if let Some(island) = weak.upgrade() {
                island.scroll_volume(dx, dy);
            }
            glib::Propagation::Stop
        });
        self.media_center.add_controller(media_scroll);

        let weak = Rc::downgrade(self);
        self.surface
            .hadjustment()
            .connect_value_changed(move |adjustment| {
                if let Some(island) = weak.upgrade() {
                    let target = f64::from(
                        (island.metrics.window_width - island.geometry.get().width.round() as i32)
                            / 2,
                    );
                    if (adjustment.value() - target).abs() > f64::EPSILON {
                        adjustment.set_value(target);
                    }
                }
            });
        self.surface
            .vadjustment()
            .connect_value_changed(|adjustment| {
                if adjustment.value().abs() > f64::EPSILON {
                    adjustment.set_value(0.0);
                }
            });

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
                && y <= f64::from(island.metrics.dashboard_header_height)
            {
                island.close();
            }
        });
        self.dashboard.add_controller(header_click);

        let weather_header_click = GestureClick::new();
        weather_header_click.set_propagation_phase(gtk::PropagationPhase::Bubble);
        let weak = Rc::downgrade(self);
        weather_header_click.connect_released(move |gesture, _, _, y| {
            if gesture.current_button() == 1
                && y <= f64::from(
                    weak.upgrade()
                        .map_or(78, |island| island.metrics.spacing(78)),
                )
                && let Some(island) = weak.upgrade()
            {
                island.close();
            }
        });
        self.weather.add_controller(weather_header_click);

        let weak = Rc::downgrade(self);
        close_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.close();
            }
        });

        let clear_notifications = self.actions.notification_clear_all.clone();
        self.notification_clear_button
            .connect_clicked(move |_| clear_notifications());

        let weak = Rc::downgrade(self);
        self.notification_inhibit_button
            .connect_toggled(move |button| {
                if let Some(island) = weak.upgrade()
                    && !island.updating_notification_inhibit.get()
                {
                    (island.actions.notification_inhibit)(button.is_active());
                }
            });

        let weak = Rc::downgrade(self);
        self.notification_expand_button
            .connect_toggled(move |button| {
                if let Some(island) = weak.upgrade() {
                    island.notifications_expanded.set(button.is_active());
                    island.apply_notification_takeover();
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

        let weak = Rc::downgrade(self);
        self.player_prev_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade()
                && let Some(service) = island
                    .latest_media
                    .borrow()
                    .as_ref()
                    .map(|state| state.service.clone())
            {
                (island.actions.media_previous)(service);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_play_pause_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade()
                && let Some(service) = island
                    .latest_media
                    .borrow()
                    .as_ref()
                    .map(|state| state.service.clone())
            {
                (island.actions.media_play_pause)(service);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_next_button.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade()
                && let Some(service) = island
                    .latest_media
                    .borrow()
                    .as_ref()
                    .map(|state| state.service.clone())
            {
                (island.actions.media_next)(service);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_switch_prev.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.switch_media_player(-1);
            }
        });

        let weak = Rc::downgrade(self);
        self.player_switch_next.connect_clicked(move |_| {
            if let Some(island) = weak.upgrade() {
                island.switch_media_player(1);
            }
        });
    }

    fn scroll_workspace(&self, dx: f64, dy: f64) {
        let direction = dominant_scroll_direction(dx, dy);
        let Some(active) = self
            .latest_hyprland
            .borrow()
            .monitor(&self.monitor_name)
            .map(|monitor| monitor.active_workspace.id)
        else {
            return;
        };
        let target = if direction > 0 {
            active.saturating_add(1)
        } else if direction < 0 {
            (active - 1).max(1)
        } else {
            active
        };
        if target != active {
            (self.actions.switch_workspace)(&self.monitor_name, target);
        }
    }

    fn scroll_volume(&self, dx: f64, dy: f64) {
        let direction = dominant_scroll_direction(dx, dy);
        if direction == 0 || !self.volume_scale.is_sensitive() {
            return;
        }
        let delta = if direction > 0 { -5.0 } else { 5.0 };
        self.volume_scale
            .set_value((self.volume_scale.value() + delta).clamp(0.0, 100.0));
    }
}
