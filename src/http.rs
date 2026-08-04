//! Shared blocking HTTP client for background pollers (weather, and future
//! network-backed features). Built on `ureq` rather than an async client:
//! nothing else in this codebase runs an async executor, and every network
//! call here happens on a dedicated `thread::spawn` poller, so blocking I/O
//! is the right shape.

use std::{sync::LazyLock, time::Duration};

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use ureq::Agent;

const TIMEOUT: Duration = Duration::from_secs(8);

static AGENT: LazyLock<Agent> = LazyLock::new(|| {
    Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into()
});

/// Fetches `url` and returns the response body as a string.
pub fn get(url: &str) -> Result<String> {
    AGENT
        .get(url)
        .call()
        .with_context(|| format!("request to {url} failed"))?
        .body_mut()
        .read_to_string()
        .context("failed to read response body")
}

/// Fetches `url` and parses the response body as JSON.
pub fn get_json<T: DeserializeOwned>(url: &str) -> Result<T> {
    let body = get(url)?;
    serde_json::from_str(&body).context("invalid JSON response")
}
