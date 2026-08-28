use std::{
    collections::{HashMap, HashSet},
    env,
    ffi::OsString,
    io::{BufRead, BufReader, ErrorKind, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

const CLIENT_ID: &str = "mithshell";
const RETRY_DELAY: Duration = Duration::from_secs(1);
const ENV_SOCKET_OVERRIDE: &str = "TARRAGON_UI_SOCKET";
const ENV_XDG_RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";

/// Resolves and caches the TarraGon UI socket path for the lifetime of the
/// process.
///
/// TarraGon's own daemon-side implementation resolves its listening path
/// with the exact same algorithm, so this must not drift from
/// [`resolve_ui_socket_path`] independently.
fn ui_socket_path() -> &'static Path {
    static SOCKET_PATH: OnceLock<PathBuf> = OnceLock::new();
    SOCKET_PATH.get_or_init(resolve_ui_socket_path)
}

/// Reads the environment once and delegates to the pure, testable
/// [`resolve_ui_socket_path_from`].
fn resolve_ui_socket_path() -> PathBuf {
    // SAFETY: `geteuid()` is a pure syscall with no invalid return value;
    // this mirrors the same call already made in `ipc::verify_peer_is_this_user`.
    let euid = unsafe { libc::geteuid() };
    resolve_ui_socket_path_from(
        env::var_os(ENV_SOCKET_OVERRIDE),
        env::var_os(ENV_XDG_RUNTIME_DIR),
        euid,
    )
}

/// The resolution order, in priority:
///
/// 1. `TARRAGON_UI_SOCKET`, used verbatim, when set and non-empty.
/// 2. `$XDG_RUNTIME_DIR/tarragon/ui.sock`, when `XDG_RUNTIME_DIR` is set and
///    non-empty.
/// 3. `/tmp/tarragon-{euid}/ui.sock`, the effective UID formatted as a
///    decimal string.
fn resolve_ui_socket_path_from(
    socket_override: Option<OsString>,
    xdg_runtime_dir: Option<OsString>,
    euid: u32,
) -> PathBuf {
    if let Some(path) = socket_override.filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(dir) = xdg_runtime_dir.filter(|value| !value.is_empty()) {
        return PathBuf::from(dir).join("tarragon").join("ui.sock");
    }
    PathBuf::from(format!("/tmp/tarragon-{euid}/ui.sock"))
}

#[derive(Debug, Clone)]
pub enum TarragonCommand {
    Query(String),
    Select(TarragonSelection),
    Status,
    Reload,
}

#[derive(Debug, Clone)]
pub enum TarragonEvent {
    Connection {
        connected: bool,
        message: Option<String>,
    },
    Results(TarragonSnapshot),
    Status(TarragonStatus),
    Reload {
        success: bool,
        message: String,
    },
    Error(String),
    Selection {
        success: bool,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct TarragonSelection {
    pub query_id: String,
    pub plugin: String,
    pub result_id: String,
    pub action: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TarragonSnapshot {
    pub query_id: String,
    pub input: String,
    #[serde(default)]
    pub started_at_unix_ms: i64,
    #[serde(default)]
    pub plugins: HashMap<String, TarragonPluginState>,
    #[serde(default)]
    pub list: Vec<TarragonResult>,
}

impl TarragonSnapshot {
    pub fn pending(&self) -> bool {
        self.plugins
            .values()
            .any(|plugin| plugin.state == "pending")
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TarragonPluginState {
    pub state: String,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub elapsed_ms: f64,
    #[serde(default)]
    pub error: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TarragonResult {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub preview_path: String,
    pub plugin: String,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub frecency_score: f64,
    #[serde(default)]
    pub actions: Vec<TarragonAction>,
}

impl TarragonResult {
    pub fn default_action(&self) -> Option<&TarragonAction> {
        self.actions
            .iter()
            .find(|action| action.default)
            .or_else(|| self.actions.first())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TarragonAction {
    pub name: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "type")]
    pub action_type: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TarragonStatus {
    #[serde(default)]
    pub connected: Vec<String>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub plugins: Vec<TarragonPlugin>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TarragonPlugin {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: String,
    pub enabled: bool,
    pub connected: bool,
    pub lifecycle: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub require_prefix: bool,
    #[serde(default)]
    pub provides_general_suggestions: bool,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub icon: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request<'a> {
    Query {
        client_id: &'a str,
        text: &'a str,
    },
    Select {
        client_id: &'a str,
        query_id: &'a str,
        plugin: &'a str,
        result_id: &'a str,
        action: &'a str,
    },
    Status {
        client_id: &'a str,
    },
    Reload {
        client_id: &'a str,
    },
    Detach {
        client_id: &'a str,
    },
}

/// The socket shared between the writer thread and the connection thread.
///
/// Holding the write half here lets the writer thread block on the command
/// channel and push a query the instant it is queued, instead of waiting for
/// the connection thread's next read to finish.
#[derive(Default)]
struct Connection {
    stream: Mutex<Option<UnixStream>>,
    pending_selection: AtomicBool,
}

impl Connection {
    fn attach(&self, stream: UnixStream) {
        *self.lock() = Some(stream);
    }

    fn detach(&self) {
        *self.lock() = None;
    }

    fn lock(&self) -> MutexGuard<'_, Option<UnixStream>> {
        self.stream.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes a command when connected, dropping the socket if the write fails
    /// so the connection thread can reconnect.
    fn send(&self, command: &TarragonCommand) {
        let mut guard = self.lock();
        let Some(stream) = guard.as_mut() else {
            return;
        };
        if write_command(stream, command).is_err() {
            *guard = None;
            return;
        }
        if matches!(command, TarragonCommand::Query(_)) {
            crate::latency::mark_write();
        }
        if matches!(command, TarragonCommand::Select(_)) {
            self.pending_selection.store(true, Ordering::Relaxed);
        }
    }

    fn take_pending_selection(&self) -> bool {
        self.pending_selection.swap(false, Ordering::Relaxed)
    }
}

pub fn start_listener(
    event_sender: Sender<TarragonEvent>,
) -> (Sender<TarragonCommand>, thread::JoinHandle<()>) {
    let (command_sender, command_receiver) = async_channel::unbounded();
    let connection = Arc::new(Connection::default());

    let writer_connection = Arc::clone(&connection);
    let writer_receiver = command_receiver.clone();
    thread::spawn(move || write_loop(&writer_receiver, &writer_connection));

    let handle = thread::spawn(move || run(&command_receiver, &connection, &event_sender));
    (command_sender, handle)
}

/// Blocks on the command channel so queries reach the socket without waiting
/// for a poll cycle.
fn write_loop(command_receiver: &Receiver<TarragonCommand>, connection: &Connection) {
    while let Ok(command) = command_receiver.recv_blocking() {
        connection.send(&command);
    }
    let mut guard = connection.lock();
    if let Some(stream) = guard.as_mut() {
        let _ = write_request(
            stream,
            &Request::Detach {
                client_id: CLIENT_ID,
            },
        );
    }
    *guard = None;
}

fn run(
    command_receiver: &Receiver<TarragonCommand>,
    connection: &Connection,
    event_sender: &Sender<TarragonEvent>,
) {
    let mut reported_disconnected = false;
    while !command_receiver.is_closed() {
        match UnixStream::connect(ui_socket_path()) {
            Ok(stream) => {
                reported_disconnected = false;
                let _ = event_sender.send_blocking(TarragonEvent::Connection {
                    connected: true,
                    message: None,
                });
                let outcome = run_connection(stream, connection, event_sender);
                connection.detach();
                if let Err(error) = outcome {
                    let _ = event_sender.send_blocking(TarragonEvent::Connection {
                        connected: false,
                        message: Some(error),
                    });
                    reported_disconnected = true;
                }
            }
            Err(error) => {
                if !reported_disconnected {
                    let _ = event_sender.send_blocking(TarragonEvent::Connection {
                        connected: false,
                        message: Some(format!("TarraGon unavailable: {error}")),
                    });
                    reported_disconnected = true;
                }
            }
        }
        thread::sleep(RETRY_DELAY);
    }
}

fn run_connection(
    stream: UnixStream,
    connection: &Connection,
    event_sender: &Sender<TarragonEvent>,
) -> Result<(), String> {
    let writer = stream.try_clone().map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut own_queries = HashSet::new();
    let mut line = String::new();

    connection.attach(writer);
    connection.send(&TarragonCommand::Status);

    // A blocking read: no timeout, so the thread parks instead of spinning.
    loop {
        match reader.read_line(&mut line) {
            Ok(0) => return Err("TarraGon closed the connection".into()),
            Ok(_) => {
                handle_message(&line, &mut own_queries, connection, event_sender);
                line.clear();
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("TarraGon read failed: {error}")),
        }
    }
}

fn write_command(writer: &mut UnixStream, command: &TarragonCommand) -> Result<(), String> {
    match command {
        TarragonCommand::Query(text) => write_request(
            writer,
            &Request::Query {
                client_id: CLIENT_ID,
                text,
            },
        ),
        TarragonCommand::Select(selection) => write_request(
            writer,
            &Request::Select {
                client_id: CLIENT_ID,
                query_id: &selection.query_id,
                plugin: &selection.plugin,
                result_id: &selection.result_id,
                action: &selection.action,
            },
        ),
        TarragonCommand::Status => write_request(
            writer,
            &Request::Status {
                client_id: CLIENT_ID,
            },
        ),
        TarragonCommand::Reload => write_request(
            writer,
            &Request::Reload {
                client_id: CLIENT_ID,
            },
        ),
    }
}

fn write_request(writer: &mut UnixStream, request: &Request<'_>) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, request)
        .map_err(|error| format!("cannot encode TarraGon request: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("cannot send TarraGon request: {error}"))
}

fn handle_message(
    line: &str,
    own_queries: &mut HashSet<String>,
    connection: &Connection,
    event_sender: &Sender<TarragonEvent>,
) {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    match message.get("type").and_then(|value| value.as_str()) {
        Some("ack") => {
            if let Some(query_id) = message.get("query_id").and_then(|value| value.as_str()) {
                own_queries.clear();
                own_queries.insert(query_id.to_owned());
            }
        }
        Some("update") => {
            let Some(query_id) = message.get("query_id").and_then(|value| value.as_str()) else {
                return;
            };
            if !own_queries.contains(query_id) {
                return;
            }
            let Some(payload) = message.get("payload").and_then(|value| value.as_str()) else {
                return;
            };
            let Ok(decoded) = STANDARD.decode(payload) else {
                return;
            };
            if let Ok(snapshot) = serde_json::from_slice::<TarragonSnapshot>(&decoded) {
                let _ = event_sender.send_blocking(TarragonEvent::Results(snapshot));
            }
        }
        Some("status") => {
            if let Ok(status) = serde_json::from_value::<TarragonStatus>(message) {
                let _ = event_sender.send_blocking(TarragonEvent::Status(status));
            }
        }
        Some("reload_response") => {
            let success = message
                .get("success")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let text = message
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("Reload failed");
            let _ = event_sender.send_blocking(TarragonEvent::Reload {
                success,
                message: text.to_owned(),
            });
        }
        Some("error") => {
            if let Some(error) = message.get("error").and_then(|value| value.as_str()) {
                let _ = event_sender.send_blocking(TarragonEvent::Error(error.to_owned()));
            }
        }
        Some("select_response") => {
            if !connection.take_pending_selection() {
                return;
            }
            let success = message
                .get("success")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let text = message
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or(if success {
                    "Action completed"
                } else {
                    "Action failed"
                });
            let _ = event_sender.send_blocking(TarragonEvent::Selection {
                success,
                message: text.to_owned(),
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_default_action_then_first_action() {
        let result = TarragonResult {
            id: "id".into(),
            label: "Label".into(),
            description: String::new(),
            icon: String::new(),
            category: String::new(),
            preview_path: String::new(),
            plugin: "plugin".into(),
            score: 0.0,
            frecency_score: 0.0,
            actions: vec![
                TarragonAction {
                    name: "secondary".into(),
                    default: false,
                    description: String::new(),
                    action_type: None,
                    query: None,
                },
                TarragonAction {
                    name: "open".into(),
                    default: true,
                    description: String::new(),
                    action_type: None,
                    query: None,
                },
            ],
        };
        assert_eq!(result.default_action().unwrap().name, "open");
    }

    #[test]
    fn parses_base64_aggregate_payload() {
        let payload = br#"{"query_id":"q-1","input":"term","started_at_unix_ms":42,"plugins":{"desktop_files":{"state":"done","count":1,"elapsed_ms":2.5}},"list":[{"id":"x","label":"Result","preview_path":"/tmp/x.png","plugin":"desktop_files","score":0.8,"frecency_score":0.2,"actions":[]}]}"#;
        let message = serde_json::json!({
            "type": "update",
            "query_id": "q-1",
            "payload": STANDARD.encode(payload),
        });
        let (sender, receiver) = async_channel::unbounded();
        let mut own = HashSet::from(["q-1".to_owned()]);
        handle_message(
            &message.to_string(),
            &mut own,
            &Connection::default(),
            &sender,
        );
        let TarragonEvent::Results(snapshot) = receiver.try_recv().unwrap() else {
            panic!("expected results event");
        };
        assert_eq!(snapshot.input, "term");
        assert_eq!(snapshot.started_at_unix_ms, 42);
        assert_eq!(snapshot.list[0].label, "Result");
        assert_eq!(snapshot.list[0].preview_path, "/tmp/x.png");
        assert_eq!(snapshot.plugins["desktop_files"].count, 1);
    }

    /// Selections are written by the separate writer thread, so the pending
    /// flag lives on the shared `Connection` rather than in the read loop. A
    /// response must be reported exactly once, and only when a selection is
    /// actually pending.
    #[test]
    fn select_response_is_reported_once_per_pending_selection() {
        let message = serde_json::json!({
            "type": "select_response",
            "success": true,
            "message": "Launched",
        })
        .to_string();
        let (sender, receiver) = async_channel::unbounded();
        let connection = Connection::default();

        // No selection in flight: the response is ignored.
        handle_message(&message, &mut HashSet::new(), &connection, &sender);
        assert!(receiver.try_recv().is_err());

        connection.pending_selection.store(true, Ordering::Relaxed);
        handle_message(&message, &mut HashSet::new(), &connection, &sender);
        let TarragonEvent::Selection { success, message } = receiver.try_recv().unwrap() else {
            panic!("expected selection event");
        };
        assert!(success);
        assert_eq!(message, "Launched");

        // The flag is consumed, so a duplicate response is not reported again.
        handle_message(
            &serde_json::json!({ "type": "select_response", "success": true }).to_string(),
            &mut HashSet::new(),
            &connection,
            &sender,
        );
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn socket_override_env_var_wins_verbatim() {
        let path = resolve_ui_socket_path_from(
            Some(OsString::from("/custom/path.sock")),
            Some(OsString::from("/run/user/1000")),
            1000,
        );
        assert_eq!(path, PathBuf::from("/custom/path.sock"));
    }

    #[test]
    fn empty_socket_override_falls_through_to_xdg_runtime_dir() {
        let path = resolve_ui_socket_path_from(
            Some(OsString::new()),
            Some(OsString::from("/run/user/1000")),
            1000,
        );
        assert_eq!(path, PathBuf::from("/run/user/1000/tarragon/ui.sock"));
    }

    #[test]
    fn falls_back_to_xdg_runtime_dir_when_unset() {
        let path = resolve_ui_socket_path_from(None, Some(OsString::from("/run/user/1000")), 1000);
        assert_eq!(path, PathBuf::from("/run/user/1000/tarragon/ui.sock"));
    }

    #[test]
    fn falls_back_to_tmp_with_euid_when_neither_is_set() {
        let path = resolve_ui_socket_path_from(None, None, 1000);
        assert_eq!(path, PathBuf::from("/tmp/tarragon-1000/ui.sock"));
    }

    #[test]
    fn empty_xdg_runtime_dir_falls_through_to_tmp_fallback() {
        let path = resolve_ui_socket_path_from(None, Some(OsString::new()), 1000);
        assert_eq!(path, PathBuf::from("/tmp/tarragon-1000/ui.sock"));
    }

    #[test]
    fn parses_plugin_inventory_status() {
        let message = serde_json::json!({
            "type": "status",
            "connected": ["desktop_files"],
            "total": 2,
            "plugins": [{
                "name": "desktop_files",
                "description": "Applications",
                "enabled": true,
                "connected": true,
                "lifecycle": "on_demand_persistent",
                "prefix": "@app",
                "capabilities": ["suggest", "icon"]
            }]
        });
        let (sender, receiver) = async_channel::unbounded();
        handle_message(
            &message.to_string(),
            &mut HashSet::new(),
            &Connection::default(),
            &sender,
        );
        let TarragonEvent::Status(status) = receiver.try_recv().unwrap() else {
            panic!("expected status event");
        };
        assert_eq!(status.total, 2);
        assert_eq!(status.plugins[0].prefix, "@app");
        assert_eq!(status.plugins[0].capabilities, ["suggest", "icon"]);
    }
}
