//! The lock screen surface.
//!
//! A real compositor-enforced lock through `gtk4-session-lock`, the
//! sibling crate to `gtk4-layer-shell` wrapping the same C library and
//! speaking `ext-session-lock-v1`. The protocol blanks every other surface
//! (including the island) while locked, and keeps the session locked if
//! this process dies -- crash-safety is the compositor's job, not ours.
//! See <https://wayland.app/protocols/ext-session-lock-v1>.
//!
//! One card-bearing window per output, each on a frozen, blurred capture
//! of that output's last frame, reusing the island's surface styles and
//! `@ms_*` palette roles.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    thread,
};

use gtk::{
    Align, Application, ApplicationWindow, Orientation, Overflow,
    gdk::{self, prelude::*},
    glib,
    prelude::*,
};
use gtk4_session_lock::Instance as SessionLockInstance;
use log::{debug, info, warn};

use super::{automatic_scale, resolved_scale, scale_class, scaled};
use crate::{
    config::LockConfig,
    lock::{Outcome, backdrop},
};

const CARD_WIDTH: i32 = 360;
/// Design-time gap between the card's rows.
const CARD_SPACING: i32 = 12;

/// Invoked when the user submits a password. Arguments are the attempt
/// generation and the secret; the caller forwards both to the PAM worker.
pub type LockSubmitAction = Rc<dyn Fn(u64, String)>;
/// Invoked once the [`LockSession`] is no longer authoritative -- either the
/// compositor refused the lock, or the session has been unlocked. The
/// receiver should drop its `Rc<LockSession>`.
pub type LockEndedAction = Rc<dyn Fn()>;

/// One output's lock surface. Built fresh for every `monitor` signal and
/// destroyed by the library itself when the output disappears or the lock
/// ends -- this struct only holds the widget handles needed to update it.
struct LockWindow {
    window: glib::WeakRef<ApplicationWindow>,
    backdrop: glib::WeakRef<gtk::Picture>,
    entry: glib::WeakRef<gtk::PasswordEntry>,
    status: glib::WeakRef<gtk::Label>,
    clock: glib::WeakRef<gtk::Label>,
    date: glib::WeakRef<gtk::Label>,
    caps: glib::WeakRef<gtk::Label>,
    connector: String,
}

impl LockWindow {
    /// `assign_window_to_monitor` gives this surface its role; `present()`
    /// is an error per the library's docs.
    fn new(application: &Application, scale: f64, connector: String) -> (Self, ApplicationWindow) {
        let window = ApplicationWindow::builder()
            .application(application)
            .title("mithshell lock")
            .decorated(false)
            .build();
        window.add_css_class("mithshell-lock");
        if let Some(class) = scale_class(scale) {
            window.add_css_class(class);
        }

        // The picture is the frozen screenshot. Until the blur lands it is
        // empty, and the opaque black window background shows through --
        // never the live desktop.
        let backdrop = gtk::Picture::new();
        backdrop.set_content_fit(gtk::ContentFit::Cover);
        backdrop.set_can_shrink(true);
        backdrop.add_css_class("lock-backdrop");
        backdrop.set_opacity(0.0);
        backdrop.set_hexpand(true);
        backdrop.set_vexpand(true);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&backdrop));

        let card = gtk::Box::new(Orientation::Vertical, scaled(CARD_SPACING, scale));
        card.add_css_class("island-surface");
        card.add_css_class("lock-card");
        card.set_overflow(Overflow::Hidden);
        card.set_halign(Align::Center);
        card.set_valign(Align::Center);
        card.set_size_request(scaled(CARD_WIDTH, scale), -1);

        let eyebrow = gtk::Label::new(Some("LOCKED"));
        eyebrow.add_css_class("eyebrow");
        eyebrow.set_halign(Align::Center);

        let clock = gtk::Label::new(None);
        clock.add_css_class("hero-time");
        clock.set_halign(Align::Center);

        let date = gtk::Label::new(None);
        date.add_css_class("hero-date");
        date.set_halign(Align::Center);
        let (time_text, date_text) = clock_text();
        clock.set_label(&time_text);
        date.set_label(&date_text);

        let user = gtk::Label::new(Some(&crate::lock::current_user()));
        user.add_css_class("lock-user");
        user.set_halign(Align::Center);
        user.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let entry = gtk::PasswordEntry::new();
        entry.add_css_class("lock-entry");
        entry.set_show_peek_icon(true);
        entry.set_activates_default(false);
        entry.set_placeholder_text(Some("Password or TOTP; Enter for YubiKey"));
        entry.set_hexpand(true);

        let caps = gtk::Label::new(Some("CAPS LOCK"));
        caps.add_css_class("lock-caps");
        caps.set_halign(Align::Center);
        caps.set_visible(false);

        let status = gtk::Label::new(None);
        status.add_css_class("lock-status");
        status.set_halign(Align::Center);
        status.set_wrap(true);
        status.set_justify(gtk::Justification::Center);
        // Reserve the row so a message appearing does not resize the card.
        status.set_size_request(-1, scaled(18, scale));

        for child in [
            eyebrow.upcast_ref::<gtk::Widget>(),
            clock.upcast_ref(),
            date.upcast_ref(),
            user.upcast_ref(),
            entry.upcast_ref(),
            caps.upcast_ref(),
            status.upcast_ref(),
        ] {
            card.append(child);
        }
        overlay.add_overlay(&card);
        window.set_child(Some(&overlay));

        (
            Self {
                window: window.downgrade(),
                backdrop: backdrop.downgrade(),
                entry: entry.downgrade(),
                status: status.downgrade(),
                clock: clock.downgrade(),
                date: date.downgrade(),
                caps: caps.downgrade(),
                connector,
            },
            window,
        )
    }

    fn set_backdrop(&self, texture: &gdk::Texture) {
        if let Some(backdrop) = self.backdrop.upgrade() {
            backdrop.set_paintable(Some(texture));
            backdrop.set_opacity(1.0);
        }
    }

    fn set_status(&self, message: &str, error: bool) {
        let Some(status) = self.status.upgrade() else {
            return;
        };
        status.set_label(message);
        if error {
            status.add_css_class("error");
        } else {
            status.remove_css_class("error");
        }
    }
}

