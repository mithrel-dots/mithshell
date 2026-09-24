//! Optional module surfaces, mounted as siblings of the central clipped island.
//!
//! Modules supply three **distinct, unparented** widgets and keep their existing
//! snapshot/action ownership. `CircleHost` owns only presentation and input state.
//! Mount `host.widget()` in the integration's outer `gtk::Fixed`, then call
//! `dispatch(Content(..))` on snapshot changes. `set_on_change` requests relayout;
//! it must not capture a strong reference back to the host.
//!
//! The caller selects Material profiles and interpolates `Visual` from the last
//! rendered value (including on reversal). Each frame calls `layout` with the
//! **current animated central bounds**, calls `render`, and only on success moves
//! the widget. Page changes are staged: keep the old page through the outgoing
//! fade, then call `commit_page(revision)` before the incoming fade. A
//! no-animation integration may render and commit synchronously. Union each
//! host's current `frame().input_rectangles()` with the
//! central input region (a missing frame contributes nothing). All origins
//! must be in the same window-local GTK logical coordinate system; translate the
//! monitor into that system first. Do not multiply by the monitor's device scale.
//! `layout` applies only the shell's design/UI scale. Shadows are not input.
//!
//! Capture `revision()` when starting a frame callback. `render` rejects outdated
//! revisions; stop that callback on rejection. Absence hides immediately, even
//! during an animation. No timers, backend subscriptions or easing live here.
//!
//! Full history remains in this same host/anchor. A module's explicit background
//! button dispatches `OpenFull`; the host deliberately does not capture arbitrary
//! child clicks, which may activate notifications or media/tray actions. Escape
//! or outside dismissal dispatches `Dismiss`. Keyboard/layer-shell routing and
//! popover input regions are the central integrator's responsibility.

// Staged contract: remove this allowance when the central integration consumes it.
#![allow(dead_code)]

mod geometry;
mod widget;

#[allow(unused_imports)] // Public handoff, consumed by the pending module workers.
pub(crate) use geometry::{CircleRequest, CircleSpec, Frame, Rect, Size, Visual, layout};
#[allow(unused_imports)]
pub(crate) use widget::{CircleContent, CircleHost};

