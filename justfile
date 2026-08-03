# Mithshell justfile
# Run `just` to list all available recipes, or `just <recipe>` to run one.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Paths
project_dir := justfile_directory()
release_binary := project_dir / "target/release/mithshell"
debug_binary := project_dir / "target/debug/mithshell"
bin_dir := env("HOME") / ".local/bin"
bin_symlink := bin_dir / "mithshell"
service_dir := env("HOME") / ".config/systemd/user"
service_file := project_dir / "contrib/mithshell.service"
config_dir := env("HOME") / ".config/mithshell"
config_file := config_dir / "config.toml"
config_example := project_dir / "config/mithshell.example.toml"

[doc("List all available recipes")]
default:
    @just --list --unsorted

# --- Build & Install ---------------------------------------------------------

[doc("Compile an optimized release binary")]
build *FLAGS:
    cargo build --release {{ FLAGS }}

[doc("Compile a debug binary")]
build-debug *FLAGS:
    cargo build {{ FLAGS }}

[doc("Symlink the release binary to ~/.local/bin/mithshell")]
install-binary: build
    mkdir -p {{ bin_dir }}
    ln -sfn {{ release_binary }} {{ bin_symlink }}
    @echo "Binary symlinked to {{ bin_symlink }}"

[doc("Install the example config only when no config exists")]
install-config:
    mkdir -p {{ config_dir }}
    if [[ ! -e {{ config_file }} ]]; then cp {{ config_example }} {{ config_file }}; echo "Config installed at {{ config_file }}"; else echo "Keeping existing {{ config_file }}"; fi

# --- Systemd Service ---------------------------------------------------------

[doc("Symlink the systemd user service")]
install-service:
    mkdir -p {{ service_dir }}
    ln -sfn {{ service_file }} {{ service_dir }}/mithshell.service
    systemctl --user daemon-reload
    @echo "Service symlinked to {{ service_dir }}/mithshell.service"

[doc("Restart mithshell.service")]
reload-service:
    systemctl --user daemon-reload
    systemctl --user restart mithshell.service

[doc("Enable and start mithshell.service on login")]
enable-service: install-service
    systemctl --user enable --now mithshell.service

[doc("Stop mithshell.service")]
stop-service:
    systemctl --user stop mithshell.service

[doc("Show service status")]
service-status:
    systemctl --user status mithshell.service || true

[doc("Follow service logs")]
service-logs:
    journalctl --user -u mithshell.service -f

[doc("Build, install, configure, and restart the user service")]
run: install-binary install-config install-service reload-service

# --- Testing, Formatting & Linting ------------------------------------------

[doc("Run all Rust tests")]
test *FLAGS:
    cargo test {{ FLAGS }}

[doc("Run tests and retain stdout/stderr")]
test-verbose *FLAGS:
    cargo test {{ FLAGS }} -- --nocapture

[doc("Format Rust source")]
fmt:
    cargo fmt

[doc("Check formatting without modifying files")]
fmt-check:
    cargo fmt --check

[doc("Run Clippy with warnings denied")]
lint:
    cargo clippy --all-targets -- -D warnings

[doc("Run formatting, Clippy, tests, and release build")]
check: fmt-check lint test build
    @echo "All checks passed."

# --- Development -------------------------------------------------------------

[doc("Run the debug daemon in the foreground")]
daemon *FLAGS: build-debug
    {{ debug_binary }} daemon {{ FLAGS }}

[doc("Run the debug daemon without animations")]
daemon-static *FLAGS: build-debug
    {{ debug_binary }} daemon --no-animations {{ FLAGS }}

[doc("Open the TarraGon launcher on the focused monitor")]
search *FLAGS: build-debug
    {{ debug_binary }} search {{ FLAGS }}

[doc("Toggle the dashboard on the focused monitor")]
toggle *FLAGS: build-debug
    {{ debug_binary }} toggle {{ FLAGS }}

[doc("Close all Mithshell overlays")]
close: build-debug
    {{ debug_binary }} close --monitor all

[doc("Print daemon status as JSON")]
status: build-debug
    {{ debug_binary }} status --json

[doc("Reload the running daemon configuration")]
reload: build-debug
    {{ debug_binary }} reload

[doc("Print CLI help")]
help: build-debug
    {{ debug_binary }} --help

# --- Benchmarking ------------------------------------------------------------

[doc("Restart the service with latency tracing enabled")]
trace-on: install-binary install-service
    mkdir -p {{ service_dir }}/mithshell.service.d
    printf '[Service]\nEnvironment=MITHSHELL_TRACE_LATENCY=1\n' > {{ service_dir }}/mithshell.service.d/trace.conf
    systemctl --user daemon-reload
    systemctl --user restart mithshell.service
    @echo 'Latency tracing enabled. Run: just bench-latency'

[doc("Restart the service with latency tracing disabled")]
trace-off:
    rm -f {{ service_dir }}/mithshell.service.d/trace.conf
    rmdir {{ service_dir }}/mithshell.service.d 2>/dev/null || true
    systemctl --user daemon-reload
    systemctl --user restart mithshell.service
    @echo "Latency tracing disabled."

[doc("Drive the launcher with wtype and report latency percentiles")]
bench-latency *FLAGS:
    {{ project_dir }}/scripts/bench-latency.sh {{ FLAGS }}

[doc("Report the daemon's memory footprint")]
bench-memory *FLAGS:
    {{ project_dir }}/scripts/bench-memory.sh {{ FLAGS }}

[doc("Print the collected latency report without driving the launcher")]
latency *FLAGS:
    {{ bin_symlink }} latency {{ FLAGS }}

[doc("Discard collected latency samples")]
latency-reset:
    {{ bin_symlink }} latency --reset

# --- Utilities ---------------------------------------------------------------

[doc("Remove Cargo build artifacts")]
clean:
    cargo clean

[doc("Update Cargo.lock within declared version constraints")]
update:
    cargo update

[doc("Remove installed binary/service symlinks without deleting config")]
uninstall:
    systemctl --user disable --now mithshell.service 2>/dev/null || true
    rm -f {{ service_dir }}/mithshell.service
    rm -f {{ bin_symlink }}
    systemctl --user daemon-reload
    @echo "Removed Mithshell binary and service symlinks; config was preserved."
