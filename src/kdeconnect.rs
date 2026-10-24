//! Optional KDE Connect phone status on the session bus. No polling or daemon
//! activation: subscribe before the initial read, and recover on owner changes.

use std::{cell::RefCell, collections::HashMap, rc::Rc, thread};

use anyhow::{Context, Result};
use async_channel::Sender;
use glib::variant::ToVariant;
use gtk::{gio, glib};
use log::{debug, warn};

use crate::state::{CellularSignal, PhoneState};

const SERVICE: &str = "org.kde.kdeconnect";
const ROOT: &str = "/modules/kdeconnect";
const DAEMON: &str = "org.kde.kdeconnect.daemon";
const DEVICE: &str = "org.kde.kdeconnect.device";
const BATTERY: &str = "org.kde.kdeconnect.device.battery";
const CONNECTIVITY: &str = "org.kde.kdeconnect.device.connectivity_report";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
type Properties = HashMap<String, glib::Variant>;

pub fn start_listener(sender: Sender<Vec<PhoneState>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let context = glib::MainContext::new();
        match context.with_thread_default(|| run(&context, sender)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!("KDE Connect listener stopped: {error:#}"),
            Err(error) => warn!("failed to create the KDE Connect context: {error}"),
        }
    })
}

fn run(context: &glib::MainContext, sender: Sender<Vec<PhoneState>>) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)?;
    let main_loop = glib::MainLoop::new(Some(context), false);
    let latest = Rc::new(RefCell::new(Vec::new()));
    let publish = Rc::new({
        let connection = connection.clone();
        let main_loop = main_loop.clone();
        move || {
            let phones = query_phones(&connection).unwrap_or_else(|error| {
                debug!("KDE Connect unavailable: {error:#}");
                Vec::new()
            });
            if *latest.borrow() != phones {
                *latest.borrow_mut() = phones.clone();
                if sender.try_send(phones).is_err() {
                    main_loop.quit();
                }
            } else if sender.is_closed() {
                main_loop.quit();
            }
        }
    });
    let on_owner = publish.clone();
    let owner = connection.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        Some(SERVICE),
        gio::DBusSignalFlags::NONE,
        move |_| on_owner(),
    );
    let on_change = publish.clone();
    let changes = connection.subscribe_to_signal(
        Some(SERVICE),
        None,
        None,
        None,
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if relevant_signal(signal.interface_name, signal.signal_name, signal.parameters) {
                on_change();
            }
        },
    );
    publish();
    main_loop.run();
    drop((owner, changes));
    Ok(())
}

fn relevant_signal(interface: &str, name: &str, parameters: &glib::Variant) -> bool {
    match interface {
        DAEMON => matches!(
            name,
            "deviceAdded" | "deviceRemoved" | "deviceVisibilityChanged" | "deviceListChanged"
        ),
        DEVICE => matches!(
            name,
            "nameChanged"
                | "typeChanged"
                | "reachableChanged"
                | "pairStateChanged"
                | "trustedChanged"
                | "pluginsChanged"
        ),
        BATTERY | CONNECTIVITY => name == "refreshed",
        PROPERTIES if name == "PropertiesChanged" => parameters
            .try_child_get::<String>(0)
            .ok()
            .flatten()
            .is_some_and(|interface| matches!(interface.as_str(), DEVICE | BATTERY | CONNECTIVITY)),
        _ => false,
    }
}

fn call(
    connection: &gio::DBusConnection,
    path: &str,
    interface: &str,
    method: &str,
    parameters: Option<&glib::Variant>,
) -> Result<glib::Variant> {
    Ok(connection.call_sync(
        Some(SERVICE),
        path,
        interface,
        method,
        parameters,
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        1_000,
        None::<&gio::Cancellable>,
    )?)
}

fn properties(connection: &gio::DBusConnection, path: &str, interface: &str) -> Result<Properties> {
    call(
        connection,
        path,
        PROPERTIES,
        "GetAll",
        Some(&(interface,).to_variant()),
    )?
    .get::<(Properties,)>()
    .map(|(properties,)| properties)
    .context("invalid KDE Connect properties")
}

