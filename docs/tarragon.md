# TarraGon frontend

Mithshell includes a native GTK4 frontend for the TarraGon daemon. TarraGon
owns discovery, querying, ranking, plugin lifecycle, and action execution;
Mithshell only presents the protocol data. New result-producing plugins do not
require Mithshell changes.

## Installation

TarraGon must expose its UI socket. The primary install path is:

```sh
mithshell setup tarragon
```

This is a standalone local command -- it never talks to the mithshell
daemon over IPC, so it works even before mithshell has ever been started.
In order:

1. Checks that `git` and a Go toolchain (`go`) are on PATH.
2. Clones TarraGon (or fast-forward pulls an existing checkout) unless a
   `tarragon` binary is already on PATH and `--force` was not passed.
3. Builds it -- `just build` inside the checkout if it has a `justfile`,
   otherwise `go build -o build/tarragon ./cmd/` -- and copies the result to
   `~/.local/bin/tarragon`.
4. Generates a default config with `tarragon config generate` unless
   `~/.config/tarragon/tarragon.{toml,yaml,json}` already exists.
5. Installs and enables `systemd/tarragon.service` from the checkout as
   `~/.config/systemd/user/tarragon.service`, unless `--no-service` was
   passed.

Flags:

```sh
mithshell setup tarragon \
  --repo https://github.com/mithrel-dots/TarraGon.git \
  --src-dir ~/.cache/mithshell/tarragon-src \
  --no-service \
  --force
```

`--repo` and `--src-dir` override the clone URL and local checkout
directory (which otherwise defaults under `$XDG_CACHE_HOME/mithshell`).
`--no-service` skips the systemd unit. `--force` rebuilds and reinstalls
even when a `tarragon` binary is already on PATH; without it, running the
command again is a no-op past the config/service steps.

### Manual install

If you'd rather not use the automated flow, the same steps done by hand:

```sh
git clone --depth 1 https://github.com/mithrel-dots/TarraGon.git
cd TarraGon && just build   # or: go build -o build/tarragon ./cmd/
install -Dm755 build/tarragon ~/.local/bin/tarragon
tarragon config generate    # only if ~/.config/tarragon/tarragon.toml doesn't exist
install -Dm644 systemd/tarragon.service ~/.config/systemd/user/tarragon.service
systemctl --user daemon-reload
systemctl --user enable --now tarragon.service
mithshell search --monitor focused
```

The same frontend opens from the dashboard's search button. The example
Hyprland binding in `contrib/hyprland.conf` uses `SUPER+SPACE`.

Mithshell resolves TarraGon's UI socket path in this order:

1. `TARRAGON_UI_SOCKET`, used verbatim, when set and non-empty.
2. `$XDG_RUNTIME_DIR/tarragon/ui.sock`, when `XDG_RUNTIME_DIR` is set and
   non-empty (the common case on any systemd or elogind session).
3. `/tmp/tarragon-{euid}/ui.sock`, the effective UID as a decimal string,
   when neither of the above is available.

The path is resolved once per process and reconnected to every second. The
search entry, plugin inventory, and reload controls become unavailable while
disconnected. TarraGon's own daemon-side resolution follows the exact same
order, so the two agree on a default without any shared configuration.

## Launcher geometry

The launcher has a base size of `820x620`, scaled with the rest of the island.
It opens in an independent top-layer window, but its internal surface retains
the island's original transition: width, height, vertical position, and content
opacity animate from the current island geometry. The independent surface is
stacked behind the island, hiding its origin beneath the persistent pill until
it expands away. The island remains free to display notifications while search
stays open. The launcher starts 20% down the monitor, clamped so the complete
surface leaves clearance for its lower shadow.

## Search and result state

Queries are throttled on the leading edge over a 16 ms window: the first
keystroke after an idle period is dispatched immediately, and only a burst
faster than the window (held keys repeating, or a very fast typist) is
coalesced into its final keystroke. Empty text is handled locally because
TarraGon does not acknowledge empty queries. Each TarraGon update is a complete
aggregate snapshot; Mithshell filters broadcasts to query IDs acknowledged on
its own connection and ignores snapshots whose input no longer matches the
entry.

The header reports:

- aggregate result count;
- completed and total targeted plugin counts;
- pending and failed plugin counts;
- maximum reported plugin latency after completion.

All normalized results are displayed in TarraGon's order. Each row can show:

- icon;
- label or result ID fallback;
- description;
- category or plugin fallback.

Up and Down change selection, Enter runs the selected result's default action,
Escape returns to the dashboard, and pointer selection is supported. Scrolling
the result pane does not move the outer island viewport.

## Preview and metadata

The right pane follows the selected result. File metadata is displayed above
the preview, followed by TarraGon ranking metadata and then the preview itself.
It displays every normalized field that is useful to a frontend:

- label and description;
- plugin and category;
- blended score and frecency score;
- `preview_path`;
- all advertised actions.

All preview loading runs on a dedicated worker. Selection changes carry a
generation number, so a slow result from an older selection cannot replace the
current preview.

### Text and source

Regular UTF-8 text files are shown in a read-only monospace `GtkTextView`
inside a two-axis `GtkScrolledWindow`. Long files and long lines can therefore
be scrolled vertically and horizontally without moving the outer island.

