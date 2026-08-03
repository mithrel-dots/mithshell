use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use log::{debug, warn};

use crate::state::{AudioState, BatteryState, BrightnessState, SystemSnapshot};

pub fn snapshot() -> SystemSnapshot {
    SystemSnapshot {
        audio: query_audio().ok(),
        brightness: query_brightness().ok().flatten(),
        battery: query_battery().ok().flatten(),
    }
}

pub fn start_poller(sender: Sender<SystemSnapshot>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            if sender.send_blocking(snapshot()).is_err() {
                return;
            }
            thread::sleep(Duration::from_secs(10));
        }
    })
}

pub fn start_audio_listener(sender: Sender<AudioState>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut previous = query_audio().ok();
        loop {
            let mut child = match Command::new("pactl")
                .arg("subscribe")
                .env("LC_ALL", "C")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) => {
                    warn!("failed to monitor audio changes with pactl: {error}");
                    thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill();
                thread::sleep(Duration::from_secs(5));
                continue;
            };

            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if !is_audio_subscription_event(&line) {
                    continue;
                }
                let Ok(audio) = query_audio() else {
                    continue;
                };
                if update_audio_state(&mut previous, audio) && sender.send_blocking(audio).is_err()
                {
                    let _ = child.kill();
                    return;
                }
            }

            let _ = child.wait();
            if sender.is_closed() {
                return;
            }
            warn!("pactl audio monitor stopped; reconnecting");
            thread::sleep(Duration::from_secs(2));
        }
    })
}

pub fn query_audio() -> Result<AudioState> {
    let output = Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .context("failed to run wpctl")?;
    if !output.status.success() {
        bail!("wpctl get-volume failed");
    }
    parse_wpctl(&String::from_utf8_lossy(&output.stdout))
}

pub fn set_volume(percent: u8) -> Result<()> {
    let status = Command::new("wpctl")
        .args([
            "set-volume",
            "@DEFAULT_AUDIO_SINK@",
            &format!("{}%", percent.min(100)),
        ])
        .status()
        .context("failed to run wpctl")?;
    if !status.success() {
        bail!("wpctl set-volume failed");
    }
    Ok(())
}

pub fn query_brightness() -> Result<Option<BrightnessState>> {
    // Keep the dashboard row hidden unless the optional brightnessctl
    // integration is actually installed. Some systems expose backlight
    // sysfs nodes without a usable user-facing control command.
    let available = Command::new("brightnessctl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !available {
        return Ok(None);
    }
    let Some(device) = backlight_devices()?.into_iter().next() else {
        return Ok(None);
    };
    let current = read_u64(&device.join("brightness"))?;
    let maximum = read_u64(&device.join("max_brightness"))?;
    if maximum == 0 {
        return Ok(None);
    }
    Ok(Some(BrightnessState {
        percent: ((current * 100) / maximum).min(100) as u8,
        device: device.to_string_lossy().into_owned(),
    }))
}

pub fn set_brightness(percent: u8) -> Result<()> {
    let device = backlight_devices()?
        .into_iter()
        .next()
        .context("no backlight device is available")?;
    let maximum = read_u64(&device.join("max_brightness"))?;
    let value = (maximum * u64::from(percent.min(100))) / 100;
    fs::write(device.join("brightness"), value.to_string())
        .context("failed to write backlight brightness; check device permissions")
}

pub fn query_battery() -> Result<Option<BatteryState>> {
    let root = Path::new("/sys/class/power_supply");
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(None);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if fs::read_to_string(path.join("type"))
            .unwrap_or_default()
            .trim()
            != "Battery"
        {
            continue;
        }
        let percent = read_u64(&path.join("capacity"))?.min(100) as u8;
        let status = fs::read_to_string(path.join("status"))
            .unwrap_or_else(|_| "Unknown".into())
            .trim()
            .to_owned();
        return Ok(Some(BatteryState { percent, status }));
    }
    Ok(None)
}

fn parse_wpctl(value: &str) -> Result<AudioState> {
    let scalar = value
        .split_whitespace()
        .find_map(|part| part.parse::<f64>().ok())
        .context("wpctl output did not contain a volume")?;
    Ok(AudioState {
        percent: (scalar * 100.0).round().clamp(0.0, 100.0) as u8,
        muted: value.contains("[MUTED]"),
    })
}

fn is_audio_subscription_event(line: &str) -> bool {
    line.contains(" on sink ") || line.contains(" on server ") || line.contains(" on card ")
}

fn update_audio_state(previous: &mut Option<AudioState>, audio: AudioState) -> bool {
    let changed = *previous != Some(audio);
    *previous = Some(audio);
    changed
}

fn backlight_devices() -> Result<Vec<PathBuf>> {
    let root = Path::new("/sys/class/backlight");
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(Vec::new());
    };
    let mut devices: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    devices.sort();
    debug!("found {} backlight devices", devices.len());
    Ok(devices)
}

fn read_u64(path: &Path) -> Result<u64> {
    fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?
        .trim()
        .parse()
        .with_context(|| format!("invalid number in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl_volume_and_mute() {
        assert_eq!(parse_wpctl("Volume: 0.42").unwrap().percent, 42);
        let muted = parse_wpctl("Volume: 0.75 [MUTED]").unwrap();
        assert_eq!(muted.percent, 75);
        assert!(muted.muted);
    }

    #[test]
    fn filters_audio_subscription_events() {
        assert!(is_audio_subscription_event("Event 'change' on sink #42"));
        assert!(is_audio_subscription_event("Event 'change' on server #0"));
        assert!(is_audio_subscription_event("Event 'change' on card #3"));
        assert!(!is_audio_subscription_event("Event 'new' on sink-input #9"));
        assert!(!is_audio_subscription_event("Event 'change' on source #2"));
    }

    #[test]
    fn treats_first_valid_audio_state_as_an_update() {
        let audio = AudioState {
            percent: 42,
            muted: false,
        };
        let mut previous = None;
        assert!(update_audio_state(&mut previous, audio));
        assert!(!update_audio_state(&mut previous, audio));
    }
}