/// A live lock. Dropping it does not unlock the session -- only a
/// successful PAM attempt ([`resolve`][Self::resolve] with
/// [`Outcome::Granted`]) or [`force_unlock`][Self::force_unlock] does, both
/// of which go through [`gtk4_session_lock::Instance::unlock`]. This is
/// deliberate: it is what makes the lock survive this process crashing.
pub struct LockSession {
    instance: SessionLockInstance,
    windows: RefCell<HashMap<String, Rc<LockWindow>>>,
    application: Application,
    config: LockConfig,
    scale: f64,
    submit: LockSubmitAction,
    on_ended: LockEndedAction,
    /// Incremented per attempt so a verdict that arrives after the user has
    /// already typed something else is ignored.
    generation: Cell<u64>,
    /// Set while PAM is deliberating; blocks further submissions.
    busy: Cell<bool>,
    attempts: Cell<u32>,
    /// Guards the entry mirroring so propagating text to the other outputs
    /// does not re-enter through their `changed` handlers.
    syncing: Cell<bool>,
    /// Set by the `failed` signal, which can fire synchronously inside
    /// `lock()`; the caller checks this because `on_ended` may have already
    /// fired before the session was stored anywhere.
    failed: Cell<bool>,
    /// Prevents the signal from finishing an explicit unlock too early.
    unlocking: Cell<bool>,
    /// Captured before `lock()` so monitor callbacks can map immediately.
    captures: RefCell<HashMap<String, backdrop::Image>>,
    backdrops: RefCell<Option<async_channel::Sender<Backdrop>>>,
    clock_source: RefCell<Option<glib::SourceId>>,
    caps_handler: RefCell<Option<(gdk::Device, glib::SignalHandlerId)>>,
}

/// A processed backdrop on its way back from a blur worker.
struct Backdrop {
    connector: String,
    image: backdrop::Image,
}

