use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gtk::{glib, prelude::*, subclass::prelude::*};

use super::{Event, Frame, Mode, Rect, Revision, State};

/// The host parents these once. Modules retain cloned handles for updates, but
/// must not put the same widget in a dashboard or another page. Full and hover
/// may share backend data/row builders, never the same GTK child instance.
pub(crate) struct CircleContent {
    pub compact: gtk::Widget,
    pub hover: gtk::Widget,
    pub full: Option<gtk::Widget>,
}

type OnChange = Rc<dyn Fn(&CircleHost)>;

pub(crate) struct CircleHost {
    surface: CircleSurface,
    stack: gtk::Stack,
    radius_style: gtk::CssProvider,
    state: Cell<State>,
    frame: Cell<Option<Frame>>,
    /// Last pointer position reported by the fixed integration root. `None`
    /// means the pointer has physically left the layer surface.
    root_pointer: Cell<Option<(f64, f64)>>,
    /// The page currently presented.  `state.mode()` is the requested page;
    /// these intentionally differ while the integration fades between them.
    presented_page: Cell<Option<Mode>>,
    on_change: RefCell<Option<OnChange>>,
    key_controller: gtk::EventControllerKey,
}

impl CircleHost {
    /// Fails before parenting anything if widgets alias or already have parents.
    /// Construction and all subsequent calls belong on GTK's main thread.
    pub(crate) fn new(content: CircleContent) -> Result<Rc<Self>, &'static str> {
        let mut pages = vec![&content.compact, &content.hover];
        pages.extend(content.full.as_ref());
        if pages.iter().any(|w| w.parent().is_some()) {
            return Err("circle content must be unparented");
        }
        for (i, page) in pages.iter().enumerate() {
            if pages[..i].contains(page) {
                return Err("circle pages must be distinct widgets");
            }
        }
        let supports_full = content.full.is_some();
        let stack = gtk::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.set_transition_type(gtk::StackTransitionType::None);
        stack.add_named(&content.compact, Some("compact"));
        stack.add_named(&scroll_page(&content.hover), Some("hover"));
        if let Some(full) = content.full {
            full.set_focusable(true);
            stack.add_named(&scroll_page(&full), Some("full"));
        }
        let background = gtk::Box::new(gtk::Orientation::Vertical, 0);
        background.add_css_class("island-surface");
        background.add_css_class("circle-surface");
        let radius_style = gtk::CssProvider::new();
        // GTK 4.14 has no replacement for widget-local providers. Keep radius
        // overrides scoped to this surface instead of leaking display-wide CSS.
        #[allow(deprecated)]
        background
            .style_context()
            .add_provider(&radius_style, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        background.append(&stack);
        let surface: CircleSurface = glib::Object::new();
        surface.set_overflow(gtk::Overflow::Hidden);
        surface.set_focusable(false);
        background.set_parent(&surface);
        surface.imp().child.replace(Some(background.upcast()));
        surface.set_visible(false);
        let host = Rc::new(Self {
            surface,
            stack,
            radius_style,
            state: Cell::new(State::new(supports_full)),
            frame: Cell::new(None),
            root_pointer: Cell::new(None),
            presented_page: Cell::new(None),
            on_change: RefCell::new(None),
            key_controller: gtk::EventControllerKey::new(),
        });
        let weak = Rc::downgrade(&host);
        host.key_controller
            .connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    if let Some(host) = weak.upgrade() {
                        host.dispatch(Event::Focus(false));
                        host.dispatch(Event::Dismiss);
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        host.surface.add_controller(host.key_controller.clone());
        // The fixed root is authoritative for pointer ownership. Keep the
        // production motion controller for GTK's normal event topology (and
        // for descendants that enter after a page commit), but never accept a
        // child leave: resizing/moving CircleSurface during animation can
        // synthesize that leave for a stationary pointer. Root leave in
        // CircleIntegration performs the real collapse, while repeated child
        // enters are harmless because Pointer(true) is idempotent.
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&host);
        motion.connect_enter(move |_, _, _| {
            if let Some(host) = weak.upgrade()
                && host
                    .root_pointer
                    .get()
                    .is_none_or(|(x, y)| host.frame.get().is_some_and(|frame| frame.contains(x, y)))
            {
                host.dispatch(Event::Pointer(true));
            }
        });
        let weak = Rc::downgrade(&host);
        motion.connect_leave(move |_| {
            if let Some(host) = weak.upgrade()
                && host.root_pointer.get().is_none()
            {
                host.dispatch(Event::Pointer(false));
            }
        });
        host.surface.add_controller(motion);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&host);
        focus.connect_enter(move |_| {
            if let Some(host) = weak.upgrade() {
                // A hover-page control (especially media play/pause) must not
                // turn transient pointer hover into a keyboard pin. Focus is
                // intentional only for the full page.
                if host.mode() == Mode::FullExpanded
                    || host.presented_page() == Some(Mode::FullExpanded)
                {
                    host.dispatch(Event::Focus(true));
                }
            }
        });
        let weak = Rc::downgrade(&host);
        focus.connect_leave(move |_| {
            if let Some(host) = weak.upgrade() {
                host.dispatch(Event::Focus(false));
            }
        });
        host.surface.add_controller(focus);
        Ok(host)
    }

    pub(crate) fn widget(&self) -> &gtk::Widget {
        self.surface.upcast_ref()
    }

    pub(crate) fn mode(&self) -> Mode {
        self.state.get().mode()
    }

    pub(crate) fn revision(&self) -> Revision {
        self.state.get().revision()
    }

    /// Authoritative current geometry for positioning and the input union.
    /// Cleared immediately on absence, regardless of pending frame callbacks.
    pub(crate) fn frame(&self) -> Option<Frame> {
        self.frame.get()
    }

    pub(crate) fn set_root_pointer(&self, point: Option<(f64, f64)>) {
        self.root_pointer.set(point);
    }

    /// A single layout invalidation hook; install before the first Content
    /// event. The hook may dispatch events or replace itself reentrantly.
    pub(crate) fn set_on_change(&self, callback: impl Fn(&CircleHost) + 'static) {
        self.on_change.replace(Some(Rc::new(callback)));
    }

    pub(crate) fn dispatch(&self, event: Event) {
        let mut state = self.state.get();
        if !state.apply(event) {
            return;
        }
        self.state.set(state);
        if state.mode() == Mode::Absent {
            self.frame.set(None);
            self.presented_page.set(None);
            self.surface.set_visible(false);
        }
        // Clone before invoking: callbacks may dispatch recursively or replace
        // the hook, neither of which may occur while `on_change` is borrowed.
        let callback = self.on_change.borrow().clone();
        if let Some(callback) = callback {
            callback(self);
        }
    }

    /// The page requested by the current state.  The presented page remains
    /// unchanged until `commit_page`, allowing an outgoing fade to finish.
    pub(crate) fn target_page(&self) -> Option<Mode> {
        match self.state.get().mode() {
            Mode::Absent => None,
            mode => Some(mode),
        }
    }

    /// The page currently visible in the stack, if any.
    pub(crate) fn presented_page(&self) -> Option<Mode> {
        self.presented_page.get()
    }

    #[cfg(test)]
    pub(crate) fn test_visible_page(&self) -> Option<String> {
        self.stack.visible_child_name().map(|name| name.to_string())
    }

    #[cfg(test)]
    pub(crate) fn test_opacity(&self) -> f64 {
        self.widget().opacity()
    }

    /// Commit the page captured for `revision` after the caller's outgoing
    /// transition.  Stale callbacks cannot reveal an old page.  A caller that
    /// has no animation can call this immediately after dispatch; a valid
    /// frame is still required before the surface becomes visible.
    pub(crate) fn commit_page(&self, revision: Revision) -> bool {
        if !self.state.get().accepts(revision) {
            return false;
        }
        let mode = self.state.get().mode();
        let name = match mode {
            Mode::Absent => return false,
            Mode::Compact => "compact",
            Mode::HoverExpanded => "hover",
            Mode::FullExpanded => "full",
        };
        self.stack.set_visible_child_name(name);
        self.presented_page.set(Some(mode));
        self.surface.set_focusable(mode == Mode::FullExpanded);
        if self.frame.get().is_some() {
            self.surface.set_visible(true);
        }
        true
    }

    pub(crate) fn focus_full_page(&self, revision: Revision) -> bool {
        if !self.state.get().accepts(revision)
            || self.mode() != Mode::FullExpanded
            || self.presented_page() != Some(Mode::FullExpanded)
            || !self.surface.is_visible()
        {
            return false;
        }
        self.surface.set_focusable(true);
        self.surface.set_can_target(true);
        self.stack.set_focusable(true);
        self.stack.set_can_target(true);
        if self.stack.grab_focus() {
            return true;
        }
        if let Some(page) = self.stack.visible_child() {
            page.set_focusable(true);
            page.set_can_target(true);
            if page.grab_focus() {
                return true;
            }
        }
        self.surface.grab_focus()
    }

    #[cfg(test)]
    pub(crate) fn test_escape_key(&self) -> glib::Propagation {
        let stopped: bool = self.key_controller.emit_by_name(
            "key-pressed",
            &[
                &gtk::gdk::Key::Escape,
                &0_u32,
                &gtk::gdk::ModifierType::empty(),
            ],
        );
        if stopped {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    }

    /// Apply only current work. `None` suppresses a present slot that cannot fit.
    /// The caller owns Fixed positioning and the window's complete input union.
    /// Visibility is enabled only here, after valid bounded geometry exists and
    /// after the integration commits a page with `commit_page`.
    pub(crate) fn render(&self, revision: Revision, frame: Option<Frame>) -> bool {
        if !self.state.get().accepts(revision) {
            return false;
        }
        let old_frame = self.frame.replace(frame);
        if let Some(frame) = frame {
            if old_frame.is_none_or(|old| old.radius != frame.radius) {
                self.radius_style.load_from_string(&format!(
                    ".circle-surface {{ border-radius: {}px; }}",
                    frame.radius
                ));
            }
            self.surface.imp().radius.set(frame.radius);
            self.surface
                .imp()
                .requested_width
                .set(frame.rect.width.round().max(1.0) as i32);
            self.surface
                .imp()
                .requested_height
                .set(frame.rect.height.round().max(1.0) as i32);
            self.surface
                .set_size_request(frame.rect.width as i32, frame.rect.height as i32);
            // The frame is supplied by Fixed after the host may already have
            // been allocated once at 0x0 (notably on a freshly mapped
            // ScrolledWindow page). Explicitly invalidate the parent layout
            // so the new frame request is allocated in the same frame.
            self.surface.queue_resize();
            self.surface.queue_draw();
            self.surface
                .set_visible(self.presented_page.get().is_some());
        } else {
            self.surface.imp().requested_width.set(0);
            self.surface.imp().requested_height.set(0);
            self.surface.set_visible(false);
        }
        // GTK visibility/focus changes can synchronously dispatch new intent.
        // Publish geometry before those calls, so disappearance wins reentrancy.
        self.state.get().accepts(revision)
    }
}

