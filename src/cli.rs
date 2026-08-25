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

    /// Open the weather forecast.
    Weather(MonitorArgs),

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

    /// Toggle notification inhibition, or enable it for a duration such as 1h or 100m.
    Inhibit {
        /// Duration using s, m, h, or d units; compounds such as 1h30m are accepted.
        #[arg(value_name = "DURATION", value_parser = parse_inhibit_duration)]
        duration_ms: Option<u64>,
    },

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

    /// Install and configure optional integrations.
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
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

fn parse_inhibit_duration(value: &str) -> Result<u64, String> {
    if value.is_empty() {
        return Err("duration cannot be empty".into());
    }
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut total_ms = 0_u64;
    while cursor < bytes.len() {
        let number_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if number_start == cursor || cursor == bytes.len() {
            return Err("use a positive number followed by s, m, h, or d".into());
        }
        let amount = value[number_start..cursor]
            .parse::<u64>()
            .map_err(|_| "duration number is too large")?;
        let multiplier = match bytes[cursor] {
            b's' => 1_000,
            b'm' => 60_000,
            b'h' => 3_600_000,
            b'd' => 86_400_000,
            _ => return Err("duration unit must be s, m, h, or d".into()),
        };
        cursor += 1;
        let component = amount
            .checked_mul(multiplier)
            .ok_or("duration is too large")?;
        total_ms = total_ms
            .checked_add(component)
            .ok_or("duration is too large")?;
    }
    if total_ms == 0 {
        return Err("duration must be greater than zero".into());
    }
    Ok(total_ms)
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

#[derive(Debug, Subcommand)]
pub enum SetupCommand {
    /// Install and enable the optional TarraGon launcher backend.
    ///
    /// Clones and builds TarraGon if it is not already on PATH, generates a
    /// default config through TarraGon's own binary, and installs its
    /// systemd user service. Safe to run more than once: an already
    /// installed `tarragon` binary is left alone unless `--force` is given.
    Tarragon(SetupTarragonArgs),
}

#[derive(Debug, Clone, Args)]
pub struct SetupTarragonArgs {
    /// Git URL to clone TarraGon from.
    #[arg(long, default_value = "https://github.com/mithrel-dots/TarraGon.git")]
    pub repo: String,

    /// Local directory to clone and build TarraGon in. Defaults to a
    /// directory under `$XDG_CACHE_HOME/mithshell`.
    #[arg(long)]
    pub src_dir: Option<PathBuf>,

    /// Skip installing and enabling the systemd user service.
    #[arg(long)]
    pub no_service: bool,

    /// Rebuild and reinstall even if a `tarragon` binary is already on PATH.
    #[arg(long)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notification_inhibit_durations() {
        for (value, expected_ms) in [
            ("1h", 3_600_000),
            ("100m", 6_000_000),
            ("1h30m", 5_400_000),
            ("2m15s", 135_000),
            ("1d", 86_400_000),
        ] {
            let cli = Cli::try_parse_from(["mithshell", "inhibit", value]).unwrap();
            assert!(matches!(
                cli.command,
                Command::Inhibit {
                    duration_ms: Some(actual)
                } if actual == expected_ms
            ));
        }
    }

    #[test]
    fn inhibit_without_duration_is_a_toggle() {
        let cli = Cli::try_parse_from(["mithshell", "inhibit"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Inhibit { duration_ms: None }
        ));
    }

    #[test]
    fn rejects_invalid_notification_inhibit_durations() {
        for value in ["0m", "1", "1x", "1.5h", "h", "18446744073709551615d"] {
            assert!(Cli::try_parse_from(["mithshell", "inhibit", value]).is_err());
        }
    }
}
