use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

pub fn config_path(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(expand_home(path));
    }
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("mithshell/config.toml"))
}

/// Optional higher-priority user stylesheet alongside the selected config.
pub fn colors_css_path(config_path: &Path) -> PathBuf {
    config_path.with_file_name("colors.css")
}

/// GTK override watched for palette changes made by external theme generators.
pub fn gtk_user_css_path() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("gtk-4.0/gtk.css"))
}

pub fn state_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("mithshell"))
}

pub fn cache_dir() -> Result<PathBuf> {
    Ok(xdg_dir("XDG_CACHE_HOME", ".cache")?.join("mithshell"))
}

pub fn runtime_dir() -> Result<PathBuf> {
    let directory = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is not set")?;
    Ok(directory.join("mithshell"))
}

pub fn default_socket_path() -> Result<PathBuf> {
    Ok(runtime_dir()?.join("ipc.sock"))
}

pub fn expand_home(path: PathBuf) -> PathBuf {
    let string = path.to_string_lossy();
    if string == "~" {
        return env::var_os("HOME").map(PathBuf::from).unwrap_or(path);
    }
    if let Some(rest) = string.strip_prefix("~/")
        && let Some(home) = env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path
}

/// Resolves an XDG base directory, falling back to `$HOME/{fallback}`.
pub(crate) fn xdg_dir(variable: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(path) = env::var_os(variable) {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(fallback))
}
