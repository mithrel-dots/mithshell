//! Media content for the optional circle surface.
//!
//! This module deliberately owns the only interpolation timer used by the
//! circle.  The central island supplies the already-selected `MediaState` and
//! actions; it does not create another MPRIS listener or media store.

#![allow(dead_code)]
#![allow(deprecated)] // ComboBoxText remains the project's GTK4-compatible selector.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{Align, Orientation, glib, prelude::*};

use super::{
    Metrics,
    circle::{CircleContent, CircleHost},
};
use crate::{
    state::{MediaState, PlaybackStatus},
    ui::icon::{self, Icon},
};

/// Callbacks are intentionally service-qualified, matching `IslandActions`.
/// The circle and dashboard therefore always target the same selected player.
#[derive(Clone)]
pub(crate) struct MediaCircleActions {
    pub play_pause: Rc<dyn Fn(String)>,
    pub next: Rc<dyn Fn(String)>,
    pub previous: Rc<dyn Fn(String)>,
    pub select: Rc<dyn Fn(String)>,
}

struct Progress {
    position_us: i64,
    length_us: Option<i64>,
    status: PlaybackStatus,
    started_at: Option<Instant>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            position_us: 0,
            length_us: None,
            status: PlaybackStatus::Stopped,
            started_at: None,
        }
    }
}

/// Progress fraction with defensive handling for MPRIS' occasionally odd
/// values (unknown duration, negative position, and position past the end).
pub(crate) fn progress_fraction(position_us: i64, length_us: Option<i64>) -> f64 {
    let Some(length) = length_us.filter(|length| *length > 0) else {
        return 0.0;
    };
    (position_us.max(0) as f64 / length as f64).clamp(0.0, 1.0)
}

fn interpolated_progress(progress: &Progress, now: Instant) -> f64 {
    let position = if progress.status == PlaybackStatus::Playing {
        progress
            .position_us
            .saturating_add(progress.started_at.map_or(0, |started| {
                now.saturating_duration_since(started)
                    .as_micros()
                    .min(i64::MAX as u128) as i64
            }))
    } else {
        progress.position_us
    };
    progress_fraction(position, progress.length_us)
}

fn timer_needed(progress: &Progress) -> bool {
    progress.status == PlaybackStatus::Playing
        && progress.length_us.is_some_and(|length| length > 0)
}

/// A mounted circle content pair. `host()` is handed to the central layout;
/// `update()` is called with the same selected snapshot used by the dashboard.
pub(crate) struct MediaCircle {
    host: Rc<CircleHost>,
    progress: Rc<RefCell<Progress>>,
    progress_area: gtk::DrawingArea,
    compact_icon: gtk::Image,
    hover_icon: gtk::Image,
    hover_title: gtk::Label,
    hover_artist: gtk::Label,
    player_select: gtk::ComboBoxText,
    previous: gtk::Button,
    play_pause: gtk::Button,
    next: gtk::Button,
    actions: MediaCircleActions,
    icon_style: crate::config::IconStyle,
    current_service: RefCell<Option<String>>,
    tick: RefCell<Option<glib::SourceId>>,
    selector_updating: Cell<bool>,
}

