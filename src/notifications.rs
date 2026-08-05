//! A minimal `org.freedesktop.Notifications` server.
//!
//! Runs on its own thread with a dedicated `glib::MainContext`, the same
//! shape as `media::start_listener`'s MPRIS watcher: a `gio::DBusConnection`
//! is opened, a small object is exported on it, and updates are forwarded to
//! the GTK main thread over an `async_channel`. Unlike the MPRIS listener
//! this side also *serves* method calls (`Notify`, `CloseNotification`, ...)
//! instead of only subscribing to signals, and accepts a second channel of
//! commands so the UI can ask it to emit `NotificationClosed`/`ActionInvoked`
//! signals back onto the bus.
//!
//! Only one process can own the `org.freedesktop.Notifications` name at a
//! time. If another notification daemon (dunst, mako, ...) already owns it,
//! `bus_own_name_on_connection` simply never reports acquisition here; the
//! object is still registered locally, but senders addressing the well-known
//! name keep talking to whichever daemon got there first, exactly like two
//! competing daemons launched by hand.

use std::{cell::Cell, collections::HashMap, rc::Rc, thread};

use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use glib::variant::ToVariant;
use gtk::{gio, glib};
use log::{debug, warn};

use crate::state::{Notification, NotificationAction, NotificationTimeout, Urgency};

const BUS_NAME: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";

const INTROSPECTION_XML: &str = r#"
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="GetCapabilities">
      <arg direction="out" name="capabilities" type="as"/>
    </method>
    <method name="Notify">
      <arg direction="in" name="app_name" type="s"/>
      <arg direction="in" name="replaces_id" type="u"/>
      <arg direction="in" name="app_icon" type="s"/>
      <arg direction="in" name="summary" type="s"/>
      <arg direction="in" name="body" type="s"/>
      <arg direction="in" name="actions" type="as"/>
      <arg direction="in" name="hints" type="a{sv}"/>
      <arg direction="in" name="expire_timeout" type="i"/>
      <arg direction="out" name="id" type="u"/>
    </method>
    <method name="CloseNotification">
      <arg direction="in" name="id" type="u"/>
    </method>
    <method name="GetServerInformation">
      <arg direction="out" name="name" type="s"/>
      <arg direction="out" name="vendor" type="s"/>
      <arg direction="out" name="version" type="s"/>
      <arg direction="out" name="spec_version" type="s"/>
    </method>
    <signal name="NotificationClosed">
      <arg name="id" type="u"/>
      <arg name="reason" type="u"/>
    </signal>
    <signal name="ActionInvoked">
      <arg name="id" type="u"/>
      <arg name="action_key" type="s"/>
    </signal>
  </interface>
</node>
"#;

/// Why a notification stopped being shown, in the numbering
/// `org.freedesktop.Notifications` assigns to `NotificationClosed`'s
/// `reason` argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    ClosedByCall = 3,
}

#[derive(Debug, Clone)]
pub enum NotificationEvent {
    Show(Notification),
    Closed { id: u32, reason: CloseReason },
}

/// Requests sent from the GTK main thread back to the D-Bus worker thread,
/// purely to emit the corresponding signal on the bus. The UI updates its
/// own state directly rather than waiting for a round trip through here.
#[derive(Debug, Clone)]
pub enum NotificationCommand {
    Close { id: u32, reason: CloseReason },
    InvokeAction { id: u32, action_key: String },
}

/// Starts the notification server thread and returns a sender for
/// [`NotificationCommand`]s the UI can use to talk back to it.
///
/// Failures (no session bus, malformed introspection XML, the name already
/// being owned) are logged from the worker thread rather than surfaced here,
/// matching `media::start_listener`/`weather::start_poller`: a daemon that
/// cannot serve notifications should still start normally.
pub fn start_server(
    events: Sender<NotificationEvent>,
) -> (Sender<NotificationCommand>, thread::JoinHandle<()>) {
    let (command_sender, command_receiver) = async_channel::unbounded();
    let handle = thread::spawn(move || {
        let context = glib::MainContext::new();
        let result = context.with_thread_default(|| run_server(&context, events, command_receiver));
        match result {
            Ok(Err(error)) => warn!("notification server stopped: {error:#}"),
            Err(error) => warn!("failed to create the notification D-Bus context: {error}"),
            Ok(Ok(())) => {}
        }
    });
    (command_sender, handle)
}

