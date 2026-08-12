//! System tray (`org.kde.StatusNotifierItem`/`StatusNotifierWatcher`) support.
//!
//! Wayland compositors (Hyprland included) don't provide a system tray on
//! their own the way X11 window managers historically brokered the
//! freedesktop "systray" protocol; instead, tray-aware apps publish a
//! `org.kde.StatusNotifierItem` object on the session bus and expect some
//! host to render it, and a `StatusNotifierWatcher` service to broker
//! between items and hosts. Nothing on a bare wlroots session normally
//! provides that watcher, so -- like most other Wayland status bars --
//! mithshell provides both roles itself: it tries to become the watcher,
//! and always registers itself as a host against whichever process ends up
//! owning it (itself, most of the time; another bar's watcher, if one
//! happens to already be running).
//!
//! Runs on its own thread with a dedicated `glib::MainContext`, the same
//! shape as `notifications::start_server`: one `gio::DBusConnection` serves
//! the watcher object, subscribes to per-item property/signal traffic, and
//! forwards a fresh item snapshot down `Sender<Vec<TrayItem>>` whenever
//! anything changes -- mirroring `media::start_listener`'s
//! `Sender<Option<MediaState>>` exactly. User-initiated actions
//! (`Activate`, scrolling, menu clicks, ...) are separate synchronous
//! functions below, called from a freshly opened connection, the same way
//! `media::play_pause`/`next`/`previous` work; fetching a context menu's
//! layout is the one exception with a reply, handled entirely within
//! `ui::island` via its own throwaway thread rather than routed through
//! here, since the result only matters to the specific button that
//! triggered it.

use std::{cell::Cell, cell::RefCell, collections::HashMap, rc::Rc, thread};

use anyhow::{Context, Result};
use async_channel::Sender;
use glib::variant::ToVariant;
use gtk::{gio, glib};
use log::{debug, warn};

use crate::state::{TrayIcon, TrayItem, TrayMenuItem, TrayStatus};

const WATCHER_BUS_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_OBJECT_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const ITEM_INTERFACE: &str = "org.kde.StatusNotifierItem";
/// Older items sometimes still expose the pre-KDE-adoption interface name.
const ITEM_INTERFACE_FALLBACK: &str = "org.freedesktop.StatusNotifierItem";
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
/// Path assumed for an item registered with just a bus name, per the
/// original convention (and what most toolkits' tray libraries still do).
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";

const WATCHER_INTROSPECTION_XML: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierWatcher">
    <method name="RegisterStatusNotifierItem">
      <arg direction="in" name="service" type="s"/>
    </method>
    <method name="RegisterStatusNotifierHost">
      <arg direction="in" name="service" type="s"/>
    </method>
    <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
    <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
    <property name="ProtocolVersion" type="i" access="read"/>
    <signal name="StatusNotifierItemRegistered">
      <arg name="service" type="s"/>
    </signal>
    <signal name="StatusNotifierItemUnregistered">
      <arg name="service" type="s"/>
    </signal>
    <signal name="StatusNotifierHostRegistered"/>
    <signal name="StatusNotifierHostUnregistered"/>
  </interface>
</node>
"#;

/// Starts the tray listener thread and returns a handle to it.
///
/// Failures (no session bus, malformed introspection XML, ...) are logged
/// from the worker thread rather than surfaced here, matching
/// `media::start_listener`/`notifications::start_server`: a daemon that
/// cannot serve a tray should still start normally.
pub fn start_listener(sender: Sender<Vec<TrayItem>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let context = glib::MainContext::new();
        let result = context.with_thread_default(|| run(&context, sender));
        match result {
            Ok(Err(error)) => warn!("tray listener stopped: {error:#}"),
            Err(error) => warn!("failed to create the tray D-Bus context: {error}"),
            Ok(Ok(())) => {}
        }
    })
}