impl MediaCircle {
    pub(super) fn new(
        metrics: Metrics,
        actions: MediaCircleActions,
    ) -> Result<Rc<Self>, &'static str> {
        let progress = Rc::new(RefCell::new(Progress::default()));
        let (compact, compact_icon, progress_area) = compact_page(metrics, progress.clone());
        let (
            hover,
            hover_icon,
            hover_title,
            hover_artist,
            player_select,
            previous,
            play_pause,
            next,
        ) = hover_page(metrics);
        let host = CircleHost::new(CircleContent {
            compact: compact.clone().upcast(),
            hover: hover.clone().upcast(),
            full: None,
        })?;
        let circle = Rc::new(Self {
            host,
            progress,
            progress_area,
            compact_icon,
            hover_icon,
            hover_title,
            hover_artist,
            player_select,
            previous,
            play_pause,
            next,
            actions,
            icon_style: metrics.icons,
            current_service: RefCell::new(None),
            tick: RefCell::new(None),
            selector_updating: Cell::new(false),
        });
        circle.connect_actions();
        Ok(circle)
    }

    pub(super) fn host(&self) -> Rc<CircleHost> {
        self.host.clone()
    }

    /// `None` hides the circle. A titled paused/stopped player remains valid so
    /// the user can resume it; an empty title/service is not valid content.
    pub(super) fn update(self: &Rc<Self>, state: Option<&MediaState>) {
        let valid = state
            .filter(|state| !state.service.trim().is_empty() && !state.title.trim().is_empty());
        if let Some(state) = valid {
            *self.current_service.borrow_mut() = Some(state.service.clone());
            let mut progress = self.progress.borrow_mut();
            progress.position_us = state.position_us;
            progress.length_us = state.length_us;
            progress.status = state.status;
            progress.started_at = (state.status == PlaybackStatus::Playing).then(Instant::now);
            drop(progress);
            self.set_media(state);
            self.host.dispatch(super::circle::Event::Content(true));
            self.restart_timer();
        } else {
            self.stop_timer();
            self.current_service.borrow_mut().take();
            self.host.dispatch(super::circle::Event::Content(false));
        }
        self.redraw_progress();
    }

    fn set_media(&self, state: &MediaState) {
        for image in [&self.compact_icon, &self.hover_icon] {
            icon::set_foreign_image(image, state.app_icon.as_deref(), Icon::Executable);
            image.set_tooltip_text(Some(&format!("{} — {}", state.title, state.player)));
        }
        self.hover_title.set_label(&state.title);
        self.hover_artist
            .set_label(state.artist.as_deref().unwrap_or_default());
        self.hover_artist.set_visible(state.artist.is_some());
        let _selector_update = SelectorUpdate::new(&self.selector_updating);
        self.player_select.remove_all();
        let mut selected_is_listed = false;
        for player in &state.players {
            selected_is_listed |= player.service == state.service;
            self.player_select
                .append(Some(&player.service), &player.player);
        }
        // A selected snapshot can briefly outlive discovery's player list.
        // Keep valid titled media visible rather than treating that race as
        // absence; the next snapshot reconciles the selector normally.
        if !selected_is_listed {
            self.player_select
                .append(Some(&state.service), &state.player);
        }
        self.player_select.set_active_id(Some(&state.service));
        self.player_select
            .set_tooltip_text(Some("Select media player"));
        self.previous.set_sensitive(state.can_go_previous);
        self.next.set_sensitive(state.can_go_next);
        self.play_pause
            .set_sensitive(state.can_play || state.can_pause);
        icon::set_button_icon(
            &self.play_pause,
            if state.status == PlaybackStatus::Playing {
                Icon::Pause
            } else {
                Icon::Play
            },
            self.icon_style,
        );
    }

    fn connect_actions(self: &Rc<Self>) {
        for (button, action) in [
            (&self.previous, self.actions.previous.clone()),
            (&self.next, self.actions.next.clone()),
            (&self.play_pause, self.actions.play_pause.clone()),
        ] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade()
                    && let Some(service) = this.current_service.borrow().clone()
                {
                    action(service);
                }
            });
        }
        let weak = Rc::downgrade(self);
        let select = self.actions.select.clone();
        self.player_select.connect_changed(move |combo| {
            if let Some(service) = combo.active_id() {
                if let Some(this) = weak.upgrade() {
                    if this.selector_updating.get() {
                        return;
                    }
                    this.current_service.replace(Some(service.to_string()));
                }
                select(service.to_string());
            }
        });
    }

    fn restart_timer(self: &Rc<Self>) {
        self.stop_timer();
        if !timer_needed(&self.progress.borrow()) {
            return;
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(owner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            owner.progress_area.queue_draw();
            let snapshot = owner.progress.borrow();
            let done = snapshot.length_us.is_some_and(|length| {
                length > 0 && interpolated_progress(&snapshot, Instant::now()) >= 1.0
            });
            if done {
                // The callback is already executing, so do not call remove on
                // its own SourceId. Dropping the handle clears the slot; Drop
                // only removes sources which are still registered.
                drop(snapshot);
                owner.tick.borrow_mut().take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        self.tick.replace(Some(source));
    }

    fn stop_timer(&self) {
        if let Some(source) = self.tick.borrow_mut().take() {
            source.remove();
        }
    }
    fn redraw_progress(&self) {
        self.progress_area.queue_draw();
    }

    /// Called by theme/config redraw integration; the ring takes its color
    /// from the DrawingArea's current GTK foreground color.
    pub(super) fn redraw_theme(&self) {
        self.progress_area.queue_draw();
    }
}

impl Drop for MediaCircle {
    fn drop(&mut self) {
        if let Some(source) = self.tick.get_mut().take() {
            source.remove();
        }
    }
}

struct SelectorUpdate<'a>(&'a Cell<bool>);

impl SelectorUpdate<'_> {
    fn new(updating: &Cell<bool>) -> SelectorUpdate<'_> {
        updating.set(true);
        SelectorUpdate(updating)
    }
}

