use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    thread,
};

use anyhow::{Context, Result, bail};
use async_channel::{Receiver, Sender};
use clap::CommandFactory;
use gtk::{Application, CssProvider, gdk, gio, glib, prelude::*};
use log::{error, info, warn};

use crate::{
    cli::{self, Cli, Command, SetupCommand, ThemeCommand, ThemeModeArg},
    config::{self, AppConfig, PaletteEngine, ThemeMode, ThemeSource},
    hyprland::{self, HyprlandUpdate},
    ipc::{self, IncomingRequest, IpcCommand, MonitorTarget, OsdKind, Request, Response},
    latency,
    lock::{self, AuthEvent, AuthRequest},
    media,
    notifications::{self, CloseReason, NotificationCommand, NotificationEvent},
    preview::{self, PreviewEvent, PreviewRequest},
    setup,
    state::{
        AudioState, HyprlandSnapshot, MediaState, Notification, OsdState, Palette, SystemSnapshot,
        WeatherState,
    },
    system,
    tarragon::{self, TarragonCommand, TarragonEvent, TarragonSnapshot, TarragonStatus},
    theme,
    ui::{self, IslandActions, IslandWindow, LockSession, LockSubmitAction},
    weather,
};

pub fn run(cli: Cli) -> Result<()> {
    // Completions are generated offline; skip resolving a runtime socket
    // path (which requires XDG_RUNTIME_DIR) for this command entirely.
    if let Command::Completions { shell } = &cli.command {
        generate_completions(*shell);
        return Ok(());
    }
    // Setup is a standalone local operation -- installing/enabling an
    // optional integration -- and must work even without a running daemon
    // or a resolvable runtime socket path, exactly like `Completions` above.
    if let Command::Setup { command } = cli.command {
        return match command {
            SetupCommand::Tarragon(args) => setup::install_tarragon(args),
        };
    }
    let socket_path = ipc::socket_path(cli.socket)?;
    match cli.command {
        Command::Daemon {
            config,
            no_animations,
        } => run_daemon(socket_path, config, !no_animations),
        command => run_client(socket_path, command),
    }
}

/// Writes a completion script for `shell` to stdout, generated from the same
/// `clap::Command` used to parse arguments so it never drifts from the CLI.
fn generate_completions(shell: clap_complete::Shell) {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
}

fn run_client(socket_path: PathBuf, command: Command) -> Result<()> {
    let is_latency = matches!(command, Command::Latency { .. });
    let is_palette = matches!(
        command,
        Command::Theme {
            command: ThemeCommand::Palette
        }
    );
    let (request, print_json) = command_request(command)?;
    let response = ipc::send(&socket_path, &request)?;
    if print_json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else if is_latency {
        print_latency_report(&response);
    } else if is_palette {
        print_palette_swatches(&response);
    } else if let Some(data) = &response.data {
        println!("{}", serde_json::to_string_pretty(data)?);
    } else {
        println!("{}", response.message);
    }
    if !response.ok {
        bail!(response.message);
    }
    Ok(())
}

/// Role labels in the same order as `Palette`'s fields, paired with the JSON
/// key `ThemeCurrent` reports them under.
const PALETTE_ROLES: &[(&str, &str)] = &[
    ("primary", "Primary"),
    ("on_primary", "On Primary"),
    ("primary_container", "Primary Container"),
    ("on_primary_container", "On Primary Container"),
    ("secondary", "Secondary"),
    ("tertiary", "Tertiary"),
    ("surface", "Surface"),
    ("surface_container_low", "Surface Container Low"),
    ("surface_container", "Surface Container"),
    ("surface_container_high", "Surface Container High"),
    ("on_surface", "On Surface"),
    ("on_surface_variant", "On Surface Variant"),
    ("outline", "Outline"),
    ("outline_variant", "Outline Variant"),
    ("error", "Error"),
];

/// Renders each palette role as a colored square (when stdout is a
/// terminal) followed by its scope name and hex value.
fn print_palette_swatches(response: &Response) {
    use std::io::IsTerminal;

    let Some(data) = &response.data else {
        println!("{}", response.message);
        return;
    };
    let colorize = std::io::stdout().is_terminal();
    if let Some(source) = data.get("source").and_then(serde_json::Value::as_str) {
        let mode = data
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        println!("source: {source}  mode: {mode}");
        println!();
    }
    for (key, label) in PALETTE_ROLES {
        let Some(hex) = data.get(*key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let square = match (colorize, parse_hex(hex)) {
            (true, Some((r, g, b))) => format!("\x1b[48;2;{r};{g};{b}m   \x1b[0m"),
            _ => "   ".to_owned(),
        };
        println!("{square}  {label:<24} {hex}");
    }
}

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

/// Renders the latency spans as a fixed-width table, in the same shape as
/// `tarragon bench` so the two are easy to read side by side.
fn print_latency_report(response: &Response) {
    let Some(spans) = response
        .data
        .as_ref()
        .and_then(|data| data.get("spans"))
        .and_then(|spans| spans.as_object())
    else {
        println!("{}", response.message);
        return;
    };
    if spans.is_empty() {
        println!("{}", response.message);
        println!("no samples recorded yet");
        return;
    }

    println!("Mithshell search latency");
    println!();
    println!(
        "{:<10}  {:>5}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
        "Span", "Runs", "Avg ms", "Min ms", "P50 ms", "P95 ms", "Max ms"
    );
    println!("{}", "-".repeat(68));
    // Fixed pipeline order rather than the map's ordering.
    for name in ["debounce", "write", "backend", "build", "paint", "total"] {
        let Some(span) = spans.get(name) else {
            continue;
        };
        let number = |key: &str| {
            span.get(key)
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0)
        };
        let count = span
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        println!(
            "{:<10}  {:>5}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}",
            name,
            count,
            number("avg_ms"),
            number("min_ms"),
            number("p50_ms"),
            number("p95_ms"),
            number("max_ms"),
        );
    }
}