impl LockSession {
    /// Requests a session lock. `on_ended` fires from the `failed`/`unlocked`
    /// signals -- the caller's cue to drop its `Rc<LockSession>`; check
    /// [`has_failed`][Self::has_failed] first, since `failed` can fire
    /// synchronously inside `lock()`.
    pub fn new(
        application: &Application,
        config: &LockConfig,
        configured_scale: f64,
        submit: LockSubmitAction,
        on_ended: LockEndedAction,
    ) -> Rc<Self> {
        let session = Rc::new(Self {
            instance: SessionLockInstance::new(),
            windows: RefCell::new(HashMap::new()),
            application: application.clone(),
            config: config.clone(),
            scale: configured_scale,
            submit,
            on_ended: on_ended.clone(),
            generation: Cell::new(0),
            busy: Cell::new(false),
            attempts: Cell::new(0),
            syncing: Cell::new(false),
            failed: Cell::new(false),
            unlocking: Cell::new(false),
            captures: RefCell::new(HashMap::new()),
            backdrops: RefCell::new(None),
            clock_source: RefCell::new(None),
            caps_handler: RefCell::new(None),
        });

        let (sender, receiver) = async_channel::unbounded::<Backdrop>();
        *session.backdrops.borrow_mut() = Some(sender);
        session.attach_backdrops(receiver);

        let weak = Rc::downgrade(&session);
        session.instance.connect_failed(move |_| {
            warn!(
                "the compositor refused to lock the session (another lock holder, or ext-session-lock-v1 is unsupported)"
            );
            if let Some(session) = weak.upgrade() {
                session.failed.set(true);
                let ended = session.on_ended.clone();
                glib::idle_add_local_once(move || ended());
            }
        });

        let weak = Rc::downgrade(&session);
        session.instance.connect_locked(move |_| {
            info!("session locked");
            if let Some(session) = weak.upgrade() {
                session.focus_active_entry();
            }
        });

        let weak = Rc::downgrade(&session);
        session.instance.connect_unlocked(move |_| {
            info!("session unlocked");
            if let Some(session) = weak.upgrade()
                && !session.unlocking.get()
            {
                let ended = session.on_ended.clone();
                glib::idle_add_local_once(move || ended());
            }
        });

        let weak = Rc::downgrade(&session);
        session.instance.connect_monitor(move |instance, monitor| {
            if let Some(session) = weak.upgrade() {
                session.add_monitor(instance, monitor);
            }
        });

        session.capture_existing_monitors();

        if !session.instance.lock() {
            // `failed` has already fired synchronously, or the instance was
            // already locked -- impossible for a fresh instance.
            warn!("gtk_session_lock_instance_lock did not start");
        }

        session.start_clock();
        session.watch_caps_lock();
        session
    }

    /// True if the compositor has already refused this lock. The caller
    /// must not store a session for which this is true; drop it instead.
    pub fn has_failed(&self) -> bool {
        self.failed.get()
    }

    /// Builds and immediately assigns a lock surface for one output.
    fn add_monitor(self: &Rc<Self>, instance: &SessionLockInstance, monitor: &gdk::Monitor) {
        let Some(connector) = monitor.connector().map(|value| value.to_string()) else {
            warn!("ignoring a monitor without a connector name");
            return;
        };
        if self.windows.borrow().contains_key(&connector) {
            // The library documents one `monitor` signal per output; stay
            // idempotent rather than trust that absolutely.
            return;
        }

        let scale = resolved_scale(self.scale, automatic_scale(monitor));
        let (window, gtk_window) = LockWindow::new(&self.application, scale, connector.clone());
        let window = Rc::new(window);
        self.connect_window(&window);

        let captured = self.captures.borrow_mut().remove(&connector);

        instance.assign_window_to_monitor(&gtk_window, monitor);
        if let Some(entry) = window.entry.upgrade() {
            entry.grab_focus();
        }
        self.windows.borrow_mut().insert(connector.clone(), window);

        let Some(image) = captured else {
            return;
        };
        let Some(sender) = self.backdrops.borrow().clone() else {
            return;
        };
        let settings = self.config.blur_settings();
        thread::spawn(move || {
            let processed = backdrop::process(&image, settings);
            let _ = sender.send_blocking(Backdrop {
                connector,
                image: processed,
            });
        });
    }

