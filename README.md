# mithshell

`mithshell` is a small GTK4 layer-shell surface for Hyprland. It rests as a
top-center workspace pill and expands into a focused dashboard with system
controls. It includes workspaces, active window and media state, automatic
volume OSD feedback, brightness and battery state, desktop notifications,
Material You colors, and a local IPC command line.

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

`grim` is optional and used only by the lock screen, to capture the frozen
wallpaper it blurs. Without it the lock still works and falls back to a solid
black backdrop.

`brightnessctl` is optional. The example Hyprland keybinds use it, and the
dashboard also probes for it: without the binary the brightness control stays
hidden entirely, while with it the level is read from and written to
`/sys/class/backlight` directly.

[TarraGon](https://github.com/iMithrellas/tarragon) is optional. When its user
service is running, Mithshell exposes its configured search plugins through an
independent launcher window.

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

### System tray

Mithshell hosts a `org.kde.StatusNotifierItem` tray itself -- no external
tray/watcher process is needed, and if another one is already running (a
previous mithshell instance, another bar), mithshell simply attaches to it
as a host instead of competing for the watcher role.

Tray icons are hidden by default and only reveal themselves while hovering
the pill; the pill's width grows to fit them and shrinks back once the
pointer leaves. Left click activates an item, middle click sends its
secondary activation, and right click opens its context menu (built from
the item's own DBusMenu when it has one, or the item's own `ContextMenu`
otherwise). Scrolling over an icon forwards the wheel delta to the item too.

```toml
[tray]
enabled = true
```

Set `enabled = false` to turn this off entirely; mithshell then neither
hosts nor watches for tray items, and the pill never grows a tray section.

### Icons

Mithshell's own controls -- media transport, volume, brightness, battery,
window buttons, the lock screen's power row -- are drawn as Nerd Font glyphs
rather than themed icons, so the shell looks the same regardless of which GTK
icon theme happens to be installed.

```toml
[icons]
style = "glyph"
```

Set `style = "symbolic"` to use themed icons everywhere instead.

Only the codepoints in the Font Awesome block are used. Nerd Fonts v3 moved
the Material Design Icons block but left that one alone, so any patched font
works whether it predates the v3 migration or not, and fontconfig finds it
without mithshell having to know its name. Installing
[Symbols Nerd Font](https://github.com/ryanoasis/nerd-fonts) gives the most
consistent result: with it every glyph is drawn from one family instead of
from whichever font fontconfig ranks first for each individual codepoint.

No font is strictly required. At startup mithshell checks each glyph against
the installed fonts and falls back to a themed icon for any it cannot draw --
per icon, not all-or-nothing -- so a missing font degrades a single control
rather than the whole shell. Run with `RUST_LOG=info` to see what was resolved.

Icons that belong to *other* programs -- tray items, notification `app_icon`
hints, MPRIS player icons, and TarraGon search results -- are always drawn
from the icon theme, since only the sending application knows what they
should be. Those still need a reasonably complete icon theme installed.

### Notifications

Mithshell implements `org.freedesktop.Notifications` itself -- no external
notification daemon (dunst, mako, ...) is needed, and running one alongside
mithshell means whichever process claims the D-Bus name first wins; only one
can own it at a time.

```toml
[notifications]
fullscreen_strategy = "fallback"
fallback_monitors = []
overlay_over_fullscreen = "off"
position = "pill"
timeout_ms = 5000
max_visible = 5
max_history = 50
gap = 8
margin = 12
```

`fullscreen_strategy` controls notifications that would otherwise land on a
fullscreen focused monitor:

* `"fallback"` (the default) uses the first configured, connected,
  non-fullscreen output in `fallback_monitors`. If none of those outputs are
  usable, it continues to any configured non-fullscreen output in Hyprland
  monitor-id order. The default empty list uses that automatic order
  immediately.
* `"all-non-fullscreen"` shows one view of the notification on every configured,
  connected, non-fullscreen output.
* `"ignore"` keeps using the focused output.

If no non-fullscreen output is available, `fallback` and
`all-non-fullscreen` log a warning and use the focused output. Fullscreen
detection includes both a monitor's normal active workspace and any visible
special workspace.

`overlay_over_fullscreen` can be `"off"` (the default), `"low"`, `"normal"`,
or `"critical"`. The urgency value is a minimum threshold: `"normal"` includes
normal and critical notifications. A qualifying notification stays on the
focused output and uses the Wayland Overlay layer, but only while that output
is fullscreen. Critical notifications otherwise remain on the normal Top
layer.

For `pill`, qualifying notifications use a dedicated Overlay window at the
pill's normal location rather than changing the layer of the entire island.
The surface is selected when each queued notification starts displaying and
does not migrate between surfaces if fullscreen starts or ends mid-display.

Toast positions use one dynamically layered window per output. If a
qualifying toast promotes that window to Overlay, lower-urgency rows are hidden
so they remain effectively occluded by the fullscreen window. Their timers
continue and they still count toward `max_visible`; an unexpired row becomes
visible again if the toast window returns to Top before it expires.

`position` controls where an incoming notification appears:

* `"pill"` (the default) -- reuses the island's own surface exactly like the
  OSD does: one notification at a time, in place of the compact pill,
  auto-advancing through a queue if more arrive while one is showing.
* `"below-pill"` -- a separate small popup centered directly under the
  island, holding up to `max_visible` stacked toasts.
* `"top-left"`, `"top-right"`, `"bottom-left"`, `"bottom-right"` -- the same
  popup, anchored to a screen corner instead of following the island.

`timeout_ms` is the fallback shown-for duration when a sender doesn't
request an explicit one. A sender that explicitly asks for persistence
(`expire_timeout = 0`) is honored in `below-pill`/corner positions, which
have a close button; in `pill` position it instead falls back to
`timeout_ms`, since the pill has no interactive dismiss control and a
persistent entry would otherwise block the queue forever.

Every notification is also kept in the dashboard's notification card
(up to `max_history`), independent of `position` and of whether its popup
has already timed out, with its own dismiss button.

Notification layout and fullscreen settings take effect after running
`mithshell reload` or restarting the daemon.

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

Mithshell reconnects automatically (see
[`docs/tarragon.md`](docs/tarragon.md) for the socket resolution order) and
displays an offline state when TarraGon is unavailable. Install and enable it
with:

```sh
mithshell setup tarragon
mithshell search --monitor focused
```

This clones and builds TarraGon (unless a `tarragon` binary is already on
PATH), generates a default config through TarraGon's own binary, and
installs and enables its systemd user service. It requires `git` and a Go
toolchain and is safe to run more than once. See
[`docs/tarragon.md`](docs/tarragon.md#installation) for flags and the
manual install steps it automates.

The current TarraGon checkout provides applications, files, calculator, web
search, and other installed plugins. Wallpaper and clipboard history will
appear in the same UI once corresponding TarraGon plugins are installed.

See [`docs/tarragon.md`](docs/tarragon.md) for controls, every represented
protocol field, plugin lifecycle semantics, previews, reload/detach behavior,
and current backend limitations.

### Lock screen

`mithshell lock` locks the session through the
[`ext-session-lock-v1`](https://wayland.app/protocols/ext-session-lock-v1)
Wayland protocol, using [`gtk4-session-lock`](https://github.com/wmww/gtk4-layer-shell)
-- the sibling crate to `gtk4-layer-shell`, wrapping the same C library and
already an installed dependency. This is a compositor-enforced lock, not a
window mithshell merely draws on top of everything: the protocol requires
the compositor to blank every other client's surface (the island included)
the instant the lock is acquired, and explicitly states that if the locking
client dies, "the compositor must not unlock the session in response... it
is acceptable for the session to be permanently locked if this happens."
That guarantee is the compositor's job, not mithshell's; killing or crashing
the daemon while locked does not expose the desktop. Hyprland has supported
this protocol since 0.52.1.

Each output gets a fresh window assigned to it as a lock surface, showing the
screen as it was the moment before locking -- captured with `grim`,
downscaled, box-blurred, and dimmed -- with a single card centred on it. The
card reuses the island's surface, typography and `@ms_*` palette roles, so a
locked session looks like the same shell rather than a separate program. It
also shows the hostname, operating system and uptime, plus battery and current
weather when those pollers have data. The card and safe captured backdrop fade
in and out using `shell.animation_ms`; `daemon --no-animations` disables those
transitions as it does for the island.

```toml
[lock]
blur_radius = 6
blur_downscale = 6
dim = 0.55
```

The window background is opaque black underneath the screenshot, so a
missing `grim` or a failed capture degrades to a blank screen rather than a
transparent one. Capturing happens before the window is handed to the
lock instance; the protocol grants clients a grace window between
requesting the lock and the compositor actually blanking the output
specifically so lock screens can render (and screenshot) without a race.

#### logind integration

The daemon also locks on request from systemd-logind, so `loginctl
lock-session`, `loginctl lock-sessions`, an idle daemon, and systemd's own
`IdleAction=lock` all reach the same lock screen as `mithshell lock` --
nothing extra to configure. The session object is resolved at startup
(`GetSessionByPID`, falling back to `auto` and then `$XDG_SESSION_ID`, which
between them cover the daemon being started outside the session's cgroup),
and the `Lock` and `Unlock` signals on that exact session are followed
thereafter.

`LockedHint` is kept in sync in the other direction, so `loginctl
show-session -p LockedHint` reports the truth to anything else on the system.
It follows the compositor's own `locked`/`unlocked` signals rather than the
request that triggered them, so a lock the compositor refuses is never
reported as taken.

Note that `Unlock` bypasses PAM, exactly as `mithshell unlock` does. logind
only forwards it after its own Polkit check, which is the same trust boundary
the IPC socket's peer-credential check provides; both are recovery paths for
a stuck prompt, not a second authentication method. If the bus or logind is
unavailable -- an elogind-less system, or a bus that goes away -- the bridge
retries with a backoff up to five minutes and the rest of the shell is
unaffected; `mithshell status` reports the resolved session path or the last
error under `logind`.

#### Authentication

Passwords are checked against PAM on a dedicated worker thread, never the GTK
main thread, because modules can block for a long time -- `pam_unix` sleeps
for seconds after a wrong password, and hardware token modules wait for a
physical touch. The service is resolved once at startup:

* `/etc/pam.d/mithshell` when an administrator has written one;
* otherwise `/etc/pam.d/login`, which every distribution ships. Whatever
  that stack does, the lock screen does too -- including multi-factor
  setups. For example:

  ```
  # /etc/pam.d/login
  auth       sufficient pam_u2f.so cue
  auth       required   pam_google_authenticator.so
  auth       requisite  pam_nologin.so
  account    include    system-local-login
  session    include    system-local-login
  password   include    system-local-login
  ```

  Here the lock screen will wait for a YubiKey touch first (the `cue`
  option's "Please touch the device" message is forwarded live to the card's
  status line as it happens, not just a static "Authenticating..."), falling
  through to a TOTP code from `pam_google_authenticator` typed into the same
  password field if no key is touched. No lock-specific configuration is
  needed for this to work; it is exactly what `/etc/pam.d/login` already
  describes.

Set `pam_service` to override the choice. Authentication additionally runs
`pam_acct_mgmt`, so expired and disabled accounts cannot unlock, and passes
`PAM_DISALLOW_NULL_AUTHTOK`, so an empty password never unlocks regardless of
what the inherited stack permits.

#### Behavior

Every output gets its own card and they share one password buffer, so it does
not matter which screen has keyboard focus. `Escape` clears the field instead
of dismissing the lock, and a Caps Lock warning appears while the modifier is
active. `mithshell reload` is refused while locked, since reloading rebuilds
every window, and so are `toggle`, `open` and `search` -- the launcher can
start applications and must not be reachable from a locked session, visible
or not.

Power off, suspend and reboot controls invoke `systemctl` on a worker thread.
They remain subject to the host's logind and Polkit policy; a rejected request
is reported on the card instead of opening an authentication dialog behind the
compositor-enforced lock.

`mithshell unlock` force-unlocks without authenticating, as a same-user,
same-machine escape hatch: run it from a different TTY or an SSH session
logged in as the same user as the locked one. The IPC socket already only
accepts connections from this user's uid (filesystem permissions plus an
`SO_PEERCRED` check), so reaching the daemon with it at all already proves
that; this command exists for when the lock screen itself is unusable
(broken PAM config, unresponsive hardware token) rather than as a second
authentication method.

**Caveat.** `mithshell unlock` only reaches a `LockSession` held by the
*currently running* daemon process. Because the fail-locked guarantee comes
from the compositor continuing to enforce a lock whose client has
disappeared, restarting the mithshell daemon (`systemctl restart`, a
crash-and-respawn, `just run`) *while locked* does not automatically
re-establish mithshell's own lock screen -- the new process starts with no
memory of being locked, while the compositor is still blanking every output
from the old, now-dead lock object, and `mithshell unlock` against the new
process correctly reports "session is not locked" without being able to
touch that stale state. Recovery in that case is whatever secure recovery
mechanism the compositor itself offers (see its documentation), not
mithshell. This is an inherent tradeoff of the fail-locked model, not
specific to mithshell -- any session-lock client has it if killed uncleanly,
which is exactly why restarting a lock client while it is actively locking
the screen is best avoided.

### Theming

Mithshell's stylesheet (`src/ui/style.css`) references 15 `@ms_*` role colors
(`ms_primary`, `ms_on_surface`, `ms_outline`, ...). Where those colors come
from is controlled by `theme.engine`, and can always be overridden with a
`colors.css` file.

The dashboard, compact bar, media popup, OSD, notification popup, and
weather popup use
[Monocraft](https://github.com/IdreesInc/Monocraft) as their display font,
falling back to `JetBrainsMono Nerd Font` and the system monospace font if
it isn't installed. Install `ttf-monocraft-git` (AUR) or the upstream release
for the intended pixel-art look; TarraGon's search UI keeps its own
`Inter`/`JetBrains Mono` pairing and is unaffected.

Chrome icons are drawn from a separate font stack that mithshell emits into
the stylesheet itself, so it always matches the font the glyph-availability
check probes. See [Icons](#icons).

#### Material You

The default engine. Current Matugen releases are binary-only; mithshell
embeds the same `material-colors` engine Matugen uses, so it can generate and
consume a scheme without requiring the `matugen` executable.

Generate from a color:

```toml
[theme]
engine = "material"
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

#### Inheriting the GTK theme

Set `engine = "gtk"` to skip generation entirely and resolve the `@ms_*`
roles from the active GTK theme's standard named colors instead
(`@accent_color`, `@window_bg_color`, `@borders`, ...) -- the same convention
libadwaita, matugen's `gtk4` template, wallust, and virtually every modern
GTK4 theme all use:

```toml
[theme]
engine = "gtk"
```

`mode`/`variant`/`source` are ignored in this mode. `mithshell theme current`
and `palette.json` report the resolved hex values.

Mithshell reads and watches `$XDG_CONFIG_HOME/gtk-4.0/gtk.css` directly for
this (parsing `@define-color` lines itself, including simple `@other-name`
aliases) rather than relying on GTK's own cascade, and regenerates the
palette whenever that file changes on disk. This is what makes tools like
matugen/wallust rewriting it after a wallpaper change take effect live,
with no `mithshell reload` or restart needed -- GTK itself only parses that
file once per process, so nothing else watching the display's style
cascade could ever pick up edits to it live.

If that file is absent or doesn't define any of the roles mithshell needs,
it falls back to a live GTK style context lookup against whatever theme is
currently active, refreshed whenever `Gtk.Settings`' theme name or dark/light
preference changes (e.g. through a theme switcher going through the
XSETTINGS/portal mechanism). Note an explicit
`gtk-application-prefer-dark-theme=`/`gtk-theme-name=` line in
`~/.config/gtk-4.0/settings.ini` is a hard pin that GTK won't override from a
live portal/XSETTINGS change; remove it if you want that fallback path to
follow live light/dark switches too.

#### Custom CSS

Place a `colors.css` file next to `config.toml` (e.g.
`$XDG_CONFIG_HOME/mithshell/colors.css`) and mithshell loads it as an
additional stylesheet at a higher priority than everything above. It can
override just a few `@ms_*` colors, or any other selector in the built-in
stylesheet -- it's plain GTK CSS layered on top, so nothing about it is
color-specific. See
[`config/colors.example.css`](config/colors.example.css) for the full list of
overridable color names. Changes are picked up by `mithshell reload`.

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

mithshell lock
mithshell unlock

mithshell status --json
mithshell reload

mithshell theme set --image ~/Pictures/wallpaper.jpg --persist
mithshell theme set --color '#8aadf4'
mithshell theme mode dark --persist
mithshell theme current --json
mithshell theme palette
mithshell theme reset
```

Targets can be `focused`, `all`, or an exact configured output. Persistent
theme commands write an override to `$XDG_STATE_HOME/mithshell/theme.toml`;
`theme reset` removes it and reapplies the main configuration.

`theme palette` prints each `@ms_*` role as a color swatch next to its name
and hex value (colored when stdout is a terminal, plain text otherwise) --
handy for eyeballing a scheme without parsing JSON.

See `contrib/hyprland.conf` for bind examples.

### Shell completions

```sh
mkdir -p ~/.zsh/completions
mithshell completions zsh > ~/.zsh/completions/_mithshell
# Or use `just install-completions` from the source tree.
```

Generated directly from the `clap` command definitions, so it never drifts
from the actual CLI. The `just run` development install refreshes the active
Zsh script whenever the binary is rebuilt. Bash, Fish, Elvish, and PowerShell
scripts are also available by replacing the shell argument above.

## Development

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
```

Set `RUST_LOG=mithshell=debug` for IPC and service diagnostics. The daemon also
accepts `--no-animations` for debugging or reduced-motion setups.

### Rendering

The daemon defaults to GTK's cairo renderer. The island is a small 2D surface,
so GPU compositing gains little while mapping the whole driver stack into a
session-long process; on an NVIDIA system this is the difference between
224 MB and 85 MB of RSS. Export `GSK_RENDERER` to override it, for example
`GSK_RENDERER=ngl`.

### Benchmarking

Search latency is instrumented behind an environment variable, so it costs
nothing unless you ask for it:

```sh
just trace-on        # restart the service with MITHSHELL_TRACE_LATENCY=1
just bench-latency   # drive the launcher with wtype, then print percentiles
just bench-memory    # RSS, PSS, private dirty, largest mappings, surfaces
just trace-off       # restart without tracing
```

`mithshell latency` prints the collected report on demand and
`mithshell latency --reset` clears it. Six spans are measured from the real key
event through to the frame that paints the results: `debounce` (keystroke to
query dispatched), `write` (dispatch to socket flush), `backend` (TarraGon
round trip), `build` (main-thread widget update), `paint` (frame presented),
and `total`.

Reading the numbers: `backend` is TarraGon's own cost, `build` is the shell's
rendering work, and `paint` is mostly waiting for the next vsync, so it has a
floor of roughly one frame interval.
