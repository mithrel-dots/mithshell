//! The media pill and the dashboard's player card, fed by MPRIS state.

use super::*;

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{Align, Orientation, glib};

use super::{IslandWindow, Metrics};
use crate::media::{VISUALIZER_BARS, VisualizerLevels};
use crate::state::{MediaPlayer, MediaState, PlaybackStatus};

pub(super) struct MediaWidgets {
    pub(super) root: gtk::Box,
    pub(super) workspaces: gtk::Box,
    pub(super) clock: gtk::Label,
    pub(super) center: gtk::Box,
    pub(super) icon: gtk::Image,
    pub(super) title: gtk::Label,
    pub(super) visualizer: gtk::DrawingArea,
    pub(super) levels: Rc<RefCell<VisualizerLevels>>,
    /// Same role as `compact_tray`, but for the media pill. Deliberately a
    /// sibling of the `CenterBox` rather than a child of its `end` slot:
    /// `CenterBox` keeps its center child centered by giving `start`/`end`
    /// equal width, so anything added to `end` visibly pads `start` (the
    /// workspace dots) by the same amount.
    pub(super) tray: gtk::Box,
}

pub(super) fn media_view(metrics: Metrics) -> MediaWidgets {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    root.set_size_request(metrics.compact_width, metrics.media_height);
    root.add_css_class("media-content");
    root.set_valign(Align::Start);

    let center_box = gtk::CenterBox::new();
    center_box.set_hexpand(true);

    let workspaces = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);

    let media = gtk::Box::new(Orientation::Horizontal, metrics.spacing(7));
    media.add_css_class("media-center");
    media.set_halign(Align::Center);
    media.set_valign(Align::Center);

    let icon = gtk::Image::new();
    icon.add_css_class("media-app-icon");
    icon.set_margin_start(metrics.spacing(2));
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
        let baseline = height * 0.5;
        context.set_line_width(bar_width);
        for (index, level) in draw_levels.borrow().iter().enumerate() {
            let x = gap + index as f64 * gap * 2.0;
            let half_height = ((height * 0.12) + (height * 0.67 * f64::from(*level) / 100.0)) / 2.0;
            context.move_to(x, baseline - half_height);
            context.line_to(x, baseline + half_height);
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

    // Hidden by default, same as `compact_tray`: only shown while hovering
    // the pill and at least one tray item exists.
    let tray = gtk::Box::new(Orientation::Horizontal, metrics.spacing(3));
    tray.add_css_class("compact-tray");
    tray.set_halign(Align::End);
    tray.set_valign(Align::Center);
    tray.set_visible(false);

    media.append(&icon);
    media.append(&visualizer);
    media.append(&title);
    center_box.set_start_widget(Some(&workspaces));
    center_box.set_center_widget(Some(&media));
    center_box.set_end_widget(Some(&clock));

    root.append(&center_box);
    root.append(&tray);
    MediaWidgets {
        root,
        workspaces,
        clock,
        center: media,
        icon,
        title,
        visualizer,
        levels,
        tray,
    }
}