fn query_phones(connection: &gio::DBusConnection) -> Result<Vec<PhoneState>> {
    let (mut ids,) = call(
        connection,
        ROOT,
        DAEMON,
        "devices",
        Some(&(false, true).to_variant()),
    )?
    .get::<(Vec<String>,)>()
    .context("invalid KDE Connect device list")?;
    ids.sort();
    let mut phones = Vec::new();
    for id in ids {
        let path = format!("{ROOT}/devices/{id}");
        let Ok(device) = properties(connection, &path, DEVICE) else {
            continue;
        };
        let Some(mut phone) = phone_state(id, &device) else {
            continue;
        };
        let battery = phone
            .reachable
            .then(|| properties(connection, &format!("{path}/battery"), BATTERY).ok())
            .flatten();
        let connectivity = phone
            .reachable
            .then(|| {
                properties(
                    connection,
                    &format!("{path}/connectivity_report"),
                    CONNECTIVITY,
                )
                .ok()
            })
            .flatten();
        let can_ring = phone.reachable
            && call(
                connection,
                &path,
                DEVICE,
                "hasPlugin",
                Some(&("kdeconnect_findmyphone",).to_variant()),
            )
            .ok()
            .and_then(|value| value.get::<(bool,)>())
            .is_some_and(|(loaded,)| loaded);
        update_plugins(
            &mut phone,
            battery.as_ref(),
            connectivity.as_ref(),
            can_ring,
        );
        phones.push(phone);
    }
    Ok(phones)
}

fn phone_state(id: String, device: &Properties) -> Option<PhoneState> {
    let kind = device.get("type")?.get::<String>()?;
    if !matches!(kind.as_str(), "phone" | "tablet") {
        return None;
    }
    // Older KDE Connect releases called pairing "trusted".
    let paired = device
        .get("isPaired")
        .or_else(|| device.get("isTrusted"))
        .and_then(glib::Variant::get)
        .unwrap_or(false);
    if !paired {
        return None;
    }
    let reachable = device
        .get("isReachable")
        .and_then(glib::Variant::get)
        .unwrap_or(false);
    Some(PhoneState {
        id,
        name: device
            .get("name")
            .and_then(glib::Variant::get::<String>)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Phone".into()),
        reachable,
        percent: None,
        charging: false,
        can_ring: false,
        cellular: None,
    })
}

fn update_plugins(
    phone: &mut PhoneState,
    battery: Option<&Properties>,
    connectivity: Option<&Properties>,
    can_ring: bool,
) {
    let battery = battery.filter(|battery| {
        phone.reachable && battery.get("hasBattery").and_then(glib::Variant::get) != Some(false)
    });
    phone.percent = battery
        .and_then(|battery| battery.get("charge"))
        .and_then(glib::Variant::get::<i32>)
        .filter(|charge| (0..=100).contains(charge))
        .map(|charge| charge as u8);
    phone.charging = battery
        .and_then(|battery| battery.get("isCharging"))
        .and_then(glib::Variant::get)
        .unwrap_or(false);
    phone.can_ring = phone.reachable && can_ring;
    phone.cellular = connectivity
        .filter(|_| phone.reachable)
        .and_then(cellular_signal);
}

fn cellular_signal(properties: &Properties) -> Option<CellularSignal> {
    let strength = properties.get("cellularNetworkStrength")?.get::<i32>()?;
    if !(-1..=4).contains(&strength) {
        return None;
    }
    let network_type = properties
        .get("cellularNetworkType")
        .and_then(glib::Variant::get::<String>)
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("unknown"));
    Some(CellularSignal {
        strength,
        network_type,
    })
}