fn run_server(
    context: &glib::MainContext,
    events: Sender<NotificationEvent>,
    commands: Receiver<NotificationCommand>,
) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)
        .context("failed to connect to the session D-Bus")?;
    let node_info = gio::DBusNodeInfo::for_xml(INTROSPECTION_XML)
        .context("failed to parse the notifications introspection XML")?;
    let interface_info = node_info
        .lookup_interface(INTERFACE)
        .context("notifications interface missing from introspection XML")?;

    let next_id = Rc::new(Cell::new(1u32));
    let method_events = events.clone();
    let registration = connection
        .register_object(OBJECT_PATH, &interface_info)
        .method_call(
            move |connection, sender, _path, _interface, method, parameters, invocation| {
                handle_method_call(
                    &connection,
                    sender,
                    method,
                    &parameters,
                    invocation,
                    &method_events,
                    &next_id,
                );
            },
        )
        .build()
        .context("failed to register the notifications D-Bus object")?;

    let _owner = gio::bus_own_name_on_connection(
        &connection,
        BUS_NAME,
        gio::BusNameOwnerFlags::NONE,
        |_, name| debug!("acquired the {name} D-Bus name"),
        |_, name| {
            warn!(
                "could not acquire the {name} D-Bus name; another notification daemon is likely already running"
            );
        },
    );

    let signal_connection = connection.clone();
    context.spawn_local(async move {
        while let Ok(command) = commands.recv().await {
            match command {
                NotificationCommand::Close { id, reason } => {
                    emit_closed(&signal_connection, id, reason);
                }
                NotificationCommand::InvokeAction { id, action_key } => {
                    let _ = signal_connection.emit_signal(
                        None,
                        OBJECT_PATH,
                        INTERFACE,
                        "ActionInvoked",
                        Some(&(id, action_key).to_variant()),
                    );
                }
            }
        }
    });

    let main_loop = glib::MainLoop::new(Some(context), false);
    main_loop.run();
    connection.unregister_object(registration).ok();
    Ok(())
}

fn emit_closed(connection: &gio::DBusConnection, id: u32, reason: CloseReason) {
    let _ = connection.emit_signal(
        None,
        OBJECT_PATH,
        INTERFACE,
        "NotificationClosed",
        Some(&(id, reason as u32).to_variant()),
    );
}

#[allow(clippy::too_many_arguments)]
fn handle_method_call(
    connection: &gio::DBusConnection,
    _sender: Option<&str>,
    method: &str,
    parameters: &glib::Variant,
    invocation: gio::DBusMethodInvocation,
    events: &Sender<NotificationEvent>,
    next_id: &Rc<Cell<u32>>,
) {
    match method {
        "Notify" => handle_notify(parameters, invocation, events, next_id),
        "CloseNotification" => {
            let Some((id,)) = parameters.get::<(u32,)>() else {
                invocation.return_dbus_error(
                    "org.freedesktop.DBus.Error.InvalidArgs",
                    "CloseNotification expects a single uint32 id",
                );
                return;
            };
            invocation.return_value(None);
            emit_closed(connection, id, CloseReason::ClosedByCall);
            let _ = events.try_send(NotificationEvent::Closed {
                id,
                reason: CloseReason::ClosedByCall,
            });
        }
        "GetCapabilities" => {
            let capabilities = vec![
                "body".to_owned(),
                "actions".to_owned(),
                "persistence".to_owned(),
            ];
            invocation.return_value(Some(&(capabilities,).to_variant()));
        }
        "GetServerInformation" => {
            invocation.return_value(Some(
                &("mithshell", "mithrel", env!("CARGO_PKG_VERSION"), "1.2").to_variant(),
            ));
        }
        other => {
            invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.UnknownMethod",
                &format!("unknown method {other}"),
            );
        }
    }
}

fn handle_notify(
    parameters: &glib::Variant,
    invocation: gio::DBusMethodInvocation,
    events: &Sender<NotificationEvent>,
    next_id: &Rc<Cell<u32>>,
) {
    type NotifyArgs = (
        String,
        u32,
        String,
        String,
        String,
        Vec<String>,
        HashMap<String, glib::Variant>,
        i32,
    );
    let Some((app_name, replaces_id, app_icon, summary, body, raw_actions, hints, expire_timeout)) =
        parameters.get::<NotifyArgs>()
    else {
        invocation.return_dbus_error(
            "org.freedesktop.DBus.Error.InvalidArgs",
            "malformed Notify call",
        );
        return;
    };

    let id = if replaces_id != 0 {
        replaces_id
    } else {
        let id = next_id.get();
        next_id.set(id.wrapping_add(1).max(1));
        id
    };

    let urgency = hints
        .get("urgency")
        .and_then(|value| value.get::<u8>())
        .map_or(Urgency::Normal, Urgency::from_hint_byte);
    let app_icon = Some(app_icon)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            hints
                .get("image-path")
                .or_else(|| hints.get("image_path"))
                .and_then(|value| value.get::<String>())
        });
    let timeout = match expire_timeout {
        0 => NotificationTimeout::Never,
        value if value > 0 => NotificationTimeout::Millis(value as u64),
        _ => NotificationTimeout::Default,
    };

    let notification = Notification {
        id,
        app_name,
        app_icon,
        summary,
        body,
        urgency,
        actions: parse_actions(&raw_actions),
        timeout,
    };

    invocation.return_value(Some(&(id,).to_variant()));
    let _ = events.try_send(NotificationEvent::Show(notification));
}

/// The `actions` array alternates `key`, `label` pairs. A trailing unpaired
/// entry (a malformed call) is dropped rather than panicking.
fn parse_actions(raw: &[String]) -> Vec<NotificationAction> {
    raw.chunks_exact(2)
        .map(|pair| NotificationAction {
            key: pair[0].clone(),
            label: pair[1].clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_actions_and_drops_a_trailing_unpaired_key() {
        let actions = parse_actions(&[
            "default".to_owned(),
            "Open".to_owned(),
            "reply".to_owned(),
            "Reply".to_owned(),
            "dangling".to_owned(),
        ]);
        assert_eq!(
            actions,
            vec![
                NotificationAction {
                    key: "default".into(),
                    label: "Open".into()
                },
                NotificationAction {
                    key: "reply".into(),
                    label: "Reply".into()
                },
            ]
        );
    }
}
