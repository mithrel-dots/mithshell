//! Central ownership for optional module circles.
//!
//! Circle widgets are siblings of the clipped island surface.  This keeps
//! their shadows and input regions out of the central surface's clip while
//! retaining one layer window, one snapshot owner, and one dismissal path.

use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
    time::{Duration, Instant},
};

use super::IslandWindow;
use super::circle::{self, CircleHost, CircleRequest, CircleSpec, Event, Rect, Size, Visual};
use super::media_circle::{MediaCircle, MediaCircleActions};
use super::notification_circle::{NotificationCircle, NotificationCircleCallbacks};
use super::profile_timing;
use super::tray_circle::TrayCircle;
use crate::config::{CircleModule, NotificationConfig, TrayConfig};
use crate::state::{MediaState, Notification, TrayItem};
use gtk::{glib, prelude::*};

struct CircleAnimation {
    revision: circle::Revision,
    from_mode: circle::Mode,
    mode: circle::Mode,
    from: Visual,
    target: Visual,
    started: Instant,
    geometry: crate::ui::motion::Profile,
    out: crate::ui::motion::Profile,
    incoming: crate::ui::motion::Profile,
    phase: AnimationPhase,
    page_committed: bool,
    opacity_start: f64,
    total: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnimationPhase {
    Outgoing,
    Incoming,
}

fn mode_rank(mode: circle::Mode) -> u8 {
    match mode {
        circle::Mode::Absent => 0,
        circle::Mode::Compact => 1,
        circle::Mode::HoverExpanded => 2,
        circle::Mode::FullExpanded => 3,
    }
}

/// Selects the container profile from the actual transition direction.  The
/// previous target is used while reversing so a pending Full→Compact change
/// remains a collapse even if the old page is still presented by the stack.
pub(crate) fn circle_transition_profile(
    from: circle::Mode,
    to: circle::Mode,
) -> crate::ui::motion::Profile {
    if mode_rank(to) < mode_rank(from) {
        crate::ui::motion::Profile::CONTAINER_COLLAPSE
    } else {
        crate::ui::motion::Profile::CONTAINER_EXPAND
    }
}

fn sample_visual(animation: &CircleAnimation, now: Instant) -> Visual {
    let elapsed = now.saturating_duration_since(animation.started);
    animation
        .from
        .interpolate(animation.target, animation.geometry.progress(elapsed))
}

struct Slot {
    module: CircleModule,
    host: Rc<CircleHost>,
    spec: CircleSpec,
}

pub(crate) struct CircleIntegration {
    slots: [Option<Slot>; 2],
    tray: Option<TrayCircle>,
    media: Option<Rc<MediaCircle>>,
    notifications: Option<Rc<NotificationCircle>>,
    animations: RefCell<[Option<CircleAnimation>; 2]>,
    tick_scheduled: Cell<bool>,
    main_promoted_for_catcher: Cell<bool>,
    owner: Weak<IslandWindow>,
}

impl CircleIntegration {
    /// Updates each circle from the fixed root's coordinates.  This is the
    /// sole pointer source for circle hover: child motion controllers are not
    /// reliable while their allocations are animated because GTK may report a
    /// synthetic leave/enter without any physical pointer movement.
    pub(crate) fn update_pointer(&self, x: f64, y: f64) {
        for slot in self.slots.iter().flatten() {
            slot.host.set_root_pointer(Some((x, y)));
            let hovered = slot.host.frame().is_some_and(|frame| frame.contains(x, y));
            slot.host.dispatch(Event::Pointer(hovered));
        }
    }

    pub(crate) fn clear_pointer(&self) {
        for slot in self.slots.iter().flatten() {
            slot.host.set_root_pointer(None);
            slot.host.dispatch(Event::Pointer(false));
        }
    }

