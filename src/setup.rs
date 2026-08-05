//! Installs and configures optional integrations, starting with the
//! TarraGon launcher backend.
//!
//! This module is a standalone local operation: unlike every other
//! subcommand it never talks to the running daemon over the IPC socket
//! (see the `Command::Setup` special case in `app::run`, handled the same
//! way as `Command::Completions`), so it works even before mithshell has
//! ever been started.

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use wait_timeout::ChildExt;

use crate::{cli::SetupTarragonArgs, config};

/// How long a `--version` probe is given before it is assumed hung and
/// killed. Only used to detect whether a tool is on PATH at all.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Config file names TarraGon's own binary might generate, checked in the
/// same directory. Mithshell does not own this format, so it only checks
/// for existence and otherwise defers entirely to `tarragon config generate`.
const TARRAGON_CONFIG_NAMES: &[&str] = &["tarragon.toml", "tarragon.yaml", "tarragon.json"];

/// Relative locations a built `tarragon` binary might end up at, checked in
/// order. TarraGon's own `justfile` build recipe is outside mithshell's
/// control, so more than one convention is worth checking before failing.
const BUILT_BINARY_CANDIDATES: &[&str] = &["build/tarragon", "bin/tarragon", "tarragon"];

pub fn install_tarragon(args: SetupTarragonArgs) -> Result<()> {
    println!("Checking for required tools...");
    require_tool("git", "git")?;
    require_tool("go", "a Go toolchain (https://go.dev/dl/)")?;

    let src_dir = resolve_src_dir(args.src_dir.as_deref(), &config::cache_dir()?);
    let already_installed = command_exists("tarragon");
    let rebuilding = should_build(already_installed, args.force);

    if !rebuilding {
        println!("tarragon is already on PATH; skipping clone and build (pass --force to rebuild)");
    } else {
        fetch_source(&args.repo, &src_dir)?;
        println!("Building TarraGon...");
        let binary = build_tarragon(&src_dir)?;
        println!("Installing tarragon to ~/.local/bin...");
        install_binary(&binary)?;
    }

    println!("Checking for an existing TarraGon config...");
    ensure_config_generated()?;

    if args.no_service {
        println!("Skipping systemd service installation (--no-service)");
    } else {
        println!("Installing the TarraGon systemd user service...");
        install_service(&src_dir)?;
    }

    print_summary(rebuilding, args.no_service);
    Ok(())
}

/// Whether the binary needs to be (re)built: only skipped when it is
/// already reachable on PATH and the caller did not ask to force it.
fn should_build(already_on_path: bool, force: bool) -> bool {
    force || !already_on_path
}

/// Resolves the checkout directory: the explicit override (expanded for a
/// leading `~`) when given, otherwise `cache_dir/tarragon-src`.
fn resolve_src_dir(explicit: Option<&Path>, cache_dir: &Path) -> PathBuf {
    explicit.map_or_else(
        || cache_dir.join("tarragon-src"),
        |path| config::expand_home(path.to_path_buf()),
    )
}

fn require_tool(binary: &str, install_hint: &str) -> Result<()> {
    if command_exists(binary) {
        Ok(())
    } else {
        bail!("`{binary}` was not found on PATH; install {install_hint}")
    }
}

/// Probes whether `name` is runnable at all, tolerating a nonzero exit (a
/// `--version` flag not being recognized still proves the binary exists).
fn command_exists(name: &str) -> bool {
    let mut child = match Command::new(name)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    match child.wait_timeout(PROBE_TIMEOUT) {
        Ok(Some(_status)) => true,
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
            false
        }
        Err(_) => false,
    }
}

fn run_command(command: &mut Command, action: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to run {action}"))?;
    if !status.success() {
        bail!("{action} failed with {status}");
    }
    Ok(())
}

/// Clones a fresh checkout, or fast-forward pulls one that already exists.
fn fetch_source(repo: &str, src_dir: &Path) -> Result<()> {
    if src_dir.join(".git").is_dir() {
        println!("Updating existing checkout at {}...", src_dir.display());
        run_command(
            Command::new("git")
                .arg("-C")
                .arg(src_dir)
                .args(["pull", "--ff-only"]),
            "git pull --ff-only",
        )
    } else {
        if let Some(parent) = src_dir.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        if src_dir.is_dir() {
            bail!(
                "{} already exists but is not a git checkout; remove it or pass a different --src-dir",
                src_dir.display()
            );
        }
        println!("Cloning {repo} into {}...", src_dir.display());
        run_command(
            Command::new("git")
                .args(["clone", "--depth", "1", repo])
                .arg(src_dir),
            "git clone",
        )
    }
}

/// Builds the checkout, preferring its own `justfile` recipe, and returns
/// the path to the resulting binary.
fn build_tarragon(src_dir: &Path) -> Result<PathBuf> {
    if src_dir.join("justfile").is_file() {
        run_command(
            Command::new("just").arg("build").current_dir(src_dir),
            "just build",
        )?;
    } else {
        run_command(
            Command::new("go")
                .args(["build", "-o", "build/tarragon", "./cmd/"])
                .current_dir(src_dir),
            "go build",
        )?;
    }
    find_built_binary(src_dir).with_context(|| {
        format!(
            "build finished but no binary was found under {} (checked: {})",
            src_dir.display(),
            BUILT_BINARY_CANDIDATES.join(", ")
        )
    })
}