Mithshell uses Tree-sitter highlight queries for:

- Rust;
- C and C++;
- Go;
- Python;
- JavaScript, JSX, TypeScript, and TSX;
- Bash and shell scripts;
- JSON;
- TOML;
- YAML;
- HTML;
- CSS;
- Markdown.

Language detection uses filename, extension, and Python/shell shebangs, with a
plain-text fallback. Metadata includes language, encoding, file size, filename,
and line count. Preview input is bounded to 512 KiB and 5,000 lines; larger
files clearly report truncation. Tree-sitter byte ranges are converted to GTK
character offsets before tags are applied, including for multibyte UTF-8 text.

### Images

PNG, JPEG, WebP, GIF, BMP, TIFF, SVG, and AVIF paths are displayed in a
contained `GtkPicture`. Image headers provide format and resolution without
decoding the complete image on GTK's main thread. Metadata also includes file
size and filename.

### Video

MP4, Matroska, WebM, MOV, AVI, M4V, WMV, FLV, MPEG, and MPG paths use
`ffprobe` for metadata and `ffmpegthumbnailer` for a frame around 10% into the
video. Available metadata includes resolution, codec, pixel format, frame rate,
frame count, duration, bit rate, file size, and filename.

Thumbnail processes have a five-second timeout and are never launched through
a shell. Generated PNGs are cached at
`$XDG_CACHE_HOME/mithshell/previews`, normally
`~/.cache/mithshell/previews`. Cache keys include canonical path, file size,
and modification time, so changed videos automatically receive new thumbnails.

Unsupported binary files retain their metadata and result icon. Preview paths
are canonicalized and must resolve to regular files. TarraGon provides no MIME
type, preview bytes, renderer hint, or preview URL, so classification is local
and intentionally conservative.

## Actions

Every action advertised by the selected result is rendered in the preview
pane. A result's explicit default action is used by Enter; if no action is
marked default, the first action is used. Results without actions remain
inspectable but cannot be executed.

Mithshell sends the exact query ID, plugin, result ID, and action name back to
TarraGon. Only one selection is accepted at a time because TarraGon's
`select_response` does not identify the originating query or action and is
broadcast to every frontend. Success closes the launcher. Failure remains open
and displays the daemon or plugin message. After five seconds without a
response, Mithshell reports that the action is still pending but keeps later
actions serialized; unlocking early could misattribute a delayed broadcast to
a newer action.

The immediate `{"type":"ok"}` response means only that TarraGon accepted the
request; Mithshell waits for `select_response` before reporting success.

## Plugin inventory

The `PLUGINS` toggle requests TarraGon status and lists every discovered
plugin. The summary includes discovered, enabled, connected, and on-call
counts. Each row displays all status metadata exposed by TarraGon:

- name, description, icon, and source;
- enabled state;
- lifecycle (`daemon`, `on_demand_persistent`, or `on_call`);
- transport connection state;
- prefix and whether it is required;
- general-suggestion eligibility;
- capabilities.

When a query exists, inventory rows additionally show aggregate state:

- `pending`;
- `done`, result count, and elapsed milliseconds;
- `empty` and elapsed milliseconds;
- `error` and the plugin error message.

An enabled `on_call` plugin is shown as available even though it is normally not
connected between invocations. An enabled, disconnected
`on_demand_persistent` plugin is shown as idle because TarraGon starts it on a
matching query.

## Reload and lifecycle

The refresh button sends TarraGon's `reload` request. Mithshell displays the
reload response and requests fresh status after success. Reload behavior is
defined by TarraGon: it discovers new plugins, reapplies supported overrides,
and starts or stops affected daemon plugins. It does not necessarily reread
all metadata for already discovered plugins.

On shutdown, the client sends a best-effort `detach` request before closing its
socket. This asks TarraGon to discard aggregate snapshots owned by Mithshell.

## Protocol implementation

The integration uses one persistent Unix stream on a worker thread. Requests
and responses are newline-delimited JSON. GTK never blocks on socket reads or
base64/JSON decoding.

Supported requests:

- `query`
- `select`
- `status`
- `reload`
- `detach`

Supported responses:

- `ack`
- `update`
- `status`
- `reload_response`
- `select_response`
- protocol `error` messages
- receipt `ok` messages

TarraGon's Go `update.payload` field is a `[]byte`, so its actual JSON wire
value is a base64 string containing the aggregate JSON. Mithshell decodes that
string before deserializing query, plugin, result, score, preview, and action
state.

## Known backend limits

These constraints come from TarraGon's current UI protocol:

- Selection responses cannot be correlated beyond serializing actions.
- Persistent plugins can remain pending indefinitely; TarraGon has no normal
  response timeout for them.
- Preview paths have no MIME or trust metadata; Mithshell applies local type,
  regular-file, size, and process-time limits.
- Status has no process ID, health history, version, or last-seen timestamp.
- Aggregate updates are broadcasts and complete snapshots, not deltas.
- There is no query cancellation, aggregate history/fetch API, or plugin
  enable/disable request on the UI socket.
- `reload` is available, but plugin installation and configuration remain
  TarraGon CLI responsibilities.

Wallpaper changing, clipboard history, and future providers will use the same
result, preview, state, and action UI when their TarraGon plugins are installed.