    pub(crate) fn owns(&self, module: CircleModule) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|slot| slot.module == module)
    }
    pub(crate) fn new(
        island: &Rc<IslandWindow>,
        left: CircleModule,
        right: CircleModule,
        tray_config: &TrayConfig,
        notification_config: &NotificationConfig,
    ) -> Result<Self, &'static str> {
        let tray = (left == CircleModule::Tray || right == CircleModule::Tray)
            .then(|| TrayCircle::new(island, tray_config))
            .transpose()?;
        let media = if left == CircleModule::Media || right == CircleModule::Media {
            let actions = MediaCircleActions {
                play_pause: island.actions.media_play_pause.clone(),
                next: island.actions.media_next.clone(),
                previous: island.actions.media_previous.clone(),
                select: {
                    let weak = Rc::downgrade(island);
                    Rc::new(move |service| {
                        if let Some(island) = weak.upgrade() {
                            island.select_media_service(service);
                        }
                    })
                },
            };
            Some(MediaCircle::new(island.metrics, actions)?)
        } else {
            None
        };
        let weak = Rc::downgrade(island);
        let callbacks = NotificationCircleCallbacks {
            invoke: island.actions.notification_invoke.clone(),
            dismiss: island.actions.notification_dismiss.clone(),
            clear: island.actions.notification_clear_all.clone(),
            inhibit: island.actions.notification_inhibit.clone(),
            open_full: Rc::new(move || {
                if let Some(island) = weak.upgrade() {
                    let host = island
                        .circles
                        .borrow()
                        .as_ref()
                        .and_then(|circles| circles.notifications.as_ref())
                        .map(|circle| circle.host.clone());
                    if let Some(host) = host {
                        host.dispatch(Event::OpenFull);
                    }
                    let weak = Rc::downgrade(&island);
                    glib::idle_add_local_once(move || {
                        if let Some(island) = weak.upgrade() {
                            island.relayout_circles();
                            island.schedule_circle_full_focus();
                        }
                    });
                }
            }),
        };
        let notifications =
            if left == CircleModule::Notifications || right == CircleModule::Notifications {
                Some(NotificationCircle::new(
                    notification_config,
                    island.metrics.icons,
                    island.metrics.scale,
                    callbacks,
                )?)
            } else {
                None
            };

        let mut result = Self {
            slots: [None, None],
            tray,
            media,
            notifications,
            animations: RefCell::new([None, None]),
            tick_scheduled: Cell::new(false),
            main_promoted_for_catcher: Cell::new(false),
            owner: Rc::downgrade(island),
        };
        for (index, module) in [left, right].into_iter().enumerate() {
            let host = match module {
                CircleModule::Tray => result.tray.as_ref().map(TrayCircle::host),
                CircleModule::Media => result.media.as_ref().map(|m| m.host()),
                CircleModule::Notifications => {
                    result.notifications.as_ref().map(|n| n.host.clone())
                }
                CircleModule::None => None,
            };
            if let Some(host) = host {
                let (spec, _) = spec_for(module, island.metrics.scale);
                island.fixed.put(host.widget(), 0.0, 0.0);
                let weak = Rc::downgrade(island);
                host.set_on_change(move |_| {
                    // Module update callbacks can be emitted while the
                    // integration is borrowed. Defer the relayout one
                    // main-loop turn to keep snapshot replacement
                    // re-entrant and to coalesce rapid state changes.
                    let weak = weak.clone();
                    gtk::glib::idle_add_local_once(move || {
                        if let Some(island) = weak.upgrade() {
                            island.relayout_circles();
                        }
                    });
                });
                result.slots[index] = Some(Slot { module, host, spec });
            }
        }
        Ok(result)
    }

    pub(crate) fn update_tray(&self, items: &[TrayItem]) {
        if let Some(circle) = &self.tray {
            circle.update(items);
        }
    }
    pub(crate) fn update_media(&self, state: Option<&MediaState>) {
        if let Some(circle) = &self.media {
            circle.update(state);
        }
    }
    pub(crate) fn update_notifications(&self, history: &[Notification]) {
        if let Some(circle) = &self.notifications {
            circle.update(history);
        }
    }
    pub(crate) fn update_inhibition(&self, active: bool, remaining: Option<&str>) {
        if let Some(circle) = &self.notifications {
            circle.update_inhibition(active, remaining);
        }
    }
    pub(crate) fn redraw_theme(&self) {
        if let Some(circle) = &self.media {
            circle.redraw_theme();
        }
    }

    pub(crate) fn full_host(&self) -> Option<Rc<CircleHost>> {
        self.slots.iter().enumerate().find_map(|(index, slot)| {
            let slot = slot.as_ref()?;
            let pending_full = self.animations.borrow()[index]
                .as_ref()
                .is_some_and(|animation| {
                    animation.mode == circle::Mode::FullExpanded
                        || animation.from_mode == circle::Mode::FullExpanded
                });
            (slot.host.mode() == circle::Mode::FullExpanded
                || slot.host.presented_page() == Some(circle::Mode::FullExpanded)
                || pending_full)
                .then(|| slot.host.clone())
        })
    }

    #[cfg(test)]
    pub(crate) fn test_host(&self, index: usize) -> Option<Rc<CircleHost>> {
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .map(|slot| slot.host.clone())
    }

    #[cfg(test)]
    pub(crate) fn test_click_media_play_pause(&self) {
        if let Some(media) = &self.media {
            media.test_click_play_pause();
        }
    }

    #[cfg(test)]
    pub(crate) fn test_media_play_pause_button(&self) -> Option<gtk::Button> {
        self.media
            .as_ref()
            .map(|media| media.test_play_pause_button())
    }

    #[cfg(test)]
    pub(crate) fn test_media_select_service(&self, service: &str) {
        if let Some(media) = &self.media {
            media.test_select_service(service);
        }
    }

    #[cfg(test)]
    pub(crate) fn test_media_service(&self) -> Option<String> {
        self.media.as_ref().and_then(|media| media.test_service())
    }

    #[cfg(test)]
    pub(crate) fn test_notification_compact_click(&self) {
        if let Some(circle) = &self.notifications {
            circle.test_click_compact();
        }
    }

    #[cfg(test)]
    pub(crate) fn test_notification_inhibition(&self) -> Option<(bool, String, bool)> {
        self.notifications
            .as_ref()
            .map(|circle| circle.test_inhibition())
    }

    pub(crate) fn relayout(&self, island: &IslandWindow) {
        if self.full_host().is_some() {
            if !self.main_promoted_for_catcher.replace(true) {
                island.promote_main_above_dismiss_catcher();
            }
        } else if self.main_promoted_for_catcher.get() {
            // Restore the layer before releasing ownership of the promotion.
            // Dashboard/search/weather remain Overlay; compact/media return to
            // Top only when no other view owns the Overlay layer.
            island.restore_main_layer_after_full_circle();
            self.main_promoted_for_catcher.set(false);
        }
        let central = island.central_circle_rect();
        let monitor = Rect {
            x: 0.0,
            y: 0.0,
            width: f64::from(island.metrics.window_width),
            height: f64::from(island.metrics.window_height),
        };
        let now = Instant::now();
        let mut animations = self.animations.borrow_mut();
        let mut commit = [false; 2];
        let slots = self
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                slot.as_ref().and_then(|slot| {
                    let mode = slot.host.mode();
                    if mode == circle::Mode::Absent {
                        animations[index] = None;
                        slot.host.widget().set_opacity(0.0);
                        return None;
                    }
                    let target = slot.spec.visual(mode)?;
                    let presented = slot.host.presented_page();
                    let has_animation = animations[index].is_some();
                    if presented == Some(mode) && !has_animation {
                        // A same-page snapshot revision invalidates stale
                        // callbacks but must not flicker or restart motion.
                        animations[index] = None;
                        slot.host.widget().set_opacity(1.0);
                        return Some(CircleRequest {
                            spec: slot.spec,
                            visual: target,
                        });
                    }
                    let restart = animations[index].as_ref().is_none_or(|animation| {
                        animation.revision != slot.host.revision() || animation.mode != mode
                    });
                    if restart {
                        if presented.is_none() {
                            animations[index] = None;
                            commit[index] = true;
                            return Some(CircleRequest {
                                spec: slot.spec,
                                visual: target,
                            });
                        }
                        let from = animations[index]
                            .as_ref()
                            .map(|animation| sample_visual(animation, now))
                            .or_else(|| presented.and_then(|old| slot.spec.visual(old)))
                            .unwrap_or(target);
                        let from_mode = animations[index]
                            .as_ref()
                            .map(|animation| animation.mode)
                            .or(presented)
                            .unwrap_or(circle::Mode::Compact);
                        let geometry_only = presented == Some(mode);
                        let opacity_start = slot.host.widget().opacity();
                        let animation_ms = island.animation_ms.get();
                        if !island.animations_enabled.get() || animation_ms == 0 {
                            animations[index] = None;
                            commit[index] = true;
                            slot.host.widget().set_opacity(1.0);
                            return Some(CircleRequest {
                                spec: slot.spec,
                                visual: target,
                            });
                        }
                        let geometry = profile_timing(
                            circle_transition_profile(from_mode, mode),
                            true,
                            animation_ms,
                        );
                        let out = profile_timing(
                            crate::ui::motion::Profile::CONTENT_OUT,
                            true,
                            animation_ms,
                        );
                        let incoming = profile_timing(
                            crate::ui::motion::Profile::CONTENT_IN,
                            true,
                            animation_ms,
                        );
                        let total = geometry.duration.max(if geometry_only {
                            incoming.duration
                        } else {
                            out.duration.saturating_add(incoming.duration)
                        });
                        animations[index] = Some(CircleAnimation {
                            revision: slot.host.revision(),
                            from_mode,
                            mode,
                            from,
                            target,
                            started: now,
                            geometry,
                            out,
                            incoming,
                            phase: if geometry_only {
                                AnimationPhase::Incoming
                            } else {
                                AnimationPhase::Outgoing
                            },
                            page_committed: geometry_only,
                            opacity_start: if geometry_only { opacity_start } else { 1.0 },
                            total,
                        });
                    }
                    let animation = animations[index].as_mut().expect("animation installed");
                    let elapsed = now.saturating_duration_since(animation.started);
                    let visual = sample_visual(animation, now);
                    let mut committed_now = false;
                    if !animation.page_committed && elapsed >= animation.out.duration {
                        animation.phase = AnimationPhase::Incoming;
                        animation.page_committed = slot.host.commit_page(animation.revision);
                        animation.opacity_start = 0.0;
                        committed_now = true;
                        slot.host.widget().set_opacity(0.0);
                    }
                    let opacity = if committed_now {
                        0.0
                    } else {
                        match animation.phase {
                            AnimationPhase::Outgoing => 1.0 - animation.out.progress(elapsed),
                            AnimationPhase::Incoming => {
                                animation.opacity_start
                                    + (1.0 - animation.opacity_start)
                                        * animation.incoming.progress(
                                            elapsed.saturating_sub(animation.out.duration),
                                        )
                            }
                        }
                    };
                    slot.host.widget().set_opacity(opacity);
                    if elapsed >= animation.total {
                        if !animation.page_committed {
                            commit[index] = true;
                        }
                        slot.host.widget().set_opacity(1.0);
                        animations[index] = None;
                    }
                    Some(CircleRequest {
                        spec: slot.spec,
                        visual,
                    })
                })
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let frames = circle::layout(central, monitor, island.metrics.scale, slots);
        for (index, (slot, frame)) in self.slots.iter().zip(frames).enumerate() {
            if let Some(slot) = slot {
                let revision = slot.host.revision();
                let rendered = slot.host.render(revision, frame);
                if rendered && (slot.host.presented_page().is_none() || commit[index]) {
                    let _ = slot.host.commit_page(revision);
                }
                if let Some(frame) = frame {
                    slot.host
                        .widget()
                        .set_visible(slot.host.presented_page().is_some());
                    island
                        .fixed
                        .move_(slot.host.widget(), frame.rect.x, frame.rect.y);
                    // Fixed normally picks this up from the size request on
                    // its next resize pass.  A mapped custom widget can still
                    // retain the previous page allocation for one pass,
                    // though; publish the committed frame to the actual GTK
                    // child now so Stack/ScrolledWindow descendants cannot
                    // observe a stale compact allocation during a rebuild.
                    slot.host.widget().allocate(
                        frame.rect.width.round().max(1.0) as i32,
                        frame.rect.height.round().max(1.0) as i32,
                        -1,
                        None,
                    );
                }
            }
        }
        // CircleSurface publishes its current frame through a custom measure
        // vfunc.  Force the outer Fixed to consume that request immediately;
        // otherwise a host first mapped while its ScrolledWindow page is
        // empty can remain allocated at 0x0 until an unrelated resize.
        // A custom CircleSurface publishes a new size request when a page
        // commits.  Queue the parent resize as well as allocation; allocation
        // alone can retain the compact page's old request on a mapped Fixed.
        island.fixed.queue_resize();
        island.fixed.queue_allocate();
        island.update_circle_input_region();
        drop(animations);
        if self.animations.borrow().iter().any(Option::is_some) {
            self.ensure_tick(island);
        }
        island.refresh_keyboard_mode();
    }

    fn ensure_tick(&self, _island: &IslandWindow) {
        if self.tick_scheduled.replace(true) {
            return;
        }
        let weak = self.owner.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            island.relayout_circles();
            if island.circle_animation_active() {
                glib::ControlFlow::Continue
            } else {
                island.clear_circle_tick_flag();
                glib::ControlFlow::Break
            }
        });
    }
}