fn command_request(command: Command) -> Result<(Request, bool)> {
    let mut json = false;
    let command = match command {
        Command::Toggle(args) => IpcCommand::Toggle {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Open(args) => IpcCommand::Open {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Search(args) => IpcCommand::Search {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Close(args) => IpcCommand::Close {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Osd {
            kind,
            value,
            timeout,
            monitor,
        } => IpcCommand::Osd {
            monitor: MonitorTarget::parse(&monitor.monitor)?,
            kind: match kind {
                cli::OsdKind::Volume => OsdKind::Volume,
                cli::OsdKind::Brightness => OsdKind::Brightness,
                cli::OsdKind::Workspace => OsdKind::Workspace,
            },
            value,
            timeout_ms: timeout,
        },
        Command::Lock => IpcCommand::Lock,
        Command::Unlock => IpcCommand::Unlock,
        Command::Reload => IpcCommand::Reload,
        Command::Status { json: print_json } => {
            json = print_json;
            IpcCommand::Status
        }
        Command::Latency {
            json: print_json,
            reset,
        } => {
            json = print_json;
            IpcCommand::Latency { reset }
        }
        Command::Theme { command } => match command {
            ThemeCommand::Set {
                image,
                color,
                mode,
                persist,
            } => {
                let source = if let Some(path) = image {
                    let path = config::expand_home(path);
                    let path = path.canonicalize().with_context(|| {
                        format!("failed to resolve theme image {}", path.display())
                    })?;
                    ThemeSource::Image { path }
                } else {
                    ThemeSource::Color {
                        value: color.context("--image or --color is required")?,
                    }
                };
                IpcCommand::ThemeSet {
                    source,
                    mode: mode.map(theme_mode),
                    persist,
                }
            }
            ThemeCommand::Mode { mode, persist } => IpcCommand::ThemeMode {
                mode: theme_mode(mode),
                persist,
            },
            ThemeCommand::Current { json: print_json } => {
                json = print_json;
                IpcCommand::ThemeCurrent
            }
            ThemeCommand::Palette => IpcCommand::ThemeCurrent,
            ThemeCommand::Reset => IpcCommand::ThemeReset,
        },
        Command::Daemon { .. } => bail!("daemon command cannot be sent over IPC"),
        Command::Completions { .. } => bail!("completions are generated locally, not over IPC"),
        Command::Setup { .. } => bail!("setup commands are handled locally, not over IPC"),
    };
    Ok((Request::new(command), json))
}

fn theme_mode(mode: ThemeModeArg) -> ThemeMode {
    match mode {
        ThemeModeArg::Dark => ThemeMode::Dark,
        ThemeModeArg::Light => ThemeMode::Light,
    }
}

fn run_daemon(
    socket_path: PathBuf,
    config_override: Option<PathBuf>,
    animations: bool,
) -> Result<()> {
    let config_path = config::config_path(config_override)?;
    let config = AppConfig::load(&config_path)?;
    ipc::prepare_runtime_socket(&socket_path)?;
    if latency::init() {
        info!("latency tracing enabled; read it with `mithshell latency`");
    }

    let application = Application::builder()
        .application_id("dev.mithrel.mithshell")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let controller_holder = Rc::new(RefCell::new(None::<Rc<Controller>>));
    let activate_holder = controller_holder.clone();
    application.connect_activate(move |application| {
        if activate_holder.borrow().is_some() {
            return;
        }
        match Controller::new(
            application,
            config_path.clone(),
            config.clone(),
            socket_path.clone(),
            animations,
        ) {
            Ok(controller) => {
                controller.clone().start();
                *activate_holder.borrow_mut() = Some(controller);
            }
            Err(error) => {
                error!("failed to start mithshell: {error:#}");
                application.quit();
            }
        }
    });

    // Clap already consumed the process arguments; keep GTK from parsing daemon flags again.
    let status = application.run_with_args(&["mithshell"]);
    controller_holder.borrow_mut().take();
    if status == glib::ExitCode::SUCCESS {
        Ok(())
    } else {
        bail!("GTK application exited with {status:?}")
    }
}

struct Controller {
    application: Application,
    config_path: PathBuf,
    config: RefCell<AppConfig>,
    theme_config: RefCell<crate::config::ThemeConfig>,
    palette: RefCell<Palette>,
    css_provider: CssProvider,
    user_css_provider: CssProvider,
    hyprland: RefCell<HyprlandSnapshot>,
    system: RefCell<SystemSnapshot>,
    media: RefCell<Option<MediaState>>,
    weather: RefCell<Option<WeatherState>>,
    visualizer: RefCell<media::VisualizerLevels>,
    tarragon_connected: Cell<bool>,
    tarragon_snapshot: RefCell<Option<TarragonSnapshot>>,
    tarragon_status: RefCell<Option<TarragonStatus>>,
    pending_volume: Cell<Option<u8>>,
    islands: RefCell<HashMap<String, Rc<IslandWindow>>>,
    /// `Some` exactly while the session is locked.
    lock: RefCell<Option<Rc<LockSession>>>,
    animations: bool,
    theme_sender: Sender<Result<Palette, String>>,
    auth_sender: Sender<AuthRequest>,
    tarragon_sender: Sender<TarragonCommand>,
    preview_sender: Sender<PreviewRequest>,
    /// Bounded, most-recent-first notification history backing every
    /// island's dashboard notification card. Capacity comes from
    /// `notifications.max_history`, re-read each time a notification lands.
    notifications: RefCell<Vec<Notification>>,
    notification_command_sender: Sender<NotificationCommand>,
    _media_listener: thread::JoinHandle<()>,
    _visualizer_listener: thread::JoinHandle<()>,
    _weather_listener: thread::JoinHandle<()>,
    _notification_listener: thread::JoinHandle<()>,
    tarragon_listener: Option<thread::JoinHandle<()>>,
    preview_listener: Option<thread::JoinHandle<()>>,
    auth_listener: Option<thread::JoinHandle<()>>,
    _gtk_css_watcher: Option<thread::JoinHandle<()>>,
}

impl Controller {
    fn new(
        application: &Application,
        config_path: PathBuf,
        config: AppConfig,
        socket_path: PathBuf,
        animations: bool,
    ) -> Result<Rc<Self>> {
        let mut initial_theme = config.theme.clone();
        if let Some(theme_override) = theme::load_override()? {
            theme::apply_override(&mut initial_theme, theme_override);
        }
        let fallback = theme::generate(&crate::config::ThemeConfig::default())?;
        let css_provider = ui::install_styles(&fallback);
        let user_css_provider =
            ui::install_user_styles(&crate::config::colors_css_path(&config_path));
        let (theme_sender, theme_receiver) = async_channel::unbounded();
        let (media_sender, media_receiver) = async_channel::unbounded();
        let media_listener = media::start_listener(media_sender);
        let (visualizer_sender, visualizer_receiver) = async_channel::bounded(2);
        let visualizer_listener = media::start_visualizer(visualizer_sender);
        let (weather_sender, weather_receiver) = async_channel::unbounded();
        let weather_listener = weather::start_poller(
            config.weather.provider,
            config.weather.city.clone(),
            weather_sender,
        );
        let (tarragon_event_sender, tarragon_event_receiver) = async_channel::unbounded();
        let (tarragon_sender, tarragon_listener) = tarragon::start_listener(tarragon_event_sender);
        let (preview_event_sender, preview_event_receiver) = async_channel::unbounded();
        let (preview_sender, preview_listener) = preview::start_loader(preview_event_sender);
        let (gtk_css_sender, gtk_css_receiver) = async_channel::unbounded();
        let gtk_css_watcher = theme::watch_gtk_css(gtk_css_sender);
        let (notification_event_sender, notification_event_receiver) = async_channel::unbounded();
        let (notification_command_sender, notification_listener) =
            notifications::start_server(notification_event_sender);
        // Resolved once here rather than at lock time so a broken PAM
        // configuration shows up in the log at startup, while the user can
        // still do something about it.
        let pam_service = lock::service_name(config.lock.pam_service.as_deref());
        let (auth_event_sender, auth_event_receiver) = async_channel::unbounded();
        let (auth_sender, auth_listener) =
            lock::start_authenticator(pam_service, auth_event_sender);

        let controller = Rc::new(Self {
            application: application.clone(),
            config_path,
            config: RefCell::new(config),
            theme_config: RefCell::new(initial_theme),
            palette: RefCell::new(fallback),
            css_provider,
            user_css_provider,
            hyprland: RefCell::new(HyprlandSnapshot::default()),
            system: RefCell::new(SystemSnapshot::default()),
            media: RefCell::new(None),
            weather: RefCell::new(None),
            visualizer: RefCell::new([0; media::VISUALIZER_BARS]),
            tarragon_connected: Cell::new(false),
            tarragon_snapshot: RefCell::new(None),
            tarragon_status: RefCell::new(None),
            pending_volume: Cell::new(None),
            islands: RefCell::new(HashMap::new()),
            lock: RefCell::new(None),
            animations,
            theme_sender,
            auth_sender,
            tarragon_sender,
            preview_sender,
            notifications: RefCell::new(Vec::new()),
            notification_command_sender,
            _media_listener: media_listener,
            _visualizer_listener: visualizer_listener,
            _weather_listener: weather_listener,
            _notification_listener: notification_listener,
            tarragon_listener: Some(tarragon_listener),
            preview_listener: Some(preview_listener),
            auth_listener: Some(auth_listener),
            _gtk_css_watcher: gtk_css_watcher,
        });

        let (ipc_sender, ipc_receiver) = async_channel::unbounded();
        ipc::start_server(socket_path, ipc_sender)?;
        controller.attach_ipc(ipc_receiver);

        let (hypr_sender, hypr_receiver) = async_channel::unbounded();
        hyprland::start_listener(hypr_sender);
        controller.attach_hyprland(hypr_receiver);

        let (system_sender, system_receiver) = async_channel::unbounded();
        system::start_poller(system_sender);
        controller.attach_system(system_receiver);
        let (audio_sender, audio_receiver) = async_channel::unbounded();
        system::start_audio_listener(audio_sender);
        controller.attach_audio(audio_receiver);
        controller.attach_media(media_receiver);
        controller.attach_weather(weather_receiver);
        controller.attach_visualizer(visualizer_receiver);
        controller.attach_tarragon(tarragon_event_receiver);
        controller.attach_preview(preview_event_receiver);
        controller.attach_theme(theme_receiver);
        controller.attach_auth(auth_event_receiver);
        controller.attach_notifications(notification_event_receiver);
        controller.attach_gtk_theme_watch();
        controller.attach_gtk_css_watch(gtk_css_receiver);

        Ok(controller)
    }

    fn start(self: Rc<Self>) {
        self.reconcile_monitors();
        self.generate_theme(self.theme_config.borrow().clone());

        if let Some(display) = gdk::Display::default() {
            let monitors = display.monitors();
            let weak = Rc::downgrade(&self);
            monitors.connect_items_changed(move |_, _, _, _| {
                if let Some(controller) = weak.upgrade() {
                    controller.reconcile_monitors();
                    // Deliberately not touched here: a `LockSession` tracks
                    // hotplugged outputs itself, through the session lock
                    // instance's own `monitor` signal.
                }
            });
        }
        info!(
            "mithshell started with config {}",
            self.config_path.display()
        );
    }

    fn attach_ipc(self: &Rc<Self>, receiver: Receiver<IncomingRequest>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(incoming) = receiver.recv().await {
                let response = weak.upgrade().map_or_else(
                    || Response::error("daemon is shutting down"),
                    |controller| controller.handle_command(incoming.request.command),
                );
                let _ = incoming.respond_to.send(response);
            }
        });
    }

    fn attach_hyprland(self: &Rc<Self>, receiver: Receiver<HyprlandUpdate>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(update) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                match update {
                    HyprlandUpdate::Snapshot(snapshot) => {
                        for island in controller.islands.borrow().values() {
                            island.update_hyprland(&snapshot);
                        }
                        *controller.hyprland.borrow_mut() = snapshot;
                    }
                    HyprlandUpdate::Unavailable(message) => warn!("Hyprland IPC: {message}"),
                }
            }
        });
    }

    fn attach_system(self: &Rc<Self>, receiver: Receiver<SystemSnapshot>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(snapshot) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                for island in controller.islands.borrow().values() {
                    island.update_system(&snapshot);
                }
                *controller.system.borrow_mut() = snapshot;
            }
        });
    }

    fn attach_audio(self: &Rc<Self>, receiver: Receiver<AudioState>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(audio) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                let suppress_osd = controller.pending_volume.get() == Some(audio.percent);
                if suppress_osd {
                    controller.pending_volume.set(None);
                }
                let snapshot = {
                    let mut system = controller.system.borrow_mut();
                    system.audio = Some(audio);
                    system.clone()
                };
                for island in controller.islands.borrow().values() {
                    island.update_system(&snapshot);
                }

                if suppress_osd {
                    continue;
                }
                match controller.target_islands(&MonitorTarget::Focused) {
                    Ok(islands) => {
                        for island in islands {
                            island.show_osd(OsdState {
                                kind: OsdKind::Volume,
                                value: audio.percent,
                                muted: audio.muted,
                                timeout_ms: 1_500,
                            });
                        }
                    }
                    Err(error) => warn!("cannot show volume OSD: {error:#}"),
                }
            }
        });
    }

    fn attach_media(self: &Rc<Self>, receiver: Receiver<Option<MediaState>>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(state) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                for island in controller.islands.borrow().values() {
                    island.update_media(state.as_ref());
                }
                *controller.media.borrow_mut() = state;
            }
        });
    }

    fn attach_weather(self: &Rc<Self>, receiver: Receiver<WeatherState>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(state) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                for island in controller.islands.borrow().values() {
                    island.update_weather(Some(&state));
                }
                *controller.weather.borrow_mut() = Some(state);
            }
        });
    }

    fn attach_visualizer(self: &Rc<Self>, receiver: Receiver<media::VisualizerLevels>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(levels) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                if controller.media.borrow().is_some() {
                    for island in controller.islands.borrow().values() {
                        island.update_visualizer(levels);
                    }
                }
                *controller.visualizer.borrow_mut() = levels;
            }
        });
    }

    fn attach_tarragon(self: &Rc<Self>, receiver: Receiver<TarragonEvent>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                match event {
                    TarragonEvent::Connection { connected, message } => {
                        controller.tarragon_connected.set(connected);
                        if !connected {
                            *controller.tarragon_snapshot.borrow_mut() = None;
                        }
                        for island in controller.islands.borrow().values() {
                            island.update_tarragon_connection(connected, message.as_deref());
                        }
                        if let Some(message) = message {
                            warn!("{message}");
                        }
                    }
                    TarragonEvent::Results(snapshot) => {
                        latency::mark_results();
                        for island in controller.islands.borrow().values() {
                            island.update_tarragon_results(&snapshot);
                        }
                        *controller.tarragon_snapshot.borrow_mut() = Some(snapshot);
                    }
                    TarragonEvent::Status(status) => {
                        for island in controller.islands.borrow().values() {
                            island.update_tarragon_status(&status);
                        }
                        *controller.tarragon_status.borrow_mut() = Some(status);
                    }
                    TarragonEvent::Reload { success, message } => {
                        for island in controller.islands.borrow().values() {
                            island.update_tarragon_reload(success, &message);
                        }
                        if success {
                            let _ = controller.tarragon_sender.try_send(TarragonCommand::Status);
                            info!("TarraGon: {message}");
                        } else {
                            warn!("TarraGon reload failed: {message}");
                        }
                    }
                    TarragonEvent::Error(message) => {
                        for island in controller.islands.borrow().values() {
                            island.update_tarragon_reload(false, &message);
                        }
                        warn!("TarraGon protocol error: {message}");
                    }
                    TarragonEvent::Selection { success, message } => {
                        for island in controller.islands.borrow().values() {
                            island.update_tarragon_selection(success, &message);
                        }
                        if success {
                            info!("TarraGon: {message}");
                        } else {
                            warn!("TarraGon action failed: {message}");
                        }
                    }
                }
            }
        });
    }

    /// Routes PAM worker messages back to the lock screen.
    ///
    /// On `Outcome::Granted`, `LockSession::resolve` itself calls
    /// `Instance::unlock`, which fires the `unlocked` signal wired in
    /// `LockSession::new`; that is what actually clears `self.lock`, not
    /// this function. This only forwards messages.
    fn attach_auth(self: &Rc<Self>, receiver: Receiver<AuthEvent>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                // Clone the handle out before calling into it: the signal
                // handlers `resolve`/`progress` can trigger take the same
                // RefCell, and holding a borrow across the call would panic.
                let Some(session) = controller.lock.borrow().clone() else {
                    continue;
                };
                match event {
                    AuthEvent::Progress {
                        generation,
                        message,
                    } => {
                        session.progress(generation, &message);
                    }
                    AuthEvent::Result {
                        generation,
                        outcome,
                    } => {
                        session.resolve(generation, &outcome);
                    }
                }
            }
        });
    }

    fn attach_preview(self: &Rc<Self>, receiver: Receiver<PreviewEvent>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                if let Err(error) = &event.result {
                    warn!("preview for {} failed: {error}", event.monitor);
                }
                if let Some(island) = controller.islands.borrow().get(&event.monitor) {
                    island.apply_file_preview(event.generation, event.result);
                }
            }
        });
    }

    /// Routes `org.freedesktop.Notifications` traffic from
    /// `notifications::start_server` to the focused island (matching how
    /// the volume/brightness/workspace OSD picks a target) and keeps the
    /// bounded history behind every dashboard's notification card in sync.
    fn attach_notifications(self: &Rc<Self>, receiver: Receiver<NotificationEvent>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                match event {
                    NotificationEvent::Show(notification) => {
                        if !controller.config.borrow().notifications.enabled {
                            continue;
                        }
                        controller.record_notification(notification.clone());
                        match controller.target_islands(&MonitorTarget::Focused) {
                            Ok(islands) => {
                                for island in islands {
                                    island.show_notification(notification.clone());
                                }
                            }
                            Err(error) => warn!("cannot show notification: {error:#}"),
                        }
                    }
                    NotificationEvent::Closed { id, .. } => {
                        for island in controller.islands.borrow().values() {
                            island.close_notification(id);
                        }
                        controller.remove_notification_record(id);
                    }
                }
            }
        });
    }

    /// Inserts (or, for a `replaces_id` call, updates) a notification at
    /// the front of the history and trims it back to
    /// `notifications.max_history`.
    fn record_notification(self: &Rc<Self>, notification: Notification) {
        {
            let mut history = self.notifications.borrow_mut();
            history.retain(|existing| existing.id != notification.id);
            history.insert(0, notification);
            let max_history = self.config.borrow().notifications.max_history;
            history.truncate(max_history.max(1));
        }
        self.refresh_notification_dashboard();
    }

    fn remove_notification_record(self: &Rc<Self>, id: u32) {
        self.notifications
            .borrow_mut()
            .retain(|existing| existing.id != id);
        self.refresh_notification_dashboard();
    }

    fn refresh_notification_dashboard(self: &Rc<Self>) {
        let history = self.notifications.borrow();
        for island in self.islands.borrow().values() {
            island.update_notification_history(history.as_slice());
        }
    }

    /// User-initiated dismissal (a toast's or the dashboard history's close
    /// button): tells the D-Bus server to announce `NotificationClosed`,
    /// and -- unlike a timer expiring -- also drops it from history and
    /// closes it on every island right away, rather than waiting on the
    /// round trip back through `attach_notifications`.
    fn dismiss_notification(self: &Rc<Self>, id: u32) {
        let _ = self
            .notification_command_sender
            .try_send(NotificationCommand::Close {
                id,
                reason: CloseReason::Dismissed,
            });
        for island in self.islands.borrow().values() {
            island.close_notification(id);
        }
        self.remove_notification_record(id);
    }

    /// A notification (or one of its actions) was activated. Announces
    /// `ActionInvoked` and then tears it down the same way
    /// `dismiss_notification` does, matching the common desktop convention
    /// that activating a notification also closes it.
    fn invoke_notification_action(self: &Rc<Self>, id: u32, action_key: String) {
        let _ = self
            .notification_command_sender
            .try_send(NotificationCommand::InvokeAction { id, action_key });
        for island in self.islands.borrow().values() {
            island.close_notification(id);
        }
        self.remove_notification_record(id);
    }

    fn attach_theme(self: &Rc<Self>, receiver: Receiver<Result<Palette, String>>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(result) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                match result {
                    Ok(palette) => {
                        ui::update_styles(&controller.css_provider, &palette);
                        for island in controller.islands.borrow().values() {
                            island.update_palette();
                        }
                        *controller.palette.borrow_mut() = palette;
                    }
                    Err(message) => warn!("theme generation failed: {message}"),
                }
            }
        });
    }

    /// Regenerates the palette whenever the system GTK theme changes, so
    /// `theme.engine = "gtk"` tracks live theme/light-dark switches instead
    /// of only updating on the next `mithshell reload` or restart.
    /// No-op when the active engine is Material, since that palette is
    /// independent of the system theme.
    fn attach_gtk_theme_watch(self: &Rc<Self>) {
        let Some(settings) = gtk::Settings::default() else {
            warn!("no GTK settings available; theme.engine = \"gtk\" will not live-update");
            return;
        };

        let weak = Rc::downgrade(self);
        settings.connect_gtk_application_prefer_dark_theme_notify(move |_| {
            if let Some(controller) = weak.upgrade() {
                controller.regenerate_gtk_palette();
            }
        });

        let weak = Rc::downgrade(self);
        settings.connect_notify_local(Some("gtk-theme-name"), move |_, _| {
            if let Some(controller) = weak.upgrade() {
                controller.regenerate_gtk_palette();
            }
        });
    }

    fn regenerate_gtk_palette(self: &Rc<Self>) {
        let config = self.theme_config.borrow().clone();
        if matches!(config.engine, PaletteEngine::Gtk) {
            self.generate_theme(config);
        }
    }

    /// Regenerates the palette whenever `$XDG_CONFIG_HOME/gtk-4.0/gtk.css`
    /// changes on disk, so `theme.engine = "gtk"` follows external palette
    /// generators (matugen, wallust, ...) rewriting it, without needing a
    /// restart -- GTK itself never re-reads that file for an already
    /// running process, so this is the only way to see those edits live.
    fn attach_gtk_css_watch(self: &Rc<Self>, receiver: Receiver<()>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while receiver.recv().await.is_ok() {
                // Tools often replace the file via write-then-rename,
                // firing more than one event per save; give the burst a
                // moment to land before coalescing it into one regenerate.
                glib::timeout_future(std::time::Duration::from_millis(150)).await;
                while receiver.try_recv().is_ok() {}
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                controller.regenerate_gtk_palette();
            }
        });
    }

    fn handle_command(self: &Rc<Self>, command: IpcCommand) -> Response {
        match self.try_handle_command(command) {
            Ok(response) => response,
            Err(error) => Response::error(format!("{error:#}")),
        }
    }

    fn try_handle_command(self: &Rc<Self>, command: IpcCommand) -> Result<Response> {
        // Anything that raises interactive chrome is refused while locked.
        // The compositor already hides the island's surface for the
        // duration of the lock (ext-session-lock-v1 blanks every normal
        // client), so these would be invisible either way -- but TarraGon
        // search can launch applications, which must not be reachable from
        // a locked session by anyone who can reach the IPC socket, visible
        // or not.
        if self.lock.borrow().is_some()
            && matches!(
                command,
                IpcCommand::Toggle { .. } | IpcCommand::Open { .. } | IpcCommand::Search { .. }
            )
        {
            bail!("the session is locked");
        }
        match command {
            IpcCommand::Toggle { monitor } => {
                let targets = self.target_islands(&monitor)?;
                for island in targets {
                    island.toggle();
                }
                Ok(Response::ok("dashboard toggled"))
            }
            IpcCommand::Open { monitor } => {
                for island in self.target_islands(&monitor)? {
                    island.open();
                }
                Ok(Response::ok("dashboard opened"))
            }
            IpcCommand::Search { monitor } => {
                if !self.tarragon_connected.get() {
                    bail!("TarraGon is not connected");
                }
                for island in self.target_islands(&monitor)? {
                    island.open_search();
                }
                Ok(Response::ok("TarraGon search opened"))
            }
            IpcCommand::Close { monitor } => {
                for island in self.target_islands(&monitor)? {
                    island.close();
                }
                Ok(Response::ok("island collapsed"))
            }
            IpcCommand::Osd {
                monitor,
                kind,
                value,
                timeout_ms,
            } => {
                let value = value.unwrap_or_else(|| self.current_osd_value(kind));
                let muted = kind == OsdKind::Volume
                    && self.system.borrow().audio.is_some_and(|audio| audio.muted);
                for island in self.target_islands(&monitor)? {
                    island.show_osd(OsdState {
                        kind,
                        value,
                        muted,
                        timeout_ms,
                    });
                }
                Ok(Response::ok(format!("{kind:?} OSD shown")))
            }
            IpcCommand::Lock => {
                if self.lock.borrow().is_some() {
                    return Ok(Response::ok("session is already locked"));
                }
                self.lock()?;
                Ok(Response::ok("session lock requested"))
            }
            IpcCommand::Unlock => {
                // Same-user, same-machine escape hatch: reaching this at
                // all already proves the caller passed the peer-credential
                // check in `ipc::handle_connection`, whatever TTY or
                // session they're sending it from. Bypasses PAM entirely,
                // by design -- this is the recovery path for a stuck
                // prompt, not a second authentication method.
                let session = self.lock.borrow().clone();
                match session {
                    Some(session) => {
                        session.force_unlock();
                        Ok(Response::ok("unlock requested"))
                    }
                    None => Ok(Response::ok("session is not locked")),
                }
            }
            IpcCommand::Reload => {
                self.reload_config()?;
                Ok(Response::ok("configuration reloaded"))
            }
            IpcCommand::Status => {
                let windows: serde_json::Map<String, serde_json::Value> = self
                    .islands
                    .borrow()
                    .iter()
                    .map(|(name, island)| (name.clone(), island.debug_state()))
                    .collect();
                let data = serde_json::json!({
                    "config": self.config_path,
                    "windows": windows,
                    "hyprland": &*self.hyprland.borrow(),
                    "system": &*self.system.borrow(),
                    "media": &*self.media.borrow(),
                    "weather": &*self.weather.borrow(),
                    "notifications": &*self.notifications.borrow(),
                    "tarragon": {
                        "connected": self.tarragon_connected.get(),
                        "results": self.tarragon_snapshot.borrow().as_ref().map_or(0, |snapshot| snapshot.list.len()),
                        "plugins": self.tarragon_status.borrow().as_ref().map_or(0, |status| status.plugins.len()),
                    },
                    "palette": &*self.palette.borrow(),
                    "lock": self.lock.borrow().as_ref().map(|session| session.debug_state()),
                });
                Ok(Response::with_data("daemon is running", data))
            }
            IpcCommand::Latency { reset } => {
                if reset {
                    latency::reset();
                    return Ok(Response::ok("latency samples cleared"));
                }
                if latency::enabled() {
                    Ok(Response::with_data("latency report", latency::report()))
                } else {
                    Ok(Response::with_data(
                        "latency tracing is disabled; restart the daemon with MITHSHELL_TRACE_LATENCY=1",
                        latency::report(),
                    ))
                }
            }
            IpcCommand::ThemeSet {
                source,
                mode,
                persist,
            } => {
                let mut theme_config = self.theme_config.borrow().clone();
                theme_config.source = source;
                if let Some(mode) = mode {
                    theme_config.mode = mode;
                }
                if persist {
                    theme::persist(&theme::ThemeOverride {
                        source: theme_config.source.clone(),
                        mode: theme_config.mode,
                    })?;
                }
                *self.theme_config.borrow_mut() = theme_config.clone();
                self.generate_theme(theme_config);
                Ok(Response::ok("theme generation started"))
            }
            IpcCommand::ThemeMode { mode, persist } => {
                let mut theme_config = self.theme_config.borrow().clone();
                theme_config.mode = mode;
                if persist {
                    theme::persist(&theme::ThemeOverride {
                        source: theme_config.source.clone(),
                        mode,
                    })?;
                }
                *self.theme_config.borrow_mut() = theme_config.clone();
                self.generate_theme(theme_config);
                Ok(Response::ok("theme mode updated"))
            }
            IpcCommand::ThemeCurrent => Ok(Response::with_data(
                "active palette",
                serde_json::to_value(&*self.palette.borrow())?,
            )),
            IpcCommand::ThemeReset => {
                theme::clear_override()?;
                let base = AppConfig::load(&self.config_path)?.theme;
                *self.theme_config.borrow_mut() = base.clone();
                self.generate_theme(base);
                Ok(Response::ok("theme override reset"))
            }
        }
    }

    /// Requests a session lock on every output, via `ext-session-lock-v1`.
    ///
    /// Collapses the dashboard/search views first purely for state hygiene
    /// (so they aren't left open once unlocked) -- it is not what hides
    /// them while locked. The compositor does that itself, for every
    /// normal client's surface, the moment the lock is acquired, and it
    /// does not undo that if this process dies. That guarantee comes
    /// entirely from the protocol; nothing in this function provides it.
    fn lock(self: &Rc<Self>) -> Result<()> {
        for island in self.islands.borrow().values() {
            island.close();
        }

        let auth_sender = self.auth_sender.clone();
        let submit: LockSubmitAction = Rc::new(move |generation: u64, password: String| {
            if auth_sender
                .try_send(AuthRequest {
                    generation,
                    password,
                })
                .is_err()
            {
                error!("the authentication worker is gone; cannot verify the password");
            }
        });

        let weak = Rc::downgrade(self);
        let on_ended = Rc::new(move || {
            if let Some(controller) = weak.upgrade() {
                // A no-op if the session was never stored here in the first
                // place (the synchronous-failure case in `LockSession::new`);
                // see `has_failed` below.
                controller.lock.borrow_mut().take();
                // The compositor blanked these while locked; force a recommit.
                for island in controller.islands.borrow().values() {
                    island.recomposite();
                }
            }
        });

        let config = self.config.borrow();
        let session = LockSession::new(
            &self.application,
            &config.lock,
            config.shell.scale,
            submit,
            on_ended,
        );
        drop(config);

        if session.has_failed() {
            bail!("the compositor refused to lock the session");
        }
        *self.lock.borrow_mut() = Some(session);
        Ok(())
    }

    fn current_osd_value(&self, kind: OsdKind) -> u8 {
        match kind {
            OsdKind::Volume => self.system.borrow().audio.map_or(0, |audio| audio.percent),
            OsdKind::Brightness => self
                .system
                .borrow()
                .brightness
                .as_ref()
                .map_or(0, |brightness| brightness.percent),
            OsdKind::Workspace => self
                .hyprland
                .borrow()
                .focused_monitor()
                .map_or(0, |monitor| monitor.active_workspace.id.clamp(0, 100) as u8),
        }
    }

    fn target_islands(&self, target: &MonitorTarget) -> Result<Vec<Rc<IslandWindow>>> {
        let islands = self.islands.borrow();
        let names: Vec<String> = match target {
            MonitorTarget::All => islands.keys().cloned().collect(),
            MonitorTarget::Named(name) => vec![name.clone()],
            MonitorTarget::Focused => {
                let name = self
                    .hyprland
                    .borrow()
                    .focused_monitor()
                    .map(|monitor| monitor.name.clone())
                    .or_else(|| islands.keys().next().cloned())
                    .context("no island output is available")?;
                vec![name]
            }
        };
        let targets: Vec<_> = names
            .iter()
            .filter_map(|name| islands.get(name).cloned())
            .collect();
        if targets.is_empty() {
            bail!("no configured island matches target {target:?}");
        }
        Ok(targets)
    }

    fn reload_config(self: &Rc<Self>) -> Result<()> {
        if self.lock.borrow().is_some() {
            bail!("cannot reload while the session is locked");
        }
        let config = AppConfig::load(&self.config_path)?;
        let mut theme_config = config.theme.clone();
        if let Some(theme_override) = theme::load_override()? {
            theme::apply_override(&mut theme_config, theme_override);
        }
        *self.config.borrow_mut() = config;
        *self.theme_config.borrow_mut() = theme_config.clone();
        ui::reload_user_styles(
            &self.user_css_provider,
            &crate::config::colors_css_path(&self.config_path),
        );
        for island in self.islands.borrow_mut().drain().map(|(_, island)| island) {
            island.destroy();
        }
        self.reconcile_monitors();
        self.generate_theme(theme_config);
        Ok(())
    }

    fn generate_theme(&self, config: crate::config::ThemeConfig) {
        // The GTK engine reads widget/style-context state and must run on
        // the main thread; only the Material engine is safe to offload.
        if matches!(config.engine, crate::config::PaletteEngine::Gtk) {
            let result = theme::generate(&config)
                .and_then(|palette| {
                    theme::export_palette(&palette)?;
                    Ok(palette)
                })
                .map_err(|error| format!("{error:#}"));
            let _ = self.theme_sender.send_blocking(result);
            return;
        }

        let sender = self.theme_sender.clone();
        thread::spawn(move || {
            let result = theme::generate(&config)
                .and_then(|palette| {
                    theme::export_palette(&palette)?;
                    Ok(palette)
                })
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send_blocking(result);
        });
    }

    fn reconcile_monitors(self: &Rc<Self>) {
        let Some(display) = gdk::Display::default() else {
            warn!("no GDK display is available");
            return;
        };
        let model = display.monitors();
        let app_config = self.config.borrow().clone();
        let shell_config = app_config.shell.clone();
        let mut available = HashSet::new();

        for index in 0..model.n_items() {
            let Some(monitor) = model.item(index).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            let Some(connector) = monitor.connector().map(|value| value.to_string()) else {
                warn!("ignoring output without a connector name");
                continue;
            };
            available.insert(connector.clone());
            if !shell_config.shows_on(&connector) || self.islands.borrow().contains_key(&connector)
            {
                continue;
            }

            let weak = Rc::downgrade(self);
            let switch_workspace = Rc::new(move |monitor: &str, workspace: i64| {
                let monitor = monitor.to_owned();
                let dispatch_monitor = monitor.clone();
                thread::spawn(move || {
                    if let Err(error) = hyprland::switch_workspace(&dispatch_monitor, workspace) {
                        warn!("failed to switch workspace: {error:#}");
                    }
                });
                if let Some(controller) = weak.upgrade() {
                    let percent = workspace.clamp(0, 100) as u8;
                    if let Some(island) = controller.islands.borrow().get(&monitor) {
                        island.show_osd(OsdState {
                            kind: OsdKind::Workspace,
                            value: percent,
                            muted: false,
                            timeout_ms: 900,
                        });
                    }
                }
            });

            let weak = Rc::downgrade(self);
            let set_volume = Rc::new(move |value: u8| {
                if let Some(controller) = weak.upgrade() {
                    controller.pending_volume.set(Some(value));
                    let weak = Rc::downgrade(&controller);
                    glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
                        if let Some(controller) = weak.upgrade()
                            && controller.pending_volume.get() == Some(value)
                        {
                            controller.pending_volume.set(None);
                        }
                    });
                }
                thread::spawn(move || {
                    if let Err(error) = system::set_volume(value) {
                        warn!("failed to set volume: {error:#}");
                    }
                });
            });
            let set_brightness = Rc::new(move |value: u8| {
                thread::spawn(move || {
                    if let Err(error) = system::set_brightness(value) {
                        warn!("failed to set brightness: {error:#}");
                    }
                });
            });
            let tarragon_sender = self.tarragon_sender.clone();
            let search = Rc::new(move |text: String| {
                let _ = tarragon_sender.try_send(TarragonCommand::Query(text));
            });
            let tarragon_sender = self.tarragon_sender.clone();
            let select = Rc::new(move |selection| {
                let _ = tarragon_sender.try_send(TarragonCommand::Select(selection));
            });
            let tarragon_sender = self.tarragon_sender.clone();
            let tarragon_status = Rc::new(move || {
                let _ = tarragon_sender.try_send(TarragonCommand::Status);
            });
            let tarragon_sender = self.tarragon_sender.clone();
            let tarragon_reload = Rc::new(move || {
                let _ = tarragon_sender.try_send(TarragonCommand::Reload);
            });
            let preview_sender = self.preview_sender.clone();
            let preview_monitor = connector.clone();
            let load_preview = Rc::new(move |generation: u64, path: String| {
                let _ = preview_sender.try_send(PreviewRequest {
                    monitor: preview_monitor.clone(),
                    generation,
                    path: PathBuf::from(path),
                });
            });
            let media_play_pause = Rc::new(move |service: String| {
                thread::spawn(move || {
                    if let Err(error) = media::play_pause(&service) {
                        warn!("failed to toggle media playback: {error:#}");
                    }
                });
            });
            let media_next = Rc::new(move |service: String| {
                thread::spawn(move || {
                    if let Err(error) = media::next(&service) {
                        warn!("failed to skip to the next track: {error:#}");
                    }
                });
            });
            let media_previous = Rc::new(move |service: String| {
                thread::spawn(move || {
                    if let Err(error) = media::previous(&service) {
                        warn!("failed to skip to the previous track: {error:#}");
                    }
                });
            });
            let notification_command_sender = self.notification_command_sender.clone();
            let notification_expired = Rc::new(move |id: u32| {
                let _ = notification_command_sender.try_send(NotificationCommand::Close {
                    id,
                    reason: CloseReason::Expired,
                });
            });
            let weak = Rc::downgrade(self);
            let notification_dismiss = Rc::new(move |id: u32| {
                if let Some(controller) = weak.upgrade() {
                    controller.dismiss_notification(id);
                }
            });
            let weak = Rc::downgrade(self);
            let notification_invoke = Rc::new(move |id: u32, action_key: String| {
                if let Some(controller) = weak.upgrade() {
                    controller.invoke_notification_action(id, action_key);
                }
            });
            let island = IslandWindow::new(
                &self.application,
                &monitor,
                connector.clone(),
                &app_config,
                IslandActions {
                    switch_workspace,
                    set_volume,
                    set_brightness,
                    search,
                    select,
                    tarragon_status,
                    tarragon_reload,
                    load_preview,
                    media_play_pause,
                    media_next,
                    media_previous,
                    notification_expired,
                    notification_dismiss,
                    notification_invoke,
                },
                self.animations,
            );
            island.update_hyprland(&self.hyprland.borrow());
            island.update_system(&self.system.borrow());
            island.update_media(self.media.borrow().as_ref());
            island.update_weather(self.weather.borrow().as_ref());
            island.update_visualizer(*self.visualizer.borrow());
            island.update_palette();
            island.update_tarragon_connection(self.tarragon_connected.get(), None);
            if let Some(snapshot) = self.tarragon_snapshot.borrow().as_ref() {
                island.update_tarragon_results(snapshot);
            }
            if let Some(status) = self.tarragon_status.borrow().as_ref() {
                island.update_tarragon_status(status);
            }
            island.update_notification_history(self.notifications.borrow().as_slice());
            self.islands.borrow_mut().insert(connector, island);
        }

        let to_remove: Vec<_> = self
            .islands
            .borrow()
            .keys()
            .filter(|name| !available.contains(*name) || !shell_config.shows_on(name))
            .cloned()
            .collect();
        for name in to_remove {
            if let Some(island) = self.islands.borrow_mut().remove(&name) {
                island.destroy();
            }
        }
        for requested in shell_config
            .monitors
            .iter()
            .filter(|name| name.as_str() != "*")
        {
            if !available.contains(requested) {
                warn!("configured monitor `{requested}` is not connected");
            }
        }
        for island in self.islands.borrow().values() {
            island.update_shell_config(&shell_config, self.animations);
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        // Deliberately does not unlock a locked session. Fail-locked is the
        // entire point of ext-session-lock-v1: dropping the `LockSession`
        // here (its `Drop` only unregisters our own timers/watchers, never
        // calls `Instance::unlock`) leaves the compositor still blanking
        // every output. A daemon restart while locked therefore cannot
        // unlock the screen; recovery is `mithshell unlock` from another
        // session, or whatever secure recovery the compositor itself
        // offers. See the "Lock screen" section of the README.
        self.tarragon_sender.close();
        self.preview_sender.close();
        self.auth_sender.close();
        if let Some(listener) = self.tarragon_listener.take() {
            let _ = listener.join();
        }
        if let Some(listener) = self.preview_listener.take() {
            let _ = listener.join();
        }
        if let Some(listener) = self.auth_listener.take() {
            let _ = listener.join();
        }
    }
}
