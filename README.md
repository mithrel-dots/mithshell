# mithshell

`mithshell` is a small GTK4 layer-shell surface for Hyprland. It rests as a
top-center workspace pill and expands into a focused dashboard with system
controls. It includes workspaces, active window and media state, automatic
volume OSD feedback, brightness and battery state, Material You colors, and a
local IPC command line.

The visual structure takes inspiration from the projects under `reference/`,
while the implementation uses native Rust services rather than Quickshell.

## Requirements

On Arch Linux:

```sh
sudo pacman -S gtk4 gtk4-layer-shell pipewire pipewire-pulse wireplumber libpulse cava
```

`wpctl` from WirePlumber provides volume reads and writes. Mithshell listens to
`pactl subscribe` for immediate default-sink volume and mute changes, including
changes made outside Mithshell.

`cava` provides the media pill's live seven-band spectrum. If it is not
available, media metadata still works and the visualizer remains at rest.

`brightnessctl` is optional and only used by the example Hyprland keybinds.
The dashboard reads `/sys/class/backlight` directly and hides the control when
no backlight exists.

[TarraGon](https://github.com/iMithrellas/tarragon) is optional. When its user
service is running, Mithshell exposes its configured search plugins through an
embedded launcher.

`ffmpeg` and `ffmpegthumbnailer` are optional rich-preview dependencies. They
provide video metadata and cached thumbnails; image and highlighted text
previews work without them.

## Build

```sh
cargo build --release
sudo install -Dm755 target/release/mithshell /usr/bin/mithshell
```

With [`just`](https://github.com/casey/just), run `just` to list the project
recipes. The complete local user-service deployment is:

```sh
just run
```

This builds the release binary, symlinks it to `~/.local/bin/mithshell`, keeps
any existing config (or installs the example on first use), symlinks the
systemd user unit, and restarts `mithshell.service`. Useful development recipes
include `just check`, `just daemon`, `just search`, `just status`, and
`just service-logs`.

Start it from Hyprland:

```conf
exec-once = mithshell daemon
```

Alternatively, install `contrib/mithshell.service` as a systemd user service.
Do not use both startup methods.

## Configuration

The default configuration path is
`$XDG_CONFIG_HOME/mithshell/config.toml`, normally
`~/.config/mithshell/config.toml`. Missing configuration uses safe defaults.
Copy `config/mithshell.example.toml` as a starting point.

Monitor selection uses exact Wayland connector names from
`hyprctl monitors -j`:

```toml
[shell]
monitors = ["DP-1", "DP-2"]
top_margin = 6
exclusive_zone = 48
animation_ms = 280
scale = 0
```

`scale = 0` automatically scales the island up on wide, unscaled outputs. Set
an explicit value such as `1.25` or `1.45` to override it.

Use `monitors = ["*"]` to display one island on every connected output. Run
`mithshell reload` after changing the file. Unknown output names are logged and
do not silently fall back to another monitor.

The island uses the top layer in every view, matching Quickshell's default for
panels and keeping it visible across regular and special workspaces.

Media width is content-driven and capped as a multiple of compact width:

```toml
[media]
max_width_factor = 1.8
```

The factor is clamped to the available dashboard canvas. Short titles only
expand as far as needed; longer titles use the maximum and are ellipsized.

### Automatic OSD and media

Volume and mute changes are detected through PipeWire's PulseAudio-compatible
service. The focused monitor's pill becomes the volume OSD automatically and
returns to its previous state after the timeout. The example Hyprland volume
binds therefore only run `wpctl`; they do not invoke `mithshell osd`.

Mithshell discovers MPRIS players directly on the session D-Bus, without
requiring `playerctl`. While a player reports `Playing` and provides a title,
the compact pill keeps its workspace indicators and clock while adding the
player's desktop icon, title, and Cava's live PipeWire spectrum in the middle.
Paused or stopped players return the pill to its normal compact state, and
`playerctld` mirrors are ignored. MPRIS queries run on a dedicated GLib context
so an unresponsive player cannot block GTK rendering.

### TarraGon search

The dashboard's search button opens a larger keyboard-focused TarraGon frontend
around 20% down the monitor, clamped to remain on-screen. It includes the
complete ranked result list, path-based
image previews, Tree-sitter highlighted and scrollable source previews, cached
video thumbnails, file/media metadata, score and frecency metadata, every
result action, aggregate plugin progress/errors/timings, loaded-plugin
inventory, status refresh, and daemon reload. Result and plugin metadata come
directly from TarraGon, so newly installed plugins work without Mithshell
changes.

Mithshell reconnects to `/tmp/tarragon-ui.sock` automatically and displays an
offline state when TarraGon is unavailable. Start TarraGon separately:

```sh
systemctl --user enable --now tarragon.service
mithshell search --monitor focused
```

The current TarraGon checkout provides applications, files, calculator, web
search, and other installed plugins. Wallpaper and clipboard history will
appear in the same UI once corresponding TarraGon plugins are installed.

See [`docs/tarragon.md`](docs/tarragon.md) for controls, every represented
protocol field, plugin lifecycle semantics, previews, reload/detach behavior,
and current backend limitations.

### Material You

Current Matugen releases are binary-only. Mithshell embeds the same
`material-colors` engine Matugen uses, so it can generate and consume a scheme
without requiring the `matugen` executable.

Generate from a color:

```toml
[theme]
mode = "dark"
variant = "tonal-spot"

[theme.source]
kind = "color"
value = "#9aa7ff"
```

Or extract a source color from an image:

```toml
[theme.source]
kind = "image"
path = "/home/you/Pictures/wallpaper.jpg"
```

Image decoding and quantization run outside the GTK thread. The active palette
is exported atomically to `$XDG_STATE_HOME/mithshell/palette.json`.

## IPC CLI

The daemon listens on a user-only Unix socket at
`$XDG_RUNTIME_DIR/mithshell/ipc.sock`.

```sh
mithshell toggle --monitor focused
mithshell open --monitor DP-2
mithshell search --monitor focused
mithshell close --monitor all

mithshell osd volume --value 60
mithshell osd brightness --value 45 --timeout 1200

mithshell status --json
mithshell reload

mithshell theme set --image ~/Pictures/wallpaper.jpg --persist
mithshell theme set --color '#8aadf4'
mithshell theme mode dark --persist
mithshell theme current --json
mithshell theme reset
```

Targets can be `focused`, `all`, or an exact configured output. Persistent
theme commands write an override to `$XDG_STATE_HOME/mithshell/theme.toml`;
`theme reset` removes it and reapplies the main configuration.

See `contrib/hyprland.conf` for bind examples.

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Set `RUST_LOG=mithshell=debug` for IPC and service diagnostics. The daemon also
accepts `--no-animations` for debugging or reduced-motion setups.
