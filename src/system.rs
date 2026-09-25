use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use log::{debug, warn};

use crate::state::{
    AudioState, BatteryState, BrightnessState, HardwareSnapshot, SystemInfoState, SystemSnapshot,
};

/// Controller-owned cache replayed to new windows and IPC status readers.
/// The launch-only override survives hardware samples and UI/config rebuilds.
pub(crate) struct SystemState {
    snapshot: SystemSnapshot,
    test_battery: Option<u8>,
}

impl SystemState {
    pub(crate) fn new(test_battery: Option<u8>) -> Self {
        let mut state = Self {
            snapshot: SystemSnapshot::default(),
            test_battery,
        };
        state.update(SystemSnapshot::default());
        state
    }

    pub(crate) fn update(&mut self, mut snapshot: SystemSnapshot) {
        snapshot.hardware = self.snapshot.hardware.clone();
        if let Some(percent) = self.test_battery {
            snapshot.battery = Some(BatteryState {
                percent,
                status: if percent == 100 {
                    "Full"
                } else {
                    "Discharging"
                }
                .into(),
            });
        }
        self.snapshot = snapshot;
    }

    pub(crate) fn update_audio(&mut self, audio: AudioState) {
        self.snapshot.audio = Some(audio);
    }

    pub(crate) fn update_hardware(&mut self, hardware: HardwareSnapshot) {
        self.snapshot.hardware = hardware;
    }

    pub(crate) fn snapshot(&self) -> &SystemSnapshot {
        &self.snapshot
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    PowerOff,
    Suspend,
    Reboot,
}

impl PowerAction {
    fn systemctl_argument(self) -> &'static str {
        match self {
            Self::PowerOff => "poweroff",
            Self::Suspend => "suspend",
            Self::Reboot => "reboot",
        }
    }
}

