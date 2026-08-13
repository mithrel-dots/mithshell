use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use log::{debug, warn};
use serde::{Deserialize, Serialize};

use crate::config::{ThemeMode, ThemeSource};

pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Request {
    pub version: u8,
    pub command: IpcCommand,
}

impl Request {
    pub fn new(command: IpcCommand) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            command,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum IpcCommand {
    Toggle {
        monitor: MonitorTarget,
    },
    Open {
        monitor: MonitorTarget,
    },
    Search {
        monitor: MonitorTarget,
    },
    Weather {
        monitor: MonitorTarget,
    },
    Close {
        monitor: MonitorTarget,
    },
    Osd {
        monitor: MonitorTarget,
        kind: OsdKind,
        value: Option<u8>,
        timeout_ms: u64,
    },
    Lock,
    Unlock,
    Reload,
    Status,
    Latency {
        reset: bool,
    },
    ThemeSet {
        source: ThemeSource,
        mode: Option<ThemeMode>,
        persist: bool,
    },
    ThemeMode {
        mode: ThemeMode,
        persist: bool,
    },
    ThemeCurrent,
    ThemeReset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorTarget {
    Focused,
    All,
    Named(String),
}

impl MonitorTarget {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "focused" => Ok(Self::Focused),
            "all" => Ok(Self::All),
            value if value.trim().is_empty() => bail!("monitor target cannot be empty"),
            value => Ok(Self::Named(value.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OsdKind {
    Volume,
    Brightness,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct IncomingRequest {
    pub request: Request,
    pub respond_to: mpsc::Sender<Response>,
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            ok: true,
            message: message.into(),
            data: Some(data),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            data: None,
        }
    }
}

pub fn send(socket_path: &Path, request: &Request) -> Result<Response> {
    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .context("failed to set IPC timeout")?;

    serde_json::to_writer(&mut stream, request).context("failed to encode IPC request")?;
    stream
        .write_all(b"\n")
        .context("failed to send IPC request")?;
    stream.flush().context("failed to flush IPC request")?;

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("failed to read IPC response")?;
    if line.is_empty() {
        bail!("daemon closed the IPC connection without a response");
    }
    serde_json::from_str(&line).context("failed to decode IPC response")
}

pub fn start_server(
    socket_path: PathBuf,
    sender: Sender<IncomingRequest>,
) -> Result<thread::JoinHandle<()>> {
    prepare_runtime_socket(&socket_path)?;
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind {}", socket_path.display()))?;
    protect_socket(&socket_path)?;

    Ok(thread::spawn(move || {
        debug!("listening for shell IPC on {}", socket_path.display());
        for connection in listener.incoming() {
            let stream = match connection {
                Ok(stream) => stream,
                Err(error) => {
                    warn!("failed to accept shell IPC connection: {error}");
                    continue;
                }
            };
            if let Err(error) = handle_connection(stream, &sender) {
                warn!("shell IPC request failed: {error:#}");
            }
        }
    }))
}

fn handle_connection(mut stream: UnixStream, sender: &Sender<IncomingRequest>) -> Result<()> {
    verify_peer_is_this_user(&stream).context("rejected IPC connection")?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .context("failed to set IPC read timeout")?;
    let mut line = String::new();
    BufReader::new(stream.try_clone()?)
        .read_line(&mut line)
        .context("failed to read IPC request")?;
    let request: Request = serde_json::from_str(&line).context("failed to decode IPC request")?;
    if request.version != PROTOCOL_VERSION {
        let response = Response::error(format!(
            "unsupported protocol version {}; expected {PROTOCOL_VERSION}",
            request.version
        ));
        serde_json::to_writer(&mut stream, &response)?;
        stream.write_all(b"\n")?;
        return Ok(());
    }

    let (response_sender, response_receiver) = mpsc::channel();
    sender
        .send_blocking(IncomingRequest {
            request,
            respond_to: response_sender,
        })
        .context("GTK command loop is not running")?;
    let response = response_receiver
        .recv_timeout(Duration::from_secs(4))
        .context("GTK command loop did not answer")?;
    serde_json::to_writer(&mut stream, &response).context("failed to encode IPC response")?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

pub fn prepare_runtime_socket(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("IPC socket path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to protect {}", parent.display()))?;

    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => bail!("another mithshell daemon is already running"),
            Err(_) => fs::remove_file(path)
                .with_context(|| format!("failed to remove stale socket {}", path.display()))?,
        }
    }
    Ok(())
}

/// Rejects a connection that did not come from this process's own uid.
///
/// The socket is already restricted to this user by directory (`0700`) and
/// file (`0600`) permissions set up in [`prepare_runtime_socket`] and
/// [`protect_socket`], so in the common case this is redundant. It matters
/// for the commands that bypass authentication -- most notably
/// [`IpcCommand::Unlock`] -- where "redundant" is exactly the property a
/// security boundary should have: a bug in one layer (a misconfigured
/// runtime dir, an unexpected setuid path) should not by itself be enough
/// to unlock the session.
fn verify_peer_is_this_user(stream: &UnixStream) -> Result<()> {
    use std::os::unix::io::AsRawFd;

    // SAFETY: `stream.as_raw_fd()` is a valid, open socket for the
    // lifetime of this call, `ucred` is a plain-old-data struct with no
    // invalid bit patterns, and `len` starts at its size as required by
    // `getsockopt(2)`.
    let credentials = unsafe {
        let mut ucred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::from_mut(&mut ucred).cast::<libc::c_void>(),
            &mut len,
        );
        if result != 0 {
            bail!("SO_PEERCRED failed: {}", std::io::Error::last_os_error());
        }
        ucred
    };

    let our_uid = unsafe { libc::geteuid() };
    if credentials.uid != our_uid {
        bail!(
            "peer uid {} does not match daemon uid {our_uid}",
            credentials.uid
        );
    }
    Ok(())
}

pub fn protect_socket(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to protect {}", path.display()))
}

pub fn socket_path(override_path: Option<PathBuf>) -> Result<PathBuf> {
    override_path.map_or_else(crate::config::default_socket_path, Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_as_json() {
        let request = Request::new(IpcCommand::Osd {
            monitor: MonitorTarget::Named("DP-2".into()),
            kind: OsdKind::Volume,
            value: Some(72),
            timeout_ms: 900,
        });
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn lock_and_unlock_round_trip_as_json() {
        for command in [IpcCommand::Lock, IpcCommand::Unlock] {
            let request = Request::new(command.clone());
            let encoded = serde_json::to_string(&request).unwrap();
            let decoded: Request = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn accepts_a_connection_from_its_own_uid() {
        let (a, b) = UnixStream::pair().unwrap();
        drop(b);
        assert!(verify_peer_is_this_user(&a).is_ok());
    }

    #[test]
    fn parses_monitor_targets() {
        assert_eq!(MonitorTarget::parse("all").unwrap(), MonitorTarget::All);
        assert_eq!(
            MonitorTarget::parse("DP-1").unwrap(),
            MonitorTarget::Named("DP-1".into())
        );
    }
}
