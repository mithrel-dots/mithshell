//! Media content for the optional circle surface.
//!
//! This module deliberately owns the only interpolation timer used by the
//! circle.  The central island supplies the already-selected `MediaState` and
//! actions; it does not create another MPRIS listener or media store.

use std::{
    cell::{Cell, RefCell},
    io::Read,
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{Align, Orientation, gdk, glib, prelude::*};

mod geometry;
use geometry::{ART_SCALE, MediaGeometry, PADDING};

pub(super) const EXPANDED_SIZE: super::circle::Size = super::circle::Size {
    width: geometry::EXPANDED_WIDTH,
    height: geometry::EXPANDED_HEIGHT,
};

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
    progress_fraction(interpolated_position(progress, now), progress.length_us)
}

fn interpolated_position(progress: &Progress, now: Instant) -> i64 {
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
    position.max(0).min(
        progress
            .length_us
            .filter(|length| *length > 0)
            .unwrap_or(i64::MAX),
    )
}

fn format_timestamp(position_us: i64) -> String {
    let seconds = position_us.max(0) / 1_000_000;
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn time_readout(progress: &Progress, now: Instant) -> String {
    let elapsed = format_timestamp(interpolated_position(progress, now));
    let total = progress
        .length_us
        .filter(|length| *length > 0)
        .map_or_else(|| "--:--".to_owned(), format_timestamp);
    format!("{elapsed} / {total}")
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
    time_label: gtk::Label,
    art_layer: gtk::Fixed,
    art_tile: gtk::Overlay,
    art_icon: gtk::Image,
    geometry: Rc<Cell<MediaGeometry>>,
    scale: f64,
    compact_art_size: f64,
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

    #[cfg(test)]
    pub(crate) fn test_artwork(&self) -> gtk::Widget {
        self.art_tile.clone().upcast()
    }
    pub(super) fn new(
        metrics: Metrics,
        actions: MediaCircleActions,
    ) -> Result<Rc<Self>, &'static str> {
        let progress = Rc::new(RefCell::new(Progress::default()));
        let geometry = Rc::new(Cell::new(MediaGeometry::default()));
        let (art_tile, art_icon) = artwork_tile(metrics);
        let progress_area = progress_track(progress.clone(), geometry.clone());
        let compact = gtk::Box::new(Orientation::Horizontal, 0);
        let (hover, hover_title, hover_artist, previous, play_pause, next) = hover_page(metrics);
        let host = CircleHost::new(CircleContent {
            compact: compact.clone().upcast(),
            hover: hover.clone().upcast(),
            full: None,
        })?;
        let art_layer = gtk::Fixed::new();
        art_layer.set_can_target(false);
        art_layer.put(&art_tile, 0.0, 0.0);
        host.add_overlay(&art_layer);
        host.add_overlay(&progress_area);
        let time_label = gtk::Label::new(None);
        time_label.add_css_class("media-circle-time");
        time_label.set_halign(Align::Center);
        time_label.set_valign(Align::End);
        time_label.set_margin_bottom(metrics.spacing(6));
        time_label.set_can_target(false);
        time_label.set_opacity(0.0);
        set_text_size(&time_label, metrics, 10.0);
        host.add_overlay(&time_label);
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
            time_label,
            art_layer,
            art_tile,
            art_icon,
            geometry,
            scale: metrics.scale,
            compact_art_size: f64::from(media_art_diameter(metrics)),
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
            art_size: (f64::from(media_art_diameter(metrics)) * ART_SCALE)
                .round()
                .max(1.0) as u32,
        });
        circle.connect_actions();
        super::circle::scale_text(circle.host.widget(), metrics.scale);
        Ok(circle)
    }

    pub(super) fn host(&self) -> Rc<CircleHost> {
        self.host.clone()
    }

    pub(super) fn layout_presentation(&self, expansion: f64) {
        let Some(frame) = self.host.frame() else {
            return;
        };
        let origin = self
            .art_layer
            .compute_bounds(self.host.widget())
            .map_or((0.0, 0.0), |bounds| {
                (f64::from(bounds.x()), f64::from(bounds.y()))
            });
        let geometry = MediaGeometry::new(
            (frame.rect.width - 2.0 * origin.0).max(1.0),
            (frame.rect.height - 2.0 * origin.1).max(1.0),
            self.compact_art_size,
            self.scale,
            expansion,
        );
        let art = geometry.art;
        let scale = (art.size / f64::from(self.art_size)) as f32;
        let transform = gtk::gsk::Transform::new()
            .translate(&gtk::graphene::Point::new(art.x as f32, art.y as f32))
            .scale(scale, scale);
        self.art_layer
            .set_child_transform(&self.art_tile, Some(&transform));
        self.art_tile.allocate(
            self.art_size as i32,
            self.art_size as i32,
            -1,
            Some(transform),
        );
        self.geometry.set(geometry);
        self.time_label
            .set_opacity(((geometry.expansion - 0.7) / 0.3).clamp(0.0, 1.0));
        self.progress_area.queue_draw();
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
            *self.progress.borrow_mut() = Progress::default();
            self.current_service.borrow_mut().take();
            self.artwork_generation
                .set(self.artwork_generation.get().wrapping_add(1));
            self.artwork_url.borrow_mut().take();
            self.artwork_texture.borrow_mut().take();
            self.fallback_icon.borrow_mut().take();
            self.art_icon.set_paintable(None::<&gdk::Paintable>);
            self.host.widget().set_tooltip_text(None);
            self.host.dispatch(super::circle::Event::Content(false));
        }
        self.redraw_progress();
    }

    fn set_media(self: &Rc<Self>, state: &MediaState) {
        self.update_artwork(state);
        let info = [
            Some(state.title.as_str()),
            state.artist.as_deref(),
            state.album.as_deref(),
            Some(state.player.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
        self.host.widget().set_tooltip_text(Some(&info));
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
            self.art_icon.set_paintable(Some(texture));
        } else if changed || self.fallback_icon.borrow().as_ref() != state.app_icon.as_ref() {
            set_compact_art(&self.art_icon, state.app_icon.as_deref(), self.icon_style);
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
            owner.art_icon.set_paintable(Some(&texture));
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
            owner.redraw_progress();
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
        let text = time_readout(&self.progress.borrow(), Instant::now());
        if self.time_label.label().as_str() != text {
            self.time_label.set_label(&text);
        }
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

fn artwork_tile(metrics: Metrics) -> (gtk::Overlay, gtk::Image) {
    let overlay = gtk::Overlay::new();
    let art_size = (f64::from(media_art_diameter(metrics)) * ART_SCALE).round() as i32;
    overlay.set_size_request(art_size, art_size);
    overlay.set_halign(Align::Start);
    overlay.set_valign(Align::Start);
    overlay.add_css_class("media-circle-art-tile");
    overlay.set_overflow(gtk::Overflow::Hidden);
    let image = gtk::Image::new();
    image.set_pixel_size(art_size);
    image.set_size_request(art_size, art_size);
    image.set_halign(Align::Fill);
    image.set_valign(Align::Fill);
    overlay.set_child(Some(&image));
    (overlay, image)
}

fn progress_track(
    progress: Rc<RefCell<Progress>>,
    geometry: Rc<Cell<MediaGeometry>>,
) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_can_target(false);
    area.add_css_class("media-circle-progress");
    area.set_draw_func(move |area, cr, _, _| {
        let geometry = geometry.get();
        let fraction = interpolated_progress(&progress.borrow(), Instant::now());
        let color = area.color();
        cr.set_line_width(geometry.stroke);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        let stroke = |fraction, opacity| {
            geometry.track.append_path(cr, fraction);
            cr.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                f64::from(color.alpha()) * opacity,
            );
            let _ = cr.stroke();
        };
        stroke(1.0, 0.16 * geometry.expansion);
        if fraction > 0.0 {
            stroke(fraction, 1.0);
        }
    });
    area
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

fn set_text_size(label: &gtk::Label, metrics: Metrics, size: f64) {
    label.add_css_class("circle-scaled-text");
    let style = gtk::CssProvider::new();
    style.load_from_string(&format!(
        "label {{ font-size: {}px; }}",
        (size * metrics.scale).round()
    ));
    #[allow(deprecated)]
    label
        .style_context()
        .add_provider(&style, gtk::STYLE_PROVIDER_PRIORITY_USER + 1);
}

fn hover_page(
    metrics: Metrics,
) -> (
    gtk::Box,
    gtk::Label,
    gtk::Label,
    gtk::Button,
    gtk::Button,
    gtk::Button,
) {
    let root = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    root.set_margin_start(metrics.spacing(PADDING));
    root.set_margin_end(metrics.spacing(PADDING));
    root.set_margin_top(metrics.spacing(PADDING));
    root.set_margin_bottom(metrics.spacing(28));
    root.set_valign(Align::Start);
    root.set_vexpand(false);
    root.add_css_class("media-circle-hover");

    let art_size = (f64::from(media_art_diameter(metrics)) * ART_SCALE).round() as i32;
    let art_slot = gtk::Box::new(Orientation::Horizontal, 0);
    art_slot.set_size_request(art_size, art_size);
    art_slot.set_valign(Align::Center);
    root.append(&art_slot);
    let details = gtk::Box::new(Orientation::Vertical, metrics.spacing(8));
    details.set_hexpand(true);
    details.set_valign(Align::Center);
    let text = gtk::Box::new(Orientation::Vertical, 0);
    text.set_hexpand(true);
    text.set_valign(Align::Center);
    let title = gtk::Label::new(None);
    title.set_xalign(0.5);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_single_line_mode(true);
    title.set_width_chars(2);
    title.set_max_width_chars(24);
    title.add_css_class("media-circle-title");
    let artist = gtk::Label::new(None);
    artist.set_xalign(0.5);
    artist.add_css_class("dim-label");
    artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    artist.set_single_line_mode(true);
    artist.set_max_width_chars(24);
    artist.add_css_class("media-circle-artist");
    let large = metrics.css_class() == Some("scale-large");
    for (label, size) in [
        (&title, if large { 13.0 } else { 14.0 }),
        (&artist, if large { 11.0 } else { 12.0 }),
    ] {
        set_text_size(label, metrics, size);
    }
    text.append(&title);
    text.append(&artist);
    details.append(&text);
    let controls = gtk::Box::new(Orientation::Horizontal, metrics.spacing(2));
    controls.set_halign(Align::Center);
    let previous = icon::icon_button(Icon::Previous, metrics.icons);
    let play = icon::icon_button(Icon::Play, metrics.icons);
    play.add_css_class("media-circle-play");
    let next = icon::icon_button(Icon::Next, metrics.icons);
    for button in [&previous, &play, &next] {
        button.add_css_class("media-circle-control");
        let style = gtk::CssProvider::new();
        style.load_from_string(&format!(
            "button {{ min-width: {}px; min-height: {}px; padding: {}px; }}",
            metrics.spacing(if button == &play { 24 } else { 20 }),
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
        controls.append(button);
    }
    details.append(&controls);
    root.append(&details);
    (root, title, artist, previous, play, next)
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
    fn elapsed_time_keeps_ticking_without_a_known_duration() {
        let progress = Progress {
            status: PlaybackStatus::Playing,
            length_us: None,
            ..Default::default()
        };
        assert!(timer_needed(&progress));
        assert!(timer_needed(&Progress {
            status: PlaybackStatus::Playing,
            length_us: Some(1),
            ..Default::default()
        }));
        assert!(!timer_needed(&Progress {
            status: PlaybackStatus::Paused,
            ..progress
        }));
    }

    #[test]
    fn time_readout_follows_playback_pause_and_duration_bounds() {
        let now = std::time::Instant::now();
        let mut progress = Progress {
            position_us: 192_000_000,
            length_us: Some(220_000_000),
            status: PlaybackStatus::Playing,
            started_at: Some(now),
        };
        let later = now + std::time::Duration::from_secs(2);
        assert_eq!(super::time_readout(&progress, now), "3:12 / 3:40");
        assert_eq!(super::time_readout(&progress, later), "3:14 / 3:40");
        progress.status = PlaybackStatus::Paused;
        assert_eq!(super::time_readout(&progress, later), "3:12 / 3:40");
        progress.position_us = 221_000_000;
        assert_eq!(super::time_readout(&progress, later), "3:40 / 3:40");
        progress.position_us = -1;
        assert_eq!(super::time_readout(&progress, later), "0:00 / 3:40");
        progress.position_us = 3_723_000_000;
        progress.length_us = Some(5_400_000_000);
        assert_eq!(super::time_readout(&progress, later), "1:02:03 / 1:30:00");
        progress.status = PlaybackStatus::Playing;
        progress.length_us = None;
        assert_eq!(super::time_readout(&progress, later), "1:02:05 / --:--");
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
            circle.tick.borrow().is_some(),
            "unknown duration still advances elapsed time"
        );
        assert_eq!(circle.time_label.label(), "0:00 / --:--");
        circle.update(Some(&test_media_state(PlaybackStatus::Paused, None)));
        assert!(
            circle.tick.borrow().is_none(),
            "pause stops the elapsed clock"
        );
        circle.update(Some(&state));
        assert!(circle.tick.borrow().is_some());
        assert_eq!(circle.time_label.label(), "0:00 / 0:10");
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
            image::RgbaImage::from_fn(640, 400, |x, y| {
                image::Rgba([(x / 3) as u8, (y / 2) as u8, 0x80, 255])
            })
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
            let mut state = test_media_state(PlaybackStatus::Paused, Some(220_000_000));
            state.position_us = 192_000_000;
            state.album = Some("Album".to_owned());
            state.app_icon = Some("audio-x-generic".to_owned());
            state.art_url = Some(gtk::gio::File::for_path(&fixture).uri().to_string());
            circle.update(Some(&state));
            let window = gtk::Window::new();
            if let Some(class) = metrics.css_class() {
                window.add_css_class(class);
            }
            let fixed = gtk::Fixed::new();
            fixed.set_size_request(metrics.spacing(350), metrics.spacing(160));
            fixed.put(circle.host.widget(), 0.0, 0.0);
            window.set_default_size(metrics.spacing(350), metrics.spacing(160));
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
            circle.layout_presentation(0.0);

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
            let decode_size = art_size * 3;
            let image = &circle.art_icon;
            let ring = &circle.progress_area;
            assert_eq!(
                image.width(),
                decode_size,
                "full-resolution art allocation at scale {scale}"
            );
            assert_eq!(
                image.height(),
                decode_size,
                "full-resolution art allocation at scale {scale}"
            );
            assert_eq!(ring.width(), art_size, "ring width at scale {scale}");
            assert_eq!(ring.height(), art_size, "ring height at scale {scale}");
            let bounds = image
                .compute_bounds(circle.host.widget())
                .expect("host-clipped artwork bounds");
            assert!((bounds.width() - art_size as f32).abs() <= 1.0);
            assert!((bounds.height() - art_size as f32).abs() <= 1.0);
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
            assert_eq!(paintable.intrinsic_width(), decode_size);
            assert_eq!(paintable.intrinsic_height(), decode_size);
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
            capture_media_fixture(&circle, &format!("media-{scale}-compact"));
            let art_parent = circle.art_tile.parent();
            circle
                .host
                .dispatch(super::super::circle::Event::Pointer(true));
            circle.host.commit_page(circle.host.revision());
            for (step, progress) in [0.0, 0.1, 0.35, 0.8, 1.0, 0.6, 0.2, 0.0]
                .into_iter()
                .enumerate()
            {
                let animated_frame = super::super::circle::Frame {
                    rect: super::super::circle::Rect {
                        width: ((32.0 + (super::EXPANDED_SIZE.width - 32.0) * progress) * scale)
                            .round(),
                        height: ((32.0 + (super::EXPANDED_SIZE.height - 32.0) * progress) * scale)
                            .round(),
                        ..frame.rect
                    },
                    ..frame
                };
                circle
                    .host
                    .render(circle.host.revision(), Some(animated_frame));
                circle.host.widget().allocate(
                    animated_frame.rect.width as i32,
                    animated_frame.rect.height as i32,
                    -1,
                    None,
                );
                circle.layout_presentation(progress);
                fixed.queue_resize();
                while gtk::glib::MainContext::default().iteration(false) {}
                assert_eq!(
                    circle.host.presented_page(),
                    Some(super::super::circle::Mode::HoverExpanded)
                );
                let moving_bounds = image
                    .compute_bounds(circle.host.widget())
                    .expect("persistent artwork during geometry animation");
                assert!(
                    (moving_bounds.x()
                        - compact_art_x
                        - metrics.spacing(super::PADDING) as f32 * progress as f32)
                        .abs()
                        <= 1.0,
                    "art must follow expansion at scale {scale}, progress {progress}: {moving_bounds:?}"
                );
                assert!(
                    (moving_bounds.y()
                        - bounds.y()
                        - metrics.spacing(super::PADDING) as f32 * progress as f32)
                        .abs()
                        <= 1.0
                );
                assert!(
                    (moving_bounds.width() - art_size as f32 * (1.0 + 2.0 * progress as f32)).abs()
                        <= 1.0
                );
                assert!((moving_bounds.height() - moving_bounds.width()).abs() <= 1.0);
                assert_eq!(circle.art_tile.parent(), art_parent);
                assert!(image.is_mapped());
                assert_eq!(circle.progress_area.opacity(), 1.0);
                capture_media_fixture(&circle, &format!("media-{scale}-step-{step}"));
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
                    width: metrics.spacing(if scale > 1.5 {
                        210
                    } else {
                        super::EXPANDED_SIZE.width as i32
                    }) as f64,
                    height: metrics.spacing(super::EXPANDED_SIZE.height as i32) as f64,
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
            hover_fixed.set_size_request(metrics.spacing(350), metrics.spacing(160));
            hover_fixed.put(hover_circle.host.widget(), 0.0, 0.0);
            hover_window.set_default_size(metrics.spacing(350), metrics.spacing(160));
            hover_window.set_child(Some(&hover_fixed));
            hover_window.present();
            while gtk::glib::MainContext::default().iteration(false) {}
            hover_circle.layout_presentation(1.0);
            assert!(
                hover_circle.host.widget().height() <= expanded.rect.height as i32,
                "media card must respect its frame at scale {scale}: {}",
                hover_circle.host.widget().height()
            );
            let expanded_art = hover_circle
                .art_icon
                .compute_bounds(hover_circle.host.widget())
                .expect("expanded cover art bounds");
            assert!(
                (expanded_art.x() - bounds.x() - metrics.spacing(super::PADDING) as f32).abs()
                    <= 1.0,
                "hover art should keep its leading inset inside the border at scale {scale}: {expanded_art:?}"
            );
            assert!(
                (expanded_art.width() - (art_size * 3) as f32).abs() <= 1.0
                    && (expanded_art.height() - (art_size * 3) as f32).abs() <= 1.0,
                "hover art must grow threefold at scale {scale}: {expanded_art:?} vs compact diameter={art_size}"
            );
            assert!(
                expanded_art.y() >= 0.0
                    && expanded_art.y() + expanded_art.height() <= expanded.rect.height as f32,
                "expanded art must fit within the card at scale {scale}: {expanded_art:?} vs {expanded:?}"
            );
            assert!(hover_circle.art_icon.is_visible());
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
                if button == &hover_circle.play_pause {
                    assert!(
                        (bounds.width() - bounds.height()).abs() <= 1.0,
                        "play button must be circular: {bounds:?}"
                    );
                }
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
            let mut ancestor = hover_circle.play_pause.parent();
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
                (expanded_art.y() - bounds.y() - metrics.spacing(super::PADDING) as f32).abs()
                    <= 1.0,
                "hover cover must align with the content padding: {expanded_art:?} at {scale}"
            );
            let geometry = hover_circle.geometry.get();
            let progress_bounds = hover_circle
                .progress_area
                .compute_bounds(hover_circle.host.widget())
                .unwrap();
            let (start_x, bar_y) = geometry.track.point(0.0);
            let (end_x, _) = geometry.track.point(1.0);
            let padding = f64::from(metrics.spacing(super::PADDING));
            assert!((start_x - geometry.stroke / 2.0 - padding).abs() <= 1.0);
            assert!(
                (end_x + geometry.stroke / 2.0 + padding - f64::from(progress_bounds.width()))
                    .abs()
                    <= 1.0
            );
            assert!(
                bar_y + f64::from(progress_bounds.y())
                    > f64::from(expanded_art.y() + expanded_art.height())
            );
            assert_eq!(hover_circle.time_label.label(), "3:12 / 3:40");
            let time_bounds = hover_circle
                .time_label
                .compute_bounds(hover_circle.host.widget())
                .unwrap();
            assert!(
                f64::from(time_bounds.y())
                    > bar_y + f64::from(progress_bounds.y()) + geometry.stroke / 2.0
            );
            assert!(time_bounds.y() + time_bounds.height() <= expanded.rect.height as f32);
            capture_media_fixture(&hover_circle, &format!("media-{scale}-constrained"));
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
                .art_icon
                .paintable()
                .expect("selected player's cover art");
            assert_eq!(next_art.intrinsic_width(), next_art.intrinsic_height());
            assert_eq!(
                hover_circle.host.widget().tooltip_text().as_deref(),
                Some("Track\nArtist\nAlbum\nVLC")
            );
            next_state.art_url = None;
            hover_circle.update(Some(&next_state));
            assert!(hover_circle.artwork_texture.borrow().is_none());
            assert_eq!(
                hover_circle.art_icon.icon_name().as_deref(),
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
            hover_circle.layout_presentation(0.0);
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
                hover_circle.art_icon.width(),
                decode_size,
                "art after leave/update at {scale}: mode={:?} presented={:?} host={}x{} compact_visible={} compact_mapped={}",
                hover_circle.host.mode(),
                hover_circle.host.presented_page(),
                hover_circle.host.widget().width(),
                hover_circle.host.widget().height(),
                hover_circle.art_icon.is_visible(),
                hover_circle.art_icon.is_mapped()
            );
            assert_eq!(
                hover_circle.progress_area.width(),
                art_size,
                "ring after leave/update at {scale}"
            );
            let collapsed_art = hover_circle
                .art_icon
                .compute_bounds(hover_circle.host.widget())
                .unwrap();
            assert!((collapsed_art.width() - art_size as f32).abs() <= 1.0);
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

    fn capture_media_fixture(circle: &super::MediaCircle, name: &str) {
        let Ok(directory) = std::env::var("MITHSHELL_UI_CAPTURE_DIR") else {
            return;
        };
        let main_loop = gtk::glib::MainLoop::new(None, false);
        let quit = main_loop.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
            quit.quit()
        });
        main_loop.run();
        let widget = circle.host.widget();
        let paintable = gtk::WidgetPaintable::new(Some(widget));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let node = loop {
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(
                &snapshot,
                f64::from(widget.width()),
                f64::from(widget.height()),
            );
            if let Some(node) = snapshot.to_node() {
                break node;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "media card must paint {name}"
            );
            widget.queue_draw();
            let quit = main_loop.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(20), move || {
                quit.quit()
            });
            main_loop.run();
        };
        let renderer = gtk::gsk::CairoRenderer::new();
        renderer.realize(None::<&gtk::gdk::Surface>).unwrap();
        let texture = renderer.render_texture(&node, None);
        std::fs::create_dir_all(&directory).unwrap();
        texture
            .save_to_png(std::path::Path::new(&directory).join(format!("{name}.png")))
            .unwrap();
        renderer.unrealize();
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
