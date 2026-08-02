use std::{
    cell::RefCell,
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    rc::Rc,
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use async_channel::Sender;
use glib::variant::ToVariant;
use gtk::{gio, glib};
use log::warn;

use crate::state::MediaState;

pub const VISUALIZER_BARS: usize = 7;
pub type VisualizerLevels = [u8; VISUALIZER_BARS];

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const PLAYER_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const CAVA_CONFIG: &str = r#"
[general]
framerate = 30
bars = 7
autosens = 1
sleep_timer = 1

[input]
method = pipewire
source = auto

[output]
method = raw
raw_target = /dev/stdout
data_format = ascii
ascii_max_range = 100
bar_delimiter = 59
frame_delimiter = 10
channels = mono

[smoothing]
noise_reduction = 80
"#;

pub fn start_listener(sender: Sender<Option<MediaState>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let context = glib::MainContext::new();
        if let Err(error) = context.with_thread_default(|| run_mpris_listener(&context, sender)) {
            warn!("failed to create the MPRIS event context: {error}");
        }
    })
}

pub fn start_visualizer(sender: Sender<VisualizerLevels>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            let mut child = match Command::new("cava")
                .args(["-p", "/dev/stdin"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(error) => {
                    warn!("real media visualization is unavailable: failed to run cava: {error}");
                    thread::sleep(Duration::from_secs(10));
                    continue;
                }
            };

            let configured = child
                .stdin
                .take()
                .is_some_and(|mut input| input.write_all(CAVA_CONFIG.as_bytes()).is_ok());
            let Some(stdout) = child.stdout.take().filter(|_| configured) else {
                let _ = child.kill();
                thread::sleep(Duration::from_secs(5));
                continue;
            };

            for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
                let Some(levels) = parse_cava_line(&line) else {
                    continue;
                };
                if sender.send_blocking(levels).is_err() {
                    let _ = child.kill();
                    return;
                }
            }

            let _ = child.wait();
            if sender.is_closed() {
                return;
            }
            warn!("cava media visualizer stopped; reconnecting");
            thread::sleep(Duration::from_secs(2));
        }
    })
}

fn run_mpris_listener(
    context: &glib::MainContext,
    sender: Sender<Option<MediaState>>,
) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)
        .context("failed to connect to the session D-Bus")?;
    let latest = Rc::new(RefCell::new(None));
    let main_loop = glib::MainLoop::new(Some(context), false);

    let name_sender = sender.clone();
    let name_latest = latest.clone();
    let name_loop = main_loop.clone();
    let names = connection.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let Some((name, _, _)) = signal.parameters.get::<(String, String, String)>() else {
                return;
            };
            if is_mpris_service(&name) && !publish(signal.connection, &name_sender, &name_latest) {
                name_loop.quit();
            }
        },
    );

    let property_sender = sender.clone();
    let property_latest = latest.clone();
    let property_loop = main_loop.clone();
    let properties = connection.subscribe_to_signal(
        None,
        Some(PROPERTIES_INTERFACE),
        Some("PropertiesChanged"),
        Some(PLAYER_PATH),
        Some(PLAYER_INTERFACE),
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if relevant_property_change(signal.parameters)
                && !publish(signal.connection, &property_sender, &property_latest)
            {
                property_loop.quit();
            }
        },
    );

    if publish(&connection, &sender, &latest) {
        main_loop.run();
    }
    drop([names, properties]);
    Ok(())
}

fn publish(
    connection: &gio::DBusConnection,
    sender: &Sender<Option<MediaState>>,
    latest: &RefCell<Option<MediaState>>,
) -> bool {
    let state = match query_active_media(connection) {
        Ok(state) => state,
        Err(error) => {
            warn!("failed to query MPRIS players: {error:#}");
            return !sender.is_closed();
        }
    };
    if *latest.borrow() == state {
        return !sender.is_closed();
    }
    *latest.borrow_mut() = state.clone();
    sender.try_send(state).is_ok()
}

