//! Lightweight Linux hardware telemetry, sampled away from GTK's main thread.
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use async_channel::{Receiver, Sender, TrySendError};

use crate::state::HardwareSnapshot;

pub trait TelemetryService: Send + 'static {
    fn sample(&mut self) -> HardwareSnapshot;
}

/// A deliberate no-data implementation for platforms without the Linux
/// procfs/sysfs interfaces; each field remains `None`.
#[derive(Default)]
pub struct UnavailableTelemetry;
impl TelemetryService for UnavailableTelemetry {
    fn sample(&mut self) -> HardwareSnapshot {
        HardwareSnapshot::default()
    }
}

#[derive(Default)]
pub struct LinuxTelemetry {
    previous: Option<RawSample>,
    previous_at: Option<Instant>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RawSample {
    cpu: Option<(u64, u64)>,
    memory: Option<(u64, u64)>,
    network: Option<BTreeMap<String, (u64, u64)>>,
}

impl TelemetryService for LinuxTelemetry {
    fn sample(&mut self) -> HardwareSnapshot {
        let raw = RawSample {
            cpu: read_cpu(Path::new("/proc/stat")),
            memory: read_memory(Path::new("/proc/meminfo")),
            network: read_network(Path::new("/sys/class/net")),
        };
        let now = Instant::now();
        let elapsed = self
            .previous_at
            .map(|at| now.duration_since(at).as_secs_f64())
            .filter(|seconds| *seconds > 0.0)
            .unwrap_or(1.0);
        let (cpu_percent, network_receive_bytes_per_second, network_transmit_bytes_per_second) =
            self.previous.as_ref().map_or((None, None, None), |old| {
                let cpu = raw
                    .cpu
                    .zip(old.cpu)
                    .and_then(|(now, before)| cpu_delta(now, before));
                let rates = raw
                    .network
                    .as_ref()
                    .zip(old.network.as_ref())
                    .map(|(now, before)| network_delta(now, before, elapsed))
                    .unwrap_or((None, None));
                (cpu, rates.0, rates.1)
            });
        self.previous = Some(raw.clone());
        self.previous_at = Some(now);
        let memory = raw
            .memory
            .map(|(used, total)| (Some(used), Some(total)))
            .unwrap_or((None, None));
        HardwareSnapshot {
            cpu_percent,
            cpu_temperature_celsius: read_temperature(),
            memory_used_bytes: memory.0,
            memory_total_bytes: memory.1,
            network_receive_bytes_per_second,
            network_transmit_bytes_per_second,
        }
    }
}

fn cpu_delta((total, idle): (u64, u64), (old_total, old_idle): (u64, u64)) -> Option<f64> {
    let total_delta = total.checked_sub(old_total)?;
    let idle_delta = idle.checked_sub(old_idle)?;
    (total_delta > 0).then(|| {
        (100.0 * total_delta.saturating_sub(idle_delta) as f64 / total_delta as f64)
            .clamp(0.0, 100.0)
    })
}
pub fn start_sampler(
    sender: Sender<HardwareSnapshot>,
    receiver: Receiver<HardwareSnapshot>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut service: Box<dyn TelemetryService> = if Path::new("/proc/stat").exists() {
            Box::new(LinuxTelemetry::default())
        } else {
            Box::new(UnavailableTelemetry)
        };
        loop {
            if !send_latest(&sender, &receiver, service.sample()) {
                break;
            }
            thread::sleep(Duration::from_secs(1));
        }
    })
}

/// Publish without waiting for GTK. When its bounded slot is full, discard
/// the stale value and retry so a resumed UI receives the freshest sample.
fn send_latest(
    sender: &Sender<HardwareSnapshot>,
    receiver: &Receiver<HardwareSnapshot>,
    mut sample: HardwareSnapshot,
) -> bool {
    loop {
        // The sampler retains a receiver clone solely to evict a stale queue
        // item. If it is now the last receiver, the GTK/controller consumer
        // has gone away; do not keep the channel alive with our helper clone.
        if receiver.receiver_count() <= 1 {
            return false;
        }
        match sender.try_send(sample) {
            Ok(()) => return true,
            Err(TrySendError::Closed(_)) => return false,
            Err(TrySendError::Full(latest)) => {
                sample = latest;
                if receiver.try_recv().is_err() {
                    thread::yield_now();
                }
            }
        }
    }
}

