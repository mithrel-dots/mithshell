# mithshell

Minimal Quickshell MVP built around a single idle top pill that morphs downward into a dark glass dashboard.

## Run

```sh
qs -p /home/mithrel/dev/mithshell
```

## Controls

Click the pill to toggle the dashboard. Right-click the splotch to close it.

IPC entrypoints:

```sh
quickshell ipc call mithshell toggleDashboard
quickshell ipc call mithshell openDashboard
quickshell ipc call mithshell closeDashboard
```

## Matugen

The shell watches `~/.config/matugen/colors.json` and falls back to a cool accent if it does not exist. It accepts common keys such as `colors.primary`, `colors.on_surface`, `primary`, and `on_surface`, so a generated template can be pointed there without changing QML.
