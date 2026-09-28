use std::rc::Rc;

use async_channel::Receiver;
use gtk::glib;
use log::warn;

use super::Controller;
use crate::{
    hyprland::HyprlandUpdate,
    ipc::{MonitorTarget, OsdKind},
    media::VisualizerLevels,
    state::{
        AudioState, HardwareSnapshot, MediaState, OsdState, SystemSnapshot, TrayItem, WeatherState,
    },
};

impl Controller {
    fn subscribe<T: 'static>(
        self: &Rc<Self>,
        receiver: Receiver<T>,
        handle: impl Fn(&Self, T) + 'static,
    ) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            while let Ok(event) = receiver.recv().await {
                let Some(controller) = weak.upgrade() else {
                    break;
                };
                handle(&controller, event);
            }
        });
    }

    pub(super) fn attach_hyprland(self: &Rc<Self>, receiver: Receiver<HyprlandUpdate>) {
        self.subscribe(receiver, |controller, update| match update {
            HyprlandUpdate::Snapshot(snapshot) => {
                for island in controller.islands.borrow().values() {
                    island.update_hyprland(&snapshot);
                }
                *controller.hyprland.borrow_mut() = snapshot;
            }
            HyprlandUpdate::Unavailable(message) => warn!("Hyprland IPC: {message}"),
        });
    }

    pub(super) fn attach_system(self: &Rc<Self>, receiver: Receiver<SystemSnapshot>) {
        self.subscribe(receiver, |controller, snapshot| {
            let snapshot = {
                let mut system = controller.system.borrow_mut();
                system.update(snapshot);
                system.snapshot().clone()
            };
            for island in controller.islands.borrow().values() {
                island.update_system(&snapshot);
            }
            if let Some(session) = controller.lock.borrow().as_ref() {
                session.update_system(&snapshot);
            }
        });
    }

    pub(super) fn attach_audio(self: &Rc<Self>, receiver: Receiver<AudioState>) {
        self.subscribe(receiver, |controller, audio| {
            let suppress_osd = controller.pending_volume.get() == Some(audio.percent);
            if suppress_osd {
                controller.pending_volume.set(None);
            }
            let snapshot = {
                let mut system = controller.system.borrow_mut();
                system.update_audio(audio);
                system.snapshot().clone()
            };
            for island in controller.islands.borrow().values() {
                island.update_system(&snapshot);
            }
            if suppress_osd {
                return;
            }
            match controller.target_islands(&MonitorTarget::Focused) {
                Ok(islands) => {
                    for island in islands {
                        island.show_osd(OsdState {
                            kind: OsdKind::Volume,
                            value: audio.percent,
                            muted: audio.muted,
                            timeout_ms: 1_500,
                        });
                    }
                }
                Err(error) => warn!("cannot show volume OSD: {error:#}"),
            }
        });
    }

    pub(super) fn attach_telemetry(self: &Rc<Self>, receiver: Receiver<HardwareSnapshot>) {
        self.subscribe(receiver, |controller, hardware| {
            let snapshot = {
                let mut system = controller.system.borrow_mut();
                system.update_hardware(hardware);
                system.snapshot().clone()
            };
            for island in controller.islands.borrow().values() {
                island.update_system(&snapshot);
            }
        });
    }

    pub(super) fn attach_media(self: &Rc<Self>, receiver: Receiver<Option<MediaState>>) {
        self.subscribe(receiver, |controller, state| {
            for island in controller.islands.borrow().values() {
                island.update_media(state.as_ref());
            }
            *controller.media.borrow_mut() = state;
        });
    }

    pub(super) fn attach_weather(self: &Rc<Self>, receiver: Receiver<WeatherState>) {
        self.subscribe(receiver, |controller, state| {
            for island in controller.islands.borrow().values() {
                island.update_weather(Some(&state));
            }
            if let Some(session) = controller.lock.borrow().as_ref() {
                session.update_weather(&state);
            }
            *controller.weather.borrow_mut() = Some(state);
        });
    }

    pub(super) fn attach_tray(self: &Rc<Self>, receiver: Receiver<Vec<TrayItem>>) {
        self.subscribe(receiver, |controller, items| {
            for island in controller.islands.borrow().values() {
                island.update_tray(&items);
            }
            *controller.tray.borrow_mut() = items;
        });
    }

    pub(super) fn attach_visualizer(self: &Rc<Self>, receiver: Receiver<VisualizerLevels>) {
        self.subscribe(receiver, |controller, levels| {
            if controller.media.borrow().is_some() {
                for island in controller.islands.borrow().values() {
                    island.update_visualizer(levels);
                }
            }
            *controller.visualizer.borrow_mut() = levels;
        });
    }
}