fn query_active_media(connection: &gio::DBusConnection) -> Result<Option<MediaState>> {
    let reply = connection.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "ListNames",
        None,
        None,
        gio::DBusCallFlags::NONE,
        1_000,
        None::<&gio::Cancellable>,
    )?;
    let (mut names,) = reply
        .get::<(Vec<String>,)>()
        .context("invalid ListNames response")?;
    names.retain(|name| is_mpris_service(name));
    names.sort();

    for name in names {
        let Some(player_properties) = query_properties(connection, &name, PLAYER_INTERFACE) else {
            continue;
        };
        let root_properties = query_properties(connection, &name, ROOT_INTERFACE);
        let app_icon = root_properties
            .as_ref()
            .and_then(|properties| properties.get("DesktopEntry"))
            .and_then(|value| value.get::<String>())
            .map(|value| value.trim_end_matches(".desktop").to_owned());
        if let Some(state) = media_state_from_properties(&name, &player_properties, app_icon) {
            return Ok(Some(state));
        }
    }
    Ok(None)
}

fn query_properties(
    connection: &gio::DBusConnection,
    service: &str,
    interface: &str,
) -> Option<HashMap<String, glib::Variant>> {
    let reply = connection
        .call_sync(
            Some(service),
            PLAYER_PATH,
            PROPERTIES_INTERFACE,
            "GetAll",
            Some(&(interface,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            700,
            None::<&gio::Cancellable>,
        )
        .ok()?;
    reply
        .get::<(HashMap<String, glib::Variant>,)>()
        .map(|(properties,)| properties)
}

fn relevant_property_change(parameters: &glib::Variant) -> bool {
    let Some((interface, changed, invalidated)) =
        parameters.get::<(String, HashMap<String, glib::Variant>, Vec<String>)>()
    else {
        return false;
    };
    interface == PLAYER_INTERFACE
        && ["PlaybackStatus", "Metadata"].iter().any(|property| {
            changed.contains_key(*property) || invalidated.iter().any(|value| value == property)
        })
}

fn is_mpris_service(name: &str) -> bool {
    name.strip_prefix(MPRIS_PREFIX)
        .is_some_and(|player| !player.is_empty() && player != "playerctld")
}

fn media_state_from_properties(
    service: &str,
    properties: &HashMap<String, glib::Variant>,
    app_icon: Option<String>,
) -> Option<MediaState> {
    if properties.get("PlaybackStatus")?.get::<String>()? != "Playing" {
        return None;
    }
    let metadata = properties
        .get("Metadata")?
        .get::<HashMap<String, glib::Variant>>()?;
    let title = metadata.get("xesam:title")?.get::<String>()?;
    if title.trim().is_empty() {
        return None;
    }
    let player = service.strip_prefix(MPRIS_PREFIX)?.to_owned();
    let app_icon = app_icon.or_else(|| {
        player
            .split(".instance")
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    });
    Some(MediaState {
        player,
        title,
        app_icon,
    })
}

fn parse_cava_line(line: &str) -> Option<VisualizerLevels> {
    let values: Vec<_> = line
        .split(';')
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<u8>().ok().map(|value| value.min(100)))
        .collect::<Option<_>>()?;
    values.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties(status: &str, title: &str) -> HashMap<String, glib::Variant> {
        let metadata = HashMap::from([("xesam:title".to_owned(), title.to_variant())]);
        HashMap::from([
            ("PlaybackStatus".to_owned(), status.to_variant()),
            ("Metadata".to_owned(), metadata.to_variant()),
        ])
    }

    #[test]
    fn recognizes_mpris_services_and_ignores_playerctld() {
        assert!(is_mpris_service("org.mpris.MediaPlayer2.spotify"));
        assert!(is_mpris_service(
            "org.mpris.MediaPlayer2.firefox.instance_1_77"
        ));
        assert!(!is_mpris_service("org.mpris.MediaPlayer2.playerctld"));
        assert!(!is_mpris_service("org.freedesktop.DBus"));
    }

    #[test]
    fn extracts_only_playing_media_with_a_title_and_icon() {
        let playing = media_state_from_properties(
            "org.mpris.MediaPlayer2.spotify",
            &properties("Playing", "A long song title"),
            Some("spotify-client".to_owned()),
        )
        .unwrap();
        assert_eq!(playing.player, "spotify");
        assert_eq!(playing.title, "A long song title");
        assert_eq!(playing.app_icon.as_deref(), Some("spotify-client"));
        assert!(
            media_state_from_properties(
                "org.mpris.MediaPlayer2.spotify",
                &properties("Paused", "A long song title"),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn parses_cava_ascii_frames() {
        assert_eq!(
            parse_cava_line("0;12;45;100;82;9;3;"),
            Some([0, 12, 45, 100, 82, 9, 3])
        );
        assert_eq!(parse_cava_line("0;1;2;"), None);
    }
}