/// Called on a worker thread by the row's explicit Ring action.
pub fn ring(id: &str) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)?;
    call(
        &connection,
        &format!("{ROOT}/devices/{id}/findmyphone"),
        "org.kde.kdeconnect.device.findmyphone",
        "ring",
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cellular_reports_preserve_zero_and_no_service_but_reject_invalid_values() {
        let mut properties = Properties::from([("cellularNetworkType".into(), "LTE".to_variant())]);
        assert_eq!(cellular_signal(&properties), None);
        for strength in [-2, -1, 0, 1, 2, 3, 4, 5] {
            properties.insert("cellularNetworkStrength".into(), strength.to_variant());
            let signal = cellular_signal(&properties);
            if (-1..=4).contains(&strength) {
                let signal = signal.unwrap();
                assert_eq!(signal.strength, strength);
                assert_eq!(signal.network_type.as_deref(), Some("LTE"));
            } else {
                assert_eq!(signal, None);
            }
        }
        properties.insert("cellularNetworkStrength".into(), 2_i32.to_variant());
        for network in ["", "Unknown", " unknown "] {
            properties.insert("cellularNetworkType".into(), network.to_variant());
            assert_eq!(cellular_signal(&properties).unwrap().network_type, None);
        }
        properties.insert("cellularNetworkStrength".into(), "2".to_variant());
        assert_eq!(cellular_signal(&properties), None);
    }

    #[test]
    fn connectivity_signals_refresh_phone_state_without_unrelated_traffic() {
        assert!(relevant_signal(
            CONNECTIVITY,
            "refreshed",
            &("LTE", 2_i32).to_variant()
        ));
        let changed =
            |interface: &str| (interface, Properties::new(), Vec::<String>::new()).to_variant();
        assert!(relevant_signal(
            PROPERTIES,
            "PropertiesChanged",
            &changed(CONNECTIVITY)
        ));
        assert!(!relevant_signal(
            PROPERTIES,
            "PropertiesChanged",
            &changed("org.mpris.MediaPlayer2.Player")
        ));
        assert!(!relevant_signal(DEVICE, "linksChanged", &().to_variant()));
    }

    #[test]
    fn phone_state_handles_missing_battery_disconnect_and_pairing() {
        let mut device = Properties::from([
            ("name".into(), "Phone".to_variant()),
            ("type".into(), "phone".to_variant()),
            ("isPaired".into(), true.to_variant()),
            ("isReachable".into(), true.to_variant()),
        ]);
        let mut battery = Properties::from([
            ("charge".into(), 73_i32.to_variant()),
            ("isCharging".into(), true.to_variant()),
        ]);
        let mut state = phone_state("id".into(), &device).unwrap();
        update_plugins(&mut state, Some(&battery), None, true);
        assert_eq!(state.percent, Some(73));
        assert!(state.charging && state.can_ring);
        for charge in [-1, 0, 100, 101] {
            battery.insert("charge".into(), charge.to_variant());
            update_plugins(&mut state, Some(&battery), None, true);
            assert_eq!(
                state.percent,
                match charge {
                    0 => Some(0),
                    100 => Some(100),
                    _ => None,
                }
            );
        }
        update_plugins(&mut state, None, None, true);
        assert_eq!(state.percent, None);
        let connectivity = Properties::from([
            ("cellularNetworkStrength".into(), 4_i32.to_variant()),
            ("cellularNetworkType".into(), "5G".to_variant()),
        ]);
        update_plugins(&mut state, Some(&battery), Some(&connectivity), true);
        assert_eq!(state.cellular.as_ref().unwrap().strength, 4);
        update_plugins(&mut state, None, None, true);
        assert_eq!(state.cellular, None);
        device.insert("isReachable".into(), false.to_variant());
        let mut offline = phone_state("id".into(), &device).unwrap();
        update_plugins(&mut offline, Some(&battery), Some(&connectivity), true);
        assert!(!offline.reachable && !offline.can_ring && !offline.charging);
        assert_eq!(offline.cellular, None);
        assert_eq!(offline.percent, None);
        device.insert("isPaired".into(), false.to_variant());
        assert!(phone_state("id".into(), &device).is_none());
        device.insert("isPaired".into(), true.to_variant());
        device.insert("type".into(), "desktop".to_variant());
        assert!(phone_state("id".into(), &device).is_none());
    }
}
