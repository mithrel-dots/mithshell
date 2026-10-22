//! Visual feedback for ginkgo voice notes.
//!
//! Subscribes to ginkgo's read-only NDJSON voice event socket
//! (`$XDG_RUNTIME_DIR/ginkgo-voice.sock`) and shows a bubble in a screen
//! corner for capture transitions, selected notes, and conversational outcomes.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    env,
    f64::consts::TAU,
    io::{BufRead, BufReader, Read},
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
const MAX_RETRY_DELAY: Duration = Duration::from_secs(16);
const MAX_MESSAGE_BYTES: u64 = 65_536;
const ORB_SIZE: i32 = 124;
/// Must match `.voice-surface { margin }`, which leaves room for its shadow.
const SHADOW_SPACE: i32 = 24;
const LINGER: Duration = Duration::from_millis(1500);
const PHASE_CLASSES: [&str; 5] = ["recording", "processing", "saved", "canceled", "failed"];

#[derive(Debug, Clone, Default, Deserialize)]
struct WireEvent {
    #[serde(rename = "type")]
    kind: String,
    recording_note: bool,
    active_note_session_id: String,
    schema_version: u32,
    epoch_id: String,
    seq: u64,
    observed_at: String,
    listening: bool,
    #[serde(rename = "final")]
    final_text: bool,
    #[serde(default)]
    note_session_id: String,
    text: String,
    #[serde(default)]
    level: Option<f64>,
    #[serde(default)]
    error: Option<String>,
    capture_state: Option<String>,
    stream_id: Option<String>,
    capture_epoch_id: Option<String>,
    namespace: Option<String>,
    mode: Option<String>,
    capture_error: Option<String>,
    utterance_id: Option<String>,
    revision: Option<u64>,
    action: Option<String>,
    note_id: Option<String>,
    note_ids: Option<Vec<String>>,
    processing: Option<String>,
    fallback_reason: Option<String>,
    image_status: Option<String>,
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
    Appended,
    Response,
    Clarify,
    Canceled,
    Failed,
}

