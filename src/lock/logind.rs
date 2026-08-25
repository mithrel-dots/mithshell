//! systemd-logind integration for external session lock/unlock requests.

use std::{process, thread, time::Duration};

use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use gtk::{gio, glib, glib::variant::ToVariant};
use log::{debug, info, warn};

const LOGIN1_NAME: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogindEvent {
    LockRequested,
    UnlockRequested,
    Connected { session_path: String },
    Unavailable { message: String },
}

#[derive(Debug, Clone, Copy)]
pub enum LogindCommand {
    SetLockedHint(bool),
}

/// First retry delay, and the unit the backoff doubles from.
const RETRY_BASE: Duration = Duration::from_secs(5);
/// Retry ceiling. Reached after ~6 failures, which on a machine without
/// logind at all is the difference between a line every 5s forever and a
/// handful of lines total.
const RETRY_MAX: Duration = Duration::from_secs(300);
/// Granularity of the retry wait, and therefore the worst-case delay that
/// shutdown pays while joining this thread.
const SHUTDOWN_POLL: Duration = Duration::from_millis(250);

pub fn start_listener(
    events: Sender<LogindEvent>,
) -> (Sender<LogindCommand>, thread::JoinHandle<()>) {
    let (commands, command_receiver) = async_channel::unbounded();
    let handle = thread::spawn(move || {
        let mut backoff = RETRY_BASE;
        let mut last_failure: Option<String> = None;
        while !command_receiver.is_closed() {
            let context = glib::MainContext::new();
            // Two failure layers, both fatal to this attempt: acquiring the
            // context, then the listener itself. Flatten so neither is lost.
            let result = context
                .with_thread_default(|| run_listener(&context, &events, command_receiver.clone()))
                .context("failed to acquire a thread-default main context")
                .and_then(|listener| listener);
            if command_receiver.is_closed() {
                break;
            }
            match result {
                // A clean return means the session bus went away rather than
                // never having been there; retry promptly.
                Ok(()) => {
                    backoff = RETRY_BASE;
                    last_failure = None;
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    // Repeating the same reason every retry is noise; only
                    // the first occurrence of each distinct one is a warning.
                    if last_failure.as_deref() == Some(message.as_str()) {
                        debug!("logind lock listener still unavailable: {message}");
                    } else {
                        warn!("logind lock listener unavailable: {message}");
                        last_failure = Some(message.clone());
                    }
                    let _ = events.send_blocking(LogindEvent::Unavailable { message });
                    backoff = next_backoff(backoff);
                }
            }
            if !wait_before_retry(&command_receiver, backoff) {
                break;
            }
        }
    });
    (commands, handle)
}

fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).min(RETRY_MAX)
}

/// Sleeps in short slices so a shutdown that closes the command channel is
/// noticed within [`SHUTDOWN_POLL`] rather than after the full backoff.
/// Returns `false` if the listener should stop.
fn wait_before_retry(commands: &Receiver<LogindCommand>, delay: Duration) -> bool {
    let mut waited = Duration::ZERO;
    while waited < delay {
        if commands.is_closed() {
            return false;
        }
        let slice = SHUTDOWN_POLL.min(delay - waited);
        thread::sleep(slice);
        waited += slice;
    }
    !commands.is_closed()
}

fn run_listener(
    context: &glib::MainContext,
    events: &Sender<LogindEvent>,
    commands: Receiver<LogindCommand>,
) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::System, None::<&gio::Cancellable>)
        .context("failed to connect to the system D-Bus")?;
    let session_path = resolve_session_path(&connection)?;
    info!("listening for logind lock requests on {session_path}");
    let _ = events.send_blocking(LogindEvent::Connected {
        session_path: session_path.clone(),
    });

    let signal_events = events.clone();
    let signals = connection.subscribe_to_signal(
        Some(LOGIN1_NAME),
        Some(SESSION_INTERFACE),
        None,
        Some(&session_path),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if let Some(event) = event_for_signal(signal.signal_name) {
                let _ = signal_events.try_send(event);
            }
        },
    );

    let main_loop = glib::MainLoop::new(Some(context), false);
    let command_loop = main_loop.clone();
    let command_connection = connection.clone();
    let command_path = session_path.clone();
    context.spawn_local(async move {
        while let Ok(command) = commands.recv().await {
            match command {
                LogindCommand::SetLockedHint(locked) => {
                    if let Err(error) = set_locked_hint(&command_connection, &command_path, locked)
                    {
                        warn!("failed to set logind LockedHint={locked}: {error:#}");
                    }
                }
            }
        }
        command_loop.quit();
    });
    main_loop.run();
    drop(signals);
    Ok(())
}