fn run(context: &glib::MainContext, sender: Sender<Vec<TrayItem>>) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)
        .context("failed to connect to the session D-Bus")?;

    let watcher = Rc::new(WatcherRegistry::default());
    // Kept alive for the rest of this function (which only returns at
    // process exit, via `main_loop.run()` below) so the object stays
    // registered; `notifications::run_server` unregisters its equivalent
    // explicitly at the end for the same reason this one doesn't need to.
    let _watcher_registration = match install_watcher_object(&connection, watcher.clone()) {
        Ok(registration) => Some(registration),
        Err(error) => {
            warn!("could not register the local tray watcher object: {error:#}");
            None
        }
    };
    let host = Rc::new(HostRegistry::new(connection.clone(), sender));
    // `watch_watcher` covers the general case (an external watcher, or a
    // future ownership transfer) by reacting to `NameOwnerChanged`. It
    // can't be relied on for our *own* acquisition below, though: this
    // whole function runs before `main_loop.run()` ever starts pumping the
    // connection, so nothing here is actually sent/delivered yet --
    // `bus_own_name_on_connection`'s queued `RequestName` and
    // `watch_watcher`'s queued `AddMatch` race once the loop does start,
    // and if our own acquisition signal lands before the `AddMatch` takes
    // effect on the bus daemon, it's simply never delivered to us. Calling
    // `on_watcher_appeared` straight from `name_acquired` sidesteps that
    // race entirely (`add_item`/`WatcherLink` are idempotent, so it's safe
    // even if `watch_watcher` *also* catches the same transition).
    let _watch = watch_watcher(host.clone());
    let name_acquired_host = host.clone();
    let _owner = gio::bus_own_name_on_connection(
        &connection,
        WATCHER_BUS_NAME,
        gio::BusNameOwnerFlags::NONE,
        move |connection, name| {
            debug!("acquired the {name} D-Bus name; hosting the system tray watcher");
            let owner = connection
                .unique_name()
                .map_or_else(String::new, |name| name.to_string());
            on_watcher_appeared(&connection, &owner, &name_acquired_host);
        },
        |_, _| {},
    );

    let main_loop = glib::MainLoop::new(Some(context), false);
    main_loop.run();
    Ok(())
}

// --- Watcher role: broker `RegisterStatusNotifierItem`/`RegisterStatusNotifierHost` ---

#[derive(Default)]
struct WatcherRegistry {
    items: RefCell<Vec<String>>,
    item_watches: RefCell<HashMap<String, gio::SignalSubscription>>,
    hosts: Cell<u32>,
}

impl WatcherRegistry {
    fn register_item(
        self: &Rc<Self>,
        connection: &gio::DBusConnection,
        sender: Option<&str>,
        service: String,
    ) {
        let (bus_name, object_path) = split_service(&service, sender);
        if bus_name.is_empty() {
            return;
        }
        // Many items (ayatana/appindicator-based ones especially: Spotify,
        // Steam, nm-applet, blueman, ...) call `RegisterStatusNotifierItem`
        // with just their bare object path, relying on the *caller's* bus
        // name (only available here, from the method call's sender) for
        // the rest. Normalizing to a self-contained `busname` or
        // `busname/path` string before storing/emitting it means every
        // host -- including our own -- can resolve it later purely from
        // `RegisteredStatusNotifierItems`/the registration signal, without
        // needing to have observed the original call itself.
        let service = normalize_service(&bus_name, &object_path);
        if self
            .items
            .borrow()
            .iter()
            .any(|existing| existing == &service)
        {
            return;
        }
        self.items.borrow_mut().push(service.clone());

        let registry = self.clone();
        let watch_service = service.clone();
        let watch = watch_name_vanished(connection, &bus_name, move |connection| {
            registry.unregister_item(connection, &watch_service);
        });
        self.item_watches
            .borrow_mut()
            .insert(service.clone(), watch);

        let _ = connection.emit_signal(
            None,
            WATCHER_OBJECT_PATH,
            WATCHER_INTERFACE,
            "StatusNotifierItemRegistered",
            Some(&(service,).to_variant()),
        );
    }

    fn unregister_item(self: &Rc<Self>, connection: &gio::DBusConnection, service: &str) {
        let removed = {
            let mut items = self.items.borrow_mut();
            let before = items.len();
            items.retain(|existing| existing != service);
            items.len() != before
        };
        // Dropping the subscription unsubscribes it.
        self.item_watches.borrow_mut().remove(service);
        if removed {
            let _ = connection.emit_signal(
                None,
                WATCHER_OBJECT_PATH,
                WATCHER_INTERFACE,
                "StatusNotifierItemUnregistered",
                Some(&(service.to_owned(),).to_variant()),
            );
        }
    }
}