pub fn snapshot() -> SystemSnapshot {
    SystemSnapshot {
        audio: query_audio().ok(),
        brightness: query_brightness().ok().flatten(),
        battery: query_battery().ok().flatten(),
        info: query_system_info().ok(),
        hardware: HardwareSnapshot::default(),
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

pub fn toggle_mute() -> Result<()> {
    let status = Command::new("wpctl")
        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
        .status()
        .context("failed to run wpctl")?;
    if !status.success() {
        bail!("wpctl set-mute failed");
    }
    Ok(())
}

pub fn unmute() -> Result<()> {
    let status = Command::new("wpctl")
        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "0"])
        .status()
        .context("failed to run wpctl")?;
    if !status.success() {
        bail!("wpctl set-mute failed");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioMutation {
    SetVolume(u8),
    ToggleMute,
}

/// Execute queued user mutations serially so a slider's unmute/set sequence
/// cannot race a later or earlier mute toggle.
pub fn start_audio_mutation_worker(
    receiver: async_channel::Receiver<AudioMutation>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || run_audio_mutations(receiver, &mut WpctlAudioBackend))
}

trait AudioMutationBackend {
    fn apply(&mut self, operation: AudioOperation) -> Result<()>;
}
struct WpctlAudioBackend;
impl AudioMutationBackend for WpctlAudioBackend {
    fn apply(&mut self, operation: AudioOperation) -> Result<()> {
        match operation {
            AudioOperation::Unmute => unmute(),
            AudioOperation::SetVolume(value) => set_volume(value),
            AudioOperation::ToggleMute => toggle_mute(),
        }
    }
}
fn run_audio_mutations(
    receiver: async_channel::Receiver<AudioMutation>,
    backend: &mut impl AudioMutationBackend,
) {
    while let Ok(mutation) = receiver.recv_blocking() {
        for operation in audio_mutation_operations(mutation) {
            if let Err(error) = backend.apply(operation) {
                warn!("audio mutation failed: {error:#}");
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioOperation {
    Unmute,
    SetVolume(u8),
    ToggleMute,
}
fn audio_mutation_operations(mutation: AudioMutation) -> Vec<AudioOperation> {
    match mutation {
        AudioMutation::SetVolume(value) => vec![
            AudioOperation::Unmute,
            AudioOperation::SetVolume(value.min(100)),
        ],
        AudioMutation::ToggleMute => vec![AudioOperation::ToggleMute],
    }
}

pub fn query_brightness() -> Result<Option<BrightnessState>> {
    if !brightnessctl_available() {
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

pub fn query_battery() -> Result<Option<BatteryState>> {
    let root = Path::new("/sys/class/power_supply");
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(None);
    };
    let mut batteries: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            fs::read_to_string(path.join("type"))
                .unwrap_or_default()
                .trim()
                == "Battery"
        })
        .collect();
    // HID devices can expose a second, often-zero battery (for example a
    // touchpad or stylus). Prefer the kernel's conventional BAT* device and
    // make the choice independent of power_supply enumeration order.
    batteries.sort_by_key(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        (!name.starts_with("BAT"), name.to_owned())
    });

    for path in batteries {
        let Ok(percent) = read_u64(&path.join("capacity")) else {
            continue;
        };
        let status = fs::read_to_string(path.join("status"))
            .unwrap_or_else(|_| "Unknown".into())
            .trim()
            .to_owned();
        return Ok(Some(BatteryState {
            percent: percent.min(100) as u8,
            status,
        }));
    }
    Ok(None)
}

pub fn query_system_info() -> Result<SystemInfoState> {
    let hostname = fs::read_to_string("/proc/sys/kernel/hostname")
        .context("failed to read the hostname")?
        .trim()
        .to_owned();
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let os_name = parse_os_name(&os_release).unwrap_or_else(|| "Linux".to_owned());
    let uptime = fs::read_to_string("/proc/uptime").context("failed to read system uptime")?;
    let uptime_seconds = parse_uptime(&uptime)?;
    Ok(SystemInfoState {
        hostname,
        os_name,
        uptime_seconds,
    })
}

pub fn request_power(action: PowerAction) -> Result<()> {
    let argument = action.systemctl_argument();
    let output = Command::new("systemctl")
        .arg(argument)
        .output()
        .with_context(|| format!("failed to run systemctl {argument}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if detail.is_empty() {
            bail!("systemctl {argument} failed");
        }
        bail!("systemctl {argument} failed: {detail}");
    }
    Ok(())
}

/// Whether `brightnessctl` can be run at all, probed once per process before
/// reading the current backlight state.
fn brightnessctl_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Command::new("brightnessctl")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
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

fn parse_os_name(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        line.strip_prefix("PRETTY_NAME=")
            .map(|value| value.trim_matches('"').replace("\\\"", "\""))
            .filter(|value| !value.is_empty())
    })
}

fn parse_uptime(contents: &str) -> Result<u64> {
    let seconds = contents
        .split_whitespace()
        .next()
        .context("uptime did not contain a value")?
        .parse::<f64>()
        .context("uptime was not numeric")?;
    if !seconds.is_finite() || seconds.is_sign_negative() {
        bail!("uptime was outside its valid range");
    }
    Ok(seconds.floor() as u64)
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

    fn device_sample(battery: Option<BatteryState>) -> SystemSnapshot {
        SystemSnapshot {
            battery,
            hardware: HardwareSnapshot::default(),
            audio: Some(AudioState {
                percent: 23,
                muted: true,
            }),
            brightness: Some(BrightnessState {
                percent: 81,
                device: "test-backlight".into(),
            }),
            info: Some(SystemInfoState {
                hostname: "test-desktop".into(),
                os_name: "Test Linux".into(),
                uptime_seconds: 120,
            }),
        }
    }

    #[test]
    fn simulated_battery_survives_device_samples_and_cached_replay() {
        for percent in [0, 1, 50, 99, 100] {
            let status = if percent == 100 {
                "Full"
            } else {
                "Discharging"
            };
            let mut state = SystemState::new(Some(percent));
            // The first monitor can be created before any poll has completed.
            let initial = state.snapshot().battery.as_ref().unwrap();
            assert_eq!(initial.percent, percent);
            assert_eq!(initial.status, status);

            for battery in [
                None,
                Some(BatteryState {
                    percent: 72,
                    status: "Charging".into(),
                }),
                Some(BatteryState {
                    percent: 100,
                    status: "Full".into(),
                }),
                None,
            ] {
                let sample = device_sample(battery);
                let mut expected = serde_json::to_value(&sample).unwrap();
                expected["battery"] = serde_json::json!({ "percent": percent, "status": status });
                state.update(sample);
                // Existing windows consume this cache after a poll. Reloads,
                // new monitors, and the lock screen replay the same snapshot.
                for _ in 0..3 {
                    let replay = state.snapshot().clone();
                    assert_eq!(serde_json::to_value(replay).unwrap(), expected);
                }
                assert_eq!(
                    state.snapshot().brightness.as_ref().unwrap().device,
                    "test-backlight"
                );
            }
        }
    }

    #[test]
    fn audio_events_preserve_simulated_battery_and_other_cached_state() {
        let mut state = SystemState::new(Some(42));
        state.update(device_sample(None));
        let mut expected = serde_json::to_value(state.snapshot()).unwrap();
        let audio = AudioState {
            percent: 65,
            muted: false,
        };
        expected["audio"] = serde_json::to_value(audio).unwrap();
        state.update_audio(audio);
        assert_eq!(serde_json::to_value(state.snapshot()).unwrap(), expected);
    }

    #[test]
    fn hardware_samples_survive_ten_second_system_refreshes() {
        let mut state = SystemState::new(None);
        let hardware = HardwareSnapshot {
            cpu_percent: Some(37.5),
            memory_used_bytes: Some(10),
            ..HardwareSnapshot::default()
        };
        state.update_hardware(hardware.clone());
        state.update(device_sample(None));
        assert_eq!(state.snapshot().hardware, hardware);
        state.update_audio(AudioState {
            percent: 50,
            muted: false,
        });
        assert_eq!(state.snapshot().hardware.cpu_percent, Some(37.5));
    }

    #[test]
    fn ordinary_system_state_preserves_hardware_and_does_not_inherit_simulation() {
        let simulated = SystemState::new(Some(42));
        assert!(simulated.snapshot().battery.is_some());
        let mut state = SystemState::new(None);
        assert_eq!(
            serde_json::to_value(state.snapshot()).unwrap(),
            serde_json::to_value(SystemSnapshot::default()).unwrap()
        );
        for battery in [
            None,
            Some(BatteryState {
                percent: 72,
                status: "Charging".into(),
            }),
            Some(BatteryState {
                percent: 100,
                status: "Full".into(),
            }),
            None,
        ] {
            let sample = device_sample(battery);
            let expected = serde_json::to_value(&sample).unwrap();
            state.update(sample);
            assert_eq!(serde_json::to_value(state.snapshot()).unwrap(), expected);
        }
    }

    #[test]
    fn parses_wpctl_volume_and_mute() {
        assert_eq!(parse_wpctl("Volume: 0.42").unwrap().percent, 42);
        let muted = parse_wpctl("Volume: 0.75 [MUTED]").unwrap();
        assert_eq!(muted.percent, 75);
        assert!(muted.muted);
    }

    #[test]
    fn audio_mutation_queue_preserves_slider_and_mute_intent_order() {
        assert_eq!(
            audio_mutation_operations(AudioMutation::SetVolume(121)),
            vec![AudioOperation::Unmute, AudioOperation::SetVolume(100)]
        );
        assert_eq!(
            audio_mutation_operations(AudioMutation::ToggleMute),
            vec![AudioOperation::ToggleMute]
        );
        let ordered = [
            AudioMutation::ToggleMute,
            AudioMutation::SetVolume(25),
            AudioMutation::ToggleMute,
        ]
        .into_iter()
        .flat_map(audio_mutation_operations)
        .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            vec![
                AudioOperation::ToggleMute,
                AudioOperation::Unmute,
                AudioOperation::SetVolume(25),
                AudioOperation::ToggleMute
            ]
        );
    }

    #[test]
    fn audio_worker_dispatches_fifo_mutations_to_its_backend() {
        #[derive(Default)]
        struct FakeBackend(Vec<AudioOperation>);
        impl AudioMutationBackend for FakeBackend {
            fn apply(&mut self, operation: AudioOperation) -> Result<()> {
                self.0.push(operation);
                Ok(())
            }
        }
        let (sender, receiver) = async_channel::unbounded();
        sender.send_blocking(AudioMutation::ToggleMute).unwrap();
        sender.send_blocking(AudioMutation::SetVolume(30)).unwrap();
        sender.send_blocking(AudioMutation::ToggleMute).unwrap();
        drop(sender);
        let mut backend = FakeBackend::default();
        run_audio_mutations(receiver, &mut backend);
        assert_eq!(
            backend.0,
            vec![
                AudioOperation::ToggleMute,
                AudioOperation::Unmute,
                AudioOperation::SetVolume(30),
                AudioOperation::ToggleMute
            ]
        );
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

    #[test]
    fn parses_system_information_sources() {
        assert_eq!(
            parse_os_name("NAME=Example\nPRETTY_NAME=\"Example Linux 42\"\n"),
            Some("Example Linux 42".to_owned())
        );
        assert_eq!(parse_uptime("93784.42 100.00\n").unwrap(), 93_784);
    }

    #[test]
    fn maps_power_actions_to_systemctl_arguments() {
        assert_eq!(PowerAction::PowerOff.systemctl_argument(), "poweroff");
        assert_eq!(PowerAction::Suspend.systemctl_argument(), "suspend");
        assert_eq!(PowerAction::Reboot.systemctl_argument(), "reboot");
    }

    #[test]
    fn conventional_batteries_sort_before_hid_batteries() {
        let mut names = ["hid-foo-battery", "BAT0", "BAT1"];
        names.sort_by_key(|name| (!name.starts_with("BAT"), (*name).to_owned()));
        assert_eq!(names, ["BAT0", "BAT1", "hid-foo-battery"]);
    }
}
