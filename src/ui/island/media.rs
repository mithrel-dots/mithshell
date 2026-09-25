//! The legacy media pill and side-circle media state, fed by MPRIS.

use super::*;

use std::{cell::RefCell, rc::Rc, time::Duration};

use gtk::{Align, Orientation, glib};

use super::{IslandWindow, Metrics};
use crate::media::{VISUALIZER_BARS, VisualizerLevels};
use crate::state::{MediaPlayer, MediaState, PlaybackStatus};

pub(super) struct MediaWidgets {
    pub(super) root: gtk::Overlay,
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
    let root = gtk::Overlay::new();
    root.set_size_request(metrics.compact_width, metrics.media_height);
    root.add_css_class("media-pill");
    root.set_valign(Align::Start);
    // Only the foreground is padded; the battery background reaches the
    // enclosing island surface's rounded clip at every density tier.
    let content = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    content.add_css_class("media-content");
    content.set_hexpand(true);
    content.set_vexpand(true);

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
    let visualizer = visualizer_widget(metrics, levels.clone());

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

    content.append(&center_box);
    content.append(&tray);
    root.set_child(Some(&content));
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

pub(super) fn visualizer_widget(
    metrics: Metrics,
    levels: Rc<RefCell<VisualizerLevels>>,
) -> gtk::DrawingArea {
    let visualizer = gtk::DrawingArea::new();
    visualizer.add_css_class("media-visualizer");
    visualizer.set_content_width(metrics.spacing(31));
    visualizer.set_content_height(metrics.spacing(18));
    visualizer.set_valign(Align::Center);
    visualizer.set_can_target(false);
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
        let baseline = height * 0.5;
        context.set_line_width(gap * 0.72);
        for (index, level) in levels.borrow().iter().enumerate() {
            let x = gap + index as f64 * gap * 2.0;
            let half_height = ((height * 0.12) + (height * 0.67 * f64::from(*level) / 100.0)) / 2.0;
            context.move_to(x, baseline - half_height);
            context.line_to(x, baseline + half_height);
            let _ = context.stroke();
        }
    });
    visualizer
}

/// Returns the same discovery snapshot with one player promoted into the
/// top-level fields consumed by the media circle. Selection is purely
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
        art_url: player.art_url.clone(),
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
    pub(crate) fn select_media_service(self: &Rc<Self>, service: String) {
        self.selected_media_service.replace(Some(service));
        let state = self.latest_media.borrow().clone();
        self.update_media(state.as_ref());
    }

    pub fn update_media(self: &Rc<Self>, state: Option<&MediaState>) {
        let in_circle = self
            .circles
            .borrow()
            .as_ref()
            .is_some_and(|c| c.owns(crate::config::CircleModule::Media));
        self.sync_compact_visualizer(
            in_circle && state.is_some_and(|state| state.status == PlaybackStatus::Playing),
        );
        let compact_state = (!in_circle)
            .then_some(state)
            .flatten()
            .filter(|state| state.status == PlaybackStatus::Playing);
        if let Some(state) = compact_state {
            self.media_title.set_label(&state.title);
            self.media_title
                .set_tooltip_text(Some(&format!("{} ({})", state.title, state.player)));
            icon::set_foreign_image(
                &self.media_icon,
                state.app_icon.as_deref(),
                Icon::Executable,
            );
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
                    island.reconcile_pill_geometry();
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
        if let Some(circles) = self.circles.borrow().as_ref() {
            circles.update_media(selected.as_ref());
        }
        *self.latest_media.borrow_mut() = selected;
    }

    pub fn update_visualizer(&self, levels: VisualizerLevels) {
        *self.media_levels.borrow_mut() = levels;
        for area in [&self.media_visualizer, &self.compact_visualizer] {
            if self.visualizer_enabled && area.is_mapped() {
                area.queue_draw();
            }
        }
    }

    fn sync_compact_visualizer(self: &Rc<Self>, playing: bool) {
        let active = self.visualizer_enabled && playing;
        if self.visualizer_active.replace(active) == active {
            return;
        }
        let revision = self.visualizer_revision.get().wrapping_add(1);
        self.visualizer_revision.set(revision);
        if active {
            self.reveal_compact_visualizer(true);
        } else {
            let weak = Rc::downgrade(self);
            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                if let Some(island) = weak.upgrade()
                    && island.visualizer_revision.get() == revision
                {
                    island.reveal_compact_visualizer(false);
                }
            });
        }
    }

    fn reveal_compact_visualizer(self: &Rc<Self>, reveal: bool) {
        self.compact_visualizer_revealer.set_transition_duration(
            if self.animations_enabled.get() {
                self.animation_ms.get()
            } else {
                0
            },
        );
        self.compact_visualizer_revealer.set_reveal_child(reveal);
        self.resize_compact();
        self.reconcile_pill_geometry();
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
            f64::from((self.metrics.window_width - width) / 2),
            0.0,
        );
        self.media_width.set(width);

        if self.current_view.get() == View::Media {
            // Keep direct media updates aligned with the currently rendered
            // animated backdrop before deciding whether a new width track is
            // needed.  This also covers unchanged-width updates.
            self.sync_pill_content_geometry(self.geometry.get());
            self.reconcile_pill_geometry();
        }
    }
}
