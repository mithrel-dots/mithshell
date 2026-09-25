//! Media content for the optional circle surface.
//!
//! This module deliberately owns the only interpolation timer used by the
//! circle.  The central island supplies the already-selected `MediaState` and
//! actions; it does not create another MPRIS listener or media store.

#![allow(dead_code)]
use std::{
    cell::{Cell, RefCell},
    io::Read,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{Align, Orientation, gdk, glib, prelude::*};

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

/// Center-crop to a square before downsampling so wide/tall artwork keeps the
/// requested resolution instead of being enlarged from a narrow thumbnail.
fn square_thumbnail(image: image::DynamicImage, size: u32) -> image::RgbaImage {
    let edge = image.width().min(image.height());
    image
        .crop_imm(
            (image.width() - edge) / 2,
            (image.height() - edge) / 2,
            edge,
            edge,
        )
        .thumbnail(size, size)
        .to_rgba8()
}

fn timer_needed(progress: &Progress) -> bool {
    progress.status == PlaybackStatus::Playing
        && progress.length_us.is_some_and(|length| length > 0)
}

struct Artwork {
    width: i32,
    height: i32,
    pixels: Vec<u8>,
}

/// Decode off the GTK thread and bound both the transfer and final image.
/// MPRIS players commonly provide an HTTPS cover URL, while local players use
/// `file://`. Center-cropping to square lets the cover fill the circle without
/// letterboxing; the app icon remains visible if loading fails.
fn load_cover(url: &str, size: u32) -> Option<Artwork> {
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    if url.starts_with("file://") {
        let path = gtk::gio::File::for_uri(url).path()?;
        std::fs::File::open(path)
            .ok()?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
    } else if url.starts_with("https://") {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(6)))
            .build()
            .into();
        agent
            .get(url)
            .call()
            .ok()?
            .body_mut()
            .as_reader()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
    } else {
        return None;
    }
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let image = square_thumbnail(image::load_from_memory(&bytes).ok()?, size);
    let (width, height) = image.dimensions();
    Some(Artwork {
        width: width as i32,
        height: height as i32,
        pixels: image.into_raw(),
    })
}

fn artwork_texture(art: Artwork) -> gdk::MemoryTexture {
    gdk::MemoryTexture::new(
        art.width,
        art.height,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(art.pixels),
        art.width as usize * 4,
    )
}

/// A mounted circle content pair. `host()` is handed to the central layout;
/// `update()` is called with the same selected snapshot used by the dashboard.
pub(crate) struct MediaCircle {
    host: Rc<CircleHost>,
    progress: Rc<RefCell<Progress>>,
    progress_area: gtk::DrawingArea,
    compact_icon: gtk::Image,
    hover_artwork: gtk::Picture,
    hover_icon: gtk::Image,
    hover_title: gtk::Label,
    hover_artist: gtk::Label,
    players: RefCell<Vec<String>>,
    source_scroll: gtk::EventControllerScroll,
    scroll_progress: Cell<f64>,
    previous: gtk::Button,
    play_pause: gtk::Button,
    next: gtk::Button,
    actions: MediaCircleActions,
    icon_style: crate::config::IconStyle,
    current_service: RefCell<Option<String>>,
    tick: RefCell<Option<glib::SourceId>>,
    artwork_url: RefCell<Option<String>>,
    artwork_texture: RefCell<Option<gdk::Texture>>,
    artwork_generation: Cell<u64>,
    fallback_icon: RefCell<Option<String>>,
    art_size: u32,
}

impl MediaCircle {
    #[cfg(test)]
    pub(crate) fn test_click_play_pause(&self) {
        self.play_pause.emit_clicked();
    }

