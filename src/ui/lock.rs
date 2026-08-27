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

use super::icon::Icon;
use super::{automatic_scale, resolved_scale, scale_class, scaled};
use crate::{
    config::{IconStyle, LockConfig},
    lock::{Outcome, backdrop},
    state::{SystemInfoState, SystemSnapshot, WeatherState},
    system::PowerAction,
};

const CARD_WIDTH: i32 = 430;
/// Design-time gap between the card's rows.
const CARD_SPACING: i32 = 12;

/// Invoked when the user submits a password. Arguments are the attempt
/// generation and the secret; the caller forwards both to the PAM worker.
pub type LockSubmitAction = Rc<dyn Fn(u64, String)>;
/// Invoked once the [`LockSession`] is no longer authoritative -- either the
/// compositor refused the lock, or the session has been unlocked. The
/// receiver should drop its `Rc<LockSession>`.
pub type LockEndedAction = Rc<dyn Fn()>;
/// Runs a logind power request away from the GTK thread and reports its result.
pub type LockPowerAction = Rc<dyn Fn(PowerAction, async_channel::Sender<Result<(), String>>)>;
/// Reports actual compositor lock acquisition/release for logind metadata.
pub type LockStateAction = Rc<dyn Fn(bool)>;

pub struct LockActions {
    pub submit: LockSubmitAction,
    pub power: LockPowerAction,
    pub ended: LockEndedAction,
    pub state_changed: LockStateAction,
}

#[derive(Clone, Copy)]
pub struct LockAnimation {
    pub enabled: bool,
    pub duration_ms: u32,
}

impl LockAnimation {
    fn duration(self) -> Option<u32> {
        (self.enabled && self.duration_ms > 0).then_some(self.duration_ms)
    }
}

/// One output's lock surface. Built fresh for every `monitor` signal and
/// destroyed by the library itself when the output disappears or the lock
/// ends -- this struct only holds the widget handles needed to update it.
struct LockWindow {
    window: glib::WeakRef<ApplicationWindow>,
    backdrop: glib::WeakRef<gtk::Picture>,
    content: glib::WeakRef<gtk::Box>,
    entry: glib::WeakRef<gtk::PasswordEntry>,
    status: glib::WeakRef<gtk::Label>,
    clock: glib::WeakRef<gtk::Label>,
    date: glib::WeakRef<gtk::Label>,
    caps: glib::WeakRef<gtk::Label>,
    system_info: glib::WeakRef<gtk::Label>,
    battery: glib::WeakRef<gtk::Label>,
    weather: glib::WeakRef<gtk::Label>,
    power_buttons: Vec<(PowerAction, glib::WeakRef<gtk::Button>)>,
    content_animation: Rc<Cell<u64>>,
    backdrop_animation: Rc<Cell<u64>>,
    connector: String,
}