/// Subscribes to `NameOwnerChanged` for `bus_name`, invoking `on_vanished`
/// once its owner disappears.
///
/// Stands in for `gio::bus_watch_name_on_connection`, whose `WatcherId`
/// return type is unfortunately unnameable from outside the `gio` crate in
/// the version this project pins: it collides with a second, unrelated
/// `WatcherId` also re-exported at the crate root (from
/// `gio::dbus_connection`), and the latter wins name resolution, so a field
/// or return type declared `gio::WatcherId` never actually type-checks
/// against what `bus_watch_name_on_connection` returns.
/// `gio::SignalSubscription` doesn't have that problem, and conveniently
/// already unsubscribes itself on drop.
fn watch_name_vanished<F: Fn(&gio::DBusConnection) + 'static>(
    connection: &gio::DBusConnection,
    bus_name: &str,
    on_vanished: F,
) -> gio::SignalSubscription {
    connection.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        Some(bus_name),
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if let Some((_, _, new_owner)) = signal.parameters.get::<(String, String, String)>()
                && new_owner.is_empty()
            {
                on_vanished(signal.connection);
            }
        },
    )
}

/// The unique bus name currently owning `name`, if any.
fn current_name_owner(connection: &gio::DBusConnection, name: &str) -> Option<String> {
    let reply = connection
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "GetNameOwner",
            Some(&(name,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1_000,
            None::<&gio::Cancellable>,
        )
        .ok()?;
    reply.get::<(String,)>().map(|(owner,)| owner)
}

fn install_watcher_object(
    connection: &gio::DBusConnection,
    registry: Rc<WatcherRegistry>,
) -> Result<gio::RegistrationId> {
    let node_info = gio::DBusNodeInfo::for_xml(WATCHER_INTROSPECTION_XML)
        .context("failed to parse the tray watcher introspection XML")?;
    let interface_info = node_info
        .lookup_interface(WATCHER_INTERFACE)
        .context("tray watcher interface missing from introspection XML")?;

    let method_registry = registry.clone();
    let property_registry = registry;
    connection
        .register_object(WATCHER_OBJECT_PATH, &interface_info)
        .method_call(
            move |connection, sender, _path, _interface, method, parameters, invocation| {
                handle_watcher_method(
                    &connection,
                    sender,
                    method,
                    &parameters,
                    invocation,
                    &method_registry,
                );
            },
        )
        .property(move |_connection, _sender, _path, _interface, property| {
            watcher_property(&property_registry, property)
        })
        .build()
        .context("failed to register the tray watcher D-Bus object")
}

fn handle_watcher_method(
    connection: &gio::DBusConnection,
    sender: Option<&str>,
    method: &str,
    parameters: &glib::Variant,
    invocation: gio::DBusMethodInvocation,
    registry: &Rc<WatcherRegistry>,
) {
    match method {
        "RegisterStatusNotifierItem" => {
            let Some((service,)) = parameters.get::<(String,)>() else {
                invocation.return_dbus_error(
                    "org.freedesktop.DBus.Error.InvalidArgs",
                    "RegisterStatusNotifierItem expects a single string",
                );
                return;
            };
            invocation.return_value(None);
            registry.register_item(connection, sender, service);
        }
        "RegisterStatusNotifierHost" => {
            invocation.return_value(None);
            registry.hosts.set(registry.hosts.get().saturating_add(1));
            let _ = connection.emit_signal(
                None,
                WATCHER_OBJECT_PATH,
                WATCHER_INTERFACE,
                "StatusNotifierHostRegistered",
                None,
            );
        }
        other => {
            invocation.return_dbus_error(
                "org.freedesktop.DBus.Error.UnknownMethod",
                &format!("unknown method {other}"),
            );
        }
    }
}

fn watcher_property(registry: &Rc<WatcherRegistry>, property: &str) -> glib::Variant {
    match property {
        "IsStatusNotifierHostRegistered" => (registry.hosts.get() > 0).to_variant(),
        "ProtocolVersion" => 0i32.to_variant(),
        _ => registry.items.borrow().clone().to_variant(),
    }
}

/// Splits a `RegisterStatusNotifierItem`/registered-items entry into a bus
/// name and object path. Well-behaved items just pass their own bus name
/// (object path defaults to `/StatusNotifierItem`); others pass
/// `"busname/object/path"` for a non-default path, or (rarely) a bare
/// `"/object/path"` meaning "on the calling connection".
fn split_service(service: &str, sender: Option<&str>) -> (String, String) {
    if let Some(rest) = service.strip_prefix('/') {
        return (sender.unwrap_or_default().to_owned(), format!("/{rest}"));
    }
    match service.find('/') {
        Some(index) => (service[..index].to_owned(), service[index..].to_owned()),
        None => (service.to_owned(), DEFAULT_ITEM_PATH.to_owned()),
    }
}

/// The inverse of `split_service`: a self-contained registration string
/// that round-trips back to `(bus_name, object_path)` through it without
/// needing a `sender` -- `bus_name` alone when `object_path` is the
/// default, `"busname/object/path"` otherwise.
fn normalize_service(bus_name: &str, object_path: &str) -> String {
    if object_path == DEFAULT_ITEM_PATH {
        bus_name.to_owned()
    } else {
        format!("{bus_name}{object_path}")
    }
}

// --- Host role: track items registered with whichever watcher is active ---

struct ItemEntry {
    item: TrayItem,
    _properties_sub: gio::SignalSubscription,
    _new_signals_sub: gio::SignalSubscription,
    _vanish_sub: gio::SignalSubscription,
}

struct WatcherLink {
    _registered_sub: gio::SignalSubscription,
    _unregistered_sub: gio::SignalSubscription,
}

struct HostRegistry {
    connection: gio::DBusConnection,
    events: Sender<Vec<TrayItem>>,
    items: RefCell<HashMap<String, ItemEntry>>,
    watcher_link: RefCell<Option<WatcherLink>>,
}

impl HostRegistry {
    fn new(connection: gio::DBusConnection, events: Sender<Vec<TrayItem>>) -> Self {
        Self {
            connection,
            events,
            items: RefCell::new(HashMap::new()),
            watcher_link: RefCell::new(None),
        }
    }

    fn publish_snapshot(&self) {
        let mut items: Vec<TrayItem> = self
            .items
            .borrow()
            .values()
            .map(|entry| entry.item.clone())
            .collect();
        items.sort_by(|a, b| a.key.cmp(&b.key));
        let _ = self.events.try_send(items);
    }

    fn add_item(self: &Rc<Self>, service: String) {
        if self.items.borrow().contains_key(&service) {
            return;
        }
        let (bus_name, object_path) = split_service(&service, None);
        if bus_name.is_empty() {
            return;
        }
        let Some(item) = fetch_tray_item(&self.connection, &bus_name, &object_path) else {
            return;
        };

        let prop_host = self.clone();
        let prop_service = service.clone();
        let properties_sub = self.connection.subscribe_to_signal(
            Some(&bus_name),
            Some(PROPERTIES_INTERFACE),
            Some("PropertiesChanged"),
            Some(&object_path),
            None,
            gio::DBusSignalFlags::NONE,
            move |_signal| prop_host.refresh_item(&prop_service),
        );

        // Some items still predate reliable `PropertiesChanged` support and
        // instead emit one of the `New*` signals directly; a full refetch on
        // any of them is cheap enough given how few items there usually are.
        let new_host = self.clone();
        let new_service = service.clone();
        let new_signals_sub = self.connection.subscribe_to_signal(
            Some(&bus_name),
            Some(ITEM_INTERFACE),
            None,
            Some(&object_path),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                if signal.signal_name.starts_with("New") {
                    new_host.refresh_item(&new_service);
                }
            },
        );

        let vanish_host = self.clone();
        let vanish_service = service.clone();
        let vanish_sub = watch_name_vanished(&self.connection, &bus_name, move |_connection| {
            vanish_host.remove_item(&vanish_service);
        });

        self.items.borrow_mut().insert(
            service,
            ItemEntry {
                item,
                _properties_sub: properties_sub,
                _new_signals_sub: new_signals_sub,
                _vanish_sub: vanish_sub,
            },
        );
        self.publish_snapshot();
    }

    fn refresh_item(self: &Rc<Self>, service: &str) {
        let Some((bus_name, object_path)) = self
            .items
            .borrow()
            .get(service)
            .map(|entry| (entry.item.service.clone(), entry.item.object_path.clone()))
        else {
            return;
        };
        let Some(item) = fetch_tray_item(&self.connection, &bus_name, &object_path) else {
            // The item stopped answering; wait for its bus name to
            // actually vanish (`_vanish_sub`) rather than dropping it
            // eagerly here on what might be a transient hiccup.
            return;
        };
        if let Some(entry) = self.items.borrow_mut().get_mut(service) {
            entry.item = item;
        }
        self.publish_snapshot();
    }

    fn remove_item(self: &Rc<Self>, service: &str) {
        if self.items.borrow_mut().remove(service).is_some() {
            self.publish_snapshot();
        }
    }

    fn clear(self: &Rc<Self>) {
        let had_items = !self.items.borrow().is_empty();
        self.items.borrow_mut().clear();
        *self.watcher_link.borrow_mut() = None;
        if had_items {
            self.publish_snapshot();
        }
    }
}