    /// Hotplugged outputs use the black fallback because capture is no longer safe.
    fn capture_existing_monitors(&self) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let monitors = display.monitors();
        for index in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            let Some(connector) = monitor.connector().map(|value| value.to_string()) else {
                continue;
            };
            match backdrop::capture(&connector) {
                Ok(image) => {
                    self.captures.borrow_mut().insert(connector, image);
                }
                Err(error) => warn!("no lock backdrop for {connector}: {error:#}"),
            }
        }
    }

    fn attach_backdrops(self: &Rc<Self>, receiver: async_channel::Receiver<Backdrop>) {
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Ok(ready) = receiver.recv().await {
                let Some(session) = weak.upgrade() else {
                    break;
                };
                let stride = ready.image.stride();
                let texture = gdk::MemoryTexture::new(
                    ready.image.width as i32,
                    ready.image.height as i32,
                    gdk::MemoryFormat::R8g8b8,
                    &glib::Bytes::from_owned(ready.image.pixels),
                    stride,
                );
                for window in session.windows.borrow().values() {
                    if window.connector == ready.connector {
                        window.set_backdrop(texture.upcast_ref());
                    }
                }
            }
        });
    }

    fn connect_window(self: &Rc<Self>, window: &Rc<LockWindow>) {
        let Some(entry) = window.entry.upgrade() else {
            return;
        };
        let Some(gtk_window) = window.window.upgrade() else {
            return;
        };

        let weak = Rc::downgrade(self);
        entry.connect_activate(move |entry| {
            if let Some(session) = weak.upgrade() {
                session.submit(&entry.text());
            }
        });

        let weak = Rc::downgrade(self);
        let connector = window.connector.clone();
        entry.connect_changed(move |entry| {
            if let Some(session) = weak.upgrade() {
                session.mirror(&connector, &entry.text());
            }
        });

        // Escape clears the buffer instead of dismissing anything.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            if let Some(session) = weak.upgrade() {
                session.clear();
            }
            glib::Propagation::Stop
        });
        gtk_window.add_controller(keys);

        // The compositor decides which lock surface has keyboard focus and
        // may move it as the pointer crosses outputs; make sure it always
        // lands on the entry rather than on the card.
        gtk_window.connect_is_active_notify(move |window| {
            if window.is_active() {
                entry.grab_focus();
            }
        });
    }

    /// Copies the password buffer to every output except the one it came from.
    fn mirror(&self, source: &str, text: &str) {
        if self.syncing.get() {
            return;
        }
        self.syncing.set(true);
        for window in self.windows.borrow().values() {
            if window.connector != source
                && let Some(entry) = window.entry.upgrade()
                && entry.text() != text
            {
                entry.set_text(text);
            }
        }
        self.syncing.set(false);
    }

    fn clear(&self) {
        self.syncing.set(true);
        for window in self.windows.borrow().values() {
            if let Some(entry) = window.entry.upgrade() {
                entry.set_text("");
            }
        }
        self.syncing.set(false);
    }

    fn submit(&self, password: &str) {
        if self.busy.get() {
            return;
        }
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.busy.set(true);
        // Disabling the entry is not just cosmetic: PAM sleeps for seconds
        // after a bad password, and letting the user queue more attempts in
        // the meantime would make the UI feel broken.
        for window in self.windows.borrow().values() {
            if let Some(entry) = window.entry.upgrade() {
                entry.set_sensitive(false);
            }
        }
        let status = if password.is_empty() {
            "Checking YubiKey…"
        } else {
            "Authenticating…"
        };
        self.broadcast_status(status, false);
        (self.submit)(generation, password.to_owned());
    }

    /// Applies a live progress message from the PAM worker (for example
    /// `pam_u2f`'s "Please touch the device"). Ignored if it belongs to an
    /// attempt the user has already abandoned.
    pub fn progress(&self, generation: u64, message: &str) {
        if generation != self.generation.get() || message.is_empty() {
            return;
        }
        self.broadcast_status(message, false);
    }

    /// Applies a final verdict from the PAM worker. On [`Outcome::Granted`]
    /// this calls [`Instance::unlock`][gtk4_session_lock::Instance::unlock],
    /// which is what actually ends the compositor-enforced lock; the
    /// `unlocked` signal handler installed in [`new`][Self::new] does the
    /// rest of the teardown.
    pub fn resolve(&self, generation: u64, outcome: &Outcome) {
        if generation != self.generation.get() {
            debug!("ignoring a stale authentication result");
            return;
        }
        self.busy.set(false);
        for window in self.windows.borrow().values() {
            if let Some(entry) = window.entry.upgrade() {
                entry.set_sensitive(true);
            }
        }
        match outcome {
            Outcome::Granted => {
                self.unlock_after_library_cleanup();
            }
            Outcome::Denied(message) => {
                let attempts = self.attempts.get().saturating_add(1);
                self.attempts.set(attempts);
                self.clear();
                let message = if attempts > 1 {
                    format!("{message} ({attempts} failed attempts)")
                } else {
                    message.clone()
                };
                self.broadcast_status(&message, true);
                self.focus_active_entry();
            }
            Outcome::Failed(message) => {
                warn!("lock screen authentication is broken: {message}");
                self.clear();
                self.broadcast_status(
                    "Authentication is unavailable; check the PAM configuration",
                    true,
                );
                self.focus_active_entry();
            }
        }
    }

    /// Unlocks without authenticating. `mithshell unlock` is a same-user,
    /// same-machine escape hatch: the IPC socket is restricted to this user
    /// by filesystem permissions and a peer-credential check.
    pub fn force_unlock(&self) {
        self.unlock_after_library_cleanup();
    }

    fn unlock_after_library_cleanup(&self) {
        self.unlocking.set(true);
        self.instance.unlock();
        self.unlocking.set(false);
        // The C library has finished destroying its assigned windows.
        (self.on_ended)();
    }

    fn focus_active_entry(&self) {
        for window in self.windows.borrow().values() {
            if let (Some(gtk_window), Some(entry)) =
                (window.window.upgrade(), window.entry.upgrade())
                && gtk_window.is_active()
            {
                entry.grab_focus();
                return;
            }
        }
        // No output reported itself active yet (can happen right after
        // `locked` fires); focus the first one so typing is never lost.
        if let Some(window) = self.windows.borrow().values().next()
            && let Some(entry) = window.entry.upgrade()
        {
            entry.grab_focus();
        }
    }

    fn broadcast_status(&self, message: &str, error: bool) {
        for window in self.windows.borrow().values() {
            window.set_status(message, error);
        }
    }

    fn start_clock(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_seconds_local(1, move || {
            let Some(session) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            session.tick_clock();
            glib::ControlFlow::Continue
        });
        *self.clock_source.borrow_mut() = Some(source);
    }

    fn tick_clock(&self) {
        let (time, date) = clock_text();
        for window in self.windows.borrow().values() {
            if let Some(clock) = window.clock.upgrade() {
                clock.set_label(&time);
            }
            if let Some(date_label) = window.date.upgrade() {
                date_label.set_label(&date);
            }
        }
    }

    /// Shows a warning while Caps Lock is on, which is the single most
    /// common reason a correct password is rejected.
    fn watch_caps_lock(self: &Rc<Self>) {
        let Some(keyboard) = gdk::Display::default()
            .and_then(|display| display.default_seat())
            .and_then(|seat| seat.keyboard())
        else {
            return;
        };
        self.apply_caps_lock(keyboard.is_caps_locked());
        let weak = Rc::downgrade(self);
        let handler = keyboard.connect_caps_lock_state_notify(move |device| {
            if let Some(session) = weak.upgrade() {
                session.apply_caps_lock(device.is_caps_locked());
            }
        });
        *self.caps_handler.borrow_mut() = Some((keyboard, handler));
    }

    fn apply_caps_lock(&self, active: bool) {
        for window in self.windows.borrow().values() {
            if let Some(caps) = window.caps.upgrade() {
                caps.set_visible(active);
            }
        }
    }

    pub fn debug_state(&self) -> serde_json::Value {
        serde_json::json!({
            "outputs": self
                .windows
                .borrow()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            "attempts": self.attempts.get(),
            "authenticating": self.busy.get(),
            "compositor_locked": self.instance.is_locked(),
        })
    }

    /// Drops our references and stops our own timers; the library destroys
    /// the lock surfaces itself. Idempotent (`failed`/`unlocked` + `Drop`).
    fn teardown(&self) {
        if let Some((keyboard, handler)) = self.caps_handler.borrow_mut().take() {
            keyboard.disconnect(handler);
        }
        if let Some(source) = self.clock_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(sender) = self.backdrops.borrow_mut().take() {
            sender.close();
        }
        self.windows.borrow_mut().clear();
    }
}

fn clock_text() -> (String, String) {
    let now = glib::DateTime::now_local().ok();
    let time = now
        .as_ref()
        .and_then(|now| now.format("%H:%M").ok())
        .map(|text| text.to_string())
        .unwrap_or_default();
    let date = now
        .as_ref()
        .and_then(|now| now.format("%A, %e %B").ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    (time, date)
}

impl Drop for LockSession {
    fn drop(&mut self) {
        self.teardown();
    }
}