fn read_cpu(path: &Path) -> Option<(u64, u64)> {
    parse_cpu(&fs::read_to_string(path).ok()?)
}

fn parse_cpu(contents: &str) -> Option<(u64, u64)> {
    let mut values = contents.lines().next()?.split_whitespace();
    if values.next()? != "cpu" {
        return None;
    }
    let values: Vec<u64> = values.map(str::parse).collect::<Result<_, _>>().ok()?;
    if values.len() < 4 {
        return None;
    }
    // Linux positions: user nice system idle iowait irq softirq steal guest
    // guest_nice. Guest ticks are already included in user/nice, so adding
    // fields 8 and 9 would count those ticks twice.
    let total = values.iter().take(8).copied().sum();
    let idle = values[3].saturating_add(*values.get(4).unwrap_or(&0));
    Some((total, idle))
}

fn read_memory(path: &Path) -> Option<(u64, u64)> {
    parse_memory(&fs::read_to_string(path).ok()?)
}
fn parse_memory(text: &str) -> Option<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        match parts.next()? {
            "MemTotal:" => total = parts.next()?.parse::<u64>().ok()?.checked_mul(1024),
            "MemAvailable:" => available = parts.next()?.parse::<u64>().ok()?.checked_mul(1024),
            _ => {}
        }
    }
    let total = total?;
    let available = available?;
    Some((total.saturating_sub(available), total))
}

/// Aggregate only interfaces backed by a device (physical NICs), excluding
/// loopback, bridges, containers and other virtual links from double counts.
fn read_network(root: &Path) -> Option<BTreeMap<String, (u64, u64)>> {
    let entries = fs::read_dir(root).ok()?;
    let mut counters = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.join("device").exists() {
            continue;
        }
        let stats = path.join("statistics");
        let read = |name| {
            fs::read_to_string(stats.join(name))
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        };
        if let (Some(r), Some(t)) = (read("rx_bytes"), read("tx_bytes")) {
            // Include ifindex so a device removed and recreated under the
            // same name cannot be mistaken for a continuous counter.
            let ifindex = fs::read_to_string(path.join("ifindex"))
                .ok()?
                .trim()
                .to_owned();
            counters.insert(
                format!("{}@{ifindex}", entry.file_name().to_string_lossy()),
                (r, t),
            );
        }
    }
    (!counters.is_empty()).then_some(counters)
}

fn network_delta(
    now: &BTreeMap<String, (u64, u64)>,
    before: &BTreeMap<String, (u64, u64)>,
    elapsed: f64,
) -> (Option<f64>, Option<f64>) {
    if now.keys().ne(before.keys()) {
        return (None, None);
    }
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut rx_valid = true;
    let mut tx_valid = true;
    for (name, (current_rx, current_tx)) in now {
        let Some((old_rx, old_tx)) = before.get(name) else {
            return (None, None);
        };
        if let Some(delta) = current_rx.checked_sub(*old_rx) {
            rx = rx.saturating_add(delta);
        } else {
            rx_valid = false;
        }
        if let Some(delta) = current_tx.checked_sub(*old_tx) {
            tx = tx.saturating_add(delta);
        } else {
            tx_valid = false;
        }
    }
    (
        rx_valid.then_some(rx as f64 / elapsed),
        tx_valid.then_some(tx as f64 / elapsed),
    )
}

fn read_temperature() -> Option<f64> {
    read_temperature_from(
        Path::new("/sys/class/thermal"),
        Path::new("/sys/class/hwmon"),
    )
}