fn find_built_binary(src_dir: &Path) -> Option<PathBuf> {
    BUILT_BINARY_CANDIDATES
        .iter()
        .map(|relative| src_dir.join(relative))
        .find(|candidate| candidate.is_file())
}

/// Copies (not symlinks, since the source checkout may later be deleted or
/// moved) the built binary to `~/.local/bin/tarragon` and marks it
/// executable.
fn install_binary(binary: &Path) -> Result<()> {
    let bin_dir = home_dir()?.join(".local/bin");
    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to create {}", bin_dir.display()))?;
    let dest = bin_dir.join("tarragon");
    fs::copy(binary, &dest)
        .with_context(|| format!("failed to copy {} to {}", binary.display(), dest.display()))?;
    let mut permissions = fs::metadata(&dest)
        .with_context(|| format!("failed to read metadata for {}", dest.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&dest, permissions)
        .with_context(|| format!("failed to make {} executable", dest.display()))?;
    println!("Installed {}", dest.display());
    Ok(())
}

/// Generates a default TarraGon config through its own binary, unless one
/// already exists. Mithshell does not hand-write TarraGon's config format,
/// since it changes independently of mithshell.
fn ensure_config_generated() -> Result<()> {
    let config_dir = config::xdg_dir("XDG_CONFIG_HOME", ".config")?.join("tarragon");
    if let Some(existing) = find_existing_config(&config_dir) {
        println!("Keeping existing config at {}", existing.display());
        return Ok(());
    }
    println!("Generating a default TarraGon config...");
    run_command(
        Command::new("tarragon").args(["config", "generate"]),
        "tarragon config generate",
    )
}

fn find_existing_config(config_dir: &Path) -> Option<PathBuf> {
    TARRAGON_CONFIG_NAMES
        .iter()
        .map(|name| config_dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Installs and enables the systemd user unit shipped in the checkout.
/// Missing the unit file is reported as a warning rather than a hard
/// failure, since a `--repo`/`--src-dir` pointed at something unexpected
/// should not fail the whole command.
fn install_service(src_dir: &Path) -> Result<()> {
    let unit_source = src_dir.join("systemd/tarragon.service");
    if !unit_source.is_file() {
        println!(
            "warning: no systemd unit found at {}; skipping service installation",
            unit_source.display()
        );
        return Ok(());
    }

    let service_dir = config::xdg_dir("XDG_CONFIG_HOME", ".config")?.join("systemd/user");
    fs::create_dir_all(&service_dir)
        .with_context(|| format!("failed to create {}", service_dir.display()))?;
    let dest = service_dir.join("tarragon.service");
    fs::copy(&unit_source, &dest).with_context(|| {
        format!(
            "failed to copy {} to {}",
            unit_source.display(),
            dest.display()
        )
    })?;

    run_command(
        Command::new("systemctl").args(["--user", "daemon-reload"]),
        "systemctl --user daemon-reload",
    )?;
    run_command(
        Command::new("systemctl").args(["--user", "enable", "--now", "tarragon.service"]),
        "systemctl --user enable --now tarragon.service",
    )?;
    println!("Enabled tarragon.service");
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn print_summary(rebuilt: bool, skipped_service: bool) {
    println!();
    println!("TarraGon setup summary:");
    println!(
        "  binary:  {}",
        if rebuilt {
            "built and installed to ~/.local/bin/tarragon"
        } else {
            "already installed, left unchanged"
        }
    );
    println!(
        "  service: {}",
        if skipped_service {
            "skipped (--no-service)"
        } else {
            "installed and enabled (or a warning was printed above)"
        }
    );
    println!();
    println!("Try it with: mithshell search");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilds_when_forced_even_if_already_on_path() {
        assert!(should_build(true, true));
        assert!(should_build(false, true));
    }

    #[test]
    fn skips_build_when_already_on_path_and_not_forced() {
        assert!(!should_build(true, false));
    }

    #[test]
    fn builds_when_nothing_is_on_path() {
        assert!(should_build(false, false));
    }

    #[test]
    fn explicit_src_dir_is_preferred_and_home_expanded() {
        let cache_dir = Path::new("/cache/mithshell");
        assert_eq!(
            resolve_src_dir(Some(Path::new("/explicit/dir")), cache_dir),
            PathBuf::from("/explicit/dir")
        );
    }

    #[test]
    fn falls_back_to_cache_dir_when_src_dir_is_unset() {
        let cache_dir = Path::new("/cache/mithshell");
        assert_eq!(
            resolve_src_dir(None, cache_dir),
            PathBuf::from("/cache/mithshell/tarragon-src")
        );
    }

    fn temporary_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mithshell-setup-test-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ))
    }

    #[test]
    fn finds_no_existing_config_in_an_empty_directory() {
        let dir = temporary_dir("empty-config");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(find_existing_config(&dir), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_an_existing_config_by_any_supported_extension() {
        let dir = temporary_dir("yaml-config");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tarragon.yaml"), "").unwrap();
        assert_eq!(find_existing_config(&dir), Some(dir.join("tarragon.yaml")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_no_built_binary_in_an_empty_checkout() {
        let dir = temporary_dir("empty-checkout");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(find_built_binary(&dir), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_a_built_binary_at_any_candidate_location() {
        let dir = temporary_dir("built-checkout");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::write(dir.join("build/tarragon"), "").unwrap();
        assert_eq!(find_built_binary(&dir), Some(dir.join("build/tarragon")));
        let _ = fs::remove_dir_all(&dir);
    }
}
