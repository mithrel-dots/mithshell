//! Visual feedback for ginkgo voice notes.
//!
//! Subscribes to ginkgo's read-only NDJSON voice event socket
//! (`$XDG_RUNTIME_DIR/ginkgo-voice.sock`) and shows a bubble in a screen
//! corner once the trigger word arms a note, until that note is processed.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    env,
    f64::consts::TAU,
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    path::PathBuf,
    rc::Rc,
    thread,
    time::{Duration, Instant},
};

use async_channel::Sender;
use gtk::{glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use log::debug;
use serde::Deserialize;

use crate::{
    config::{AppConfig, VoiceAnimation, VoiceConfig},
    ui::motion::{Profile, Transition},
};

const RETRY_DELAY: Duration = Duration::from_secs(1);
const ORB_SIZE: i32 = 124;
/// Must match `.voice-surface { margin }`, which leaves room for its shadow.
const SHADOW_SPACE: i32 = 24;
const LINGER: Duration = Duration::from_millis(1500);
const PHASE_CLASSES: [&str; 5] = ["recording", "processing", "saved", "canceled", "failed"];

#[derive(Debug, Clone, Default, Deserialize)]
struct WireEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    recording_note: bool,
    #[serde(default)]
    active_note_session_id: String,
    #[serde(default)]
    note_session_id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    level: Option<f64>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone)]
enum VoiceEvent {
    Connected(bool),
    Event(WireEvent),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Outcome {
    #[default]
    None,
    Saved,
    Canceled,
    Failed,
}

#[derive(Debug, Default)]
struct VoiceState {
    connected: bool,
    recording: bool,
    level: f64,
    /// Smoothed level used for drawing.
    display_level: f64,
    /// Seconds, for the processing spinner.
    phase: f64,
    text: String,
    outcome: Outcome,
    /// Note sessions armed by the trigger word that have not yet been saved,
    /// canceled, or failed. The bubble is only shown while one is in flight.
    pending: HashSet<String>,
    /// Keeps the bubble up briefly after the last session settles.
    lingering: bool,
}

impl VoiceState {
    fn active(&self) -> bool {
        self.connected && (self.recording || !self.pending.is_empty())
    }

    fn phase_class(&self) -> &'static str {
        match self.outcome {
            Outcome::Failed => "failed",
            _ if self.recording => "recording",
            _ if !self.pending.is_empty() => "processing",
            Outcome::Canceled => "canceled",
            _ => "saved",
        }
    }

    fn status(&self) -> &'static str {
        match self.outcome {
            Outcome::Failed => "Failed",
            _ if self.recording => "Listening",
            _ if !self.pending.is_empty() => "Processing",
            Outcome::Canceled => "Canceled",
            _ => "Saved",
        }
    }
}

fn socket_path() -> PathBuf {
    if let Some(path) = env::var_os("GINKGO_VOICE_SOCKET").filter(|v| !v.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(dir) = env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir).join("ginkgo-voice.sock");
    }
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".local/share/ginkgo/ipc-voice.sock")
}

fn start_listener(sender: Sender<VoiceEvent>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let path = socket_path();
        loop {
            match UnixStream::connect(&path) {
                Ok(stream) => {
                    if sender.send_blocking(VoiceEvent::Connected(true)).is_err() {
                        return;
                    }
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { break };
                        match serde_json::from_str::<WireEvent>(&line) {
                            Ok(event) => {
                                if sender.send_blocking(VoiceEvent::Event(event)).is_err() {
                                    return;
                                }
                            }
                            Err(error) => debug!("ignoring malformed ginkgo voice event: {error}"),
                        }
                    }
                    if sender.send_blocking(VoiceEvent::Connected(false)).is_err() {
                        return;
                    }
                }
                Err(error) => debug!("ginkgo voice socket unavailable: {error}"),
            }
            thread::sleep(RETRY_DELAY);
        }
    })
}

/// Enter/exit animation of the whole window: fades its opacity or slides its
/// horizontal layer-shell margin in from beyond the anchored screen edge.
struct Presence {
    window: gtk::ApplicationWindow,
    config: VoiceConfig,
    enter: Profile,
    exit: Profile,
    /// 0 is fully hidden, 1 fully shown.
    value: Cell<f64>,
    from: Cell<f64>,
    target: Cell<f64>,
    started: Cell<Option<Instant>>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
}

impl Presence {
    fn side(&self) -> Edge {
        if self.config.position.is_left() {
            Edge::Left
        } else {
            Edge::Right
        }
    }

    fn apply(&self, value: f64) {
        self.value.set(value);
        match self.config.animation {
            VoiceAnimation::Fade => self.window.set_opacity(value),
            VoiceAnimation::Slide => {
                let width = self
                    .window
                    .width()
                    .max(self.window.measure(gtk::Orientation::Horizontal, -1).1);
                let hidden = -f64::from(width);
                let margin =
                    hidden + (f64::from(self.config.margin - SHADOW_SPACE) - hidden) * value;
                self.window.set_margin(self.side(), margin.round() as i32);
            }
        }
    }

