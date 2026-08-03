use serde::{Deserialize, Serialize};

use crate::{config::ThemeMode, ipc::OsdKind};

#[derive(Debug, Clone, Default, Serialize)]
pub struct HyprlandSnapshot {
    pub monitors: Vec<HyprlandMonitor>,
    pub workspaces: Vec<Workspace>,
    pub active_window: Option<ActiveWindow>,
}

impl HyprlandSnapshot {
    pub fn focused_monitor(&self) -> Option<&HyprlandMonitor> {
        self.monitors.iter().find(|monitor| monitor.focused)
    }

    pub fn monitor(&self, name: &str) -> Option<&HyprlandMonitor> {
        self.monitors.iter().find(|monitor| monitor.name == name)
    }

    pub fn workspaces_for(&self, monitor: &str) -> Vec<&Workspace> {
        let mut workspaces: Vec<_> = self
            .workspaces
            .iter()
            .filter(|workspace| workspace.monitor == monitor && workspace.id > 0)
            .collect();
        workspaces.sort_by_key(|workspace| workspace.id);
        workspaces
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HyprlandMonitor {
    pub id: i64,
    pub name: String,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceRef,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkspaceRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Workspace {
    pub id: i64,
    pub name: String,
    pub monitor: String,
    pub windows: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ActiveWindow {
    pub class: String,
    pub title: String,
    pub monitor: i64,
    pub workspace: WorkspaceRef,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SystemSnapshot {
    pub audio: Option<AudioState>,
    pub brightness: Option<BrightnessState>,
    pub battery: Option<BatteryState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AudioState {
    pub percent: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaState {
    pub player: String,
    /// Full MPRIS D-Bus service name (e.g. `org.mpris.MediaPlayer2.spotify`),
    /// kept around so play/pause/next/previous calls can target the right
    /// player. Not part of the public IPC surface.
    #[serde(skip_serializing)]
    pub service: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub app_icon: Option<String>,
    /// Track position at the moment this state was captured, in
    /// microseconds. Since `MediaState` is only ever produced for a
    /// `Playing` player, a progress bar can interpolate forward from this
    /// baseline using a local clock instead of polling MPRIS continuously.
    pub position_us: i64,
    pub length_us: Option<i64>,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrightnessState {
    pub percent: u8,
    #[serde(skip_serializing)]
    pub device: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryState {
    pub percent: u8,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Palette {
    pub source: String,
    pub mode: ThemeMode,
    pub primary: String,
    pub on_primary: String,
    pub primary_container: String,
    pub on_primary_container: String,
    pub secondary: String,
    pub tertiary: String,
    pub surface: String,
    pub surface_container_low: String,
    pub surface_container: String,
    pub surface_container_high: String,
    pub on_surface: String,
    pub on_surface_variant: String,
    pub outline: String,
    pub outline_variant: String,
    pub error: String,
}

impl Palette {
    pub fn css(&self) -> String {
        format!(
            r#"
@define-color ms_primary {primary};
@define-color ms_on_primary {on_primary};
@define-color ms_primary_container {primary_container};
@define-color ms_on_primary_container {on_primary_container};
@define-color ms_secondary {secondary};
@define-color ms_tertiary {tertiary};
@define-color ms_surface {surface};
@define-color ms_surface_low {surface_low};
@define-color ms_surface_container {surface_container};
@define-color ms_surface_high {surface_high};
@define-color ms_on_surface {on_surface};
@define-color ms_on_surface_variant {on_surface_variant};
@define-color ms_outline {outline};
@define-color ms_outline_variant {outline_variant};
@define-color ms_error {error};
"#,
            primary = self.primary,
            on_primary = self.on_primary,
            primary_container = self.primary_container,
            on_primary_container = self.on_primary_container,
            secondary = self.secondary,
            tertiary = self.tertiary,
            surface = self.surface,
            surface_low = self.surface_container_low,
            surface_container = self.surface_container,
            surface_high = self.surface_container_high,
            on_surface = self.on_surface,
            on_surface_variant = self.on_surface_variant,
            outline = self.outline,
            outline_variant = self.outline_variant,
            error = self.error,
        )
    }
}

#[derive(Debug, Clone)]
pub struct OsdState {
    pub kind: OsdKind,
    pub value: u8,
    pub muted: bool,
    pub timeout_ms: u64,
}
