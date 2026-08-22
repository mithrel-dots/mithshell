//! Click-handler signatures the controller injects into every island.
//!
//! Split out of the window module so the action surface stays reviewable
//! independently from the widget tree that invokes it.

use std::rc::Rc;

use crate::tarragon::TarragonSelection;

pub type WorkspaceAction = Rc<dyn Fn(&str, i64)>;
pub type ValueAction = Rc<dyn Fn(u8)>;
pub type SearchAction = Rc<dyn Fn(String)>;
pub type SelectionAction = Rc<dyn Fn(TarragonSelection)>;
pub type UnitAction = Rc<dyn Fn()>;
pub type PreviewAction = Rc<dyn Fn(u64, String)>;
/// Argument is the target MPRIS player's full D-Bus service name.
pub type MediaAction = Rc<dyn Fn(String)>;
pub type NotificationCloseAction = Rc<dyn Fn(u32)>;
pub type NotificationExpireAction = Rc<dyn Fn(u32, u64)>;
/// Arguments are a notification id and the invoked action's key.
pub type NotificationInvokeAction = Rc<dyn Fn(u32, String)>;
/// Arguments are a tray item's `service`/`object_path` and pointer
/// coordinates, for `Activate`/`SecondaryActivate`/`ContextMenu`.
pub type TrayPointAction = Rc<dyn Fn(String, String, i32, i32)>;
/// Arguments are a tray item's `service`/`object_path`, a scroll delta and
/// whether it was along the horizontal axis.
pub type TrayScrollAction = Rc<dyn Fn(String, String, i32, bool)>;
/// Arguments are a tray item's `service`, its DBusMenu object path, and the
/// clicked entry's id.
pub type TrayMenuEventAction = Rc<dyn Fn(String, String, i32)>;

#[derive(Clone)]
pub struct IslandActions {
    pub switch_workspace: WorkspaceAction,
    pub set_volume: ValueAction,
    pub set_brightness: ValueAction,
    pub search: SearchAction,
    pub select: SelectionAction,
    pub tarragon_status: UnitAction,
    pub tarragon_reload: UnitAction,
    pub load_preview: PreviewAction,
    pub media_play_pause: MediaAction,
    pub media_next: MediaAction,
    pub media_previous: MediaAction,
    pub notification_expired: NotificationExpireAction,
    pub notification_dismiss: NotificationCloseAction,
    pub notification_invoke: NotificationInvokeAction,
    pub tray_activate: TrayPointAction,
    pub tray_secondary_activate: TrayPointAction,
    pub tray_context_menu: TrayPointAction,
    pub tray_scroll: TrayScrollAction,
    pub tray_menu_event: TrayMenuEventAction,
}

/// Buttons owned by `dashboard_view`/`search_view`/`weather_view` that need
/// click handlers wired up centrally in `connect_interactions`. Grouped into
/// one struct instead of separate parameters purely to keep that function's
/// signature manageable.
pub(super) struct OverlayButtons<'a> {
    pub(super) close_button: &'a gtk::Button,
    pub(super) search_button: &'a gtk::Button,
    pub(super) weather_button: &'a gtk::Button,
    pub(super) search_back_button: &'a gtk::Button,
    pub(super) search_reload_button: &'a gtk::Button,
    pub(super) weather_back_button: &'a gtk::Button,
}