fn watch_watcher(host: Rc<HostRegistry>) -> gio::SignalSubscription {
    let signal_host = host.clone();
    let subscription = host.connection.subscribe_to_signal(
        Some("org.freedesktop.DBus"),
        Some("org.freedesktop.DBus"),
        Some("NameOwnerChanged"),
        Some("/org/freedesktop/DBus"),
        Some(WATCHER_BUS_NAME),
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let Some((_, _, new_owner)) = signal.parameters.get::<(String, String, String)>()
            else {
                return;
            };
            if new_owner.is_empty() {
                signal_host.clear();
            } else {
                on_watcher_appeared(signal.connection, &new_owner, &signal_host);
            }
        },
    );
    // `NameOwnerChanged` only reports future transitions; check whether the
    // watcher (possibly our own, just requested a moment ago via
    // `bus_own_name_on_connection`, or another process') already owns the
    // name right now, since we'd otherwise miss an acquisition that landed
    // before this subscription was in place.
    if let Some(owner) = current_name_owner(&host.connection, WATCHER_BUS_NAME) {
        on_watcher_appeared(&host.connection, &owner, &host);
    }
    subscription
}

fn on_watcher_appeared(
    connection: &gio::DBusConnection,
    name_owner: &str,
    host: &Rc<HostRegistry>,
) {
    let host_id = connection
        .unique_name()
        .map_or_else(String::new, |name| name.to_string());
    let _ = connection.call_sync(
        Some(name_owner),
        WATCHER_OBJECT_PATH,
        WATCHER_INTERFACE,
        "RegisterStatusNotifierHost",
        Some(&(host_id,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        2_000,
        None::<&gio::Cancellable>,
    );

    let registered_host = host.clone();
    let registered_sub = connection.subscribe_to_signal(
        Some(name_owner),
        Some(WATCHER_INTERFACE),
        Some("StatusNotifierItemRegistered"),
        Some(WATCHER_OBJECT_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if let Some((service,)) = signal.parameters.get::<(String,)>() {
                registered_host.add_item(service);
            }
        },
    );
    let unregistered_host = host.clone();
    let unregistered_sub = connection.subscribe_to_signal(
        Some(name_owner),
        Some(WATCHER_INTERFACE),
        Some("StatusNotifierItemUnregistered"),
        Some(WATCHER_OBJECT_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            if let Some((service,)) = signal.parameters.get::<(String,)>() {
                unregistered_host.remove_item(&service);
            }
        },
    );
    *host.watcher_link.borrow_mut() = Some(WatcherLink {
        _registered_sub: registered_sub,
        _unregistered_sub: unregistered_sub,
    });

    let existing = connection
        .call_sync(
            Some(name_owner),
            WATCHER_OBJECT_PATH,
            PROPERTIES_INTERFACE,
            "Get",
            Some(&(WATCHER_INTERFACE, "RegisteredStatusNotifierItems").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2_000,
            None::<&gio::Cancellable>,
        )
        .ok()
        .and_then(|reply| reply.get::<(glib::Variant,)>())
        .and_then(|(value,)| value.get::<Vec<String>>());
    for service in existing.into_iter().flatten() {
        host.add_item(service);
    }
}

fn fetch_tray_item(
    connection: &gio::DBusConnection,
    bus_name: &str,
    object_path: &str,
) -> Option<TrayItem> {
    let properties = fetch_properties(connection, bus_name, object_path, ITEM_INTERFACE)
        .or_else(|| fetch_properties(connection, bus_name, object_path, ITEM_INTERFACE_FALLBACK))?;
    Some(build_tray_item(bus_name, object_path, &properties))
}

fn fetch_properties(
    connection: &gio::DBusConnection,
    bus_name: &str,
    object_path: &str,
    interface: &str,
) -> Option<HashMap<String, glib::Variant>> {
    let reply = connection
        .call_sync(
            Some(bus_name),
            object_path,
            PROPERTIES_INTERFACE,
            "GetAll",
            Some(&(interface,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1_500,
            None::<&gio::Cancellable>,
        )
        .ok()?;
    let (properties,) = reply.get::<(HashMap<String, glib::Variant>,)>()?;
    (!properties.is_empty()).then_some(properties)
}

fn build_tray_item(
    bus_name: &str,
    object_path: &str,
    properties: &HashMap<String, glib::Variant>,
) -> TrayItem {
    let id = properties
        .get("Id")
        .and_then(|value| value.get::<String>())
        .unwrap_or_default();
    let title = properties
        .get("Title")
        .and_then(|value| value.get::<String>())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| id.clone());
    let status = properties
        .get("Status")
        .and_then(|value| value.get::<String>())
        .map_or(TrayStatus::Passive, |status| match status.as_str() {
            "Active" => TrayStatus::Active,
            "NeedsAttention" => TrayStatus::NeedsAttention,
            _ => TrayStatus::Passive,
        });
    let icon = icon_from_properties(properties);
    let tooltip = tooltip_from_properties(properties)
        .or_else(|| Some(title.clone()).filter(|title| !title.is_empty()));
    let item_is_menu = properties
        .get("ItemIsMenu")
        .and_then(|value| value.get::<bool>())
        .unwrap_or(false);
    let menu_path = properties
        .get("Menu")
        .and_then(|value| value.get::<glib::variant::ObjectPath>())
        .map(|path| path.to_string())
        .filter(|path| path != "/" && !path.is_empty());

    TrayItem {
        key: format!("{bus_name}{object_path}"),
        service: bus_name.to_owned(),
        object_path: object_path.to_owned(),
        id,
        title,
        tooltip,
        icon,
        status,
        item_is_menu,
        menu_path,
    }
}

fn icon_from_properties(properties: &HashMap<String, glib::Variant>) -> TrayIcon {
    if let Some(name) = properties
        .get("IconName")
        .and_then(|value| value.get::<String>())
        .filter(|name| !name.is_empty())
    {
        return TrayIcon::Name(name);
    }
    if let Some(pixmaps) = properties
        .get("IconPixmap")
        .and_then(|value| value.get::<Vec<(i32, i32, Vec<u8>)>>())
        && let Some((width, height, argb)) = pixmaps
            .into_iter()
            .filter(|(width, height, _)| *width > 0 && *height > 0)
            .max_by_key(|(width, height, _)| width.saturating_mul(*height))
        && argb.len() as i64 == i64::from(width) * i64::from(height) * 4
    {
        return TrayIcon::Pixmap {
            width,
            height,
            argb,
        };
    }
    TrayIcon::None
}

/// The freedesktop tooltip struct is `(icon-name, icon-pixmap, title, text)`;
/// only the text (falling back to the title) is used here, since the icon
/// shown is always the item's own icon rather than the tooltip's.
fn tooltip_from_properties(properties: &HashMap<String, glib::Variant>) -> Option<String> {
    let (_, _, title, text) =
        properties
            .get("ToolTip")?
            .get::<(String, Vec<(i32, i32, Vec<u8>)>, String, String)>()?;
    let combined = if text.is_empty() { title } else { text };
    (!combined.is_empty()).then_some(combined)
}

// --- User-initiated actions: fresh short-lived connections, mirroring `media::control` ---

fn call_item(
    service: &str,
    object_path: &str,
    method: &str,
    params: Option<&glib::Variant>,
) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)
        .context("failed to connect to the session D-Bus")?;
    connection
        .call_sync(
            Some(service),
            object_path,
            ITEM_INTERFACE,
            method,
            params,
            None,
            gio::DBusCallFlags::NONE,
            1_500,
            None::<&gio::Cancellable>,
        )
        .with_context(|| format!("{method} failed for {service}{object_path}"))?;
    Ok(())
}

/// Primary (left) click.
pub fn activate(service: &str, object_path: &str, x: i32, y: i32) -> Result<()> {
    call_item(service, object_path, "Activate", Some(&(x, y).to_variant()))
}

/// Middle click, per the spec "a secondary and less important form of activation".
pub fn secondary_activate(service: &str, object_path: &str, x: i32, y: i32) -> Result<()> {
    call_item(
        service,
        object_path,
        "SecondaryActivate",
        Some(&(x, y).to_variant()),
    )
}

/// Right click fallback for items that did not advertise a `Menu` object.
pub fn context_menu(service: &str, object_path: &str, x: i32, y: i32) -> Result<()> {
    call_item(
        service,
        object_path,
        "ContextMenu",
        Some(&(x, y).to_variant()),
    )
}

pub fn scroll(service: &str, object_path: &str, delta: i32, horizontal: bool) -> Result<()> {
    let orientation = if horizontal { "horizontal" } else { "vertical" };
    call_item(
        service,
        object_path,
        "Scroll",
        Some(&(delta, orientation).to_variant()),
    )
}

/// Fetches and parses a `com.canonical.dbusmenu` layout, for a right click
/// on an item that advertised a `Menu` object path.
pub fn menu_layout(service: &str, menu_path: &str) -> Result<TrayMenuItem> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)
        .context("failed to connect to the session D-Bus")?;
    // Some clients populate their menu lazily and expect `AboutToShow` on
    // the root before `GetLayout` reflects it; best-effort per spec, so
    // failures here are not fatal to fetching whatever layout exists.
    let _ = connection.call_sync(
        Some(service),
        menu_path,
        MENU_INTERFACE,
        "AboutToShow",
        Some(&(0i32,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        1_000,
        None::<&gio::Cancellable>,
    );
    let reply = connection
        .call_sync(
            Some(service),
            menu_path,
            MENU_INTERFACE,
            "GetLayout",
            Some(&(0i32, -1i32, Vec::<String>::new()).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2_000,
            None::<&gio::Cancellable>,
        )
        .context("GetLayout failed")?;
    let (_revision, root) = reply
        .get::<(u32, RawMenuNode)>()
        .context("malformed DBusMenu layout")?;
    Ok(menu_item_from_raw(&root))
}

/// Sends a `com.canonical.dbusmenu` `Event` call for `event_id` (typically
/// `"clicked"`) on menu entry `id`.
pub fn menu_event(service: &str, menu_path: &str, id: i32, event_id: &str) -> Result<()> {
    let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>)
        .context("failed to connect to the session D-Bus")?;
    connection
        .call_sync(
            Some(service),
            menu_path,
            MENU_INTERFACE,
            "Event",
            Some(&(id, event_id, "".to_variant(), 0u32).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1_000,
            None::<&gio::Cancellable>,
        )
        .context("Event failed")?;
    Ok(())
}

/// `(id, properties, children)`, where `children` is an array of variants
/// each wrapping another node of this same shape -- DBusMenu's recursive
/// `(ia{sv}av)` layout type.
type RawMenuNode = (i32, HashMap<String, glib::Variant>, Vec<glib::Variant>);

fn menu_item_from_raw(node: &RawMenuNode) -> TrayMenuItem {
    let (id, properties, children) = node;
    let separator = properties
        .get("type")
        .and_then(|value| value.get::<String>())
        .as_deref()
        == Some("separator");
    let label = properties
        .get("label")
        .and_then(|value| value.get::<String>())
        .map(|label| strip_mnemonic(&label))
        .unwrap_or_default();
    let enabled = properties
        .get("enabled")
        .and_then(|value| value.get::<bool>())
        .unwrap_or(true);
    let visible = properties
        .get("visible")
        .and_then(|value| value.get::<bool>())
        .unwrap_or(true);
    let checked = properties
        .get("toggle-type")
        .and_then(|value| value.get::<String>())
        .filter(|toggle_type| !toggle_type.is_empty())
        .and_then(|_| {
            properties
                .get("toggle-state")
                .and_then(|value| value.get::<i32>())
        })
        .map(|state| state == 1);
    let children = children
        .iter()
        .filter_map(|child| child.get::<RawMenuNode>())
        .map(|child| menu_item_from_raw(&child))
        .collect();
    TrayMenuItem {
        id: *id,
        label,
        enabled,
        visible,
        separator,
        checked,
        children,
    }
}

/// Strips single `_` mnemonic markers from a DBusMenu label (GTK/Qt
/// convention), keeping a literal underscore where the source doubled it.
fn strip_mnemonic(label: &str) -> String {
    let mut result = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '_' {
            if chars.peek() == Some(&'_') {
                result.push('_');
                chars.next();
            }
            continue;
        }
        result.push(character);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_bare_bus_name() {
        assert_eq!(
            split_service("org.foo.Bar", None),
            ("org.foo.Bar".to_owned(), DEFAULT_ITEM_PATH.to_owned())
        );
    }

    #[test]
    fn splits_a_compound_service_with_a_custom_path() {
        assert_eq!(
            split_service("org.foo.Bar/org/ayatana/NotificationItem/Item1", None),
            (
                "org.foo.Bar".to_owned(),
                "/org/ayatana/NotificationItem/Item1".to_owned()
            )
        );
    }

    #[test]
    fn splits_a_bare_path_using_the_sender_as_the_bus_name() {
        assert_eq!(
            split_service("/StatusNotifierItem", Some(":1.42")),
            (":1.42".to_owned(), "/StatusNotifierItem".to_owned())
        );
    }

    /// Regression test for a real-world bug: ayatana/appindicator-based
    /// items (Spotify, Steam, nm-applet, blueman, ...) call
    /// `RegisterStatusNotifierItem` with a bare, non-default object path
    /// like `/org/ayatana/NotificationItem/spotify_client`, expecting the
    /// watcher to remember the caller's bus name for them. Without
    /// normalizing before storing, `RegisteredStatusNotifierItems` and the
    /// registration signal would carry an unresolvable bare path once the
    /// original `sender` is out of scope, and every host (including our
    /// own) would silently fail to add the item.
    #[test]
    fn normalizes_and_round_trips_an_ayatana_style_bare_path_registration() {
        let (bus_name, object_path) = split_service(
            "/org/ayatana/NotificationItem/spotify_client",
            Some(":1.99"),
        );
        assert_eq!(bus_name, ":1.99");
        assert_eq!(object_path, "/org/ayatana/NotificationItem/spotify_client");

        let normalized = normalize_service(&bus_name, &object_path);
        assert_eq!(
            normalized,
            ":1.99/org/ayatana/NotificationItem/spotify_client"
        );

        // Resolvable again with no `sender` at all, exactly like a host
        // that only ever sees the normalized string.
        assert_eq!(split_service(&normalized, None), (bus_name, object_path));
    }

    #[test]
    fn normalize_collapses_the_default_path_to_a_bare_bus_name() {
        assert_eq!(
            normalize_service("org.foo.Bar", DEFAULT_ITEM_PATH),
            "org.foo.Bar"
        );
    }

    #[test]
    fn strips_single_underscore_mnemonics_but_keeps_doubled_ones() {
        assert_eq!(strip_mnemonic("_Quit"), "Quit");
        assert_eq!(strip_mnemonic("Save __As__"), "Save _As_");
        assert_eq!(strip_mnemonic("No mnemonic"), "No mnemonic");
    }

    #[test]
    fn parses_a_menu_layout_with_a_separator_and_a_submenu() {
        let root_properties: HashMap<String, glib::Variant> =
            HashMap::from([("label".to_owned(), "Root".to_variant())]);
        let separator_properties: HashMap<String, glib::Variant> =
            HashMap::from([("type".to_owned(), "separator".to_variant())]);
        let child_properties: HashMap<String, glib::Variant> = HashMap::from([
            ("label".to_owned(), "_Open".to_variant()),
            ("enabled".to_owned(), false.to_variant()),
        ]);
        let submenu_properties: HashMap<String, glib::Variant> =
            HashMap::from([("label".to_owned(), "More".to_variant())]);

        let separator: RawMenuNode = (2, separator_properties, Vec::new());
        let child: RawMenuNode = (3, child_properties, Vec::new());
        let submenu: RawMenuNode = (4, submenu_properties, vec![child.to_variant()]);
        let root: RawMenuNode = (
            0,
            root_properties,
            vec![separator.to_variant(), submenu.to_variant()],
        );

        let parsed = menu_item_from_raw(&root);
        assert_eq!(parsed.label, "Root");
        assert_eq!(parsed.children.len(), 2);
        assert!(parsed.children[0].separator);
        assert_eq!(parsed.children[1].label, "More");
        let open = &parsed.children[1].children[0];
        assert_eq!(open.label, "Open");
        assert!(!open.enabled);
    }
}