/// Apply resolved-scale typography to new labels, including rebuilt snapshots.
/// Widget-local providers avoid scale leaking between outputs.
pub(crate) fn scale_text(root: &impl gtk::prelude::IsA<gtk::Widget>, scale: f64) {
    use gtk::prelude::*;
    let root = root.as_ref();
    if root.is::<gtk::Label>()
        && !root.has_css_class("circle-scaled-text")
        && !root.has_css_class("notification-circle-bell")
    {
        let style = gtk::CssProvider::new();
        let size = if root.has_css_class(crate::ui::icon::GLYPH_CLASS) {
            16.0
        } else {
            14.0
        };
        style.load_from_string(&format!("* {{ font-size: {}px; }}", (size * scale).round()));
        #[allow(deprecated)]
        root.style_context()
            .add_provider(&style, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2);
        root.add_css_class("circle-scaled-text");
    }
    let mut child = root.first_child();
    while let Some(widget) = child {
        scale_text(&widget, scale);
        child = widget.next_sibling();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Absent,
    Compact,
    HoverExpanded,
    FullExpanded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Content(bool),
    Pointer(bool),
    /// Aggregate all open menus, including nested DBusMenu popovers, before
    /// sending false. GTK pointer leaves during a menu grab must not collapse.
    Menu(bool),
    Focus(bool),
    OpenFull,
    /// Clears full/hover intent, but never overrides an active menu/focus pin.
    /// The integrator should close menus/release keyboard focus when dismissing.
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Revision(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct State {
    present: bool,
    hovered: bool,
    menu: bool,
    focus: bool,
    full: bool,
    supports_full: bool,
    revision: Revision,
}

impl State {
    pub(crate) fn new(supports_full: bool) -> Self {
        Self {
            present: false,
            hovered: false,
            menu: false,
            focus: false,
            full: false,
            supports_full,
            revision: Revision(0),
        }
    }

    pub(crate) fn mode(self) -> Mode {
        if !self.present {
            Mode::Absent
        } else if self.full {
            Mode::FullExpanded
        } else if self.hovered || self.menu || self.focus {
            Mode::HoverExpanded
        } else {
            Mode::Compact
        }
    }

    pub(crate) fn revision(self) -> Revision {
        self.revision
    }

    /// Returns true for changes to intent/pins as well as effective mode. This
    /// invalidates work captured before dismissal, disappearance or re-entry.
    pub(crate) fn apply(&mut self, event: Event) -> bool {
        let old = *self;
        match event {
            Event::Content(present) => {
                // Content(true) is also the snapshot invalidation signal.  A
                // module can remain present while its widget tree/data is
                // replaced, so do not let an old frame callback survive that
                // snapshot boundary.
                let snapshot_refresh = present && self.present;
                self.present = present;
                if !present {
                    self.hovered = false;
                    self.menu = false;
                    self.focus = false;
                    self.full = false;
                }
                if snapshot_refresh {
                    self.revision.0 = self.revision.0.wrapping_add(1);
                    return true;
                }
            }
            _ if !self.present => return false,
            Event::Pointer(hovered) => self.hovered = hovered,
            Event::Menu(menu) => self.menu = menu,
            Event::Focus(focus) => self.focus = focus,
            Event::OpenFull => self.full = self.supports_full,
            Event::Dismiss => {
                self.full = false;
                self.hovered = false;
            }
        }
        if *self == old {
            return false;
        }
        self.revision.0 = self.revision.0.wrapping_add(1);
        true
    }

    pub(crate) fn accepts(self, revision: Revision) -> bool {
        self.present && self.revision == revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_grab_and_focus_hold_expansion_until_both_are_released() {
        let mut state = State::new(false);
        for event in [
            Event::Content(true),
            Event::Pointer(true),
            Event::Menu(true),
            Event::Pointer(false),
            Event::Focus(true),
            Event::Menu(false),
        ] {
            state.apply(event);
        }
        assert_eq!(state.mode(), Mode::HoverExpanded);
        state.apply(Event::Dismiss);
        assert_eq!(state.mode(), Mode::HoverExpanded);
        state.apply(Event::Focus(false));
        assert_eq!(state.mode(), Mode::Compact);
        state.apply(Event::OpenFull);
        assert_eq!(state.mode(), Mode::Compact);
    }

    #[test]
    fn full_history_survives_pointer_leave_but_not_dismissal_or_empty_history() {
        let mut state = State::new(true);
        state.apply(Event::Content(true));
        state.apply(Event::Pointer(true));
        state.apply(Event::OpenFull);
        state.apply(Event::Pointer(false));
        assert_eq!(state.mode(), Mode::FullExpanded);
        state.apply(Event::Dismiss);
        assert_eq!(state.mode(), Mode::Compact);
        state.apply(Event::OpenFull);
        state.apply(Event::Menu(true));
        let old = state.revision();
        state.apply(Event::Content(false));
        assert_eq!(state.mode(), Mode::Absent);
        assert!(!state.accepts(old));
        state.apply(Event::Focus(true)); // stale focus from a removed child
        state.apply(Event::OpenFull);
        state.apply(Event::Content(true));
        assert_eq!(state.mode(), Mode::Compact);
        assert!(!state.accepts(old));
    }

    #[test]
    fn reversal_invalidates_in_flight_work_even_if_target_returns_to_same_mode() {
        let mut state = State::new(false);
        state.apply(Event::Content(true));
        state.apply(Event::Pointer(true));
        let old = state.revision();
        state.apply(Event::Pointer(false));
        state.apply(Event::Pointer(true));
        assert_eq!(state.mode(), Mode::HoverExpanded);
        assert!(!state.accepts(old));
        assert!(state.accepts(state.revision()));
    }

    #[test]
    fn repeated_present_content_invalidates_the_previous_snapshot() {
        let mut state = State::new(false);
        assert!(state.apply(Event::Content(true)));
        let old = state.revision();
        assert!(state.apply(Event::Content(true)));
        assert_eq!(state.mode(), Mode::Compact);
        assert!(!state.accepts(old));
        assert!(state.accepts(state.revision()));
    }
}
