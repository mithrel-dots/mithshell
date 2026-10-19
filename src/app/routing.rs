//! Shared monitor selection for notifications and system feedback.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use log::warn;

use crate::{
    config::{FullscreenStrategy, NotificationConfig},
    ipc::MonitorTarget,
    state::{HyprlandSnapshot, Urgency},
};

pub(super) fn target_names<T>(
    snapshot: &HyprlandSnapshot,
    islands: &HashMap<String, T>,
    target: &MonitorTarget,
    routing: Option<(&NotificationConfig, Urgency)>,
) -> Result<Vec<String>> {
    let source = match target {
        MonitorTarget::Focused => snapshot.focused_monitor(),
        MonitorTarget::Named(name) => snapshot.monitor(name),
        MonitorTarget::All => None,
    };
    let mut names = match target {
        MonitorTarget::All => islands.keys().cloned().collect(),
        MonitorTarget::Named(name) => vec![name.clone()],
        MonitorTarget::Focused => vec![
            source
                .map(|monitor| monitor.name.clone())
                .or_else(|| islands.keys().next().cloned())
                .context("no island output is available")?,
        ],
    };

    // A named target can be the source of automatic workspace feedback. IPC
    // connector/all overrides pass no routing policy and stay exact.
    if let Some((config, urgency)) = routing
        && let Some(source) = source
        && source.fullscreen
        && config.fullscreen_strategy != FullscreenStrategy::Ignore
        && !config.overlay_applies(urgency, true)
    {
        let mut usable: Vec<_> = snapshot
            .monitors
            .iter()
            .filter(|monitor| !monitor.fullscreen && islands.contains_key(&monitor.name))
            .collect();
        usable.sort_by_key(|monitor| monitor.id);

        let fallback: Vec<String> = match config.fullscreen_strategy {
            FullscreenStrategy::Fallback => config
                .fallback_monitors
                .iter()
                .find(|name| usable.iter().any(|monitor| monitor.name == **name))
                .cloned()
                .or_else(|| usable.first().map(|monitor| monitor.name.clone()))
                .into_iter()
                .collect(),
            FullscreenStrategy::AllNonFullscreen => usable
                .into_iter()
                .map(|monitor| monitor.name.clone())
                .collect(),
            FullscreenStrategy::Ignore => unreachable!("handled above"),
        };
        if fallback.is_empty() {
            warn!(
                "no non-fullscreen popup output is available; using monitor `{}`",
                source.name
            );
        } else {
            names = fallback;
        }
    }

    names.retain(|name| islands.contains_key(name));
    if names.is_empty() {
        bail!("no configured island matches target {target:?}");
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::NotificationOverlay,
        state::{HyprlandMonitor, WorkspaceRef},
    };

    fn snapshot() -> HyprlandSnapshot {
        HyprlandSnapshot {
            // Deliberately out of monitor-id order.
            monitors: [(3, "DP-3", false), (2, "DP-2", false), (1, "DP-1", true)]
                .into_iter()
                .map(|(id, name, fullscreen)| HyprlandMonitor {
                    id,
                    name: name.into(),
                    focused: id == 1,
                    active_workspace: WorkspaceRef::default(),
                    special_workspace: WorkspaceRef::default(),
                    fullscreen,
                })
                .collect(),
            ..HyprlandSnapshot::default()
        }
    }

    fn islands() -> HashMap<String, ()> {
        ["DP-1", "DP-2", "DP-3"]
            .map(|name| (name.to_owned(), ()))
            .into()
    }

    #[test]
    fn fallback_uses_preferences_then_monitor_id_and_only_configured_outputs() {
        let snapshot = snapshot();
        let mut islands = islands();
        let mut config = NotificationConfig::default();
        let route = |islands: &HashMap<String, ()>, config: &NotificationConfig| {
            target_names(
                &snapshot,
                islands,
                &MonitorTarget::Focused,
                Some((config, Urgency::Normal)),
            )
            .unwrap()
        };
        assert_eq!(route(&islands, &config), ["DP-2"]);
        config.fallback_monitors = vec!["disconnected".into(), "DP-1".into(), "DP-3".into()];
        assert_eq!(route(&islands, &config), ["DP-3"]);
        islands.remove("DP-3");
        assert_eq!(route(&islands, &config), ["DP-2"]);
        islands.remove("DP-2");
        assert_eq!(route(&islands, &config), ["DP-1"]);
        islands.remove("DP-1");
        assert!(
            target_names(
                &snapshot,
                &islands,
                &MonitorTarget::Focused,
                Some((&config, Urgency::Normal)),
            )
            .is_err()
        );
    }

    #[test]
    fn fullscreen_strategies_and_overlay_threshold_share_the_same_routing() {
        let mut snapshot = snapshot();
        let islands = islands();
        let mut config = NotificationConfig {
            fullscreen_strategy: FullscreenStrategy::AllNonFullscreen,
            ..NotificationConfig::default()
        };
        let route = |snapshot: &HyprlandSnapshot, config: &NotificationConfig, urgency| {
            target_names(
                snapshot,
                &islands,
                &MonitorTarget::Focused,
                Some((config, urgency)),
            )
            .unwrap()
        };
        assert_eq!(route(&snapshot, &config, Urgency::Normal), ["DP-2", "DP-3"]);
        config.overlay_over_fullscreen = NotificationOverlay::Critical;
        assert_eq!(route(&snapshot, &config, Urgency::Normal), ["DP-2", "DP-3"]);
        assert_eq!(route(&snapshot, &config, Urgency::Critical), ["DP-1"]);
        config.overlay_over_fullscreen = NotificationOverlay::Normal;
        assert_eq!(route(&snapshot, &config, Urgency::Normal), ["DP-1"]);
        config.overlay_over_fullscreen = NotificationOverlay::Off;
        config.fullscreen_strategy = FullscreenStrategy::Ignore;
        assert_eq!(route(&snapshot, &config, Urgency::Normal), ["DP-1"]);
        config.fullscreen_strategy = FullscreenStrategy::Fallback;
        snapshot
            .monitors
            .iter_mut()
            .for_each(|monitor| monitor.fullscreen = true);
        assert_eq!(route(&snapshot, &config, Urgency::Normal), ["DP-1"]);
        snapshot
            .monitors
            .iter_mut()
            .for_each(|monitor| monitor.fullscreen = false);
        assert_eq!(route(&snapshot, &config, Urgency::Normal), ["DP-1"]);
    }

    #[test]
    fn explicit_targets_stay_exact_and_automatic_workspace_feedback_can_reroute() {
        let snapshot = snapshot();
        let islands = islands();
        let config = NotificationConfig::default();
        let source = MonitorTarget::Named("DP-1".into());
        assert_eq!(
            target_names(&snapshot, &islands, &source, None).unwrap(),
            ["DP-1"]
        );
        assert_eq!(
            target_names(
                &snapshot,
                &islands,
                &source,
                Some((&config, Urgency::Normal))
            )
            .unwrap(),
            ["DP-2"]
        );
        assert_eq!(
            target_names(&snapshot, &islands, &MonitorTarget::All, None)
                .unwrap()
                .len(),
            3
        );
        assert!(
            target_names(
                &snapshot,
                &islands,
                &MonitorTarget::Named("missing".into()),
                None,
            )
            .is_err()
        );
        assert_eq!(
            target_names(&snapshot, &islands, &MonitorTarget::Focused, None).unwrap(),
            ["DP-1"]
        );
        let only_island = HashMap::from([("DP-2".to_owned(), ())]);
        assert_eq!(
            target_names(
                &HyprlandSnapshot::default(),
                &only_island,
                &MonitorTarget::Focused,
                Some((&config, Urgency::Normal))
            )
            .unwrap(),
            ["DP-2"]
        );
    }
}