    #[cfg(test)]
    pub(crate) fn test_play_pause_button(&self) -> gtk::Button {
        self.play_pause.clone()
    }
    pub(super) fn new(
        metrics: Metrics,
        actions: MediaCircleActions,
    ) -> Result<Rc<Self>, &'static str> {
        let progress = Rc::new(RefCell::new(Progress::default()));
        let (compact, compact_icon, progress_area) = compact_page(metrics, progress.clone());
        let (
            hover,
            hover_artwork,
            hover_icon,
            hover_title,
            hover_artist,
            previous,
            play_pause,
            next,
        ) = hover_page(metrics);
        let host = CircleHost::new(CircleContent {
            compact: compact.clone().upcast(),
            hover: hover.clone().upcast(),
            full: None,
        })?;
        let source_scroll =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        // The CircleHost wraps expanded pages in a ScrolledWindow. Capture
        // wheel events first so they change source instead of shifting a
        // clipped page; this also works directly on the compact artwork.
        source_scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        host.widget().add_controller(source_scroll.clone());
        let circle = Rc::new(Self {
            host,
            progress,
            progress_area,
            compact_icon,
            hover_artwork,
            hover_icon,
            hover_title,
            hover_artist,
            players: RefCell::new(Vec::new()),
            source_scroll,
            scroll_progress: Cell::new(0.0),
            previous,
            play_pause,
            next,
            actions,
            icon_style: metrics.icons,
            current_service: RefCell::new(None),
            tick: RefCell::new(None),
            artwork_url: RefCell::new(None),
            artwork_texture: RefCell::new(None),
            artwork_generation: Cell::new(0),
            fallback_icon: RefCell::new(None),
            art_size: media_art_diameter(metrics).max(1) as u32,
        });
        circle.connect_actions();
        super::circle::scale_text(circle.host.widget(), metrics.scale);
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
            self.artwork_generation
                .set(self.artwork_generation.get().wrapping_add(1));
            self.artwork_url.borrow_mut().take();
            self.artwork_texture.borrow_mut().take();
            self.fallback_icon.borrow_mut().take();
            self.hover_artwork.set_paintable(None::<&gdk::Paintable>);
            self.hover_artwork.set_visible(false);
            self.hover_icon.set_visible(true);
            self.host.dispatch(super::circle::Event::Content(false));
        }
        self.redraw_progress();
    }

    fn set_media(self: &Rc<Self>, state: &MediaState) {
        self.update_artwork(state);
        for image in [&self.compact_icon, &self.hover_icon] {
            image.set_tooltip_text(Some(&format!("{} — {}", state.title, state.player)));
        }
        self.hover_title.set_label(&state.title);
        self.hover_artist
            .set_label(state.artist.as_deref().unwrap_or_default());
        self.hover_artist.set_visible(state.artist.is_some());
        let mut players: Vec<String> = state
            .players
            .iter()
            .map(|player| player.service.clone())
            .collect();
        // Discovery can briefly lag the selected snapshot. Keep that source
        // available until the next snapshot reconciles the list.
        if !players.contains(&state.service) {
            players.push(state.service.clone());
        }
        if *self.players.borrow() != players {
            *self.players.borrow_mut() = players;
            self.scroll_progress.set(0.0);
        }
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

    fn select_player(&self, service: &str) {
        if self.current_service.borrow().as_deref() == Some(service) {
            return;
        }
        self.current_service.replace(Some(service.to_owned()));
        (self.actions.select)(service.to_owned());
    }

    fn step_player(&self, direction: i32) -> bool {
        let players = self.players.borrow();
        if players.len() < 2 {
            return false;
        }
        let index = players
            .iter()
            .position(|service| self.current_service.borrow().as_deref() == Some(service))
            .unwrap_or_default();
        let next = (index as i32 + direction).rem_euclid(players.len() as i32) as usize;
        let service = players[next].clone();
        drop(players);
        self.select_player(&service);
        true
    }

    fn scroll_player(&self, dx: f64, dy: f64) -> bool {
        let delta = if dy.abs() >= dx.abs() { dy } else { dx };
        if !delta.is_finite() || delta == 0.0 || self.players.borrow().len() < 2 {
            return false;
        }
        let previous = self.scroll_progress.get();
        let progress = if previous.signum() != delta.signum() {
            delta
        } else {
            previous + delta
        };
        if !progress.is_finite() {
            self.scroll_progress.set(0.0);
            return false;
        }
        if progress.abs() < 1.0 {
            self.scroll_progress.set(progress);
            return true;
        }
        self.scroll_progress.set(progress.fract());
        // Smooth trackpads can report several whole steps in one event. Limit
        // callbacks per frame for extreme devices but retain the fractional
        // distance so normal gestures never lose their overshoot.
        let steps = progress.abs().floor().min(32.0) as usize;
        let direction = if progress > 0.0 { 1 } else { -1 };
        for _ in 0..steps {
            self.step_player(direction);
        }
        true
    }

    fn update_artwork(self: &Rc<Self>, state: &MediaState) {
        let url = state.art_url.as_deref();
        let changed = self.artwork_url.borrow().as_deref() != url;
        if changed {
            self.artwork_generation
                .set(self.artwork_generation.get().wrapping_add(1));
            *self.artwork_url.borrow_mut() = state.art_url.clone();
            self.artwork_texture.borrow_mut().take();
        }
        if let Some(texture) = self.artwork_texture.borrow().as_ref() {
            for image in [&self.compact_icon, &self.hover_icon] {
                image.set_paintable(Some(texture));
            }
            self.hover_artwork.set_paintable(Some(texture));
            self.hover_artwork.set_visible(true);
            self.hover_icon.set_visible(false);
        } else if changed || self.fallback_icon.borrow().as_ref() != state.app_icon.as_ref() {
            for image in [&self.compact_icon, &self.hover_icon] {
                set_compact_art(image, state.app_icon.as_deref(), self.icon_style);
            }
            self.hover_artwork.set_paintable(None::<&gdk::Paintable>);
            self.hover_artwork.set_visible(false);
            self.hover_icon.set_visible(true);
        }
        *self.fallback_icon.borrow_mut() = state.app_icon.clone();
        let Some(url) = state.art_url.clone().filter(|_| changed) else {
            return;
        };
        let generation = self.artwork_generation.get();
        let size = self.art_size;
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(load_cover(&url, size));
        });
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let Ok(Some(art)) = receiver.recv().await else {
                return;
            };
            let Some(owner) = weak.upgrade() else {
                return;
            };
            if owner.artwork_generation.get() != generation {
                return;
            }
            let texture: gdk::Texture = artwork_texture(art).upcast();
            for image in [&owner.compact_icon, &owner.hover_icon] {
                image.set_paintable(Some(&texture));
            }
            owner.hover_artwork.set_paintable(Some(&texture));
            owner.hover_artwork.set_visible(true);
            owner.hover_icon.set_visible(false);
            owner.artwork_texture.replace(Some(texture));
        });
    }

    fn connect_actions(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.source_scroll.connect_scroll(move |_, dx, dy| {
            if weak
                .upgrade()
                .is_some_and(|circle| circle.scroll_player(dx, dy))
            {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
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

    #[cfg(test)]
    pub(crate) fn test_select_service(&self, service: &str) {
        if self.players.borrow().iter().any(|source| source == service) {
            self.select_player(service);
        }
    }

    #[cfg(test)]
    pub(crate) fn test_service(&self) -> Option<String> {
        self.current_service.borrow().clone()
    }
}

impl Drop for MediaCircle {
    fn drop(&mut self) {
        if let Some(source) = self.tick.get_mut().take() {
            source.remove();
        }
    }
}

fn compact_page(
    metrics: Metrics,
    progress: Rc<RefCell<Progress>>,
) -> (gtk::Overlay, gtk::Image, gtk::DrawingArea) {
    // Keep artwork/ring inside the island surface's border, which consumes
    // space from the Stack allocation on every side.
    let overlay = gtk::Overlay::new();
    let art_size = media_art_diameter(metrics);
    overlay.set_size_request(art_size, art_size);
    // The right-side circle expands from a fixed left edge. Its compact page
    // stays presented through the outgoing part of that animation; centering
    // this overlay in the growing Stack sends the cover to the right before
    // the hover page commits and snaps it back. Pin it to that same left edge.
    overlay.set_halign(Align::Start);
    overlay.set_valign(Align::Center);
    overlay.add_css_class("media-circle-compact");
    overlay.add_css_class("media-circle-art-tile");
    overlay.set_overflow(gtk::Overflow::Hidden);
    let image = gtk::Image::new();
    image.set_pixel_size(art_size);
    image.set_size_request(art_size, art_size);
    image.set_halign(Align::Fill);
    image.set_valign(Align::Fill);
    overlay.set_child(Some(&image));
    let area = gtk::DrawingArea::new();
    area.set_content_width(art_size);
    area.set_content_height(art_size);
    area.set_size_request(art_size, art_size);
    area.add_css_class("media-circle-progress");
    let draw_progress = progress;
    area.set_draw_func(move |area, cr, width, height| {
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
        let color = area.color();
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

/// File-backed app icons can have a huge natural size; decode and downsample
/// them before handing them to GTK, center-cropped for the full-bleed circle.
fn set_compact_art(image: &gtk::Image, name: Option<&str>, style: crate::config::IconStyle) {
    let art_size = image.pixel_size().max(1) as u32;
    if let Some(path) = name.map(str::trim).filter(|name| name.starts_with('/')) {
        if let Ok(source) = image::open(path) {
            let source = square_thumbnail(source, art_size);
            let (width, height) = source.dimensions();
            let texture = gtk::gdk::MemoryTexture::new(
                width as i32,
                height as i32,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_owned(source.into_raw()),
                width as usize * 4,
            );
            image.set_paintable(Some(&texture));
        } else {
            icon::set_icon(image.upcast_ref(), Icon::Executable, style);
        }
    } else {
        icon::set_foreign_image(image, name, Icon::Executable);
    }
}

fn media_art_diameter(metrics: Metrics) -> i32 {
    const COMPACT_DIAMETER: i32 = 32;
    let border = match metrics.css_class() {
        Some("scale-medium") => 2,
        Some("scale-large") => 3,
        _ => 1,
    };
    (metrics.spacing(COMPACT_DIAMETER) - border * 2).max(1)
}

fn hover_page(
    metrics: Metrics,
) -> (
    gtk::Box,
    gtk::Picture,
    gtk::Image,
    gtk::Label,
    gtk::Label,
    gtk::Button,
    gtk::Button,
    gtk::Button,
) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(1));
    root.set_margin_start(metrics.spacing(8));
    root.set_margin_end(metrics.spacing(8));
    // The viewport centers the row; extra vertical margins would compete
    // with the scaled border and two-line text for the fixed hover height.
    root.set_valign(Align::Center);
    root.set_vexpand(false);
    root.add_css_class("media-circle-hover");

    let art_size = media_art_diameter(metrics);
    let art_tile = gtk::Overlay::new();
    art_tile.set_size_request(art_size, art_size);
    art_tile.set_halign(Align::Start);
    art_tile.set_valign(Align::Center);
    art_tile.set_overflow(gtk::Overflow::Hidden);
    art_tile.add_css_class("media-circle-art-tile");
    let artwork = gtk::Picture::new();
    artwork.set_content_fit(gtk::ContentFit::Cover);
    artwork.set_can_shrink(true);
    artwork.set_size_request(art_size, art_size);
    artwork.set_halign(Align::Fill);
    artwork.set_valign(Align::Fill);
    artwork.set_can_target(false);
    artwork.set_visible(false);
    artwork.add_css_class("media-circle-artwork");
    art_tile.set_child(Some(&artwork));
    let icon = gtk::Image::new();
    icon.set_pixel_size(metrics.spacing(24));
    icon.set_size_request(metrics.spacing(24), metrics.spacing(24));
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    art_tile.add_overlay(&icon);
    root.append(&art_tile);
    let text = gtk::Box::new(Orientation::Vertical, 0);
    text.set_hexpand(true);
    text.set_valign(Align::Center);
    let title = gtk::Label::new(None);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_single_line_mode(true);
    title.set_width_chars(2);
    title.set_max_width_chars(12);
    let artist = gtk::Label::new(None);
    artist.set_xalign(0.0);
    artist.add_css_class("dim-label");
    artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    artist.set_single_line_mode(true);
    artist.set_max_width_chars(12);
    text.append(&title);
    text.append(&artist);
    root.append(&text);
    let previous = icon::icon_button(Icon::Previous, metrics.icons);
    let play = icon::icon_button(Icon::Play, metrics.icons);
    let next = icon::icon_button(Icon::Next, metrics.icons);
    for button in [&previous, &play, &next] {
        button.add_css_class("media-circle-control");
        let style = gtk::CssProvider::new();
        style.load_from_string(&format!(
            "button {{ min-width: {}px; min-height: {}px; padding: {}px; }}",
            metrics.spacing(20),
            metrics.spacing(24),
            metrics.spacing(3)
        ));
        #[allow(deprecated)]
        button
            .style_context()
            .add_provider(&style, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
        if let Some(image) = button.child().and_downcast::<gtk::Image>() {
            image.set_pixel_size(metrics.spacing(16));
        }
        root.append(button);
    }
    (root, artwork, icon, title, artist, previous, play, next)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gtk::prelude::*;

    use crate::{config::IconStyle, state::MediaPlayer};

    use super::{PlaybackStatus, Progress, progress_fraction, timer_needed};
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
    fn square_crop_happens_before_downsampling() {
        let landscape = image::RgbaImage::from_pixel(320, 96, image::Rgba([0x42, 0x80, 0xc0, 255]));
        let thumb = super::square_thumbnail(image::DynamicImage::ImageRgba8(landscape), 64);
        assert_eq!(thumb.dimensions(), (64, 64));
    }

    /// Runs under a private Broadway display. This is intentionally ignored in
    /// ordinary unit runs because GTK tests cannot share a display safely.
    /// `target/run-media-circle-gtk.py` supplies the isolated display.
    #[test]
    #[ignore = "requires the project-local Broadway runner"]
    fn gtk_update_selection_and_timer_lifecycle() {
        gtk::init().expect("Broadway GTK display");
        let display = gtk::gdk::Display::default().expect("Broadway display");
        let monitor = display
            .monitors()
            .item(0)
            .expect("Broadway monitor")
            .downcast::<gtk::gdk::Monitor>()
            .expect("monitor type");
        let metrics = super::Metrics::new(&monitor, 1.0, 1.0, IconStyle::Symbolic);
        let selected = Rc::new(Cell::new(0));
        let select_count = selected.clone();
        let actions = super::MediaCircleActions {
            play_pause: Rc::new(|_| {}),
            next: Rc::new(|_| {}),
            previous: Rc::new(|_| {}),
            select: Rc::new(move |_| select_count.set(select_count.get() + 1)),
        };
        let circle = super::MediaCircle::new(metrics, actions).expect("media circle");
        let drawing_area = circle.progress_area.downgrade();
        let state = test_media_state(PlaybackStatus::Playing, Some(10_000_000));
        circle.update(Some(&state));
        assert_eq!(selected.get(), 0, "snapshot update switched sources");
        assert!(
            circle.tick.borrow().is_some(),
            "known duration starts timer"
        );

        // The same callback path as wheel selection, unlike a snapshot update.
        circle.test_select_service("org.test.other");
        assert_eq!(selected.get(), 1, "user selection emits exactly once");

        circle.update(Some(&test_media_state(PlaybackStatus::Playing, None)));
        assert!(
            circle.tick.borrow().is_none(),
            "unknown duration has no timer"
        );
        circle.update(Some(&state));
        assert!(circle.tick.borrow().is_some());
        drop(circle);
        assert!(
            drawing_area.upgrade().is_none(),
            "draw callback retained DrawingArea"
        );
    }

    #[test]
    #[ignore = "requires the project-local Broadway runner"]
    fn compact_art_and_ring_fit_mapped_circle_at_runtime_scales() {
        gtk::init().expect("Broadway GTK display");
        let palette = crate::theme::generate(&crate::config::ThemeConfig::default()).unwrap();
        let _styles = crate::ui::install_styles(&palette);
        let display = gtk::gdk::Display::default().expect("Broadway display");
        let monitor = display
            .monitors()
            .item(0)
            .expect("Broadway monitor")
            .downcast::<gtk::gdk::Monitor>()
            .expect("monitor type");
        for scale in [1.0, 1.45, 1.9] {
            let metrics = super::Metrics::new(&monitor, scale, 1.0, IconStyle::Symbolic);
            let fixture = std::env::temp_dir().join(format!("media-circle-{scale}.png"));
            image::RgbaImage::from_fn(320, 96, |x, y| image::Rgba([x as u8, y as u8, 0x80, 255]))
                .save(&fixture)
                .expect("oversized file-backed test artwork");
            let fixture = fixture.to_string_lossy().into_owned();
            let actions = super::MediaCircleActions {
                play_pause: Rc::new(|_| {}),
                next: Rc::new(|_| {}),
                previous: Rc::new(|_| {}),
                select: Rc::new(|_| {}),
            };
            let circle = super::MediaCircle::new(metrics, actions).expect("media circle");
            let mut state = test_media_state(PlaybackStatus::Paused, Some(10_000_000));
            state.app_icon = Some("audio-x-generic".to_owned());
            state.art_url = Some(gtk::gio::File::for_path(&fixture).uri().to_string());
            circle.update(Some(&state));
            let window = gtk::Window::new();
            if let Some(class) = metrics.css_class() {
                window.add_css_class(class);
            }
            let fixed = gtk::Fixed::new();
            fixed.set_size_request(400, 200);
            fixed.put(circle.host.widget(), 0.0, 0.0);
            window.set_default_size(400, 200);
            window.set_child(Some(&fixed));
            let frame = super::super::circle::Frame {
                rect: super::super::circle::Rect {
                    x: 0.0,
                    y: 0.0,
                    width: metrics.spacing(32) as f64,
                    height: metrics.spacing(32) as f64,
                },
                radius: metrics.spacing(16) as f64,
            };
            circle.host.render(circle.host.revision(), Some(frame));
            circle.host.commit_page(circle.host.revision());
            window.present();
            while gtk::glib::MainContext::default().iteration(false) {}

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while circle.artwork_texture.borrow().is_none() && std::time::Instant::now() < deadline
            {
                while gtk::glib::MainContext::default().iteration(false) {}
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(
                circle.artwork_texture.borrow().is_some(),
                "cover art loaded at {scale}"
            );

            let diameter = metrics.spacing(32);
            let art_size = super::media_art_diameter(metrics);
            let decode_size = art_size;
            let image = &circle.compact_icon;
            let ring = &circle.progress_area;
            assert_eq!(image.width(), art_size, "file art width at scale {scale}");
            assert_eq!(image.height(), art_size, "file art height at scale {scale}");
            assert_eq!(ring.width(), art_size, "ring width at scale {scale}");
            assert_eq!(ring.height(), art_size, "ring height at scale {scale}");
            let bounds = image
                .compute_bounds(circle.host.widget())
                .expect("host-clipped artwork bounds");
            assert!(
                (bounds.x() * 2.0 + bounds.width() - diameter as f32).abs() <= 1.0
                    && (bounds.y() * 2.0 + bounds.height() - diameter as f32).abs() <= 1.0,
                "compact artwork must be centered inside the border: {bounds:?} at {scale}"
            );
            assert!(bounds.x() >= 0.0 && bounds.y() >= 0.0);
            assert!(
                bounds.x() + bounds.width() <= diameter as f32,
                "x bounds {bounds:?}, host width {}, scale {scale}",
                circle.host.widget().width()
            );
            assert!(
                bounds.y() + bounds.height() <= diameter as f32,
                "y bounds {bounds:?}, host height {}, scale {scale}",
                circle.host.widget().height()
            );
            let paintable = image.paintable().expect("file artwork texture");
            assert!(paintable.intrinsic_width() <= decode_size);
            assert!(paintable.intrinsic_height() <= decode_size);
            assert_eq!(paintable.intrinsic_width(), paintable.intrinsic_height());
            assert!(
                circle
                    .host
                    .widget()
                    .pick(
                        f64::from(diameter) / 2.0,
                        f64::from(diameter) / 2.0,
                        gtk::PickFlags::DEFAULT,
                    )
                    .is_some(),
                "compact circle is targetable at scale {scale}"
            );
            let compact_art_x = bounds.x();
            // A 420ms hover keeps the compact page presented while its frame
            // grows. The art must not slide toward the middle of that frame
            // before the expanded page replaces it.
            circle
                .host
                .dispatch(super::super::circle::Event::Pointer(true));
            for design_width in [48, 120, 210] {
                let animated_frame = super::super::circle::Frame {
                    rect: super::super::circle::Rect {
                        width: metrics.spacing(design_width) as f64,
                        ..frame.rect
                    },
                    ..frame
                };
                circle
                    .host
                    .render(circle.host.revision(), Some(animated_frame));
                circle
                    .host
                    .widget()
                    .allocate(animated_frame.rect.width as i32, diameter, -1, None);
                fixed.queue_resize();
                while gtk::glib::MainContext::default().iteration(false) {}
                assert_eq!(
                    circle.host.presented_page(),
                    Some(super::super::circle::Mode::Compact)
                );
                let moving_bounds = image
                    .compute_bounds(circle.host.widget())
                    .expect("artwork during outgoing hover animation");
                assert!(
                    (moving_bounds.x() - compact_art_x).abs() <= 1.0,
                    "art slides at scale {scale}, width {design_width}: {moving_bounds:?} vs x={compact_art_x}"
                );
            }

            window.close();
            while gtk::glib::MainContext::default().iteration(false) {}

            // Map a new, production CircleHost in the actual media side-lane
            // width. All controls must fit without making
            // the shared scroller horizontally scrollable.
            let selected = Rc::new(Cell::new(0));
            let on_select = selected.clone();
            let hover_circle = super::MediaCircle::new(
                metrics,
                super::MediaCircleActions {
                    play_pause: Rc::new(|_| {}),
                    next: Rc::new(|_| {}),
                    previous: Rc::new(|_| {}),
                    select: Rc::new(move |_| on_select.set(on_select.get() + 1)),
                },
            )
            .expect("hover media circle");
            hover_circle.update(Some(&state));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while hover_circle.artwork_texture.borrow().is_none()
                && std::time::Instant::now() < deadline
            {
                while gtk::glib::MainContext::default().iteration(false) {}
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(hover_circle.artwork_texture.borrow().is_some());
            hover_circle
                .host
                .dispatch(super::super::circle::Event::Pointer(true));
            let expanded = super::super::circle::Frame {
                rect: super::super::circle::Rect {
                    x: 0.0,
                    y: 0.0,
                    width: metrics.spacing(if scale > 1.5 { 210 } else { 220 }) as f64,
                    height: metrics.spacing(40) as f64,
                },
                radius: metrics.spacing(16) as f64,
            };
            hover_circle
                .host
                .render(hover_circle.host.revision(), Some(expanded));
            hover_circle.host.commit_page(hover_circle.host.revision());
            let hover_window = gtk::Window::new();
            if let Some(class) = metrics.css_class() {
                hover_window.add_css_class(class);
            }
            let hover_fixed = gtk::Fixed::new();
            hover_fixed.set_hexpand(true);
            hover_fixed.set_vexpand(true);
            hover_fixed.set_size_request(metrics.spacing(250), metrics.spacing(48));
            hover_fixed.put(hover_circle.host.widget(), 0.0, 0.0);
            hover_window.set_default_size(metrics.spacing(250), metrics.spacing(48));
            hover_window.set_child(Some(&hover_fixed));
            hover_window.present();
            while gtk::glib::MainContext::default().iteration(false) {}
            assert!(
                hover_circle.host.widget().height() <= metrics.spacing(40),
                "media hover is shallow at scale {scale}: {}",
                hover_circle.host.widget().height()
            );
            let expanded_art = hover_circle
                .hover_artwork
                .compute_bounds(hover_circle.host.widget())
                .expect("circle-sized hover cover art bounds");
            assert!(
                (expanded_art.x() - bounds.x() - metrics.spacing(8) as f32).abs() <= 1.0,
                "hover art should keep its leading inset inside the border at scale {scale}: {expanded_art:?}"
            );
            assert!(
                (expanded_art.width() - art_size as f32).abs() <= 1.0
                    && (expanded_art.height() - art_size as f32).abs() <= 1.0,
                "hover art must remain circle-sized at scale {scale}: {expanded_art:?} vs diameter={art_size}"
            );
            assert!(
                expanded_art.y() >= 0.0
                    && expanded_art.y() + expanded_art.height() <= expanded.rect.height as f32,
                "circle-sized hover art must fit within the pill at scale {scale}: {expanded_art:?} vs {expanded:?}"
            );
            assert!(!hover_circle.hover_icon.is_visible());
            for button in [
                &hover_circle.previous,
                &hover_circle.play_pause,
                &hover_circle.next,
            ] {
                let bounds = button
                    .compute_bounds(hover_circle.host.widget())
                    .expect("media control bounds");
                assert!(bounds.width() >= metrics.spacing(26) as f32 - 1.0);
                assert!(bounds.height() >= metrics.spacing(30) as f32 - 1.0);
                let icon = button.child().expect("transport icon");
                if let Some(image) = icon.downcast_ref::<gtk::Image>() {
                    assert_eq!(image.pixel_size(), metrics.spacing(16));
                } else if let Some(label) = icon.downcast_ref::<gtk::Label>() {
                    assert_eq!(
                        label.pango_context().font_description().unwrap().size(),
                        metrics.spacing(16) * gtk::pango::SCALE
                    );
                }
                assert!(
                    bounds.x() >= 0.0 && bounds.x() + bounds.width() <= expanded.rect.width as f32,
                    "media control {bounds:?} outside frame {:?} at scale {scale}",
                    expanded.rect
                );
                assert!(
                    bounds.y() >= 0.0
                        && bounds.y() + bounds.height() <= expanded.rect.height as f32,
                    "media control {bounds:?} outside frame {:?} at scale {scale}",
                    expanded.rect
                );
                assert!(
                    hover_circle
                        .host
                        .widget()
                        .pick(
                            f64::from(bounds.x() + bounds.width() / 2.0),
                            f64::from(bounds.y() + bounds.height() / 2.0),
                            gtk::PickFlags::DEFAULT,
                        )
                        .is_some()
                );
            }
            let mut ancestor = hover_circle.hover_icon.parent();
            let mut scroller = None;
            while let Some(widget) = ancestor {
                if let Ok(found) = widget.clone().downcast::<gtk::ScrolledWindow>() {
                    scroller = Some(found);
                    break;
                }
                ancestor = widget.parent();
            }
            let scroller = scroller.expect("host media scroller");
            assert_eq!(scroller.hscrollbar_policy(), gtk::PolicyType::External);
            assert_eq!(scroller.vscrollbar_policy(), gtk::PolicyType::External);
            assert!(
                (expanded_art.y() * 2.0 + expanded_art.height() - expanded.rect.height as f32)
                    .abs()
                    <= 1.0,
                "hover cover must be vertically centered: {expanded_art:?} at {scale}"
            );
            assert!(
                scroller.hadjustment().upper() <= scroller.hadjustment().page_size() + 1.0,
                "unexpected horizontal media scroll at scale {scale}: upper={}, page={}",
                scroller.hadjustment().upper(),
                scroller.hadjustment().page_size()
            );
            assert!(
                hover_circle
                    .source_scroll
                    .emit_by_name::<bool>("scroll", &[&0.0_f64, &1.0_f64]),
            );
            assert_eq!(
                selected.get(),
                1,
                "wheel selected another source at {scale}"
            );
            assert_eq!(
                hover_circle.test_service().as_deref(),
                Some("org.test.other")
            );
            hover_circle
                .source_scroll
                .emit_by_name::<bool>("scroll", &[&0.0_f64, &-1.0_f64]);
            assert_eq!(
                hover_circle.test_service().as_deref(),
                Some("org.test.player")
            );
            for fraction in [0.4_f64, 0.4_f64] {
                hover_circle
                    .source_scroll
                    .emit_by_name::<bool>("scroll", &[&0.0_f64, &fraction]);
            }
            assert_eq!(
                selected.get(),
                2,
                "fractional wheel input has not crossed a step"
            );
            hover_circle
                .source_scroll
                .emit_by_name::<bool>("scroll", &[&0.0_f64, &0.3_f64]);
            assert_eq!(selected.get(), 3, "fractions accumulate to one source step");
            assert_eq!(
                hover_circle.test_service().as_deref(),
                Some("org.test.other")
            );
            hover_circle
                .source_scroll
                .emit_by_name::<bool>("scroll", &[&0.0_f64, &2.1_f64]);
            assert_eq!(
                selected.get(),
                5,
                "multi-unit wheel input applies each step"
            );
            assert_eq!(
                hover_circle.test_service().as_deref(),
                Some("org.test.other"),
                "two steps wrap back to the same source"
            );
            assert!(
                (hover_circle.scroll_progress.get() - 0.2).abs() < 1e-9,
                "preserve fractional overshoot across wheel events"
            );
            hover_circle
                .source_scroll
                .emit_by_name::<bool>("scroll", &[&0.0_f64, &-1.0_f64]);
            assert_eq!(selected.get(), 6, "reverse wheel direction changes source");
            assert_eq!(
                hover_circle.test_service().as_deref(),
                Some("org.test.player")
            );
            let next_fixture = std::env::temp_dir().join(format!("media-circle-next-{scale}.png"));
            image::RgbaImage::from_pixel(96, 320, image::Rgba([0x40, 0x98, 0xd0, 255]))
                .save(&next_fixture)
                .expect("second player cover art");
            let mut next_state = state.clone();
            next_state.service = "org.test.other".to_owned();
            next_state.player = "VLC".to_owned();
            next_state.art_url = Some(gtk::gio::File::for_path(&next_fixture).uri().to_string());
            hover_circle.update(Some(&next_state));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while hover_circle.artwork_texture.borrow().is_none()
                && std::time::Instant::now() < deadline
            {
                while gtk::glib::MainContext::default().iteration(false) {}
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let next_art = hover_circle
                .hover_icon
                .paintable()
                .expect("selected player's cover art");
            assert_eq!(next_art.intrinsic_width(), next_art.intrinsic_height());
            assert_eq!(
                hover_circle.hover_icon.tooltip_text().as_deref(),
                Some("Track — VLC")
            );
            next_state.art_url = None;
            hover_circle.update(Some(&next_state));
            assert!(hover_circle.artwork_texture.borrow().is_none());
            assert_eq!(
                hover_circle.hover_icon.icon_name().as_deref(),
                Some("audio-x-generic")
            );
            hover_circle
                .host
                .dispatch(super::super::circle::Event::Pointer(false));
            hover_circle
                .host
                .render(hover_circle.host.revision(), Some(frame));
            hover_circle.host.commit_page(hover_circle.host.revision());
            // CircleIntegration publishes each committed host frame directly
            // before queueing the parent allocation on the live Fixed root.
            hover_circle
                .host
                .widget()
                .allocate(diameter, diameter, -1, None);
            hover_fixed.queue_resize();
            hover_window.queue_resize();
            let wait = gtk::glib::MainLoop::new(None, false);
            let done = wait.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(60), move || {
                done.quit();
            });
            wait.run();
            while gtk::glib::MainContext::default().iteration(false) {}
            assert_eq!(
                hover_circle.compact_icon.width(),
                art_size,
                "art after leave/update at {scale}: mode={:?} presented={:?} host={}x{} compact_visible={} compact_mapped={}",
                hover_circle.host.mode(),
                hover_circle.host.presented_page(),
                hover_circle.host.widget().width(),
                hover_circle.host.widget().height(),
                hover_circle.compact_icon.is_visible(),
                hover_circle.compact_icon.is_mapped()
            );
            assert_eq!(
                hover_circle.progress_area.width(),
                art_size,
                "ring after leave/update at {scale}"
            );
            hover_circle
                .source_scroll
                .emit_by_name::<bool>("scroll", &[&0.0_f64, &-1.0_f64]);
            assert_eq!(
                hover_circle.test_service().as_deref(),
                Some("org.test.player"),
                "wheel switches source from the compact circle at scale {scale}"
            );

            hover_window.close();
            while gtk::glib::MainContext::default().iteration(false) {}
            let _ = std::fs::remove_file(fixture);
            let _ = std::fs::remove_file(next_fixture);
        }
    }

    #[cfg(test)]
    fn test_media_state(
        status: PlaybackStatus,
        length_us: Option<i64>,
    ) -> crate::state::MediaState {
        let player = |service: &str| MediaPlayer {
            player: service.to_owned(),
            service: service.to_owned(),
            title: "Track".to_owned(),
            artist: Some("Artist".to_owned()),
            album: None,
            app_icon: None,
            art_url: None,
            position_us: 1_000,
            length_us,
            can_play: true,
            can_pause: true,
            can_go_next: true,
            can_go_previous: true,
            status,
        };
        crate::state::MediaState {
            player: "org.test.player".to_owned(),
            service: "org.test.player".to_owned(),
            title: "Track".to_owned(),
            artist: Some("Artist".to_owned()),
            album: None,
            app_icon: None,
            art_url: None,
            position_us: 1_000,
            length_us,
            can_play: true,
            can_pause: true,
            can_go_next: true,
            can_go_previous: true,
            status,
            players: vec![player("org.test.player"), player("org.test.other")],
        }
    }
}