fn scroll_page(child: &gtk::Widget) -> gtk::ScrolledWindow {
    child.set_hexpand(true);
    child.set_vexpand(true);
    child.set_halign(gtk::Align::Fill);
    child.set_valign(gtk::Align::Fill);
    gtk::ScrolledWindow::builder()
        .child(child)
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .has_frame(false)
        .build()
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CircleSurface {
        pub child: RefCell<Option<gtk::Widget>>,
        pub radius: Cell<f64>,
        // CircleSurface has a custom measure vfunc, so GTK does not
        // automatically fold set_size_request into its measured minimum.
        // Keep the current integration frame as the only host constraint;
        // page natural sizes must never determine the compact allocation.
        pub requested_width: Cell<i32>,
        pub requested_height: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CircleSurface {
        const NAME: &'static str = "MithshellCircleSurface";
        type Type = super::CircleSurface;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CircleSurface {
        fn dispose(&self) {
            if let Some(child) = self.child.borrow_mut().take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for CircleSurface {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let requested = match orientation {
                gtk::Orientation::Horizontal => self.requested_width.get(),
                gtk::Orientation::Vertical => self.requested_height.get(),
                _ => 0,
            };
            // The current frame, not Stack/ScrolledWindow natural sizes, owns
            // allocation.  Returning it as min and natural also makes Fixed
            // allocate real pages instead of a 0x0 host.
            (requested, requested, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let rect = gtk::graphene::Rect::new(0.0, 0.0, obj.width() as f32, obj.height() as f32);
            snapshot.push_rounded_clip(&gtk::gsk::RoundedRect::from_rect(
                rect,
                self.radius.get() as f32,
            ));
            if let Some(child) = self.child.borrow().as_ref() {
                obj.snapshot_child(child, snapshot);
            }
            snapshot.pop();
        }

        fn contains(&self, x: f64, y: f64) -> bool {
            let obj = self.obj();
            Frame {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: f64::from(obj.width()),
                    height: f64::from(obj.height()),
                },
                radius: self.radius.get(),
            }
            .contains(x, y)
        }
    }
}

glib::wrapper! {
    pub struct CircleSurface(ObjectSubclass<imp::CircleSurface>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
