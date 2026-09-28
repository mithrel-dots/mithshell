# Repository map

Mithshell is one Rust crate with a thin binary entry point. Backend workers send
plain state over channels; the application controller owns GTK-thread state and
routes updates to the windows. Keep blocking I/O out of UI callbacks.

## Application and configuration

- `src/main.rs`: renderer selection, logging, argument parsing, and exit status.
- `src/cli.rs`: command definitions and argument validation.
- `src/app/mod.rs`: daemon startup, controller lifecycle, IPC dispatch, monitor
  reconciliation, and cross-service coordination.
- `src/app/client.rs`: CLI-to-IPC translation, completions, and terminal output.
- `src/app/events.rs`: desktop snapshot subscriptions and their weak-owner
  receive loop. A subscription must not keep the controller alive.
- `src/ipc.rs`: socket transport, peer checks, and serialized request/response types.
- `src/config/mod.rs`: configuration schema, defaults, and load-time validation.
- `src/config/motion.rs`: motion schema and numeric validation.
- `src/config/paths.rs`: XDG paths and home expansion.
- `src/state.rs`: snapshots shared by backends and UI; preserve serialized shapes
  when changing their internal construction.

## Desktop integrations

- `src/hyprland.rs`: compositor snapshots, events, and workspace dispatch.
- `src/system.rs`: audio mutations, battery/backlight reads, system information,
  and the controller's cached system snapshot.
- `src/telemetry.rs`: sampled CPU, memory, network, and temperature measurements.
- `src/media/mod.rs`: MPRIS discovery, metadata, and playback controls.
- `src/media/visualizer.rs`: Cava process and spectrum frames.
- `src/notifications.rs`: freedesktop notification D-Bus server.
- `src/tray.rs`: StatusNotifier watcher/host and DBusMenu integration.
- `src/weather.rs`, `src/weather/`: polling policy and provider-specific parsing.
- `src/lock.rs`, `src/lock/`: PAM worker, logind bridge, and screenshot processing.

The controller decides where service results appear. Backends should not depend
on particular island pages or circle placement.

## Search and appearance

- `src/tarragon.rs`: launcher protocol, connection management, and active-query
  filtering. Its socket resolution follows TarraGon's own contract.
- `src/preview/mod.rs`: coalesced preview requests, file metadata, text/image/video
  loading, and thumbnail caching.
- `src/preview/syntax.rs`: language detection and Tree-sitter grammar setup.
- `src/setup.rs`: local installation of the optional TarraGon integration.
- `src/latency.rs`: opt-in launcher timing.
- `src/theme.rs`: Material/GTK palettes, overrides, and GTK stylesheet watching.
- `src/http.rs`: shared HTTP client configuration.

## GTK UI

- `src/ui/mod.rs`: styles and shared scaling policy.
- `src/ui/format.rs`: display formatting shared across windows.
- `src/ui/icon.rs`: chrome glyphs and foreign-icon fallbacks.
- `src/ui/motion.rs`: named transition profiles and easing.
- `src/ui/lock.rs`: compositor session-lock windows and authentication presentation.
- `src/ui/island/mod.rs`: island state and shared geometry/helpers.
- `src/ui/island/window.rs`, `interactions.rs`, `view.rs`: construction, input,
  and presentation transitions.
- Other `src/ui/island/*.rs` modules own individual presentations. Circle content
  lives in `*_circle.rs`; `circle/` owns generic host geometry/state, and
  `circle_integration.rs` connects it to the island.
- `src/ui/style.css`: shared shell, launcher, and lock styling.
- `src/ui/island.css`: persistent-header/dashboard styling layered after it.

GTK widgets have one parent. Share state and builders between presentations,
but construct separate widgets when both presentations need to own them.

## Tests and supporting files

Most unit tests live beside their implementation. Cross-presentation island
regressions live in `src/ui/island/tests.rs`, keeping the production module
readable while retaining private-state access.

Use `cargo fmt --check`, `cargo test`, and
`cargo clippy --all-targets -- -D warnings` for routine checks. Tests that need a
display use `#[ignore]`; `scripts/run-ui-regressions-gtk.py` discovers all ignored
tests and gives each its own Broadway/Chromium session. Pass exact Rust test names
to select a subset. Broadway checks real GTK allocation and picking; compositor
layer ordering and session-lock behavior require a Wayland compositor.

- `config/`: installable configuration examples.
- `contrib/`: systemd unit and Hyprland bindings.
- `scripts/`: GTK regression runner and benchmarks.
- `docs/`: architecture, motion, and TarraGon references.
- `docs/design/`: standalone visual reference assets.
- `reference/`: ignored local upstream checkouts.
- `.worktrees/`, `target/`: ignored development/build artifacts.