fn read_temperature_from(thermal_root: &Path, hwmon_root: &Path) -> Option<f64> {
    let mut candidates: Vec<(u8, std::path::PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(thermal_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let kind = fs::read_to_string(path.join("type"))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if is_cpu_thermal_type(&kind) {
                let priority = if has_package_label(&kind) { 0 } else { 1 };
                candidates.push((priority, path.join("temp")));
            }
        }
    }
    if let Ok(entries) = fs::read_dir(hwmon_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = fs::read_to_string(path.join("name"))
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if !is_cpu_hwmon(&name) {
                continue;
            }
            if let Ok(temps) = fs::read_dir(&path) {
                for temp in temps.flatten().map(|entry| entry.path()).filter(|path| {
                    path.file_name().is_some_and(|name| {
                        name.to_string_lossy().starts_with("temp")
                            && name.to_string_lossy().ends_with("_input")
                    })
                }) {
                    let label_path = temp.with_file_name(
                        temp.file_name()?
                            .to_string_lossy()
                            .replace("_input", "_label"),
                    );
                    let label = fs::read_to_string(label_path)
                        .unwrap_or_default()
                        .trim()
                        .to_ascii_lowercase();
                    // Recognized CPU drivers only: prefer package/Tctl/Tdie,
                    // then CPU/core channels. Unlabelled channels on those
                    // drivers are an acceptable final fallback.
                    let priority = match label.as_str() {
                        "package id 0" | "package" | "tctl" | "tdie" => 0,
                        value if value.starts_with("physical id") || value.contains("package") => 0,
                        value if value.contains("cpu") || value.starts_with("core") => 1,
                        "" => 2,
                        _ => continue,
                    };
                    candidates.push((priority, temp));
                }
            }
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    candidates
        .into_iter()
        .find_map(|(_, path)| parse_temperature(&fs::read_to_string(path).ok()?))
}

fn is_cpu_thermal_type(kind: &str) -> bool {
    kind.contains("cpu")
        || kind.contains("package")
        || kind.contains("pkg_temp")
        || kind.contains("tctl")
        || kind.contains("tdie")
}
fn has_package_label(label: &str) -> bool {
    label.contains("package")
        || label.contains("pkg")
        || label.contains("tctl")
        || label.contains("tdie")
}
fn is_cpu_hwmon(name: &str) -> bool {
    matches!(name, "coretemp" | "k10temp" | "zenpower" | "zenpower3")
}
fn parse_temperature(value: &str) -> Option<f64> {
    let raw = value.trim().parse::<f64>().ok()?;
    if !raw.is_finite() {
        return None;
    }
    let celsius = if raw.abs() > 1000.0 {
        raw / 1000.0
    } else {
        raw
    };
    // Broad physical sanity bounds reject corrupted/non-temperature values
    // without discarding valid high junction readings based on an assumed
    // thermal-throttle threshold.
    (-50.0..=300.0).contains(&celsius).then_some(celsius)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_parser_and_reset_delta() {
        assert!(read_cpu(Path::new("/proc/stat")).is_some());
        assert!((cpu_delta((150, 40), (100, 20)).unwrap() - 60.0).abs() < 0.001);
        assert_eq!(cpu_delta((1, 1), (100, 20)), None);
    }

    fn interfaces(entries: &[(&str, u64, u64)]) -> BTreeMap<String, (u64, u64)> {
        entries
            .iter()
            .map(|(name, rx, tx)| ((*name).to_owned(), (*rx, *tx)))
            .collect()
    }

    #[test]
    fn network_rebaselines_membership_and_detects_per_interface_resets() {
        let one = interfaces(&[("eth0", 1000, 2000)]);
        let added = interfaces(&[("eth0", 1100, 2200), ("wlan0", 9000, 8000)]);
        assert_eq!(network_delta(&added, &one, 1.0), (None, None)); // new NIC history is not a burst
        let stable = interfaces(&[("eth0", 1200, 2400), ("wlan0", 9100, 8050)]);
        assert_eq!(
            network_delta(&stable, &added, 2.0),
            (Some(100.0), Some(125.0))
        );
        let removed = interfaces(&[("eth0", 1300, 2500)]);
        assert_eq!(network_delta(&removed, &stable, 1.0), (None, None));
        let readded = interfaces(&[("eth0", 5, 8), ("wlan0", 5, 8)]);
        assert_eq!(network_delta(&readded, &removed, 1.0), (None, None));
        let same_name_new_device = interfaces(&[("eth0@2", 5, 8)]);
        assert_eq!(
            network_delta(&same_name_new_device, &one, 1.0),
            (None, None)
        );
        let after_rebaseline = interfaces(&[("eth0", 15, 18), ("wlan0", 25, 38)]);
        assert_eq!(
            network_delta(&after_rebaseline, &readded, 1.0),
            (Some(30.0), Some(40.0))
        );
        let reset_rx = interfaces(&[("eth0", 2, 28), ("wlan0", 35, 48)]);
        assert_eq!(
            network_delta(&reset_rx, &after_rebaseline, 1.0),
            (None, Some(20.0))
        );
        let reset_tx = interfaces(&[("eth0", 20, 3), ("wlan0", 45, 58)]);
        assert_eq!(
            network_delta(&reset_tx, &after_rebaseline, 1.0),
            (Some(25.0), None)
        );
    }

    #[test]
    fn telemetry_delivery_is_bounded_latest_wins_and_closes_cleanly() {
        let (sender, receiver) = async_channel::bounded(1);
        let sampler_receiver = receiver.clone();
        for cpu in [Some(1.0), Some(2.0), Some(3.0)] {
            assert!(send_latest(
                &sender,
                &sampler_receiver,
                HardwareSnapshot {
                    cpu_percent: cpu,
                    ..HardwareSnapshot::default()
                }
            ));
        }
        assert_eq!(receiver.len(), 1);
        assert_eq!(receiver.try_recv().unwrap().cpu_percent, Some(3.0));
        drop(receiver);
        assert!(!send_latest(
            &sender,
            &sampler_receiver,
            HardwareSnapshot::default()
        ));
    }
    #[test]
    fn cpu_total_ignores_guest_ticks_already_in_user_and_nice() {
        assert_eq!(
            parse_cpu("cpu 100 20 30 400 5 6 7 8 90 10\n"),
            Some((576, 405))
        );
    }
    #[test]
    fn parses_memory_as_used_and_total() {
        assert_eq!(
            parse_memory("MemTotal: 100 kB\nMemAvailable: 25 kB\n"),
            Some((75 * 1024, 100 * 1024))
        );
    }
    #[test]
    fn unavailable_means_none_not_synthetic_zero() {
        assert_eq!(UnavailableTelemetry.sample(), HardwareSnapshot::default());
    }

    #[test]
    fn selects_cpu_package_temperature_and_ignores_unrelated_sensors() {
        let base = std::env::temp_dir().join(format!("mithshell-telemetry-{}", std::process::id()));
        let thermal = base.join("thermal");
        let hwmon = base.join("hwmon");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(thermal.join("thermal_zone0")).unwrap();
        fs::create_dir_all(hwmon.join("hwmon0")).unwrap();
        fs::create_dir_all(hwmon.join("hwmon1")).unwrap();
        fs::write(thermal.join("thermal_zone0/type"), "acpitz\n").unwrap();
        fs::write(thermal.join("thermal_zone0/temp"), "42000\n").unwrap();
        fs::write(hwmon.join("hwmon0/name"), "nvme\n").unwrap();
        fs::write(hwmon.join("hwmon0/temp1_input"), "55000\n").unwrap();
        fs::write(hwmon.join("hwmon1/name"), "coretemp\n").unwrap();
        fs::write(hwmon.join("hwmon1/temp1_input"), "65000\n").unwrap();
        fs::write(hwmon.join("hwmon1/temp1_label"), "Package id 0\n").unwrap();
        fs::write(hwmon.join("hwmon1/temp2_input"), "58000\n").unwrap();
        fs::write(hwmon.join("hwmon1/temp2_label"), "Core 0\n").unwrap();
        assert_eq!(read_temperature_from(&thermal, &hwmon), Some(65.0));
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn sensor_selection_prioritizes_package_and_rejects_invalid_values() {
        assert!(is_cpu_thermal_type("x86_pkg_temp"));
        assert!(!is_cpu_thermal_type("gpu-thermal"));
        assert!(is_cpu_hwmon("k10temp"));
        assert!(!is_cpu_hwmon("nvme"));
        assert_eq!(parse_temperature("99000\n"), Some(99.0));
        assert_eq!(parse_temperature("301000\n"), None);
        assert_eq!(parse_temperature("NaN"), None);
        assert_eq!(parse_temperature("inf"), None);
    }
}