impl LockWindow {
    /// `assign_window_to_monitor` gives this surface its role; `present()`
    /// is an error per the library's docs.
    fn new(
        application: &Application,
        scale: f64,
        connector: String,
        system: &SystemSnapshot,
        weather_state: Option<&WeatherState>,
        animation: LockAnimation,
        icons: IconStyle,
    ) -> (Self, ApplicationWindow) {
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
        if animation.duration().is_some() {
            card.set_opacity(0.0);
        }

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

        let system_info = gtk::Label::new(None);
        system_info.add_css_class("lock-system-info");
        system_info.set_halign(Align::Center);
        system_info.set_ellipsize(gtk::pango::EllipsizeMode::End);
        system_info.set_max_width_chars(48);

        let context = gtk::Box::new(Orientation::Horizontal, scaled(8, scale));
        context.add_css_class("lock-context");
        context.set_halign(Align::Center);
        let battery = gtk::Label::new(None);
        battery.add_css_class("lock-info-chip");
        battery.set_ellipsize(gtk::pango::EllipsizeMode::End);
        battery.set_max_width_chars(20);
        let weather = gtk::Label::new(None);
        weather.add_css_class("lock-info-chip");
        weather.set_ellipsize(gtk::pango::EllipsizeMode::End);
        weather.set_max_width_chars(28);
        context.append(&battery);
        context.append(&weather);

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

        let power_row = gtk::Box::new(Orientation::Horizontal, scaled(8, scale));
        power_row.add_css_class("lock-power-row");
        power_row.set_homogeneous(true);
        let power_specs = [
            (PowerAction::PowerOff, Icon::Shutdown, "Power off"),
            (PowerAction::Suspend, Icon::Suspend, "Suspend"),
            (PowerAction::Reboot, Icon::Reboot, "Reboot"),
        ];
        let mut power_buttons = Vec::with_capacity(power_specs.len());
        for (action, icon, label) in power_specs {
            let button = power_button(icon, label, icons);
            if matches!(action, PowerAction::PowerOff | PowerAction::Reboot) {
                button.add_css_class("danger");
            }
            power_row.append(&button);
            power_buttons.push((action, button.downgrade()));
        }

        for child in [
            eyebrow.upcast_ref::<gtk::Widget>(),
            clock.upcast_ref(),
            date.upcast_ref(),
            user.upcast_ref(),
            system_info.upcast_ref(),
            context.upcast_ref(),
            entry.upcast_ref(),
            caps.upcast_ref(),
            status.upcast_ref(),
            power_row.upcast_ref(),
        ] {
            card.append(child);
        }
        overlay.add_overlay(&card);
        window.set_child(Some(&overlay));

        let lock_window = Self {
            window: window.downgrade(),
            backdrop: backdrop.downgrade(),
            content: card.downgrade(),
            entry: entry.downgrade(),
            status: status.downgrade(),
            clock: clock.downgrade(),
            date: date.downgrade(),
            caps: caps.downgrade(),
            system_info: system_info.downgrade(),
            battery: battery.downgrade(),
            weather: weather.downgrade(),
            power_buttons,
            content_animation: Rc::new(Cell::new(0)),
            backdrop_animation: Rc::new(Cell::new(0)),
            connector,
        };
        lock_window.update_system(system);
        lock_window.update_weather(weather_state);
        (lock_window, window)
    }

    fn set_backdrop(&self, texture: &gdk::Texture, animation: LockAnimation) {
        if let Some(backdrop) = self.backdrop.upgrade() {
            backdrop.set_paintable(Some(texture));
            if let Some(duration_ms) = animation.duration() {
                animate_opacity(
                    backdrop.upcast_ref(),
                    1.0,
                    duration_ms,
                    &self.backdrop_animation,
                );
            } else {
                backdrop.set_opacity(1.0);
            }
        }
    }

    fn fade_in(&self, animation: LockAnimation) {
        let Some(content) = self.content.upgrade() else {
            return;
        };
        if let Some(duration_ms) = animation.duration() {
            animate_opacity(
                content.upcast_ref(),
                1.0,
                duration_ms,
                &self.content_animation,
            );
        } else {
            content.set_opacity(1.0);
        }
    }