    fn set_shown(self: &Rc<Self>, shown: bool) {
        let target = if shown { 1.0 } else { 0.0 };
        if self.target.get() == target {
            return;
        }
        self.target.set(target);
        self.from.set(self.value.get());
        self.started.set(Some(Instant::now()));
        if shown && !self.window.is_visible() {
            self.apply(0.0);
            self.window.set_visible(true);
        }
        if self.tick.borrow().is_some() {
            return;
        }
        let presence = self.clone();
        let id = self.window.add_tick_callback(move |_, _| presence.step());
        *self.tick.borrow_mut() = Some(id);
    }

    fn step(&self) -> glib::ControlFlow {
        let target = self.target.get();
        let profile = if target > 0.5 { self.enter } else { self.exit };
        let elapsed = self
            .started
            .get()
            .map_or(Duration::MAX, |start| start.elapsed());
        let from = self.from.get();
        self.apply(from + (target - from) * profile.progress(elapsed));
        if !profile.is_complete(elapsed) {
            return glib::ControlFlow::Continue;
        }
        self.tick.borrow_mut().take();
        if target < 0.5 {
            self.window.set_visible(false);
        }
        glib::ControlFlow::Break
    }
}

/// Creates the voice bubble window and starts listening. The window stays
/// alive for the lifetime of the application.
pub fn start(application: &gtk::Application, config: &AppConfig, animations: bool) {
    let voice = config.voice;
    if !voice.enabled {
        return;
    }
    let (sender, receiver) = async_channel::unbounded();
    start_listener(sender);

    let motion = config.shell.motion;
    let legacy_ms = config.shell.animation_ms;
    let (enter, exit) = match voice.animation {
        VoiceAnimation::Fade => (Transition::IslandFadeIn, Transition::IslandFadeOut),
        VoiceAnimation::Slide => (Transition::PeekEnter, Transition::PeekExit),
    };
    let enter = enter.resolve(motion, animations, legacy_ms);
    let exit = exit.resolve(motion, animations, legacy_ms);

    let left = voice.position.is_left();
    let window = gtk::ApplicationWindow::new(application);
    window.add_css_class("mithshell-window");
    window.set_decorated(false);
    window.init_layer_shell();
    window.set_namespace(Some("mithshell-voice"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::None);
    let vertical = if voice.position.is_top() {
        Edge::Top
    } else {
        Edge::Bottom
    };
    let horizontal = if left { Edge::Left } else { Edge::Right };
    window.set_anchor(vertical, true);
    window.set_anchor(horizontal, true);
    window.set_margin(vertical, voice.margin - SHADOW_SPACE);
    window.set_margin(horizontal, voice.margin - SHADOW_SPACE);
    window.set_exclusive_zone(0);

    let surface = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    surface.add_css_class("voice-surface");
    surface.set_halign(if left {
        gtk::Align::Start
    } else {
        gtk::Align::End
    });
    surface.set_valign(gtk::Align::Center);

    let orb = gtk::DrawingArea::new();
    orb.add_css_class("voice-orb");
    orb.set_content_width(ORB_SIZE);
    orb.set_content_height(ORB_SIZE);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text.add_css_class("voice-text");
    text.set_valign(gtk::Align::Center);
    let status = gtk::Label::new(None);
    status.add_css_class("voice-status");
    let transcript = gtk::Label::new(None);
    transcript.add_css_class("voice-transcript");
    transcript.set_max_width_chars(36);
    transcript.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    let xalign = if left { 0.0 } else { 1.0 };
    status.set_xalign(xalign);
    transcript.set_xalign(xalign);
    text.append(&status);
    text.append(&transcript);

    if left {
        surface.append(&orb);
        surface.append(&text);
    } else {
        text.add_css_class("trailing");
        surface.append(&text);
        surface.append(&orb);
    }
    window.set_child(Some(&surface));

    // Click-through.
    window.connect_realize(|window| {
        if let Some(surface) = window.surface() {
            surface.set_input_region(Some(&gtk::cairo::Region::create()));
        }
    });

    let presence = Rc::new(Presence {
        window: window.clone(),
        config: voice,
        enter,
        exit,
        value: Cell::new(0.0),
        from: Cell::new(0.0),
        target: Cell::new(0.0),
        started: Cell::new(None),
        tick: RefCell::new(None),
    });

    let state = Rc::new(RefCell::new(VoiceState::default()));

    let draw_state = state.clone();
    orb.set_draw_func(move |orb, cr, width, height| {
        draw_orb(orb, cr, width, height, &draw_state.borrow());
    });

    // Recording follows the mic level; processing pulses on its own.
    let tick_state = state.clone();
    orb.add_tick_callback(move |orb, clock| {
        let mut state = tick_state.borrow_mut();
        state.phase = clock.frame_time() as f64 / 1_000_000.0;
        let target = if state.recording {
            state.level
        } else if !state.pending.is_empty() {
            0.3 + 0.2 * (state.phase * TAU * 0.8).sin()
        } else {
            0.0
        };
        state.display_level += (target - state.display_level) * 0.25;
        state.level *= 0.92;
        orb.queue_draw();
        glib::ControlFlow::Continue
    });

    let refresh = {
        let state = state.clone();
        let presence = presence.clone();
        Rc::new(move || {
            let s = state.borrow();
            let phase = s.phase_class();
            for class in PHASE_CLASSES {
                if class == phase {
                    surface.add_css_class(class);
                } else {
                    surface.remove_css_class(class);
                }
            }
            status.set_label(s.status());
            transcript.set_label(&s.text);
            transcript.set_visible(!s.text.is_empty());
            presence.set_shown(s.active() || s.lingering);
            orb.queue_draw();
        })
    };

    glib::MainContext::default().spawn_local(async move {
        while let Ok(event) = receiver.recv().await {
            let was_active = state.borrow().active();
            {
                let mut s = state.borrow_mut();
                match event {
                    VoiceEvent::Connected(connected) => {
                        s.connected = connected;
                        if !connected {
                            s.recording = false;
                            s.pending.clear();
                            s.lingering = false;
                        }
                    }
                    VoiceEvent::Event(event) => apply(&mut s, event),
                }
                if !was_active && s.active() {
                    // A new trigger starts a fresh bubble.
                    s.lingering = false;
                    s.outcome = Outcome::None;
                    s.text.clear();
                }
            }
            let settled = was_active && !state.borrow().active();
            if settled {
                state.borrow_mut().lingering = true;
                let state = state.clone();
                let refresh = refresh.clone();
                glib::timeout_add_local_once(LINGER, move || {
                    {
                        let mut s = state.borrow_mut();
                        if s.active() {
                            return;
                        }
                        s.lingering = false;
                    }
                    refresh();
                });
            }
            refresh();
        }
    });
}

fn apply(s: &mut VoiceState, event: WireEvent) {
    s.recording = event.recording_note;
    if !event.active_note_session_id.is_empty() {
        s.pending.insert(event.active_note_session_id);
    }
    match event.kind.as_str() {
        "level" => {
            if let Some(level) = event.level {
                s.level = s.level.max(level);
            }
        }
        // Ambient captions are ignored; only text attributed to a triggered
        // note (or spoken while one is being recorded) is shown.
        "transcript" if s.recording || s.pending.contains(&event.note_session_id) => {
            s.text = event.text;
        }
        "note.saved" => {
            s.pending.remove(&event.note_session_id);
            s.outcome = Outcome::Saved;
        }
        "note.canceled" if s.pending.remove(&event.note_session_id) => {
            s.outcome = Outcome::Canceled;
        }
        "note.error" => {
            s.pending.remove(&event.note_session_id);
            s.outcome = Outcome::Failed;
            s.text = event.error.unwrap_or_default();
        }
        "error" if s.active() => {
            s.pending.clear();
            s.outcome = Outcome::Failed;
            s.text = event.error.unwrap_or_default();
        }
        _ => {}
    }
}

fn draw_orb(
    orb: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
    state: &VoiceState,
) {
    let color = orb.color();
    let (r, g, b) = (
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
    );
    let (cx, cy) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
    let max_r = cx.min(cy);
    let core = max_r * 0.46;
    let level = state.display_level.clamp(0.0, 1.0);

    // Outer halo follows the level.
    let halo = core + (max_r - core) * level;
    cr.set_source_rgba(r, g, b, 0.16);
    cr.arc(cx, cy, halo, 0.0, TAU);
    let _ = cr.fill();
    let inner = core + (max_r - core) * level * 0.55;
    cr.set_source_rgba(r, g, b, 0.22);
    cr.arc(cx, cy, inner, 0.0, TAU);
    let _ = cr.fill();

    // Ring: a rotating arc while processing, a faint full ring otherwise.
    let ring = core + max_r * 0.12;
    cr.set_line_width(max_r * 0.047);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    if state.recording || state.pending.is_empty() {
        cr.set_source_rgba(r, g, b, 0.45);
        cr.arc(cx, cy, ring, 0.0, TAU);
    } else {
        let start = state.phase * TAU * 0.9;
        cr.set_source_rgba(r, g, b, 0.9);
        cr.arc(cx, cy, ring, start, start + TAU * 0.3);
    }
    let _ = cr.stroke();

    // Core with a soft highlight.
    let gradient = gtk::cairo::RadialGradient::new(
        cx - core * 0.35,
        cy - core * 0.35,
        core * 0.1,
        cx,
        cy,
        core,
    );
    let lift = |c: f64| c + (1.0 - c) * 0.35;
    gradient.add_color_stop_rgba(0.0, lift(r), lift(g), lift(b), 1.0);
    gradient.add_color_stop_rgba(1.0, r, g, b, 1.0);
    let _ = cr.set_source(&gradient);
    cr.arc(cx, cy, core, 0.0, TAU);
    let _ = cr.fill();
}