fn event_for_signal(name: &str) -> Option<LogindEvent> {
    match name {
        "Lock" => Some(LogindEvent::LockRequested),
        "Unlock" => Some(LogindEvent::UnlockRequested),
        _ => None,
    }
}

fn resolve_session_path(connection: &gio::DBusConnection) -> Result<String> {
    let pid_parameters = (process::id(),).to_variant();
    if let Ok(path) = call_session_lookup(connection, "GetSessionByPID", &pid_parameters) {
        return Ok(path);
    }

    let auto_parameters = ("auto",).to_variant();
    if let Ok(path) = call_session_lookup(connection, "GetSession", &auto_parameters) {
        return Ok(path);
    }

    let session_id = std::env::var("XDG_SESSION_ID")
        .context("logind could not resolve this process and XDG_SESSION_ID is unset")?;
    let parameters = (session_id.as_str(),).to_variant();
    call_session_lookup(connection, "GetSession", &parameters)
        .context("logind could not resolve the current session")
}

fn call_session_lookup(
    connection: &gio::DBusConnection,
    method: &str,
    parameters: &glib::Variant,
) -> Result<String> {
    let reply = connection
        .call_sync(
            Some(LOGIN1_NAME),
            MANAGER_PATH,
            MANAGER_INTERFACE,
            method,
            Some(parameters),
            None,
            gio::DBusCallFlags::NONE,
            2_000,
            None::<&gio::Cancellable>,
        )
        .with_context(|| format!("logind {method} failed"))?;
    let (path,) = reply
        .get::<(glib::variant::ObjectPath,)>()
        .context("logind returned an invalid session object path")?;
    Ok(path.to_string())
}

fn set_locked_hint(
    connection: &gio::DBusConnection,
    session_path: &str,
    locked: bool,
) -> Result<()> {
    connection
        .call_sync(
            Some(LOGIN1_NAME),
            session_path,
            SESSION_INTERFACE,
            "SetLockedHint",
            Some(&(locked,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2_000,
            None::<&gio::Cancellable>,
        )
        .context("logind SetLockedHint failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_logind_lock_lifecycle_signals() {
        assert_eq!(event_for_signal("Lock"), Some(LogindEvent::LockRequested));
        assert_eq!(
            event_for_signal("Unlock"),
            Some(LogindEvent::UnlockRequested)
        );
        assert_eq!(event_for_signal("PauseDevice"), None);
    }

    #[test]
    fn backoff_doubles_up_to_the_ceiling_and_stays_there() {
        assert_eq!(next_backoff(RETRY_BASE), RETRY_BASE * 2);
        assert_eq!(next_backoff(RETRY_MAX), RETRY_MAX);
        assert_eq!(next_backoff(RETRY_MAX / 2 + RETRY_BASE), RETRY_MAX);

        let mut delay = RETRY_BASE;
        for _ in 0..64 {
            delay = next_backoff(delay);
        }
        assert_eq!(delay, RETRY_MAX);
    }

    #[test]
    fn retry_wait_gives_up_immediately_once_the_command_channel_closes() {
        let (commands, receiver) = async_channel::unbounded::<LogindCommand>();
        commands.close();

        let started = std::time::Instant::now();
        assert!(!wait_before_retry(&receiver, RETRY_MAX));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn retry_wait_sleeps_through_a_short_delay_while_the_channel_is_open() {
        let (commands, receiver) = async_channel::unbounded::<LogindCommand>();

        let started = std::time::Instant::now();
        assert!(wait_before_retry(&receiver, Duration::from_millis(60)));
        assert!(started.elapsed() >= Duration::from_millis(50));
        drop(commands);
    }
}