impl IslandWindow {
    pub(crate) fn central_circle_rect(&self) -> Rect {
        let geometry = self.geometry.get();
        Rect {
            x: (f64::from(self.metrics.window_width) - geometry.width) / 2.0,
            y: geometry.y,
            width: geometry.width,
            height: geometry.height,
        }
    }

    pub(crate) fn relayout_circles(&self) {
        if let Some(circles) = self.circles.borrow().as_ref() {
            circles.relayout(self);
        }
    }

    pub(crate) fn schedule_circle_full_focus(&self) {
        if self.search_open.get() && self.search_entry.has_focus() {
            return;
        }
        let Some((host, revision)) = self
            .circles
            .borrow()
            .as_ref()
            .and_then(CircleIntegration::full_host)
            .map(|host| {
                let revision = host.revision();
                (host, revision)
            })
        else {
            return;
        };
        let weak = self.owner_weak();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(island) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if island.search_open.get() && island.search_entry.has_focus() {
                return glib::ControlFlow::Break;
            }
            if host.focus_full_page(revision) {
                if let Some(root) = host.widget().root() {
                    root.set_focus(Some(host.widget()));
                }
                glib::ControlFlow::Break
            } else if host.mode() == circle::Mode::FullExpanded && host.revision() == revision {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }

    fn owner_weak(&self) -> Weak<IslandWindow> {
        self.circles
            .borrow()
            .as_ref()
            .map_or_else(Weak::new, |circles| circles.owner.clone())
    }

    pub(crate) fn dismiss_full_circle(&self) -> bool {
        let host = self
            .circles
            .borrow()
            .as_ref()
            .and_then(CircleIntegration::full_host);
        if let Some(host) = host {
            host.dispatch(Event::Focus(false));
            host.dispatch(Event::Dismiss);
            self.relayout_circles();
            if self.circle_full_active() {
                self.present_dismiss_catcher_behind_main();
            }
            true
        } else {
            false
        }
    }

    pub(crate) fn circle_full_active(&self) -> bool {
        self.circles
            .borrow()
            .as_ref()
            .and_then(CircleIntegration::full_host)
            .is_some()
    }

    fn circle_animation_active(&self) -> bool {
        self.circles
            .borrow()
            .as_ref()
            .is_some_and(|circles| circles.animations.borrow().iter().any(Option::is_some))
    }

    fn clear_circle_tick_flag(&self) {
        if let Some(circles) = self.circles.borrow().as_ref() {
            circles.tick_scheduled.set(false);
        }
    }

    pub(crate) fn circle_debug_state(&self) -> serde_json::Value {
        let circles = self.circles.borrow();
        let Some(circles) = circles.as_ref() else {
            return serde_json::json!({});
        };
        let slots: Vec<_> = circles.slots.iter().map(|slot| {
            slot.as_ref().map_or_else(|| serde_json::json!({"module":"none","present":false}), |slot| {
                let frame = slot.host.frame().map(|f| serde_json::json!({"x":f.rect.x,"y":f.rect.y,"width":f.rect.width,"height":f.rect.height,"radius":f.radius}));
                serde_json::json!({"module": format!("{:?}", slot.module).to_lowercase(), "mode": format!("{:?}", slot.host.mode()).to_lowercase(), "present": slot.host.frame().is_some(), "frame": frame})
            })
        }).collect();
        serde_json::json!({"slots": slots})
    }

    pub(crate) fn update_circle_input_region(&self) {
        let Some(surface) = self.window.surface() else {
            return;
        };
        let geometry = self.geometry.get();
        let region = gtk::cairo::Region::create();
        let central = gtk::cairo::RectangleInt::new(
            ((f64::from(self.metrics.window_width) - geometry.width) / 2.0).round() as i32,
            geometry.y.round() as i32,
            geometry.width.round().max(1.0) as i32,
            geometry.height.round().max(1.0) as i32,
        );
        let central_region = gtk::cairo::Region::create_rectangle(&central);
        let _ = region.union(&central_region);
        let hover = gtk::cairo::RectangleInt::new(
            (f64::from(self.metrics.window_width - self.metrics.media_max_width) / 2.0
                - f64::from(self.metrics.spacing(8))) as i32,
            0,
            self.metrics.media_max_width + self.metrics.spacing(16),
            self.metrics.compact_height + self.metrics.spacing(16),
        );
        let hover_region = gtk::cairo::Region::create_rectangle(&hover);
        let _ = region.union(&hover_region);
        if let Some(circles) = self.circles.borrow().as_ref() {
            for frame in circles
                .slots
                .iter()
                .flatten()
                .filter_map(|slot| slot.host.frame())
            {
                for rect in frame.input_rectangles() {
                    let rect_region = gtk::cairo::Region::create_rectangle(&rect);
                    let _ = region.union(&rect_region);
                }
            }
        }
        surface.set_input_region(Some(&region));
    }
}

fn spec_for(module: CircleModule, _scale: f64) -> (CircleSpec, Visual) {
    // The neutral pill is 32 design pixels high.  Layout applies the shell
    // scale once; the old 48px value made compact circles 48*scale high.
    let diameter = 32.0;
    let hover = match module {
        CircleModule::Notifications => Size {
            width: 300.0,
            height: 250.0,
        },
        CircleModule::Tray => Size {
            width: 300.0,
            height: 150.0,
        },
        CircleModule::Media => Size {
            width: 320.0,
            height: 110.0,
        },
        CircleModule::None => Size {
            width: diameter,
            height: diameter,
        },
    };
    let full = (module == CircleModule::Notifications).then_some(Size {
        width: 420.0,
        height: 520.0,
    });
    let spec = CircleSpec {
        diameter,
        hover,
        full,
    };
    (spec, spec.visual(circle::Mode::Compact).unwrap())
}