/// Formats a microsecond duration as `M:SS`, or `H:MM:SS` past one hour.
pub(super) fn format_media_time(microseconds: i64) -> String {
    let total_seconds = (microseconds.max(0) / 1_000_000) as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Returns the same discovery snapshot with one player promoted into the
/// top-level fields consumed by the dashboard controls. Selection is purely
/// presentational: it never invokes Play/PlayPause and therefore cannot
/// disturb another player's playback.
pub(super) fn media_state_for_player(state: &MediaState, service: Option<&str>) -> MediaState {
    let player: &MediaPlayer = service
        .and_then(|service| {
            state
                .players
                .iter()
                .find(|player| player.service == service)
        })
        .or_else(|| {
            state
                .players
                .iter()
                .find(|player| player.service == state.service)
        })
        .unwrap_or_else(|| {
            // Every MediaState is built from at least one discovered player.
            state.players.first().expect("media state without players")
        });
    MediaState {
        player: player.player.clone(),
        service: player.service.clone(),
        title: player.title.clone(),
        artist: player.artist.clone(),
        album: player.album.clone(),
        app_icon: player.app_icon.clone(),
        position_us: player.position_us,
        length_us: player.length_us,
        can_play: player.can_play,
        can_pause: player.can_pause,
        can_go_next: player.can_go_next,
        can_go_previous: player.can_go_previous,
        status: player.status,
        players: state.players.clone(),
    }
}

impl IslandWindow {
    pub fn update_media(self: &Rc<Self>, state: Option<&MediaState>) {
        let compact_state = state.filter(|state| state.status == PlaybackStatus::Playing);
        if let Some(state) = compact_state {
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
        if compact_state.is_some() {
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

        let selected = state.map(|state| {
            let requested = self.selected_media_service.borrow().clone();
            let selected = media_state_for_player(state, requested.as_deref());
            *self.selected_media_service.borrow_mut() = Some(selected.service.clone());
            selected
        });
        self.update_player_card(selected.as_ref());
        *self.latest_media.borrow_mut() = selected;
    }

    /// Updates the always-visible media player card in the dashboard. Unlike
    /// the compact pill above (`update_media`'s first half, gated on
    /// `media_playing`), this card is part of the dashboard layout and shows
    /// an idle placeholder when nothing is playing instead of disappearing.
    fn update_player_card(&self, state: Option<&MediaState>) {
        match state {
            Some(state) => {
                self.player_card.remove_css_class("unavailable");
                self.player_title.set_label(&state.title);
                self.player_artist
                    .set_label(state.artist.as_deref().unwrap_or_default());
                self.player_artist.set_visible(state.artist.is_some());
                if let Some(icon) = state.app_icon.as_deref() {
                    self.player_icon.set_icon_name(Some(icon));
                    self.player_icon.set_visible(true);
                } else {
                    self.player_icon.set_visible(false);
                }
                self.player_prev_button.set_sensitive(state.can_go_previous);
                self.player_play_pause_button
                    .set_sensitive(state.can_play || state.can_pause);
                self.player_next_button.set_sensitive(state.can_go_next);
                self.player_switch_row.set_visible(state.players.len() > 1);
                let selected = state
                    .players
                    .iter()
                    .position(|player| player.service == state.service)
                    .unwrap_or(0);
                self.player_switch_label.set_label(&format!(
                    "{} / {}  //  {}",
                    selected + 1,
                    state.players.len(),
                    state.player.replace('.', " ")
                ));
                self.player_progress_base_us.set(state.position_us);
                self.player_length_us.set(state.length_us.unwrap_or(0));
                self.player_progress_started_at
                    .set((state.status == PlaybackStatus::Playing).then(Instant::now));
                self.player_active
                    .set(state.status == PlaybackStatus::Playing);
                self.player_play_pause_button.set_icon_name(
                    if state.status == PlaybackStatus::Playing {
                        "media-playback-pause-symbolic"
                    } else {
                        "media-playback-start-symbolic"
                    },
                );
                self.tick_player_progress();
            }
            None => {
                self.player_card.add_css_class("unavailable");
                self.player_title.set_label("Nothing playing");
                self.player_artist.set_label("");
                self.player_artist.set_visible(false);
                self.player_icon.set_visible(false);
                self.player_prev_button.set_sensitive(false);
                self.player_play_pause_button.set_sensitive(false);
                self.player_next_button.set_sensitive(false);
                self.player_switch_row.set_visible(false);
                self.player_progress.set_fraction(0.0);
                self.player_elapsed_label.set_label("--:--");
                self.player_duration_label.set_label("--:--");
                self.player_active.set(false);
                self.player_progress_started_at.set(None);
            }
        }
    }

    /// Advances the player card's progress bar between MPRIS updates by
    /// interpolating from the last known position using a local clock,
    /// rather than polling MPRIS for `Position` on a timer.
    fn tick_player_progress(&self) {
        let elapsed_us = self
            .player_progress_started_at
            .get()
            .filter(|_| self.player_active.get())
            .map_or(0, |started| started.elapsed().as_micros() as i64);
        let position_us = (self.player_progress_base_us.get() + elapsed_us).max(0);
        let length_us = self.player_length_us.get();
        if length_us > 0 {
            let position_us = position_us.min(length_us);
            self.player_progress
                .set_fraction((position_us as f64 / length_us as f64).clamp(0.0, 1.0));
            self.player_duration_label
                .set_label(&format_media_time(length_us));
        } else {
            self.player_progress.set_fraction(0.0);
            self.player_duration_label.set_label("--:--");
        }
        self.player_elapsed_label
            .set_label(&format_media_time(position_us));
    }

    pub(super) fn start_player_progress_timer(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(500), move || {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            island.tick_player_progress();
            glib::ControlFlow::Continue
        });
    }

    pub fn update_visualizer(&self, levels: VisualizerLevels) {
        *self.media_levels.borrow_mut() = levels;
        self.media_visualizer.queue_draw();
    }

    pub(super) fn switch_media_player(&self, direction: i32) {
        let Some(state) = self.latest_media.borrow().clone() else {
            return;
        };
        if state.players.len() < 2 {
            return;
        }
        let current = state
            .players
            .iter()
            .position(|player| player.service == state.service)
            .unwrap_or(0);
        let next = (current as i32 + direction).rem_euclid(state.players.len() as i32) as usize;
        let selected_state = media_state_for_player(&state, Some(&state.players[next].service));
        *self.selected_media_service.borrow_mut() = Some(selected_state.service.clone());
        self.update_player_card(Some(&selected_state));
        *self.latest_media.borrow_mut() = Some(selected_state);
    }

    pub(super) fn resize_media(self: &Rc<Self>) {
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

        // The tray is a sibling of the `CenterBox` rather than part of its
        // `end` slot, so it only adds to the total -- it deliberately does
        // not participate in the `start`/`end` symmetry that keeps the
        // track title centered.
        let tray_visible = self.tray_visible();
        self.media_tray.set_visible(tray_visible);
        let tray_width = if tray_visible {
            self.metrics.spacing(8)
                + measure_clamped(&self.media_tray, self.metrics.compact_tray_max_width)
        } else {
            0
        };

        let center_gaps = self.metrics.spacing(if icon_width > 0 { 14 } else { 7 });
        let natural = self.metrics.spacing(36)
            + workspace_width.max(clock_width) * 2
            + icon_width
            + visualizer_width
            + center_gaps
            + title_width
            + tray_width;
        let width = natural.clamp(self.metrics.compact_width, self.metrics.media_max_width);
        self.media
            .set_size_request(width, self.metrics.media_height);
        self.content.move_(
            &self.media,
            f64::from((self.metrics.search_width - width) / 2),
            0.0,
        );
        self.media_width.set(width);

        if self.current_view.get() == View::Media {
            self.set_view(View::Media);
        }
    }
}