impl Drop for SelectorUpdate<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn compact_page(
    metrics: Metrics,
    progress: Rc<RefCell<Progress>>,
) -> (gtk::Overlay, gtk::Image, gtk::DrawingArea) {
    let overlay = gtk::Overlay::new();
    overlay.set_size_request(metrics.spacing(48), metrics.spacing(48));
    overlay.add_css_class("media-circle-compact");
    let image = gtk::Image::new();
    image.set_pixel_size(metrics.spacing(30));
    image.set_halign(Align::Center);
    image.set_valign(Align::Center);
    overlay.set_child(Some(&image));
    let area = gtk::DrawingArea::new();
    area.set_content_width(metrics.spacing(48));
    area.set_content_height(metrics.spacing(48));
    area.add_css_class("media-circle-progress");
    let draw_progress = progress;
    let draw_area = area.clone();
    area.set_draw_func(move |_, cr, width, height| {
        let fraction = interpolated_progress(&draw_progress.borrow(), Instant::now());
        let radius = f64::from(width.min(height)) * 0.5 - 2.0;
        cr.set_line_width(2.5);
        cr.arc(
            f64::from(width) / 2.0,
            f64::from(height) / 2.0,
            radius,
            -std::f64::consts::FRAC_PI_2,
            -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * fraction,
        );
        let color = draw_area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        let _ = cr.stroke();
    });
    overlay.add_overlay(&area);
    (overlay, image, area)
}

fn hover_page(
    metrics: Metrics,
) -> (
    gtk::Box,
    gtk::Image,
    gtk::Label,
    gtk::Label,
    gtk::ComboBoxText,
    gtk::Button,
    gtk::Button,
    gtk::Button,
) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    root.set_margin_start(metrics.spacing(10));
    root.set_margin_end(metrics.spacing(10));
    root.set_margin_top(metrics.spacing(6));
    root.set_margin_bottom(metrics.spacing(6));
    root.add_css_class("media-circle-hover");
    let icon = gtk::Image::new();
    icon.set_pixel_size(metrics.spacing(32));
    root.append(&icon);
    let text = gtk::Box::new(Orientation::Vertical, 0);
    let title = gtk::Label::new(None);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_single_line_mode(true);
    let artist = gtk::Label::new(None);
    artist.set_xalign(0.0);
    artist.add_css_class("dim-label");
    text.append(&title);
    text.append(&artist);
    root.append(&text);
    let select = gtk::ComboBoxText::new();
    select.set_hexpand(false);
    root.append(&select);
    let previous = icon::icon_button(Icon::Previous, metrics.icons);
    let play = icon::icon_button(Icon::Play, metrics.icons);
    let next = icon::icon_button(Icon::Next, metrics.icons);
    for button in [&previous, &play, &next] {
        button.add_css_class("media-circle-control");
        root.append(button);
    }
    (root, icon, title, artist, select, previous, play, next)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{PlaybackStatus, Progress, SelectorUpdate, progress_fraction, timer_needed};
    #[test]
    fn progress_is_safe_for_unknown_and_invalid_values() {
        assert_eq!(progress_fraction(-1, None), 0.0);
        assert_eq!(progress_fraction(10, Some(0)), 0.0);
        assert_eq!(progress_fraction(-10, Some(100)), 0.0);
        assert_eq!(progress_fraction(200, Some(100)), 1.0);
        assert_eq!(progress_fraction(25, Some(100)), 0.25);
    }

    #[test]
    fn unknown_duration_never_starts_a_playback_timer() {
        let progress = Progress {
            status: PlaybackStatus::Playing,
            length_us: None,
            ..Default::default()
        };
        assert!(!timer_needed(&progress));
        assert!(timer_needed(&Progress {
            status: PlaybackStatus::Playing,
            length_us: Some(1),
            ..Default::default()
        }));
    }

    #[test]
    fn selector_updates_suppress_snapshot_emissions_but_not_user_changes() {
        let updating = Cell::new(false);
        let emitted = Cell::new(0);
        {
            let _guard = SelectorUpdate::new(&updating);
            if !updating.get() {
                emitted.set(emitted.get() + 1);
            }
        }
        assert_eq!(emitted.get(), 0);
        if !updating.get() {
            emitted.set(emitted.get() + 1);
        }
        assert_eq!(emitted.get(), 1);
    }
}
