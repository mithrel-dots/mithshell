use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use log::{debug, warn};

use crate::state::{ActiveWindow, HyprlandClient, HyprlandMonitor, HyprlandSnapshot, Workspace};

#[derive(Debug)]
pub enum HyprlandUpdate {
    Snapshot(HyprlandSnapshot),
    Unavailable(String),
}

pub fn snapshot() -> Result<HyprlandSnapshot> {
    let mut monitors: Vec<HyprlandMonitor> = query_json("monitors")?;
    let workspaces: Vec<Workspace> = query_json("workspaces")?;
    let clients: Vec<HyprlandClient> = query_json("clients")?;
    let active_window_value: serde_json::Value = query_json("activewindow")?;
    let active_window = if active_window_value
        .as_object()
        .is_none_or(serde_json::Map::is_empty)
    {
        None
    } else {
        Some(
            serde_json::from_value::<ActiveWindow>(active_window_value)
                .context("failed to decode active Hyprland window")?,
        )
    };

    for monitor in &mut monitors {
        monitor.fullscreen = monitor_has_fullscreen_client(monitor, &clients);
    }

    Ok(HyprlandSnapshot {
        monitors,
        workspaces,
        active_window,
    })
}

fn monitor_has_fullscreen_client(monitor: &HyprlandMonitor, clients: &[HyprlandClient]) -> bool {
    clients.iter().any(|client| {
        let workspace_visible = client.workspace.id == monitor.active_workspace.id
            || (monitor.special_workspace.id != 0
                && client.workspace.id == monitor.special_workspace.id);
        client.monitor == monitor.id && workspace_visible && client.fullscreen & 0b10 != 0
    })
}

pub fn start_listener(sender: Sender<HyprlandUpdate>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match snapshot() {
                Ok(snapshot) => {
                    let _ = sender.send_blocking(HyprlandUpdate::Snapshot(snapshot));
                }
                Err(error) => {
                    let _ = sender.send_blocking(HyprlandUpdate::Unavailable(error.to_string()));
                }
            }

            let event_path = match socket_path(".socket2.sock") {
                Ok(path) => path,
                Err(error) => {
                    let _ = sender.send_blocking(HyprlandUpdate::Unavailable(error.to_string()));
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };

            let stream = match UnixStream::connect(&event_path) {
                Ok(stream) => stream,
                Err(error) => {
                    warn!("failed to connect to {}: {error}", event_path.display());
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };

            debug!("listening for Hyprland events on {}", event_path.display());
            let mut lines = BufReader::new(stream).lines();
            while let Some(Ok(line)) = lines.next() {
                if !refreshing_event(&line) {
                    continue;
                }
                match snapshot() {
                    Ok(snapshot) => {
                        if sender
                            .send_blocking(HyprlandUpdate::Snapshot(snapshot))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ =
                            sender.send_blocking(HyprlandUpdate::Unavailable(error.to_string()));
                    }
                }
            }

            warn!("Hyprland event socket disconnected; reconnecting");
            thread::sleep(Duration::from_secs(1));
        }
    })
}

pub fn switch_workspace(monitor: &str, workspace: i64) -> Result<()> {
    dispatch(
        &format!("hl.dsp.focus({{ monitor = {} }})", lua_string(monitor)),
        &format!("focusmonitor {monitor}"),
    )?;
    dispatch(
        &format!("hl.dsp.focus({{ workspace = '{workspace}' }})"),
        &format!("workspace {workspace}"),
    )?;
    Ok(())
}

fn dispatch(lua: &str, legacy: &str) -> Result<()> {
    if request(&format!("dispatch {lua}")).is_ok() {
        return Ok(());
    }
    request(&format!("dispatch {legacy}")).map(|_| ())
}

fn lua_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn query_json<T: serde::de::DeserializeOwned>(command: &str) -> Result<T> {
    let response = request(&format!("j/{command}"))?;
    serde_json::from_str(&response)
        .with_context(|| format!("failed to decode Hyprland `{command}` response"))
}

fn request(request: &str) -> Result<String> {
    let path = socket_path(".socket.sock")?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("failed to connect to {}", path.display()))?;
    stream
        .write_all(request.as_bytes())
        .with_context(|| format!("failed to write Hyprland request `{request}`"))?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish Hyprland request")?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("failed to read Hyprland response")?;
    if response.starts_with("error") {
        bail!("Hyprland rejected `{request}`: {response}");
    }
    Ok(response)
}

fn socket_path(socket_name: &str) -> Result<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is not set")?;
    let signature = env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE is not set")?;
    Ok(PathBuf::from(runtime)
        .join("hypr")
        .join(signature)
        .join(socket_name))
}

fn refreshing_event(line: &str) -> bool {
    const EVENTS: &[&str] = &[
        "workspace>>",
        "workspacev2>>",
        "focusedmon>>",
        "focusedmonv2>>",
        "activespecial>>",
        "activewindow>>",
        "activewindowv2>>",
        "openwindow>>",
        "closewindow>>",
        "movewindow>>",
        "movewindowv2>>",
        "fullscreen>>",
        "createworkspace>>",
        "destroyworkspace>>",
        "configreloaded>>",
    ];
    EVENTS.iter().any(|event| line.starts_with(event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_pointer_and_layout_events() {
        assert!(refreshing_event("workspacev2>>3,3"));
        assert!(refreshing_event("activewindow>>kitty,shell"));
        assert!(refreshing_event("activespecial>>special:magic,DP-1"));
        assert!(refreshing_event("fullscreen>>1"));
        assert!(!refreshing_event("mousemove>>300,200"));
    }

    #[test]
    fn detects_only_the_fullscreen_bit_on_visible_workspaces() {
        let monitor = HyprlandMonitor {
            id: 1,
            name: "DP-1".into(),
            focused: true,
            active_workspace: crate::state::WorkspaceRef {
                id: 4,
                name: "4".into(),
            },
            special_workspace: crate::state::WorkspaceRef {
                id: -99,
                name: "special:magic".into(),
            },
            fullscreen: false,
        };
        let client = |workspace, fullscreen| HyprlandClient {
            monitor: 1,
            workspace: crate::state::WorkspaceRef {
                id: workspace,
                name: workspace.to_string(),
            },
            fullscreen,
        };

        assert!(!monitor_has_fullscreen_client(&monitor, &[client(4, 0)]));
        assert!(!monitor_has_fullscreen_client(&monitor, &[client(4, 1)]));
        assert!(monitor_has_fullscreen_client(&monitor, &[client(4, 2)]));
        assert!(monitor_has_fullscreen_client(&monitor, &[client(4, 3)]));
        assert!(monitor_has_fullscreen_client(&monitor, &[client(-99, 2)]));
        assert!(!monitor_has_fullscreen_client(&monitor, &[client(8, 2)]));
    }

    #[test]
    fn escapes_lua_dispatch_strings() {
        assert_eq!(lua_string("DP-1"), "'DP-1'");
        assert_eq!(lua_string("odd\\'name"), "'odd\\\\\\'name'");
    }
}
