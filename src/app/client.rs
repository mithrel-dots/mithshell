use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::CommandFactory;

use crate::{
    cli::{self, Cli, Command, ThemeCommand, ThemeModeArg},
    config::{self, ThemeMode, ThemeSource},
    ipc::{self, IpcCommand, MonitorTarget, OsdKind, Request, Response},
};

pub(super) fn generate_completions(shell: clap_complete::Shell) {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
}

pub(super) fn run(socket_path: PathBuf, command: Command) -> Result<()> {
    let is_latency = matches!(command, Command::Latency { .. });
    let is_palette = matches!(
        command,
        Command::Theme {
            command: ThemeCommand::Palette
        }
    );
    let (request, print_json) = command_request(command)?;
    let response = ipc::send(&socket_path, &request)?;
    if print_json {
        println!("{}", serde_json::to_string_pretty(&response)?);
    } else if is_latency {
        print_latency_report(&response);
    } else if is_palette {
        print_palette_swatches(&response);
    } else if let Some(data) = &response.data {
        println!("{}", serde_json::to_string_pretty(data)?);
    } else {
        println!("{}", response.message);
    }
    if !response.ok {
        bail!(response.message);
    }
    Ok(())
}

const PALETTE_ROLES: &[(&str, &str)] = &[
    ("primary", "Primary"),
    ("on_primary", "On Primary"),
    ("primary_container", "Primary Container"),
    ("on_primary_container", "On Primary Container"),
    ("secondary", "Secondary"),
    ("tertiary", "Tertiary"),
    ("surface", "Surface"),
    ("surface_container_low", "Surface Container Low"),
    ("surface_container", "Surface Container"),
    ("surface_container_high", "Surface Container High"),
    ("on_surface", "On Surface"),
    ("on_surface_variant", "On Surface Variant"),
    ("outline", "Outline"),
    ("outline_variant", "Outline Variant"),
    ("error", "Error"),
];

fn print_palette_swatches(response: &Response) {
    use std::io::IsTerminal;

    let Some(data) = &response.data else {
        println!("{}", response.message);
        return;
    };
    let colorize = std::io::stdout().is_terminal();
    if let Some(source) = data.get("source").and_then(serde_json::Value::as_str) {
        let mode = data
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        println!("source: {source}  mode: {mode}");
        println!();
    }
    for (key, label) in PALETTE_ROLES {
        let Some(hex) = data.get(*key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let square = match (colorize, parse_hex_rgb(hex)) {
            (true, Some((r, g, b))) => format!("\x1b[48;2;{r};{g};{b}m   \x1b[0m"),
            _ => "   ".to_owned(),
        };
        println!("{square}  {label:<24} {hex}");
    }
}

fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

fn print_latency_report(response: &Response) {
    let Some(spans) = response
        .data
        .as_ref()
        .and_then(|data| data.get("spans"))
        .and_then(|spans| spans.as_object())
    else {
        println!("{}", response.message);
        return;
    };
    if spans.is_empty() {
        println!("{}", response.message);
        println!("no samples recorded yet");
        return;
    }
    println!("Mithshell search latency");
    println!();
    println!(
        "{:<10}  {:>5}  {:>8}  {:>8}  {:>8}  {:>8}  {:>8}",
        "Span", "Runs", "Avg ms", "Min ms", "P50 ms", "P95 ms", "Max ms"
    );
    println!("{}", "-".repeat(68));
    const PIPELINE_ORDER: &[&str] = &["debounce", "write", "backend", "build", "paint", "total"];
    for name in PIPELINE_ORDER {
        let Some(span) = spans.get(*name) else {
            continue;
        };
        let number = |key: &str| {
            span.get(key)
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0)
        };
        let count = span
            .get("count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        println!(
            "{:<10}  {:>5}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}  {:>8.2}",
            name,
            count,
            number("avg_ms"),
            number("min_ms"),
            number("p50_ms"),
            number("p95_ms"),
            number("max_ms")
        );
    }
}

fn command_request(command: Command) -> Result<(Request, bool)> {
    let mut json = false;
    let command = match command {
        Command::Toggle(args) => IpcCommand::Toggle {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Open(args) => IpcCommand::Open {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Search(args) => IpcCommand::Search {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Weather(args) => IpcCommand::Weather {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Close(args) => IpcCommand::Close {
            monitor: MonitorTarget::parse(&args.monitor)?,
        },
        Command::Osd {
            kind,
            value,
            timeout,
            monitor,
        } => IpcCommand::Osd {
            monitor: MonitorTarget::parse(&monitor.monitor)?,
            kind: match kind {
                cli::OsdKind::Volume => OsdKind::Volume,
                cli::OsdKind::Brightness => OsdKind::Brightness,
                cli::OsdKind::Workspace => OsdKind::Workspace,
            },
            value,
            timeout_ms: timeout,
        },
        Command::Lock => IpcCommand::Lock,
        Command::Unlock => IpcCommand::Unlock,
        Command::Reload => IpcCommand::Reload,
        Command::Inhibit { duration_ms } => IpcCommand::Inhibit { duration_ms },
        Command::Status { json: print_json } => {
            json = print_json;
            IpcCommand::Status
        }
        Command::Latency {
            json: print_json,
            reset,
        } => {
            json = print_json;
            IpcCommand::Latency { reset }
        }
        Command::Theme { command } => match command {
            ThemeCommand::Set {
                image,
                color,
                mode,
                persist,
            } => {
                let source = if let Some(path) = image {
                    let path = config::expand_home(path);
                    let path = path.canonicalize().with_context(|| {
                        format!("failed to resolve theme image {}", path.display())
                    })?;
                    ThemeSource::Image { path }
                } else {
                    ThemeSource::Color {
                        value: color.context("--image or --color is required")?,
                    }
                };
                IpcCommand::ThemeSet {
                    source,
                    mode: mode.map(theme_mode),
                    persist,
                }
            }
            ThemeCommand::Mode { mode, persist } => IpcCommand::ThemeMode {
                mode: theme_mode(mode),
                persist,
            },
            ThemeCommand::Current { json: print_json } => {
                json = print_json;
                IpcCommand::ThemeCurrent
            }
            ThemeCommand::Palette => IpcCommand::ThemeCurrent,
            ThemeCommand::Reset => IpcCommand::ThemeReset,
        },
        Command::Daemon { .. } => bail!("daemon command cannot be sent over IPC"),
        Command::Completions { .. } => bail!("completions are generated locally, not over IPC"),
        Command::Setup { .. } => bail!("setup commands are handled locally, not over IPC"),
    };
    Ok((Request::new(command), json))
}

fn theme_mode(mode: ThemeModeArg) -> ThemeMode {
    match mode {
        ThemeModeArg::Dark => ThemeMode::Dark,
        ThemeModeArg::Light => ThemeMode::Light,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_hex_rgb;

    #[test]
    fn palette_rgb_rejects_malformed_and_multibyte_input() {
        assert_eq!(parse_hex_rgb("#9Aa7ff"), Some((154, 167, 255)));
        for value in ["#a€bc", "#gg0000", "#fff", "123456"] {
            assert_eq!(parse_hex_rgb(value), None, "{value}");
        }
    }
}
