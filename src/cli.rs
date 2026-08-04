use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Override the shell IPC socket path.
    #[arg(long, global = true, env = "MITHSHELL_SOCKET")]
    pub socket: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the long-lived GTK shell.
    Daemon {
        /// Read configuration from this path instead of the XDG default.
        #[arg(long)]
        config: Option<PathBuf>,

        /// Disable geometry and content animations.
        #[arg(long)]
        no_animations: bool,
    },

    /// Toggle the dashboard.
    Toggle(MonitorArgs),

    /// Open the dashboard.
    Open(MonitorArgs),

    /// Open the TarraGon search frontend.
    Search(MonitorArgs),

    /// Collapse the island.
    Close(MonitorArgs),

    /// Show transient system feedback.
    Osd {
        #[arg(value_enum)]
        kind: OsdKind,

        /// Value from 0 to 100. When omitted, the daemon queries the system.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
        value: Option<u8>,

        /// Time before the OSD returns to the compact state.
        #[arg(long, default_value_t = 1500)]
        timeout: u64,

        #[command(flatten)]
        monitor: MonitorArgs,
    },

    /// Lock the session behind a PAM password prompt.
    Lock,

    /// Force-unlock a locked session without authenticating.
    ///
    /// A same-user, same-machine escape hatch: send this from a different
    /// TTY or SSH session logged in as the same user as the locked
    /// session. The IPC socket is already restricted to this user by
    /// filesystem permissions and a peer-credential check, so reaching the
    /// daemon at all already proves that.
    Unlock,

    /// Reload the TOML configuration.
    Reload,

    /// Query the running daemon.
    Status {
        /// Print the full machine-readable status response.
        #[arg(long)]
        json: bool,
    },

    /// Report search latency percentiles collected with MITHSHELL_TRACE_LATENCY=1.
    Latency {
        /// Print the full machine-readable latency report.
        #[arg(long)]
        json: bool,

        /// Discard collected samples instead of reporting them.
        #[arg(long)]
        reset: bool,
    },

    /// Generate and inspect Material You color schemes.
    Theme {
        #[command(subcommand)]
        command: ThemeCommand,
    },

    /// Print a shell completion script to stdout.
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Clone, Args)]
pub struct MonitorArgs {
    /// Target `focused`, `all`, or an exact output name such as `DP-2`.
    #[arg(long, default_value = "focused")]
    pub monitor: String,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OsdKind {
    Volume,
    Brightness,
    Workspace,
}

#[derive(Debug, Subcommand)]
pub enum ThemeCommand {
    /// Generate and apply a scheme from an image or color.
    Set {
        /// Extract a source color from this image.
        #[arg(long, conflicts_with = "color", required_unless_present = "color")]
        image: Option<PathBuf>,

        /// Use this CSS hex color as the source.
        #[arg(long, conflicts_with = "image", required_unless_present = "image")]
        color: Option<String>,

        /// Override the configured light or dark mode.
        #[arg(long, value_enum)]
        mode: Option<ThemeModeArg>,

        /// Keep this source as an XDG state override across restarts.
        #[arg(long)]
        persist: bool,
    },

    /// Change light or dark mode without replacing the source.
    Mode {
        #[arg(value_enum)]
        mode: ThemeModeArg,

        #[arg(long)]
        persist: bool,
    },

    /// Print the active scheme.
    Current {
        #[arg(long)]
        json: bool,
    },

    /// Print the active scheme as color swatches labeled with each role.
    Palette,

    /// Remove a persisted theme override and reapply the configuration.
    Reset,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ThemeModeArg {
    Dark,
    Light,
}