    fn fade_out(&self, duration_ms: u32) {
        if let Some(content) = self.content.upgrade() {
            content.set_sensitive(false);
            animate_opacity(
                content.upcast_ref(),
                0.0,
                duration_ms,
                &self.content_animation,
            );
        }
        if let Some(backdrop) = self.backdrop.upgrade() {
            animate_opacity(
                backdrop.upcast_ref(),
                0.0,
                duration_ms,
                &self.backdrop_animation,
            );
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

    fn update_system(&self, snapshot: &SystemSnapshot) {
        if let Some(label) = self.system_info.upgrade() {
            label.set_label(
                &snapshot
                    .info
                    .as_ref()
                    .map(system_info_text)
                    .unwrap_or_default(),
            );
            label.set_visible(snapshot.info.is_some());
        }
        if let Some(label) = self.battery.upgrade() {
            if let Some(battery) = &snapshot.battery {
                label.set_label(&format!("BAT {}% · {}", battery.percent, battery.status));
                label.set_visible(true);
            } else {
                label.set_visible(false);
            }
        }
    }

    fn update_weather(&self, state: Option<&WeatherState>) {
        let Some(label) = self.weather.upgrade() else {
            return;
        };
        if let Some(state) = state {
            label.set_label(&format!(
                "{}°C · {} · {}",
                state.current_c, state.description, state.location
            ));
            label.set_visible(true);
        } else {
            label.set_visible(false);
        }
    }

    fn set_power_sensitive(&self, sensitive: bool) {
        for (_, button) in &self.power_buttons {
            if let Some(button) = button.upgrade() {
                button.set_sensitive(sensitive);
            }
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
    animation: LockAnimation,
    /// How the power-row icons are drawn; see [`crate::ui::icon`].
    icons: IconStyle,
    initial_system: RefCell<SystemSnapshot>,
    initial_weather: RefCell<Option<WeatherState>>,
    submit: LockSubmitAction,
    power: LockPowerAction,
    on_ended: LockEndedAction,
    state_changed: LockStateAction,
    /// Incremented per attempt so a verdict that arrives after the user has
    /// already typed something else is ignored.
    generation: Cell<u64>,
    /// Set while PAM is deliberating; blocks further submissions.
    busy: Cell<bool>,
    power_pending: Cell<bool>,
    attempts: Cell<u32>,
    /// Guards the entry mirroring so propagating text to the other outputs
    /// does not re-enter through their `changed` handlers.
    syncing: Cell<bool>,
    /// Set by the `failed` signal, which can fire synchronously inside
    /// `lock()`; the caller checks this because `on_ended` may have already
    /// fired before the session was stored anywhere.
    failed: Cell<bool>,
    /// Guards the exit transition and prevents the signal from finishing an
    /// explicit unlock too early.
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
    // Every parameter is an independent piece of session state with no
    // meaningful grouping; bundling them into a struct would only move the
    // list somewhere else.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        application: &Application,
        config: &LockConfig,
        configured_scale: f64,
        system: &SystemSnapshot,
        weather: Option<&WeatherState>,
        animation: LockAnimation,
        icons: IconStyle,
        actions: LockActions,
    ) -> Rc<Self> {
        let session = Rc::new(Self {
            instance: SessionLockInstance::new(),
            windows: RefCell::new(HashMap::new()),
            application: application.clone(),
            config: config.clone(),
            scale: configured_scale,
            animation,
            icons,
            initial_system: RefCell::new(system.clone()),
            initial_weather: RefCell::new(weather.cloned()),
            submit: actions.submit,
            power: actions.power,
            on_ended: actions.ended,
            state_changed: actions.state_changed,
            generation: Cell::new(0),
            busy: Cell::new(false),
            power_pending: Cell::new(false),
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
                (session.state_changed)(true);
                session.focus_active_entry();
            }
        });

        let weak = Rc::downgrade(&session);
        session.instance.connect_unlocked(move |_| {
            info!("session unlocked");
            if let Some(session) = weak.upgrade() {
                (session.state_changed)(false);
                if !session.unlocking.get() {
                    let ended = session.on_ended.clone();
                    glib::idle_add_local_once(move || ended());
                }
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
        let system = self.initial_system.borrow();
        let weather = self.initial_weather.borrow();
        let (window, gtk_window) = LockWindow::new(
            &self.application,
            scale,
            connector.clone(),
            &system,
            weather.as_ref(),
            self.animation,
            self.icons,
        );
        drop(weather);
        drop(system);
        let window = Rc::new(window);
        self.connect_window(&window);

        let captured = self.captures.borrow_mut().remove(&connector);

        // gtk-session-lock destroys the assigned window when its monitor is
        // invalidated. Detach it first: GTK 4.22.4 otherwise reaches
        // gdk_wayland_toplevel_remove_from_session with a null toplevel
        // (GNOME/gtk#8098). Keep it alive through the library's after-handler,
        // but remove the stale connector immediately so a replacement monitor
        // can rebuild its surface during the same main-loop turn.
        let weak_session = Rc::downgrade(self);
        let weak_window = gtk_window.downgrade();
        let invalidated_connector = connector.clone();
        monitor.connect_invalidate(move |_| {
            let retained_window = weak_window.upgrade();
            if let Some(window) = retained_window.as_ref() {
                window.set_application(None::<&Application>);
            }
            if let Some(session) = weak_session.upgrade() {
                session.windows.borrow_mut().remove(&invalidated_connector);
            }
            glib::idle_add_local_once(move || drop(retained_window));
        });

        // `assign_window_to_monitor` may round-trip Wayland, so publish the
        // entry first; an output disappearing during that round-trip must not
        // be reinserted after the invalidation handler removes it.
        self.windows
            .borrow_mut()
            .insert(connector.clone(), window.clone());
        instance.assign_window_to_monitor(&gtk_window, monitor);
        if let Some(entry) = window.entry.upgrade() {
            entry.grab_focus();
        }
        if !self.unlocking.get() {
            window.fade_in(self.animation);
        }

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
                if session.unlocking.get() {
                    continue;
                }
                for window in session.windows.borrow().values() {
                    if window.connector == ready.connector {
                        window.set_backdrop(texture.upcast_ref(), session.animation);
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

        for (action, button) in &window.power_buttons {
            let Some(button) = button.upgrade() else {
                continue;
            };
            let action = *action;
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(session) = weak.upgrade() {
                    session.request_power(action);
                }
            });
        }

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
    /// this starts the exit transition and then calls
    /// [`Instance::unlock`][gtk4_session_lock::Instance::unlock], which is
    /// what actually ends the compositor-enforced lock.
    pub fn resolve(self: &Rc<Self>, generation: u64, outcome: &Outcome) {
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
                self.begin_unlock();
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
    pub fn force_unlock(self: &Rc<Self>) {
        self.begin_unlock();
    }

    pub fn update_system(&self, snapshot: &SystemSnapshot) {
        *self.initial_system.borrow_mut() = snapshot.clone();
        for window in self.windows.borrow().values() {
            window.update_system(snapshot);
        }
    }

    pub fn update_weather(&self, state: &WeatherState) {
        *self.initial_weather.borrow_mut() = Some(state.clone());
        for window in self.windows.borrow().values() {
            window.update_weather(Some(state));
        }
    }

    fn request_power(self: &Rc<Self>, action: PowerAction) {
        if self.power_pending.replace(true) {
            return;
        }
        self.set_power_sensitive(false);
        self.broadcast_status(power_pending_message(action), false);

        let (sender, receiver) = async_channel::bounded(1);
        (self.power)(action, sender);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = receiver
                .recv()
                .await
                .unwrap_or_else(|_| Err("the power worker stopped unexpectedly".to_owned()));
            if let Some(session) = weak.upgrade() {
                session.resolve_power(action, result);
            }
        });
    }

    fn resolve_power(self: &Rc<Self>, action: PowerAction, result: Result<(), String>) {
        match result {
            Ok(()) if action == PowerAction::Suspend => {
                self.broadcast_status("Suspend requested", false);
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
                    if let Some(session) = weak.upgrade() {
                        session.power_pending.set(false);
                        session.set_power_sensitive(true);
                        session.broadcast_status("", false);
                    }
                });
            }
            Ok(()) => self.broadcast_status(power_pending_message(action), false),
            Err(message) => {
                warn!("{} request failed: {message}", power_action_name(action));
                self.power_pending.set(false);
                self.set_power_sensitive(true);
                self.broadcast_status(
                    &format!("Could not {}: {message}", power_action_name(action)),
                    true,
                );
            }
        }
    }

    fn set_power_sensitive(&self, sensitive: bool) {
        for window in self.windows.borrow().values() {
            window.set_power_sensitive(sensitive);
        }
    }

    fn begin_unlock(self: &Rc<Self>) {
        if self.unlocking.replace(true) {
            return;
        }
        let Some(duration_ms) = self.animation.duration() else {
            self.unlock_after_library_cleanup();
            return;
        };
        for window in self.windows.borrow().values() {
            window.fade_out(duration_ms);
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(u64::from(duration_ms)),
            move || {
                if let Some(session) = weak.upgrade() {
                    session.unlock_after_library_cleanup();
                }
            },
        );
    }

    fn unlock_after_library_cleanup(&self) {
        // GTK 4.22.4 crashes in `gdk_wayland_toplevel_remove_from_session`
        // when "window-removed" fires for a window whose surface is already
        // gone, as happens for the lock surfaces during the C library's
        // teardown. Detaching the windows first skips that path; the
        // upstream NULL guard is unreleased (GNOME/gtk#8098).
        for window in self.windows.borrow().values() {
            if let Some(window) = window.window.upgrade() {
                window.set_application(None::<&Application>);
            }
        }
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

fn animate_opacity(
    widget: &gtk::Widget,
    target: f64,
    duration_ms: u32,
    animation_generation: &Rc<Cell<u64>>,
) {
    let generation = animation_generation.get().wrapping_add(1);
    animation_generation.set(generation);
    let animation_generation = animation_generation.clone();
    let start_opacity = widget.opacity();
    let start_time = Cell::new(None::<i64>);
    let duration_us = i64::from(duration_ms) * 1_000;
    widget.add_tick_callback(move |widget, frame_clock| {
        if animation_generation.get() != generation {
            return glib::ControlFlow::Break;
        }
        let now = frame_clock.frame_time();
        let started = start_time.get().unwrap_or_else(|| {
            start_time.set(Some(now));
            now
        });
        let progress = ((now - started) as f64 / duration_us as f64).clamp(0.0, 1.0);
        widget.set_opacity(start_opacity + (target - start_opacity) * smoothstep(progress));
        if progress >= 1.0 {
            widget.set_opacity(target);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn smoothstep(progress: f64) -> f64 {
    progress * progress * (3.0 - 2.0 * progress)
}

fn power_button(icon: Icon, label: &str, style: IconStyle) -> gtk::Button {
    let content = gtk::Box::new(Orientation::Horizontal, 6);
    content.set_halign(Align::Center);
    content.append(&crate::ui::icon::icon_widget(icon, style));
    content.append(&gtk::Label::new(Some(label)));
    let button = gtk::Button::new();
    button.add_css_class("lock-power-button");
    button.set_tooltip_text(Some(label));
    button.set_child(Some(&content));
    button
}

fn system_info_text(info: &SystemInfoState) -> String {
    format!(
        "{} · {} · up {}",
        info.hostname,
        info.os_name,
        format_uptime(info.uptime_seconds)
    )
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn power_action_name(action: PowerAction) -> &'static str {
    match action {
        PowerAction::PowerOff => "power off",
        PowerAction::Suspend => "suspend",
        PowerAction::Reboot => "reboot",
    }
}

fn power_pending_message(action: PowerAction) -> &'static str {
    match action {
        PowerAction::PowerOff => "Powering off…",
        PowerAction::Suspend => "Suspending…",
        PowerAction::Reboot => "Rebooting…",
    }
}

impl Drop for LockSession {
    fn drop(&mut self) {
        self.teardown();
    }
}

#[cfg(test)]
mod tests {
    use super::{LockAnimation, format_uptime, smoothstep};

    #[test]
    fn formats_uptime_at_useful_precision() {
        assert_eq!(format_uptime(42), "0m");
        assert_eq!(format_uptime(3_720), "1h 2m");
        assert_eq!(format_uptime(183_600), "2d 3h");
    }

    #[test]
    fn lock_animation_honors_disable_and_configured_duration() {
        assert_eq!(
            LockAnimation {
                enabled: false,
                duration_ms: 280,
            }
            .duration(),
            None
        );
        assert_eq!(
            LockAnimation {
                enabled: true,
                duration_ms: 280,
            }
            .duration(),
            Some(280)
        );
    }

    #[test]
    fn fade_easing_preserves_endpoints() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(smoothstep(1.0), 1.0);
    }
}