#[derive(Debug, Default)]
struct VoiceState {
    connected: bool,
    awaiting_snapshot: bool,
    epoch_id: String,
    seq: Option<u64>,
    listening: bool,
    capture_state: String,
    stream_id: Option<String>,
    capture_epoch_id: Option<String>,
    namespace: Option<String>,
    mode: Option<String>,
    capture_error: Option<String>,
    active_session: String,
    /// Revisions are keyed by opaque utterance identity, never audio offsets.
    transcripts: HashMap<String, WireEvent>,
    outcomes: HashMap<String, WireEvent>,
    displayed_session: String,
    details: String,
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
        self.connected
            && (self.recording
                || !self.pending.is_empty()
                || matches!(
                    self.capture_state.as_str(),
                    "starting" | "renewing" | "stopping" | "error"
                )
                || matches!(self.outcome, Outcome::Response | Outcome::Clarify))
    }

    fn phase_class(&self) -> &'static str {
        match self.outcome {
            Outcome::Failed => "failed",
            _ if self.capture_state == "error" => "failed",
            Outcome::Clarify | Outcome::Response => "processing",
            _ if matches!(
                self.capture_state.as_str(),
                "starting" | "renewing" | "stopping"
            ) =>
            {
                "processing"
            }
            _ if self.recording => "recording",
            _ if !self.pending.is_empty() => "processing",
            Outcome::Canceled => "canceled",
            _ => "saved",
        }
    }

    fn status(&self) -> &'static str {
        match self.outcome {
            _ if !self.connected => "Disconnected",
            _ if self.capture_state == "error" => "Capture failed",
            _ if self.capture_state == "starting" => "Initializing microphone",
            _ if self.capture_state == "renewing" => "Renewing microphone",
            _ if self.capture_state == "stopping" => "Stopping microphone",
            Outcome::Clarify => "Clarification needed",
            Outcome::Response => match self
                .outcomes
                .get(&self.displayed_session)
                .and_then(|e| e.action.as_deref())
            {
                Some("search") => "Search results",
                Some("read") => "Note contents",
                Some("summarize") => "Summary",
                Some("cancel") => "Canceled",
                _ => "Response",
            },
            Outcome::Failed => "Failed",
            Outcome::Appended => "Appended to note",
            Outcome::Saved => "Note saved",
            Outcome::Canceled => "Canceled",
            _ if self.recording => "Listening",
            _ if !self.pending.is_empty() => "Processing",
            _ if self.listening => "Listening",
            _ if self.capture_state == "canceled" => "Canceled",
            _ => "Idle",
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
        let mut retry = RETRY_DELAY;
        loop {
            match UnixStream::connect(&path) {
                Ok(stream) => {
                    if sender.send_blocking(VoiceEvent::Connected(true)).is_err() {
                        return;
                    }
                    let started = Instant::now();
                    let mut reader = BufReader::new(stream);
                    let mut first = true;
                    loop {
                        match read_event(&mut reader) {
                            Ok(event) => {
                                if first && event.kind != "state" {
                                    break;
                                }
                                first = false;
                                if sender.send_blocking(VoiceEvent::Event(event)).is_err() {
                                    return;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    if started.elapsed() >= MAX_RETRY_DELAY {
                        retry = RETRY_DELAY;
                    }
                    if sender.send_blocking(VoiceEvent::Connected(false)).is_err() {
                        return;
                    }
                }
                Err(error) => debug!("ginkgo voice socket unavailable: {error}"),
            }
            thread::sleep(retry);
            retry = (retry * 2).min(MAX_RETRY_DELAY);
        }
    })
}

fn read_event(reader: &mut impl BufRead) -> Result<WireEvent, String> {
    // Bound allocations, require a complete NDJSON frame. Never write or
    // half-close this read-only subscription.
    let mut line = String::new();
    reader
        .take(MAX_MESSAGE_BYTES)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if !line.ends_with('\n') {
        return Err("incomplete or oversized voice event".into());
    }
    decode_event(&line)
}

fn decode_event(line: &str) -> Result<WireEvent, String> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    // serde's Option accepts null, but the protocol only permits omission.
    for key in [
        "capture_state",
        "stream_id",
        "capture_epoch_id",
        "namespace",
        "mode",
        "capture_error",
        "utterance_id",
        "revision",
        "action",
        "note_id",
        "note_ids",
        "processing",
        "fallback_reason",
        "image_status",
        "level",
        "speech",
        "error",
        "note_session_id",
        "audio_start_ms",
        "audio_end_ms",
    ] {
        if value.get(key).is_some_and(|v| v.is_null()) {
            return Err("null optional field".into());
        }
    }
    for key in ["audio_start_ms", "audio_end_ms"] {
        if value.get(key).is_some_and(|v| v.as_u64().is_none()) {
            return Err("invalid audio offset".into());
        }
    }
    if value.get("speech").is_some_and(|v| !v.is_boolean()) {
        return Err("invalid speech activity".into());
    }
    let event: WireEvent = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if event.schema_version != 1 || event.epoch_id.is_empty() || event.observed_at.is_empty() {
        return Err("invalid envelope".into());
    }
    if event.kind == "transcript"
        && (event.utterance_id.as_ref().is_none_or(String::is_empty)
            || event.revision.unwrap_or(0) == 0)
    {
        return Err("invalid transcript".into());
    }
    Ok(event)
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
    let context = gtk::Label::new(None);
    context.set_max_width_chars(36);
    context.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let transcript = gtk::Label::new(None);
    transcript.add_css_class("voice-transcript");
    transcript.set_max_width_chars(36);
    transcript.set_wrap(true);
    transcript.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    let details = gtk::Label::new(None);
    details.set_wrap(true);
    details.set_max_width_chars(36);
    let xalign = if left { 0.0 } else { 1.0 };
    status.set_xalign(xalign);
    context.set_xalign(xalign);
    transcript.set_xalign(xalign);
    details.set_xalign(xalign);
    text.append(&status);
    text.append(&context);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.append(&transcript);
    body.append(&details);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_max_content_height(360);
    scroll.set_propagate_natural_height(true);
    scroll.set_child(Some(&body));
    text.append(&scroll);

    if left {
        surface.append(&orb);
        surface.append(&text);
    } else {
        text.add_css_class("trailing");
        surface.append(&text);
        surface.append(&orb);
    }
    window.set_child(Some(&surface));

    // Conversation responses can be long; allow scrolling while they are shown.
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
            let stream_context = [s.namespace.as_deref(), s.mode.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            context.set_label(&stream_context);
            context.set_visible(!stream_context.is_empty());
            let body = s.capture_error.as_deref().unwrap_or(&s.text);
            transcript.set_label(body);
            transcript.set_visible(!body.is_empty());
            details.set_label(&s.details);
            details.set_visible(!s.details.is_empty());
            if let Some(surface) = presence.window.surface() {
                if matches!(s.outcome, Outcome::Response | Outcome::Clarify) {
                    surface.set_input_region(None);
                } else {
                    surface.set_input_region(Some(&gtk::cairo::Region::create()));
                }
            }
            presence.set_shown(s.active() || s.lingering);
            orb.queue_draw();
        })
    };

    glib::MainContext::default().spawn_local(async move {
        let linger_generation = Rc::new(Cell::new(0_u64));
        while let Ok(event) = receiver.recv().await {
            let was_active = state.borrow().active();
            let outcome = matches!(&event, VoiceEvent::Event(e)
                if matches!(e.kind.as_str(), "note.saved" | "note.canceled" | "note.error" | "error"));
            {
                let mut s = state.borrow_mut();
                match event {
                    VoiceEvent::Connected(connected) => {
                        s.connected = connected;
                        s.awaiting_snapshot = connected;
                        if !connected {
                            s.recording = false;
                            s.pending.clear();
                            s.lingering = false;
                        }
                    }
                    VoiceEvent::Event(event) => apply(&mut s, event),
                }
            }
            let settled = state.borrow().connected && was_active && !state.borrow().active();
            if settled || outcome {
                state.borrow_mut().lingering = true;
                let generation = linger_generation.get().wrapping_add(1);
                linger_generation.set(generation);
                let linger_generation = linger_generation.clone();
                let state = state.clone();
                let refresh = refresh.clone();
                glib::timeout_add_local_once(LINGER, move || {
                    {
                        let mut s = state.borrow_mut();
                        if s.active() || linger_generation.get() != generation {
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
    let snapshot = s.awaiting_snapshot;
    let epoch_changed = s.epoch_id != event.epoch_id;
    if !snapshot && !epoch_changed && s.seq.is_some_and(|seq| event.seq <= seq) {
        return;
    }
    let stream_changed = s.stream_id != event.stream_id;
    if snapshot || epoch_changed || stream_changed {
        // A snapshot has no history and cannot settle missed outcomes. Discard
        // stale pending work rather than leaving a spinner running forever.
        *s = VoiceState {
            connected: s.connected,
            ..VoiceState::default()
        };
    }
    s.awaiting_snapshot = false;
    s.epoch_id = event.epoch_id.clone();
    s.seq = Some(event.seq);
    if s.capture_epoch_id != event.capture_epoch_id {
        // Only the audio meter resets on renewal; conversation identity survives.
        s.level = 0.0;
        s.display_level = 0.0;
    }
    s.capture_state = event.capture_state.clone().unwrap_or_default();
    s.capture_epoch_id = event.capture_epoch_id.clone();
    s.stream_id = event.stream_id.clone();
    s.namespace = event.namespace.clone();
    s.mode = event.mode.clone();
    s.capture_error = event.capture_error.clone();
    s.listening = event.listening;
    if event.recording_note && s.active_session != event.active_note_session_id {
        s.text.clear();
        s.details.clear();
        s.outcome = Outcome::None;
        s.lingering = false;
        s.displayed_session = event.active_note_session_id.clone();
    }
    s.recording = event.recording_note;
    s.active_session = event.active_note_session_id.clone();
    if !event.active_note_session_id.is_empty() {
        s.pending.insert(event.active_note_session_id.clone());
    }
    match event.kind.as_str() {
        "level" => {
            if let Some(level) = event
                .level
                .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
            {
                s.level = s.level.max(level);
            }
        }
        "transcript" => {
            let Some(id) = event.utterance_id.as_ref() else {
                return;
            };
            if event.revision.unwrap_or(0) == 0
                || s.transcripts
                    .get(id)
                    .is_some_and(|old| old.final_text || old.revision >= event.revision)
            {
                return;
            }
            // Never infer attribution from the currently recording session.
            if !event.note_session_id.is_empty() && !s.outcomes.contains_key(&event.note_session_id)
            {
                s.outcome = Outcome::None;
                s.text = event.text.clone();
                s.displayed_session = event.note_session_id.clone();
                s.details = format!("Session: {}", event.note_session_id);
            }
            s.transcripts.insert(id.clone(), event);
        }
        "note.saved" | "conversation.response" | "note.canceled" | "note.error" => {
            s.pending.remove(&event.note_session_id);
            s.outcome = match event.kind.as_str() {
                "note.saved" if event.action.as_deref() == Some("append") => Outcome::Appended,
                "note.saved" => Outcome::Saved,
                "conversation.response" if event.action.as_deref() == Some("clarify") => {
                    Outcome::Clarify
                }
                "conversation.response" => Outcome::Response,
                "note.canceled" => Outcome::Canceled,
                _ => Outcome::Failed,
            };
            s.displayed_session = event.note_session_id.clone();
            s.text = event.error.clone().unwrap_or_else(|| event.text.clone());
            let mut details = vec![format!("Session: {}", event.note_session_id)];
            if let Some(id) = &event.note_id {
                details.push(format!("Note: {id}"));
            }
            if let Some(ids) = &event.note_ids {
                details.extend(
                    ids.iter()
                        .enumerate()
                        .map(|(i, id)| format!("{}. {id}", i + 1)),
                );
            }
            for value in [
                &event.processing,
                &event.fallback_reason,
                &event.image_status,
            ] {
                if let Some(value) = value {
                    details.push(value.clone());
                }
            }
            s.details = details.join("\n");
            s.lingering = true;
            s.outcomes.insert(event.note_session_id.clone(), event);
        }
        "error" => {
            s.pending.remove(&event.note_session_id);
            s.outcome = Outcome::Failed;
            s.text = event.error.unwrap_or_default();
            s.displayed_session = event.note_session_id;
            s.details = if s.displayed_session.is_empty() {
                String::new()
            } else {
                format!("Session: {}", s.displayed_session)
            };
            s.lingering = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn event(seq: u64, fields: Value) -> WireEvent {
        let mut base = json!({
            "type": "state", "schema_version": 1, "epoch_id": "daemon/opaque",
            "seq": seq, "observed_at": "2026-10-22T12:00:00Z", "listening": false,
            "recording_note": false, "active_note_session_id": "", "text": "", "final": false
        });
        base.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        decode_event(&base.to_string()).unwrap()
    }

    fn state() -> VoiceState {
        VoiceState {
            connected: true,
            awaiting_snapshot: true,
            ..Default::default()
        }
    }

    #[test]
    fn socket_frames_are_bounded_and_subscription_sends_no_bytes() {
        use std::io::{Cursor, ErrorKind, Write};

        let line = concat!(
            "{\"type\":\"state\",\"schema_version\":1,\"epoch_id\":\"e\",\"seq\":0,",
            "\"observed_at\":\"2026-10-22T12:00:00Z\",\"listening\":false,",
            "\"recording_note\":false,\"active_note_session_id\":\"\",\"text\":\"\",\"final\":false}\n"
        );
        let (client, mut server) = UnixStream::pair().unwrap();
        server.write_all(line.as_bytes()).unwrap();
        server.write_all(line.as_bytes()).unwrap();
        let mut reader = BufReader::new(client);
        assert_eq!(read_event(&mut reader).unwrap().kind, "state");
        assert_eq!(read_event(&mut reader).unwrap().seq, 0);
        server.set_nonblocking(true).unwrap();
        // WouldBlock, rather than EOF, also verifies there was no write-half close.
        assert_eq!(
            server.read(&mut [0; 1]).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        assert!(read_event(&mut Cursor::new(line.trim_end())).is_err());
        assert!(read_event(&mut Cursor::new(vec![b' '; MAX_MESSAGE_BYTES as usize + 1])).is_err());
        assert!(read_event(&mut Cursor::new(b"\xff\n")).is_err());
    }

    #[test]
    fn revisions_replace_and_delayed_results_keep_their_session() {
        let mut s = state();
        apply(&mut s, event(0, json!({})));
        for (seq, revision, text, final_text) in [
            (1, 1, "first", false),
            (2, 2, "replacement", false),
            (3, 1, "stale", false),
            (4, 3, "final", true),
            (5, 4, "too late", true),
        ] {
            apply(
                &mut s,
                event(
                    seq,
                    json!({"type": "transcript", "utterance_id": "opaque/u",
                "revision": revision, "text": text, "final": final_text, "note_session_id": "old"}),
                ),
            );
        }
        assert_eq!(s.text, "final");
        assert_eq!(s.transcripts.len(), 1);
        apply(
            &mut s,
            event(
                6,
                json!({"recording_note": true, "active_note_session_id": "new"}),
            ),
        );
        apply(
            &mut s,
            event(
                7,
                json!({"type": "note.saved", "note_session_id": "old", "note_id": "n",
            "recording_note": true, "active_note_session_id": "new", "text": "Created it"}),
            ),
        );
        assert_eq!(s.displayed_session, "old");
        assert!(s.pending.contains("new"));
        assert!(!s.pending.contains("old"));
        assert_eq!(s.outcomes["old"].note_id.as_deref(), Some("n"));
        assert_eq!(s.status(), "Note saved");
        apply(
            &mut s,
            event(
                8,
                json!({"type": "transcript", "utterance_id": "ambient", "revision": 1,
            "text": "ambient speech", "recording_note": true, "active_note_session_id": "new"}),
            ),
        );
        assert_eq!(s.text, "Created it");
    }

    #[test]
    fn renewal_preserves_conversation_and_resets_only_audio_meter() {
        let mut s = state();
        apply(
            &mut s,
            event(
                0,
                json!({"capture_state": "starting", "stream_id": "stream",
            "capture_epoch_id": "clock-a", "namespace": "work", "mode": "ai"}),
            ),
        );
        assert!(s.active());
        assert_eq!(s.status(), "Initializing microphone");
        apply(
            &mut s,
            event(
                1,
                json!({"capture_state": "streaming", "stream_id": "stream",
            "capture_epoch_id": "clock-a", "listening": true, "recording_note": true,
            "active_note_session_id": "session"}),
            ),
        );
        apply(
            &mut s,
            event(
                2,
                json!({"type": "transcript", "utterance_id": "u", "revision": 1,
            "text": "question", "note_session_id": "session", "capture_state": "streaming",
            "stream_id": "stream", "capture_epoch_id": "clock-a", "audio_start_ms": 10000}),
            ),
        );
        apply(
            &mut s,
            event(
                3,
                json!({"capture_state": "renewing", "stream_id": "stream",
            "capture_epoch_id": "clock-a"}),
            ),
        );
        assert!(s.connected);
        assert_eq!(s.status(), "Renewing microphone");
        assert!(s.pending.contains("session"));
        s.level = 0.8;
        apply(
            &mut s,
            event(
                4,
                json!({"capture_state": "streaming", "listening": true,
            "stream_id": "stream", "capture_epoch_id": "clock-b"}),
            ),
        );
        assert_eq!(s.level, 0.0);
        assert_eq!(s.text, "question");
        assert_eq!(s.transcripts.len(), 1);
        apply(
            &mut s,
            event(
                5,
                json!({"type": "transcript", "utterance_id": "u2", "revision": 1,
            "text": "after renewal", "note_session_id": "session", "capture_state": "streaming",
            "stream_id": "stream", "capture_epoch_id": "clock-b", "audio_start_ms": 0}),
            ),
        );
        assert_eq!(s.text, "after renewal");
        assert_eq!(s.transcripts.len(), 2);
    }

    #[test]
    fn conversational_outcomes_are_not_transcripts() {
        let mut s = state();
        apply(&mut s, event(0, json!({})));
        for (i, action, label) in [
            (1, "search", "Search results"),
            (2, "clarify", "Clarification needed"),
            (3, "summarize", "Summary"),
            (4, "read", "Note contents"),
            (5, "cancel", "Canceled"),
        ] {
            apply(
                &mut s,
                event(
                    i,
                    json!({"type": "conversation.response", "note_session_id": "s",
                "text": "Response or question", "action": action, "note_ids": ["z", "a"]}),
                ),
            );
            assert_eq!(s.status(), label);
            assert!(s.active());
            assert!(s.transcripts.is_empty());
            assert_eq!(s.outcomes["s"].note_ids.as_ref().unwrap(), &["z", "a"]);
            assert!(s.details.contains("1. z\n2. a"));
        }
    }

    #[test]
    fn writes_preserve_acknowledgment_and_processing_metadata() {
        let mut s = state();
        for (i, action, label) in [
            (0, "create", "Note saved"),
            (1, "append", "Appended to note"),
        ] {
            apply(
                &mut s,
                event(
                    i,
                    json!({"type": "note.saved", "note_session_id": "s", "note_id": "n",
                "action": action, "text": "Done", "processing": "raw_fallback",
                "fallback_reason": "unavailable", "image_status": "retained"}),
                ),
            );
            assert_eq!(s.status(), label);
            assert_eq!(s.text, "Done");
            assert!(s.details.contains("raw_fallback\nunavailable\nretained"));
            assert!(s.transcripts.is_empty());
        }
    }

    #[test]
    fn reconnect_snapshot_replaces_state_even_at_same_sequence() {
        let mut s = state();
        apply(
            &mut s,
            event(
                10,
                json!({"capture_state": "streaming", "stream_id": "old",
            "recording_note": true, "active_note_session_id": "lost"}),
            ),
        );
        s.text = "stale partial".into();
        s.awaiting_snapshot = true;
        apply(
            &mut s,
            event(10, json!({"capture_state": "renewing", "stream_id": "old"})),
        );
        assert!(s.pending.is_empty());
        assert!(!s.recording);
        assert!(s.text.is_empty());
        assert_eq!(s.status(), "Renewing microphone");
        apply(&mut s, event(9, json!({"capture_state": "error"})));
        assert_eq!(s.status(), "Renewing microphone");
        apply(
            &mut s,
            event(
                0,
                json!({"epoch_id": "new daemon", "capture_state": "starting"}),
            ),
        );
        assert_eq!(s.seq, Some(0));
        assert_eq!(s.status(), "Initializing microphone");
        apply(
            &mut s,
            event(
                1,
                json!({"epoch_id": "new daemon", "type": "transcript",
            "utterance_id": "reused", "revision": 1, "note_session_id": "s", "text": "fresh"}),
            ),
        );
        assert_eq!(s.text, "fresh");
    }

    #[test]
    fn stream_change_resets_local_identity_but_new_outcomes_still_arrive() {
        let mut s = state();
        apply(
            &mut s,
            event(
                0,
                json!({"stream_id": "one", "recording_note": true,
            "active_note_session_id": "old"}),
            ),
        );
        apply(
            &mut s,
            event(1, json!({"stream_id": "two", "capture_state": "starting"})),
        );
        assert!(s.pending.is_empty());
        apply(
            &mut s,
            event(
                2,
                json!({"stream_id": "two", "type": "note.saved",
            "note_session_id": "old", "note_id": "n"}),
            ),
        );
        assert_eq!(s.displayed_session, "old");
        assert_eq!(s.outcome, Outcome::Saved);
    }

    #[test]
    fn legacy_optional_fields_and_additive_events() {
        let mut s = state();
        apply(&mut s, event(0, json!({})));
        assert_eq!(s.status(), "Idle");
        apply(
            &mut s,
            event(
                1,
                json!({"type": "future.event", "future": {"anything": null}, "listening": true}),
            ),
        );
        assert_eq!(s.status(), "Listening");
        apply(
            &mut s,
            event(
                2,
                json!({"type": "conversation.response", "note_session_id": "s", "text": "Legacy response"}),
            ),
        );
        assert_eq!(s.status(), "Response");
        apply(
            &mut s,
            event(
                3,
                json!({"type": "note.saved", "note_session_id": "s", "note_id": "n"}),
            ),
        );
        assert_eq!(s.status(), "Note saved");
        let base = json!({"type":"state", "schema_version":1, "epoch_id":"e", "seq":0,
            "observed_at":"2026-10-22T12:00:00Z", "listening":false, "recording_note":false,
            "active_note_session_id":"", "text":"", "final":false});
        let mut invalid = base.clone();
        invalid["capture_state"] = Value::Null;
        assert!(decode_event(&invalid.to_string()).is_err());
        invalid = base;
        invalid.as_object_mut().unwrap().remove("listening");
        assert!(decode_event(&invalid.to_string()).is_err());
    }

    #[test]
    fn errors_and_cancellation_are_attributed_without_clearing_other_work() {
        let mut s = state();
        apply(
            &mut s,
            event(
                0,
                json!({"recording_note": true, "active_note_session_id": "new"}),
            ),
        );
        for (i, kind, expected) in [
            (1, "note.error", Outcome::Failed),
            (2, "note.canceled", Outcome::Canceled),
            (3, "error", Outcome::Failed),
        ] {
            apply(
                &mut s,
                event(
                    i,
                    json!({"type": kind, "note_session_id": "old", "error": "problem"}),
                ),
            );
            assert_eq!(s.outcome, expected);
            assert_eq!(s.displayed_session, "old");
            assert!(s.pending.contains("new"));
        }
        apply(
            &mut s,
            event(
                4,
                json!({"capture_state": "error", "capture_error": "Microphone unavailable"}),
            ),
        );
        assert_eq!(s.status(), "Capture failed");
        assert!(s.active());
    }
}
