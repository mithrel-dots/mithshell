use super::media::media_state_for_player;
use super::weather::weather_provider_label;
use super::{Geometry, IslandActions, IslandWindow, View, hover_geometry};
use crate::config::{AppConfig, CircleModule, LauncherPresentation};
use crate::state::{
    MediaPlayer, MediaState, Notification, NotificationAction, NotificationTimeout, PlaybackStatus,
    TrayIcon, TrayItem, TrayStatus,
};
use crate::tarragon::TarragonSelection;
use crate::ui::resolved_scale;
use crate::weather::WeatherProvider;
use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

#[test]
fn hover_geometry_is_forward_and_reversible() {
    let resting = Geometry {
        width: 200.0,
        height: 32.0,
        y: 0.0,
    };
    let raised = hover_geometry(resting, 4.0, true);
    assert!(raised.width > resting.width);
    assert!(raised.height > resting.height);
    assert!(raised.y > resting.y);
    assert_eq!(hover_geometry(resting, 4.0, false), resting);
}

#[test]
fn default_animation_value_selects_named_profile() {
    let profile = crate::ui::motion::Transition::HoverEnter.resolve(None, true, 280);
    assert_eq!(
        profile.duration,
        crate::ui::motion::Profile::HOVER_ENTER.duration
    );
    let override_profile = crate::ui::motion::Transition::HoverEnter.resolve(None, true, 333);
    assert_eq!(
        override_profile.duration,
        std::time::Duration::from_millis(333)
    );
}

#[test]
fn circle_transition_profiles_cover_all_ordered_modes_and_reversals() {
    use super::circle::Mode;
    use crate::ui::island::circle_integration::circle_transition_profile;

    let compact = Mode::Compact;
    let hover = Mode::HoverExpanded;
    let full = Mode::FullExpanded;
    let cases = [
        (compact, hover, false),
        (compact, full, false),
        (hover, compact, true),
        (hover, full, false),
        (full, compact, true),
        (full, hover, true),
    ];
    for (from, to, collapse) in cases {
        let profile = circle_transition_profile(from, to);
        assert_eq!(
            profile.duration,
            if collapse {
                crate::ui::motion::Profile::CONTAINER_COLLAPSE.duration
            } else {
                crate::ui::motion::Profile::CONTAINER_EXPAND.duration
            },
            "unexpected profile for {from:?} -> {to:?}"
        );
    }

    // The reversal uses the pending target as its source, not the still
    // presented GTK page: Full→Compact must remain a 200 ms collapse.
    assert_eq!(
        circle_transition_profile(full, compact).duration,
        std::time::Duration::from_millis(200)
    );
    assert_eq!(
        circle_transition_profile(compact, full).duration,
        std::time::Duration::from_millis(500)
    );
}

#[test]
#[ignore = "requires an isolated GTK display; run scripts/run-ui-regressions-gtk.py"]
#[allow(deprecated)]
fn integrated_search_return_uses_real_finish_and_scheduler_path() {
    gtk::init().expect("GTK display");
    let application = gtk::Application::new(
        Some("org.mithshell.presentation-test"),
        gtk::gio::ApplicationFlags::NON_UNIQUE,
    );
    application.connect_activate(|_| {});
    application
        .register(None::<&gtk::gio::Cancellable>)
        .expect("register GTK application");
    let display = gtk::gdk::Display::default().expect("GTK display");
    let monitor = display
        .monitors()
        .item(0)
        .and_downcast::<gtk::gdk::Monitor>()
        .expect("Broadway monitor");
    let actions = IslandActions {
        switch_workspace: Rc::new(|_, _| {}),
        set_volume: Rc::new(|_| {}),
        toggle_mute: Rc::new(|| {}),
        search: Rc::new(|_| {}),
        select: Rc::new(|_: TarragonSelection| {}),
        tarragon_status: Rc::new(|| {}),
        tarragon_reload: Rc::new(|| {}),
        load_preview: Rc::new(|_, _| {}),
        media_play_pause: Rc::new(|_| {}),
        media_next: Rc::new(|_| {}),
        media_previous: Rc::new(|_| {}),
        notification_expired: Rc::new(|_, _| {}),
        notification_dismiss: Rc::new(|_| {}),
        notification_invoke: Rc::new(|_, _| {}),
        notification_clear_all: Rc::new(|| {}),
        notification_inhibit: Rc::new(|_| {}),
        tray_activate: Rc::new(|_, _, _, _| {}),
        tray_secondary_activate: Rc::new(|_, _, _, _| {}),
        tray_context_menu: Rc::new(|_, _, _, _| {}),
        tray_scroll: Rc::new(|_, _, _, _| {}),
        tray_menu_event: Rc::new(|_, _, _| {}),
    };
    let independent_actions = actions.clone();
    let mut config = AppConfig::default();
    config.launcher.presentation = LauncherPresentation::Integrated;
    config.shell.animation_ms = 280;
    let island = IslandWindow::new_for_test(
        &application,
        &monitor,
        "broadway-test".to_owned(),
        &config,
        actions,
        true,
    );
    application.activate();
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    let drain_search = |island: &Rc<IslandWindow>| {
        let frame_loop = gtk::glib::MainLoop::new(None, false);
        let weak = Rc::downgrade(island);
        let loop_for_timeout = frame_loop.clone();
        let completed = Rc::new(std::cell::Cell::new(false));
        let completed_in_callback = completed.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            if let Some(island) = weak.upgrade() {
                if island.current_view.get() == View::Search
                    && !island.view_transition_active.get()
                    && island.search.opacity() == 1.0
                {
                    completed_in_callback.set(true);
                    loop_for_timeout.quit();
                    return gtk::glib::ControlFlow::Break;
                }
            } else {
                loop_for_timeout.quit();
                return gtk::glib::ControlFlow::Break;
            }
            if std::time::Instant::now() >= deadline {
                loop_for_timeout.quit();
                gtk::glib::ControlFlow::Break
            } else {
                gtk::glib::ControlFlow::Continue
            }
        });
        frame_loop.run();
        assert!(
            completed.get(),
            "animated search transition did not settle before deadline"
        );
    };
    island.open_search();
    drain_search(&island);
    // The outside catcher must be a real mapped/pickable GTK child, not
    // merely a compositor surface reported at monitor size.  This is the
    // regression that a synthetic GestureClick emission cannot cover.
    let catcher_width = island.dismiss_area.allocated_width();
    let catcher_height = island.dismiss_area.allocated_height();
    assert!(
        catcher_width > island.metrics.dashboard_width,
        "mapped catcher width {catcher_width} did not cover the dashboard"
    );
    assert!(
        catcher_height > island.metrics.dashboard_height,
        "mapped catcher height {catcher_height} did not cover the dashboard"
    );
    let far_pick = island.dismiss_area.pick(
        f64::from(catcher_width - 1),
        f64::from(catcher_height - 1),
        gtk::PickFlags::DEFAULT,
    );
    assert!(
        far_pick.is_some(),
        "mapped catcher had no GTK pick target at its far corner"
    );
    // Seed the normal GTK focus before exercising the same production
    // return scheduler below; Broadway cannot activate a layer surface.
    island.search_entry.set_can_focus(true);
    gtk::prelude::RootExt::set_focus(&island.window, Some(&island.search_entry));
    island.search_entry.grab_focus();
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    assert_eq!(island.current_view.get(), View::Search);
    island.ensure_integrated_search_host();
    island.ensure_integrated_search_host();
    assert!(
        island
            .search
            .parent()
            .is_some_and(|parent| { parent == island.content.clone().upcast::<gtk::Widget>() })
    );
    let focus_window = island
        .focus_root
        .borrow()
        .clone()
        .downcast::<gtk::Window>()
        .expect("test focus window");
    let temporary_focus = gtk::Button::with_label("temporary focus");
    island.content.put(&temporary_focus, 0.0, 0.0);
    temporary_focus.set_can_focus(true);
    temporary_focus.set_can_target(true);
    focus_window.present();
    temporary_focus.grab_focus();
    gtk::prelude::RootExt::set_focus(&focus_window, Some(&temporary_focus));

    island.osd_active.set(true);
    island.reconcile_view();
    assert_eq!(island.current_view.get(), View::Osd);
    let osd_loop = gtk::glib::MainLoop::new(None, false);
    let osd_weak = Rc::downgrade(&island);
    let osd_loop_for_timeout = osd_loop.clone();
    let osd_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        if let Some(island) = osd_weak.upgrade() {
            if !island.view_transition_active.get() {
                osd_loop_for_timeout.quit();
                return gtk::glib::ControlFlow::Break;
            }
        } else {
            osd_loop_for_timeout.quit();
            return gtk::glib::ControlFlow::Break;
        }
        if std::time::Instant::now() >= osd_deadline {
            osd_loop_for_timeout.quit();
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
    osd_loop.run();
    assert!(!island.view_transition_active.get());
    assert!(island.search_focus_pending.get());
    island.osd_active.set(false);
    island.reconcile_view();
    drain_search(&island);
    assert_eq!(island.current_view.get(), View::Search);
    // finish_view consumed the pending ownership handoff and queued the
    // guarded production scheduler.
    assert!(!island.search_focus_pending.get());
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    assert!(!island.search_focus_pending.get());
    assert!(island.search_entry.has_focus());

    island.schedule_search_entry_focus();
    let queued_generation = island.search_focus_generation.get();
    // Keep the setup above deterministic, then exercise the real
    // production close/reconcile path with the configured motion profile.
    // The nested loop below is intentional: GTK tick callbacks require a
    // running mapped frame clock, not merely pending idle iterations.
    let drain_terminal = |island: &Rc<IslandWindow>| {
        let frame_loop = gtk::glib::MainLoop::new(None, false);
        let weak = Rc::downgrade(island);
        let loop_for_timeout = frame_loop.clone();
        let completed = Rc::new(std::cell::Cell::new(false));
        let completed_in_callback = completed.clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            if let Some(island) = weak.upgrade() {
                if !island.view_transition_active.get() && island.compact.opacity() == 1.0 {
                    completed_in_callback.set(true);
                    loop_for_timeout.quit();
                    return gtk::glib::ControlFlow::Break;
                }
            } else {
                loop_for_timeout.quit();
                return gtk::glib::ControlFlow::Break;
            }
            if std::time::Instant::now() >= deadline {
                loop_for_timeout.quit();
                gtk::glib::ControlFlow::Break
            } else {
                gtk::glib::ControlFlow::Continue
            }
        });
        frame_loop.run();
        assert!(
            completed.get(),
            "animated view transition did not settle before deadline"
        );
    };
    island.close();
    assert!(island.search_focus_generation.get() > queued_generation);
    island.set_pointer_in_hover_region(true);
    drain_terminal(&island);
    assert!(!island.view_transition_active.get());
    assert!(island.compact.is_visible());
    assert!(island.compact.can_target());
    assert_eq!(island.compact.opacity(), 1.0);
    assert!(!island.search.is_visible());
    assert_eq!(island.search.opacity(), 0.0);

    // Supersede an animated open immediately with close, then leave the
    // pointer stationary while the replacement transition settles.
    island.open_search();
    island.close();
    island.set_pointer_in_hover_region(true);
    drain_terminal(&island);
    assert!(!island.view_transition_active.get());
    assert!(island.compact.is_visible());
    assert!(island.compact.can_target());
    assert_eq!(island.compact.opacity(), 1.0);
    assert!(!island.search.is_visible());
    assert_eq!(island.search.opacity(), 0.0);

    // Weather uses the same page transition owner; verify a subsequent
    // page change cannot leave the compact terminal state stale either.
    island.open_weather();
    island.close();
    island.set_pointer_in_hover_region(true);
    drain_terminal(&island);
    assert!(!island.view_transition_active.get());
    assert!(island.compact.is_visible());
    assert!(island.compact.can_target());
    assert_eq!(island.compact.opacity(), 1.0);
    assert!(!island.weather.is_visible());
    assert_eq!(island.weather.opacity(), 0.0);

    // Independent mode exercises search.rs::dismiss_search_window as
    // well as the shared page transition, including rapid close from the
    // mapped search surface.
    let mut independent_config = config.clone();
    independent_config.launcher.presentation = LauncherPresentation::Independent;
    independent_config.shell.animation_ms = 280;
    let independent = IslandWindow::new_for_test(
        &application,
        &monitor,
        "broadway-independent-test".to_owned(),
        &independent_config,
        independent_actions,
        true,
    );
    independent.open_search();
    independent.close();
    independent.set_pointer_in_hover_region(true);
    let independent_loop = gtk::glib::MainLoop::new(None, false);
    let independent_weak = Rc::downgrade(&independent);
    let independent_loop_for_timeout = independent_loop.clone();
    let independent_completed = Rc::new(std::cell::Cell::new(false));
    let independent_completed_in_callback = independent_completed.clone();
    let independent_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        if let Some(island) = independent_weak.upgrade() {
            if !island.view_transition_active.get() && !island.search_window.is_visible() {
                independent_completed_in_callback.set(true);
                independent_loop_for_timeout.quit();
                return gtk::glib::ControlFlow::Break;
            }
        } else {
            independent_loop_for_timeout.quit();
            return gtk::glib::ControlFlow::Break;
        }
        if std::time::Instant::now() >= independent_deadline {
            independent_loop_for_timeout.quit();
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
    independent_loop.run();
    assert!(
        independent_completed.get(),
        "independent search transition did not settle before deadline"
    );
    assert!(!independent.view_transition_active.get());
    assert!(independent.compact.is_visible());
    assert!(independent.compact.can_target());
    assert_eq!(independent.compact.opacity(), 1.0);
    assert!(!independent.search_window.is_visible());
    assert!(!independent.search.is_visible());
    independent.destroy();

    // Make any direct refocus observable without relying on compositor
    // focus activation: closing the launcher removes the entry's focus
    // eligibility before the stale idle callback is drained.
    island.search_entry.set_can_focus(false);
    let button = gtk::Button::with_label("deliberate focus");
    // The legacy page canvas is hidden while the persistent header is
    // active. Focus must belong to a mapped sibling, not that hidden page.
    island.fixed.put(&button, 0.0, 0.0);
    button.set_can_focus(true);
    button.set_can_target(true);
    focus_window.present();
    button.grab_focus();
    gtk::prelude::RootExt::set_focus(&focus_window, Some(&button));
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    assert!(button.has_focus());
    island.destroy();
}

#[test]
#[ignore = "requires an isolated GTK display; run scripts/run-ui-regressions-gtk.py"]
fn circle_integration_real_widgets_and_callbacks() {
    gtk::init().expect("GTK display");
    let application = gtk::Application::new(
        Some("org.mithshell.circle-integration-test"),
        gtk::gio::ApplicationFlags::NON_UNIQUE,
    );
    application.connect_activate(|_| {});
    application
        .register(None::<&gtk::gio::Cancellable>)
        .expect("register GTK application");
    let display = gtk::gdk::Display::default().expect("Broadway display");
    let monitor = display
        .monitors()
        .item(0)
        .and_downcast::<gtk::gdk::Monitor>()
        .expect("Broadway monitor");
    fn motion_controller(widget: &gtk::Widget) -> gtk::EventControllerMotion {
        let controllers = widget.observe_controllers();
        for index in 0..controllers.n_items() {
            let controller = controllers.item(index).expect("controller");
            if let Ok(motion) = controller.downcast::<gtk::EventControllerMotion>() {
                return motion;
            }
        }
        panic!("production motion controller missing");
    }
    let calls = Rc::new(std::cell::Cell::new(0));
    let services = Rc::new(RefCell::new(Vec::<String>::new()));
    let mut actions = IslandActions {
        switch_workspace: Rc::new(|_, _| {}),
        set_volume: Rc::new(|_| {}),
        toggle_mute: Rc::new(|| {}),
        search: Rc::new(|_| {}),
        select: Rc::new(|_| {}),
        tarragon_status: Rc::new(|| {}),
        tarragon_reload: Rc::new(|| {}),
        load_preview: Rc::new(|_, _| {}),
        media_play_pause: Rc::new(|_| {}),
        media_next: Rc::new(|_| {}),
        media_previous: Rc::new(|_| {}),
        notification_expired: Rc::new(|_, _| {}),
        notification_dismiss: Rc::new(|_| {}),
        notification_invoke: Rc::new(|_, _| {}),
        notification_clear_all: Rc::new(|| {}),
        notification_inhibit: Rc::new(|_| {}),
        tray_activate: Rc::new(|_, _, _, _| {}),
        tray_secondary_activate: Rc::new(|_, _, _, _| {}),
        tray_context_menu: Rc::new(|_, _, _, _| {}),
        tray_scroll: Rc::new(|_, _, _, _| {}),
        tray_menu_event: Rc::new(|_, _, _| {}),
    };
    let callback_count = calls.clone();
    let callback_services = services.clone();
    actions.media_play_pause = Rc::new(move |service| {
        callback_count.set(callback_count.get() + 1);
        callback_services.borrow_mut().push(service);
    });
    let matrix_actions = actions.clone();
    let mut config = AppConfig::default();
    config.shell.animation_ms = 0;
    config.shell.scale = 1.0;
    config.circles.left = CircleModule::Media;
    config.circles.right = CircleModule::Notifications;
    let island = IslandWindow::new_for_test(
        &application,
        &monitor,
        "broadway-test".into(),
        &config,
        actions,
        false,
    );
    application.activate();
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }

    let media = MediaState {
        player: "Test Player".into(),
        service: "org.test.Player".into(),
        title: "Track".into(),
        artist: Some("Artist".into()),
        album: None,
        app_icon: Some("audio-x-generic".into()),
        art_url: None,
        position_us: 25,
        length_us: Some(100),
        can_play: true,
        can_pause: true,
        can_go_next: true,
        can_go_previous: true,
        status: PlaybackStatus::Playing,
        players: vec![
            MediaPlayer {
                player: "Test Player".into(),
                service: "org.test.Player".into(),
                title: "Track".into(),
                artist: Some("Artist".into()),
                album: None,
                app_icon: Some("audio-x-generic".into()),
                art_url: None,
                position_us: 25,
                length_us: Some(100),
                can_play: true,
                can_pause: true,
                can_go_next: true,
                can_go_previous: true,
                status: PlaybackStatus::Playing,
            },
            MediaPlayer {
                player: "Second Player".into(),
                service: "org.test.Second".into(),
                title: "Second Track".into(),
                artist: None,
                album: None,
                app_icon: Some("audio-x-generic".into()),
                art_url: None,
                position_us: 10,
                length_us: Some(200),
                can_play: true,
                can_pause: true,
                can_go_next: true,
                can_go_previous: true,
                status: PlaybackStatus::Paused,
            },
        ],
    };
    let tray = TrayItem {
        key: "test/item".into(),
        service: "org.test".into(),
        object_path: "/StatusNotifierItem".into(),
        id: "item".into(),
        title: "Item".into(),
        tooltip: Some("Item".into()),
        icon: TrayIcon::Name("application-x-executable".into()),
        status: TrayStatus::Active,
        item_is_menu: false,
        menu_path: None,
    };
    let notification = Notification {
        id: 1,
        received_at_unix_seconds: 0,
        app_name: "Test".into(),
        app_icon: None,
        summary: "Hello".into(),
        body: "Body".into(),
        urgency: crate::state::Urgency::Normal,
        actions: vec![NotificationAction {
            key: "default".into(),
            label: "Open".into(),
        }],
        timeout: NotificationTimeout::Never,
    };
    island.update_media(Some(&media));
    assert!(island.compact_visualizer_revealer.reveals_child());
    assert_eq!(island.current_view.get(), View::Compact);
    let playing_width = island.compact_width.get();
    let drain_visualizer = || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(550);
        while std::time::Instant::now() < deadline {
            while gtk::glib::MainContext::default().iteration(false) {}
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let mut paused = media.clone();
    paused.status = PlaybackStatus::Paused;
    paused.players[0].status = PlaybackStatus::Paused;
    island.update_media(Some(&paused));
    assert!(
        island.compact_visualizer_revealer.reveals_child(),
        "pause has a grace period"
    );
    island.update_media(Some(&media));
    drain_visualizer();
    assert!(
        island.compact_visualizer_revealer.reveals_child(),
        "resume cancels stale collapse"
    );
    island.update_visualizer([75; crate::media::VISUALIZER_BARS]);
    assert_eq!(
        *island.media_levels.borrow(),
        [75; crate::media::VISUALIZER_BARS]
    );
    let controllers = island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_host(0)
        .unwrap()
        .widget()
        .observe_controllers();
    let media_scroll = (0..controllers.n_items())
        .find_map(|index| {
            controllers
                .item(index)
                .and_downcast::<gtk::EventControllerScroll>()
        })
        .expect("production media source scroll controller");
    // No new MPRIS snapshot arrives between wheel events. Browsing the paused
    // player must not replace the playing snapshot that drives the visualizer.
    for (direction, service) in [
        (1.0_f64, "org.test.Second"),
        (1.0_f64, "org.test.Player"),
        (-1.0_f64, "org.test.Second"),
    ] {
        assert!(media_scroll.emit_by_name::<bool>("scroll", &[&0.0_f64, &direction]));
        assert_eq!(
            island
                .circles
                .borrow()
                .as_ref()
                .unwrap()
                .test_media_service()
                .as_deref(),
            Some(service),
        );
        drain_visualizer();
        assert!(
            island.compact_visualizer_revealer.reveals_child(),
            "source scrolling must retain the visualizer while another player is playing"
        );
        assert!(island.compact_visualizer.is_mapped());
        assert_eq!(island.current_view.get(), View::Compact);
    }
    assert_eq!(calls.get(), 0, "browsing sources must not change playback");
    island.update_media(Some(&paused));
    // Scrolling during a real pause must not cancel its pending hide either.
    assert!(media_scroll.emit_by_name::<bool>("scroll", &[&0.0_f64, &1.0_f64]));
    drain_visualizer();
    assert!(!island.compact_visualizer_revealer.reveals_child());
    assert!(island.compact_width.get() <= playing_width);
    assert_eq!(
        island
            .compact_visualizer_revealer
            .measure(gtk::Orientation::Horizontal, -1)
            .1,
        0
    );
    island.update_media(Some(&media));
    island.update_tray(std::slice::from_ref(&tray));
    island.update_notification_history(std::slice::from_ref(&notification));
    island.update_notification_inhibition(true, Some(std::time::Duration::from_secs(65)));
    island.relayout_circles();
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    island
        .circles
        .borrow()
        .as_ref()
        .expect("circle integration")
        .test_click_media_play_pause();
    assert_eq!(calls.get(), 1, "real media button callback route");
    let state = island.debug_state();
    assert_eq!(state["scale"], 1.0);
    assert_eq!(state["circles"]["slots"][0]["module"], "media");
    assert_eq!(state["circles"]["slots"][1]["module"], "notifications");
    assert_eq!(
        state["circles"]["slots"][0]["present"].as_bool(),
        Some(true)
    );
    assert_eq!(
        state["circles"]["slots"][1]["present"].as_bool(),
        Some(true)
    );
    // Actual CircleHost state transitions exercise target/presented page,
    // synchronous no-animation commits, invalidation, and disappearance.
    let media_host = {
        let circles = island.circles.borrow();
        circles
            .as_ref()
            .expect("circle integration")
            .test_host(0)
            .expect("media slot")
    };
    assert_eq!(media_host.mode(), super::circle::Mode::Compact);
    assert_eq!(
        media_host.presented_page(),
        Some(super::circle::Mode::Compact)
    );
    let revision = media_host.revision();
    media_host.dispatch(super::circle::Event::Pointer(true));
    assert_ne!(media_host.mode(), super::circle::Mode::Absent);
    assert!(!media_host.commit_page(revision));
    media_host.dispatch(super::circle::Event::Content(false));
    assert_eq!(media_host.mode(), super::circle::Mode::Absent);
    assert!(media_host.frame().is_none());
    island.relayout_circles();
    assert_eq!(
        island.debug_state()["circles"]["slots"][0]["present"].as_bool(),
        Some(false)
    );

    // Legacy content is suppressed by assignment; moving assignment back
    // to no circles is covered by constructing all three valid matrices.
    for (left, right, visualizer) in [
        (CircleModule::None, CircleModule::None, true),
        (CircleModule::Tray, CircleModule::None, true),
        (CircleModule::None, CircleModule::Media, true),
        (CircleModule::None, CircleModule::Media, false),
        (CircleModule::None, CircleModule::None, false),
    ] {
        let mut matrix = AppConfig::default();
        matrix.shell.animation_ms = 0;
        matrix.shell.scale = 1.0;
        matrix.circles.left = left;
        matrix.circles.right = right;
        matrix.media.visualizer = visualizer;
        let test = IslandWindow::new_for_test(
            &application,
            &monitor,
            "broadway-test".into(),
            &matrix,
            matrix_actions.clone(),
            false,
        );
        test.update_tray(std::slice::from_ref(&tray));
        test.update_media(Some(&media));
        let legacy_media = left != CircleModule::Media && right != CircleModule::Media;
        assert_eq!(
            test.media_title.label().as_str(),
            if legacy_media { "Track" } else { "" },
            "legacy media title should be populated only without a media circle"
        );
        assert_eq!(test.media.is_visible(), legacy_media);
        assert_eq!(test.media_visualizer.get_visible(), visualizer);
        assert_eq!(
            test.compact_visualizer_revealer.reveals_child(),
            visualizer && right == CircleModule::Media
        );
        test.set_tray_hovered(true);
        test.relayout_circles();
        assert!(test.debug_state()["circles"]["slots"].is_array());
        if left == CircleModule::Tray {
            assert_eq!(test.debug_state()["tray_visible"], false);
            assert_eq!(test.debug_state()["tray_visible_media"], false);
        }
    }
    assert_eq!(calls.get(), 1, "state updates do not duplicate callbacks");

    // Real host page transitions use the production GTK motion controller
    // and actual GTK stack page, not a mirrored state helper.
    island.update_media(Some(&media));
    let media_host = island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_host(0)
        .unwrap();
    let media_motion = motion_controller(media_host.widget());
    let _: () = media_motion.emit_by_name("enter", &[&0.0_f64, &0.0_f64]);
    island.relayout_circles();
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    assert_eq!(
        media_host.presented_page(),
        Some(super::circle::Mode::HoverExpanded)
    );
    assert_eq!(media_host.test_visible_page().as_deref(), Some("hover"));
    let media_frame = media_host.frame().expect("rendered media frame");
    assert!(media_frame.radius > 0.0);
    assert!(!media_frame.contains(media_frame.rect.x - 1.0, media_frame.rect.y - 1.0));
    let play_pause = island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_media_play_pause_button()
        .expect("production media play/pause button");
    assert!(play_pause.is_mapped());
    assert!(play_pause.grab_focus());
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    let focus_root = play_pause.root().expect("mapped media focus root");
    assert_eq!(
        focus_root.focus().map(|focused| focused == play_pause),
        Some(true)
    );
    let services_before_click = services.borrow().len();
    play_pause.emit_clicked();
    assert_eq!(calls.get(), 2, "real media button callback route");
    assert_eq!(services.borrow().len(), services_before_click + 1);
    assert_eq!(
        services.borrow().last().map(String::as_str),
        Some("org.test.Player")
    );
    let _: () = media_motion.emit_by_name("leave", &[]);
    island.relayout_circles();
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    assert_eq!(media_host.mode(), super::circle::Mode::Compact);
    assert_eq!(
        media_host.presented_page(),
        Some(super::circle::Mode::Compact)
    );
    assert_eq!(
        focus_root.focus().map(|focused| focused == play_pause),
        Some(true)
    );

    island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_media_select_service("org.test.Second");
    assert_eq!(
        island
            .circles
            .borrow()
            .as_ref()
            .unwrap()
            .test_media_service()
            .as_deref(),
        Some("org.test.Second")
    );
    assert_eq!(island.debug_state()["notification_history_visible"], false);
    assert_eq!(
        island
            .circles
            .borrow()
            .as_ref()
            .unwrap()
            .test_notification_inhibition(),
        Some((true, "2m".to_owned(), true))
    );

    island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_notification_compact_click();
    let notification_host = island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_host(1)
        .unwrap();
    let focus_root = notification_host
        .widget()
        .root()
        .expect("mapped GTK focus root");
    let focus_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while focus_root
        .focus()
        .is_none_or(|focused| focused != *notification_host.widget())
        && std::time::Instant::now() < focus_deadline
    {
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        focus_root
            .focus()
            .map(|focused| focused == *notification_host.widget()),
        Some(true),
        "deferred production full-page focus must own the mapped GTK root"
    );
    assert!(island.dismiss_window.is_visible());
    assert!(island.window.is_visible());
    assert_eq!(notification_host.mode(), super::circle::Mode::FullExpanded);
    assert_eq!(
        notification_host.presented_page(),
        Some(super::circle::Mode::FullExpanded)
    );
    assert_eq!(
        notification_host.test_visible_page().as_deref(),
        Some("full")
    );
    assert_eq!(
        notification_host.test_escape_key(),
        gtk::glib::Propagation::Stop
    );
    assert_eq!(notification_host.mode(), super::circle::Mode::Compact);
    assert_eq!(
        notification_host.presented_page(),
        Some(super::circle::Mode::FullExpanded)
    );
    assert!(island.circle_full_active());
    assert!(island.dismiss_window.is_visible());
    let interruption = gtk::Button::with_label("focus interruption");
    interruption.set_can_focus(true);
    interruption.set_can_target(true);
    island.fixed.put(&interruption, 0.0, 0.0);
    interruption.grab_focus();
    focus_root.set_focus(Some(&interruption));
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    assert_eq!(
        focus_root
            .focus()
            .map(|focused| focused == *interruption.upcast_ref::<gtk::Widget>()),
        Some(true)
    );
    island.relayout_circles();
    assert_eq!(notification_host.mode(), super::circle::Mode::Compact);
    let dismiss_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while island.circle_full_active() && std::time::Instant::now() < dismiss_deadline {
        island.relayout_circles();
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    assert!(!island.circle_full_active());
    assert_eq!(
        notification_host.presented_page(),
        Some(super::circle::Mode::Compact)
    );

    island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_notification_compact_click();
    let reopen_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while !island.circle_full_active() && std::time::Instant::now() < reopen_deadline {
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    while gtk::glib::MainContext::default().pending() {
        gtk::glib::MainContext::default().iteration(false);
    }
    assert!(island.circle_full_active());
    assert!(island.dismiss_window.is_visible());
    island.test_emit_dismiss_click();
    assert_eq!(notification_host.mode(), super::circle::Mode::Compact);
    let catcher_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while island.dismiss_window.is_visible() && std::time::Instant::now() < catcher_deadline {
        island.relayout_circles();
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    assert!(!island.dismiss_window.is_visible());

    // Exercise a nonzero full-page collapse through the production clock.
    // The outgoing full page and catcher must remain present until the
    // circle animation commits its incoming page.
    island.animation_ms.set(100);
    island.animations_enabled.set(true);
    island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_notification_compact_click();
    let full_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while notification_host.presented_page() != Some(super::circle::Mode::FullExpanded)
        && std::time::Instant::now() < full_deadline
    {
        island.relayout_circles();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        notification_host.presented_page(),
        Some(super::circle::Mode::FullExpanded)
    );
    assert!(island.dismiss_window.is_visible());
    island.test_emit_dismiss_click();
    island.relayout_circles();
    assert_eq!(
        notification_host.presented_page(),
        Some(super::circle::Mode::FullExpanded),
        "outgoing full page remains committed during collapse"
    );
    assert!(island.dismiss_window.is_visible());
    let full_out_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while island.circle_full_active() && std::time::Instant::now() < full_out_deadline {
        island.relayout_circles();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!island.circle_full_active());
    assert_eq!(
        notification_host.presented_page(),
        Some(super::circle::Mode::Compact)
    );
    assert!(!island.dismiss_window.is_visible());

    // Independent search is another owner of the catcher.  Dismissing a
    // full circle must preserve it while the independent search remains
    // mapped, then the second real outside gesture closes search and the
    // final search animation removes the catcher.
    let mut independent_config = config.clone();
    independent_config.launcher.presentation = LauncherPresentation::Independent;
    independent_config.shell.animation_ms = 100;
    independent_config.circles.left = CircleModule::Notifications;
    independent_config.circles.right = CircleModule::None;
    let independent = IslandWindow::new_for_test(
        &application,
        &monitor,
        "broadway-independent-test".into(),
        &independent_config,
        matrix_actions.clone(),
        true,
    );
    independent.update_notification_history(&[notification]);
    independent.relayout_circles();
    let independent_host = independent
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_host(0)
        .unwrap();
    independent
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_notification_compact_click();
    let independent_full_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while independent_host.presented_page() != Some(super::circle::Mode::FullExpanded)
        && std::time::Instant::now() < independent_full_deadline
    {
        independent.relayout_circles();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        independent_host.presented_page(),
        Some(super::circle::Mode::FullExpanded)
    );
    independent.open_search();
    assert!(independent.search_window.is_visible());
    assert!(independent.dismiss_window.is_visible());
    independent.test_emit_dismiss_click();
    assert!(independent.search_window.is_visible());
    assert!(independent.dismiss_window.is_visible());
    let independent_circle_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while independent.circle_full_active()
        && std::time::Instant::now() < independent_circle_deadline
    {
        independent.relayout_circles();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(!independent.circle_full_active());
    assert!(independent.search_window.is_visible());
    assert!(independent.dismiss_window.is_visible());
    // Keep the circle's real nonzero collapse above; use the production
    // immediate search-close branch so this assertion is about the final
    // catcher-needed reconciliation, not a second animation clock.
    independent.animation_ms.set(0);
    independent.test_emit_dismiss_click();
    assert!(!independent.search_window.is_visible());
    assert!(!independent.dismiss_window.is_visible());

    let _styles = crate::ui::install_styles(&crate::theme::generate_gtk());
    let pump = |duration| {
        let main_loop = gtk::glib::MainLoop::new(None, false);
        let quit = main_loop.clone();
        gtk::glib::timeout_add_local_once(duration, move || quit.quit());
        main_loop.run();
    };
    island.animations_enabled.set(false);
    island.update_media(Some(&media));
    media_host.dispatch(super::circle::Event::Pointer(false));
    island.relayout_circles();
    pump(std::time::Duration::from_millis(60));
    let artwork = island
        .circles
        .borrow()
        .as_ref()
        .unwrap()
        .test_media_artwork()
        .unwrap();
    let art_parent = artwork.parent();
    let compact_art = artwork.compute_bounds(media_host.widget()).unwrap();
    let compact_frame = media_host.frame().unwrap();

    // The very first expanded frame must retain the compact artwork position.
    // Subsequent frames move the same widget with the geometry, even across
    // MPRIS refreshes; a new page must never jump to its final inset.
    island
        .motion
        .set(Some(crate::config::MotionConfig::default()));
    island.animations_enabled.set(true);
    media_host.dispatch(super::circle::Event::Pointer(true));
    island.relayout_circles();
    assert_eq!(media_host.test_visible_page().as_deref(), Some("hover"));
    assert_eq!(
        artwork.compute_bounds(media_host.widget()),
        Some(compact_art)
    );
    let mut saw_intermediate = false;
    let mut previous_x = compact_art.x();
    for _ in 0..12 {
        pump(std::time::Duration::from_millis(40));
        let bounds = artwork.compute_bounds(media_host.widget()).unwrap();
        let frame = media_host.frame().unwrap();
        let progress = (frame.rect.width - compact_frame.rect.width)
            / (super::media_circle::EXPANDED_SIZE.width - compact_frame.rect.width);
        assert!((bounds.x() - compact_art.x() - 12.0 * progress as f32).abs() <= 1.0);
        assert!((bounds.y() - compact_art.y() - 12.0 * progress as f32).abs() <= 1.0);
        assert!(
            (bounds.width() - compact_art.width() * (1.0 + 2.0 * progress as f32)).abs() <= 1.0
        );
        assert!(bounds.x() >= previous_x);
        saw_intermediate |= bounds.x() > compact_art.x() && bounds.x() < compact_art.x() + 12.0;
        previous_x = bounds.x();
        assert_eq!(artwork.parent(), art_parent);
        assert!(artwork.is_mapped());
        assert_eq!(media_host.test_opacity(), 1.0);
        island.update_media(Some(&media));
    }
    assert!(saw_intermediate, "artwork must move across multiple frames");
    assert_eq!(
        media_host.frame().unwrap().rect.width,
        super::media_circle::EXPANDED_SIZE.width
    );
    let expanded_art = artwork.compute_bounds(media_host.widget()).unwrap();

    media_host.dispatch(super::circle::Event::Pointer(false));
    island.relayout_circles();
    assert_eq!(
        artwork.compute_bounds(media_host.widget()),
        Some(expanded_art)
    );
    pump(std::time::Duration::from_millis(220));
    assert!(
        media_host.frame().unwrap().rect.width > compact_frame.rect.width,
        "default collapse must last longer than 200ms"
    );
    assert_eq!(media_host.test_visible_page().as_deref(), Some("hover"));
    assert_eq!(media_host.test_opacity(), 1.0);
    let reversing_frame = media_host.frame();
    let reversing_art = artwork.compute_bounds(media_host.widget());
    media_host.dispatch(super::circle::Event::Pointer(true));
    island.relayout_circles();
    assert_eq!(media_host.frame(), reversing_frame);
    assert_eq!(artwork.compute_bounds(media_host.widget()), reversing_art);
    pump(std::time::Duration::from_millis(450));
    assert_eq!(
        artwork.compute_bounds(media_host.widget()),
        Some(expanded_art)
    );
    media_host.dispatch(super::circle::Event::Pointer(false));
    island.relayout_circles();
    for _ in 0..11 {
        pump(std::time::Duration::from_millis(40));
        assert_eq!(media_host.test_opacity(), 1.0);
        assert_eq!(artwork.parent(), art_parent);
    }
    assert_eq!(media_host.test_visible_page().as_deref(), Some("compact"));
    assert_eq!(
        artwork.compute_bounds(media_host.widget()),
        Some(compact_art)
    );
    island.animations_enabled.set(false);
    media_host.dispatch(super::circle::Event::Pointer(true));
    island.relayout_circles();
    assert_eq!(
        artwork.compute_bounds(media_host.widget()),
        Some(expanded_art)
    );
    media_host.dispatch(super::circle::Event::Pointer(false));
    island.relayout_circles();
    assert_eq!(
        artwork.compute_bounds(media_host.widget()),
        Some(compact_art)
    );

    // Sample the mapped tray on the production frame clock, including updates
    // faster than its transition and reversals before either endpoint.
    let items: Vec<_> = (0..6)
        .map(|index| TrayItem {
            key: format!("test/item-{index}"),
            ..tray.clone()
        })
        .collect();
    for (scale, side) in [(1.0, 0), (1.9, 1)] {
        let mut config = AppConfig::default();
        config.shell.scale = scale;
        config.shell.animation_ms = 0;
        config.circles.left = if side == 0 {
            CircleModule::Tray
        } else {
            CircleModule::None
        };
        config.circles.right = if side == 1 {
            CircleModule::Tray
        } else {
            CircleModule::None
        };
        let fixture = IslandWindow::new_for_test(
            &application,
            &monitor,
            "tray-motion".into(),
            &config,
            matrix_actions.clone(),
            false,
        );
        fixture.update_tray(&items);
        fixture.relayout_circles();
        pump(std::time::Duration::from_millis(50));
        let host = fixture
            .circles
            .borrow()
            .as_ref()
            .unwrap()
            .test_host(side)
            .unwrap();
        let compact = host.frame().unwrap();
        host.dispatch(super::circle::Event::Pointer(true));
        fixture.relayout_circles();
        let expanded = host.frame().unwrap();
        assert!(expanded.rect.width > compact.rect.width);
        host.dispatch(super::circle::Event::Pointer(false));
        fixture.relayout_circles();
        pump(std::time::Duration::from_millis(50));
        fixture
            .motion
            .set(Some(crate::config::MotionConfig::default()));
        fixture.animations_enabled.set(true);

        host.dispatch(super::circle::Event::Pointer(true));
        fixture.relayout_circles();
        let mut previous = compact;
        let mut saw_intermediate = false;
        for _ in 0..14 {
            pump(std::time::Duration::from_millis(40));
            let frame = host
                .frame()
                .expect("tray stays mapped throughout expansion");
            assert!(host.widget().is_mapped());
            assert_eq!(host.test_opacity(), 1.0, "tray backdrop must never blink");
            assert!(frame.rect.width >= previous.rect.width);
            saw_intermediate |=
                frame.rect.width > compact.rect.width && frame.rect.width < expanded.rect.width;
            previous = frame;
            fixture.update_tray(&items);
        }
        assert!(
            saw_intermediate,
            "tray must expand over multiple painted frames"
        );
        assert_eq!(
            host.frame(),
            Some(expanded),
            "snapshots must not restart the clock"
        );

        host.dispatch(super::circle::Event::Pointer(false));
        fixture.relayout_circles();
        assert_eq!(
            host.frame(),
            Some(expanded),
            "reversal starts at the painted frame"
        );
        pump(std::time::Duration::from_millis(80));
        let reversing = host.frame().unwrap();
        assert!(reversing.rect.width < expanded.rect.width);
        assert!(reversing.rect.width > compact.rect.width);
        assert_eq!(host.test_opacity(), 1.0);
        host.dispatch(super::circle::Event::Pointer(true));
        fixture.relayout_circles();
        assert_eq!(host.frame(), Some(reversing));
        pump(std::time::Duration::from_millis(450));
        assert_eq!(host.frame(), Some(expanded));
        host.dispatch(super::circle::Event::Pointer(false));
        fixture.relayout_circles();
        for _ in 0..8 {
            pump(std::time::Duration::from_millis(40));
            assert_eq!(
                host.test_opacity(),
                1.0,
                "tray backdrop must stay visible on exit"
            );
        }
        assert_eq!(host.frame(), Some(compact));

        fixture.animations_enabled.set(false);
        host.dispatch(super::circle::Event::Pointer(true));
        fixture.relayout_circles();
        assert_eq!(host.frame(), Some(expanded));
        assert_eq!(host.test_visible_page().as_deref(), Some("hover"));
        fixture.update_tray(&[]);
        assert!(host.frame().is_none());
        assert!(!host.widget().is_visible());
        fixture.destroy();
    }
}

#[test]
#[ignore = "requires an isolated GTK display; run scripts/run-ui-regressions-gtk.py"]
fn real_circle_allocations_and_gtk_picking_survive_scale_and_rebuilds() {
    gtk::init().expect("GTK display");
    let application = gtk::Application::new(
        Some("org.mithshell.real-pick-regression"),
        gtk::gio::ApplicationFlags::NON_UNIQUE,
    );
    application.connect_activate(|_| {});
    application
        .register(None::<&gtk::gio::Cancellable>)
        .expect("register GTK application");
    let display = gtk::gdk::Display::default().expect("Broadway display");
    let monitor = display
        .monitors()
        .item(0)
        .and_downcast::<gtk::gdk::Monitor>()
        .expect("Broadway monitor");

    fn drain() {
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
    }
    fn point_in(root: &gtk::Widget, widget: &gtk::Widget) -> gtk::graphene::Point {
        #[allow(deprecated)]
        let allocation = widget.allocation();
        let mut x = allocation.width() as f32 / 2.0;
        let mut y = allocation.height() as f32 / 2.0;
        let mut current = widget.clone();
        while current != *root {
            #[allow(deprecated)]
            let offset = current.allocation();
            x += offset.x() as f32;
            y += offset.y() as f32;
            current = current.parent().expect("widget attached to root");
        }
        gtk::graphene::Point::new(x, y)
    }
    fn ancestry_has(widget: &gtk::Widget, class: &str) -> bool {
        let mut current = Some(widget.clone());
        while let Some(candidate) = current {
            if candidate.has_css_class(class) {
                return true;
            }
            current = candidate.parent();
        }
        false
    }
    fn motion_controller(widget: &gtk::Widget) -> gtk::EventControllerMotion {
        let controllers = widget.observe_controllers();
        for index in 0..controllers.n_items() {
            let controller = controllers.item(index).expect("controller");
            if let Ok(motion) = controller.downcast::<gtk::EventControllerMotion>() {
                return motion;
            }
        }
        panic!("production motion controller missing");
    }
    fn first_descendant<W: gtk::prelude::IsA<gtk::Widget> + Clone + 'static>(
        root: &gtk::Widget,
    ) -> Option<W> {
        let mut child = root.first_child();
        while let Some(candidate) = child {
            child = candidate.next_sibling();
            if let Ok(found) = candidate.clone().downcast::<W>() {
                return Some(found);
            }
            if let Some(found) = first_descendant::<W>(&candidate) {
                return Some(found);
            }
        }
        None
    }
    fn first_allocated<W: gtk::prelude::IsA<gtk::Widget> + Clone + 'static>(
        root: &gtk::Widget,
    ) -> Option<W> {
        let mut child = root.first_child();
        while let Some(candidate) = child {
            child = candidate.next_sibling();
            if candidate.is_mapped()
                && candidate.width() > 0
                && candidate.height() > 0
                && let Ok(found) = candidate.clone().downcast::<W>()
            {
                return Some(found);
            }
            if let Some(found) = first_allocated::<W>(&candidate) {
                return Some(found);
            }
        }
        None
    }
    fn emit_root_primary_click(root: &gtk::Widget, x: f64, y: f64) {
        let controllers = root.observe_controllers();
        for index in 0..controllers.n_items() {
            let controller = controllers.item(index).expect("controller");
            if let Ok(click) = controller.downcast::<gtk::GestureClick>() {
                click.set_button(1);
                click.emit_by_name::<()>("pressed", &[&1_i32, &x, &y]);
                click.emit_by_name::<()>("released", &[&1_i32, &x, &y]);
                return;
            }
        }
        panic!("root fallback click controller missing");
    }

    for cycle in 0..10 {
        for scale in [0.75, 1.0, 1.4, 1.45, 1.75] {
            let mut config = AppConfig::default();
            config.shell.scale = scale;
            config.shell.animation_ms = 20;
            config.circles.left = CircleModule::Tray;
            config.circles.right = CircleModule::Notifications;
            let actions = IslandActions {
                switch_workspace: Rc::new(|_, _| {}),
                set_volume: Rc::new(|_| {}),
                toggle_mute: Rc::new(|| {}),
                search: Rc::new(|_| {}),
                select: Rc::new(|_| {}),
                tarragon_status: Rc::new(|| {}),
                tarragon_reload: Rc::new(|| {}),
                load_preview: Rc::new(|_, _| {}),
                media_play_pause: Rc::new(|_| {}),
                media_next: Rc::new(|_| {}),
                media_previous: Rc::new(|_| {}),
                notification_expired: Rc::new(|_, _| {}),
                notification_dismiss: Rc::new(|_| {}),
                notification_invoke: Rc::new(|_, _| {}),
                notification_clear_all: Rc::new(|| {}),
                notification_inhibit: Rc::new(|_| {}),
                tray_activate: Rc::new(|_, _, _, _| {}),
                tray_secondary_activate: Rc::new(|_, _, _, _| {}),
                tray_context_menu: Rc::new(|_, _, _, _| {}),
                tray_scroll: Rc::new(|_, _, _, _| {}),
                tray_menu_event: Rc::new(|_, _, _| {}),
            };
            let island = IslandWindow::new_for_test(
                &application,
                &monitor,
                format!("pick-{cycle}-{scale}"),
                &config,
                actions,
                true,
            );
            // This is the production snapshot path that creates the actual
            // workspace buttons and gives the central pill a nonempty target.
            island.update_hyprland(&crate::state::HyprlandSnapshot {
                monitors: vec![crate::state::HyprlandMonitor {
                    id: 0,
                    name: format!("pick-{cycle}-{scale}"),
                    focused: true,
                    active_workspace: crate::state::WorkspaceRef {
                        id: 1,
                        name: "1".into(),
                    },
                    special_workspace: Default::default(),
                    fullscreen: false,
                }],
                workspaces: vec![crate::state::Workspace {
                    id: 1,
                    name: "1".into(),
                    monitor: format!("pick-{cycle}-{scale}"),
                    windows: 1,
                }],
                active_window: None,
            });
            let tray = TrayItem {
                key: "pick/item".into(),
                service: "org.pick".into(),
                object_path: "/StatusNotifierItem".into(),
                id: "pick".into(),
                title: "Pick item".into(),
                tooltip: Some("Pick item".into()),
                icon: TrayIcon::Name("application-x-executable".into()),
                status: TrayStatus::Active,
                item_is_menu: false,
                menu_path: None,
            };
            island.update_tray(std::slice::from_ref(&tray));
            island.update_notification_history(&[]);
            island.relayout_circles();
            application.activate();
            drain();
            island.fixed.queue_allocate();
            drain();

            let expected = (32.0 * scale).ceil() as i32;
            let tray_host = island
                .circles
                .borrow()
                .as_ref()
                .expect("production circle integration")
                .test_host(0)
                .expect("tray host");
            let tray_widget = tray_host.widget();
            assert_eq!(
                tray_widget.width(),
                expected,
                "compact width at scale {scale}"
            );
            assert_eq!(
                tray_widget.height(),
                expected,
                "compact height at scale {scale}"
            );
            assert!(tray_widget.is_mapped());
            assert!(tray_widget.width() > 0 && tray_widget.height() > 0);

            let root = island.fixed.clone().upcast::<gtk::Widget>();
            let root_motion = motion_controller(&root);
            let tray_motion = motion_controller(tray_widget);
            let compact_bounds = island.compact.compute_bounds(&island.fixed).unwrap();
            let center_x = f64::from(compact_bounds.x()) + f64::from(compact_bounds.width()) / 2.0;
            let center_y = f64::from(compact_bounds.y()) + f64::from(compact_bounds.height()) / 2.0;
            let _: () = root_motion.emit_by_name("leave", &[]);
            let _: () = tray_motion.emit_by_name("leave", &[]);
            drain();
            assert!(!island.pointer_in_hover_region.get());
            assert!(!island.tray_hovered.get());
            let _: () = root_motion.emit_by_name("enter", &[&center_x, &center_y]);
            drain();
            assert!(island.pointer_in_hover_region.get());
            assert!(island.tray_hovered.get());
            assert_eq!(tray_host.mode(), super::circle::Mode::Compact);
            assert!(tray_host.frame().is_some_and(|frame| {
                (frame.rect.width - expected as f64).abs() < f64::EPSILON
            }));
            let tray_frame = tray_host.frame().expect("compact tray frame");
            let tray_x = tray_frame.rect.x + tray_frame.rect.width / 2.0;
            let tray_y = tray_frame.rect.y + tray_frame.rect.height / 2.0;
            let _: () = root_motion.emit_by_name("enter", &[&tray_x, &tray_y]);
            drain();
            island.relayout_circles();
            assert_eq!(tray_host.mode(), super::circle::Mode::HoverExpanded);
            assert_eq!(tray_host.mode(), super::circle::Mode::HoverExpanded);
            // Animation remains enabled in this production-flow test.
            // Repeated frame reallocations must not manufacture a leave
            // and collapse a stationary pointer.
            for _ in 0..12 {
                std::thread::sleep(std::time::Duration::from_millis(4));
                drain();
                island.relayout_circles();
                assert_eq!(tray_host.mode(), super::circle::Mode::HoverExpanded);
            }
            assert!(
                tray_host
                    .frame()
                    .is_some_and(|frame| frame.rect.width > expected as f64)
            );
            let _: () = root_motion.emit_by_name("leave", &[]);
            let _: () = tray_motion.emit_by_name("leave", &[]);
            drain();
            assert!(!island.pointer_in_hover_region.get());
            assert!(!island.tray_hovered.get());
            assert_eq!(tray_host.mode(), super::circle::Mode::Compact);
            // The previous hover-to-compact animation can move the circle
            // along its side lane. Pick at the current settled compact
            // position rather than the stale pre-hover pointer location.
            std::thread::sleep(std::time::Duration::from_millis(28));
            drain();
            island.relayout_circles();
            let settled_frame = tray_host.frame().expect("settled compact tray frame");
            let tray_x = settled_frame.rect.x + settled_frame.rect.width / 2.0;
            let tray_y = settled_frame.rect.y + settled_frame.rect.height / 2.0;
            let _: () = root_motion.emit_by_name("enter", &[&0.0_f64, &0.0_f64]);
            drain();
            assert!(!island.pointer_in_hover_region.get());
            assert_eq!(tray_host.mode(), super::circle::Mode::Compact);
            let _: () = root_motion.emit_by_name("enter", &[&center_x, &center_y]);
            drain();
            assert!(island.pointer_in_hover_region.get());
            let _: () = root_motion.emit_by_name("enter", &[&tray_x, &tray_y]);
            drain();
            island.relayout_circles();
            island.fixed.queue_allocate();
            drain();
            let picked_tray = root
                .pick(tray_x, tray_y, gtk::PickFlags::DEFAULT)
                .expect("tray compact pick");
            assert!(
                ancestry_has(&picked_tray, "circle-surface"),
                "tray pick at ({tray_x},{tray_y}) got {} frame={:?} widget bounds={:?}",
                picked_tray.type_().name(),
                tray_host.frame(),
                tray_widget.compute_bounds(&root)
            );
            assert!(!picked_tray.has_css_class("mithshell-hover-region"));

            // Enter/leave are sent through the real mapped host controller;
            // the signal synthesis is GTK-local (not a compositor/GDK event).
            island.relayout_circles();
            island.fixed.queue_allocate();
            std::thread::sleep(std::time::Duration::from_millis(28));
            drain();
            island.relayout_circles();
            drain();
            assert_eq!(tray_host.mode(), super::circle::Mode::HoverExpanded);
            let tray_scroller = first_allocated::<gtk::ScrolledWindow>(tray_widget)
                .expect("production tray scroller");
            assert!(tray_scroller.width() > 0 && tray_scroller.height() > 0);
            let hover_page = first_allocated::<gtk::Box>(&tray_scroller.clone().upcast())
                .expect("production tray row");
            assert!(hover_page.width() > 0 && hover_page.height() > 0);
            let item_button = first_descendant::<gtk::Button>(&hover_page.clone().upcast())
                .expect("production tray item button");
            assert!(item_button.is_mapped());
            assert!(item_button.width() > 0 && item_button.height() > 0);
            let item_point = point_in(&root, &item_button.clone().upcast());
            let picked_item = root
                .pick(
                    f64::from(item_point.x()),
                    f64::from(item_point.y()),
                    gtk::PickFlags::DEFAULT,
                )
                .expect("tray item pick");
            assert!(ancestry_has(&picked_item, "tray-icon"));
            assert!(ancestry_has(&picked_item, "circle-surface"));
            let _: () = root_motion.emit_by_name("leave", &[]);
            let _: () = tray_motion.emit_by_name("leave", &[]);
            island.relayout_circles();
            drain();
            assert_eq!(tray_host.mode(), super::circle::Mode::Compact);

            // Open the existing tray full page through the production
            // host event, then inspect the committed Stack/ScrolledWindow
            // hierarchy rather than allocating the row directly.
            tray_host.dispatch(super::circle::Event::OpenFull);
            island.relayout_circles();
            island.fixed.queue_allocate();
            drain();
            std::thread::sleep(std::time::Duration::from_millis(28));
            drain();
            island.relayout_circles();
            assert_eq!(tray_host.mode(), super::circle::Mode::FullExpanded);
            assert_eq!(
                tray_host.presented_page(),
                Some(super::circle::Mode::FullExpanded)
            );
            assert_eq!(tray_host.test_visible_page().as_deref(), Some("full"));
            let full_scroller = first_allocated::<gtk::ScrolledWindow>(tray_widget)
                .expect("production tray full scroller");
            let full_page = first_allocated::<gtk::Box>(&full_scroller.clone().upcast())
                .expect("production tray full row");
            let full_button = first_descendant::<gtk::Button>(&full_page.clone().upcast())
                .expect("production tray full item button");
            for widget in [
                full_scroller.clone().upcast::<gtk::Widget>(),
                full_page.clone().upcast::<gtk::Widget>(),
                full_button.clone().upcast::<gtk::Widget>(),
            ] {
                assert!(widget.is_mapped());
                assert!(widget.width() > 0 && widget.height() > 0);
                let bounds = widget
                    .compute_bounds(tray_widget)
                    .expect("inside tray host");
                assert!(bounds.x() >= 0.0 && bounds.y() >= 0.0);
                // GTK's theme imposes a 34px minimum on buttons even at
                // sub-1.0 shell scales; the rounded host clips that child
                // to its own shallow frame while keeping its center
                // clickable. At normal scales the whole row must fit.
                if scale >= 1.4 {
                    assert!(bounds.x() + bounds.width() <= tray_widget.width() as f32 + 1.0);
                    assert!(bounds.y() + bounds.height() <= tray_widget.height() as f32 + 1.0);
                }
            }
            assert!(ancestry_has(&full_button.clone().upcast(), "tray-icon"));

            let workspace =
                first_allocated::<gtk::Button>(&island.compact_workspaces.clone().upcast())
                    .expect("workspace button");
            let compact_root = island.compact.clone().upcast::<gtk::Widget>();
            let workspace_point = point_in(&root, &workspace.clone().upcast());
            let workspace_local = point_in(&compact_root, &workspace.clone().upcast());
            let picked_workspace = compact_root
                .pick(
                    f64::from(workspace_local.x()),
                    f64::from(workspace_local.y()),
                    gtk::PickFlags::DEFAULT,
                )
                .expect("workspace pick");
            assert!(ancestry_has(&picked_workspace, "workspace-dot"));
            assert!(ancestry_has(&picked_workspace, "compact-content"));
            // The root fallback must not claim a descendant workspace
            // target merely because it lies inside the central rectangle.
            emit_root_primary_click(
                &root,
                f64::from(workspace_point.x()),
                f64::from(workspace_point.y()),
            );
            drain();
            assert!(!island.dashboard_open.get());

            // Pick the actual central pill background and exercise the
            // production fixed-root fallback controller.
            let compact_point = point_in(&root, &island.compact);
            let picked_compact = root
                .pick(
                    f64::from(compact_point.x()),
                    f64::from(compact_point.y()),
                    gtk::PickFlags::DEFAULT,
                )
                .expect("central pill pick");
            assert!(!picked_compact.has_css_class("mithshell-hover-region"));
            emit_root_primary_click(
                &root,
                island.central_circle_rect().x + island.central_circle_rect().width / 2.0,
                island.central_circle_rect().y + island.central_circle_rect().height / 2.0,
            );
            drain();
            assert!(island.dashboard_open.get());
            assert_eq!(island.current_view.get(), View::Dashboard);
            assert!(island.dashboard_open.get());
        }
    }
}

fn player(service: &str, status: PlaybackStatus) -> MediaPlayer {
    MediaPlayer {
        player: service.to_owned(),
        service: service.to_owned(),
        title: format!("track from {service}"),
        artist: None,
        album: None,
        app_icon: None,
        art_url: None,
        position_us: 0,
        length_us: None,
        can_play: true,
        can_pause: true,
        can_go_next: true,
        can_go_previous: true,
        status,
    }
}

#[test]
fn explicit_scale_is_not_capped() {
    assert_eq!(resolved_scale(2.4, 1.45), 2.4);
}

#[test]
fn labels_the_selected_weather_provider() {
    assert_eq!(
        weather_provider_label("WEATHER", WeatherProvider::Wttr),
        "WEATHER  //  WTTR.IN"
    );
    assert_eq!(
        weather_provider_label("UPDATED", WeatherProvider::OpenMeteo),
        "UPDATED  //  OPEN-METEO.COM"
    );
}

#[test]
fn media_selection_promotes_requested_player_without_changing_status() {
    let playing = player("playing", PlaybackStatus::Playing);
    let paused = player("paused", PlaybackStatus::Paused);
    let state = MediaState {
        player: playing.player.clone(),
        service: playing.service.clone(),
        title: playing.title.clone(),
        artist: None,
        album: None,
        app_icon: None,
        art_url: None,
        position_us: 0,
        length_us: None,
        can_play: true,
        can_pause: true,
        can_go_next: true,
        can_go_previous: true,
        status: PlaybackStatus::Playing,
        players: vec![playing, paused],
    };

    let selected = media_state_for_player(&state, Some("paused"));
    assert_eq!(selected.service, "paused");
    assert_eq!(selected.status, PlaybackStatus::Paused);
    assert_eq!(selected.players.len(), 2);
}

#[test]
#[ignore = "requires the project-local Broadway GTK runner"]
fn persistent_header_peek_open_geometry_input_and_reversal() {
    use crate::state::{HardwareSnapshot, SystemSnapshot};
    use std::time::{Duration, Instant};

    gtk::init().expect("Broadway GTK display");
    let _styles = crate::ui::install_styles(&crate::theme::generate_gtk());
    let app = gtk::Application::new(
        Some("org.mithshell.persistent-island-test"),
        gtk::gio::ApplicationFlags::NON_UNIQUE,
    );
    app.connect_activate(|_| {});
    app.register(None::<&gtk::gio::Cancellable>)
        .expect("register GTK application");
    let monitor = gtk::gdk::Display::default()
        .expect("Broadway display")
        .monitors()
        .item(0)
        .and_downcast::<gtk::gdk::Monitor>()
        .expect("Broadway monitor");
    let actions = IslandActions {
        switch_workspace: Rc::new(|_, _| {}),
        set_volume: Rc::new(|_| {}),
        toggle_mute: Rc::new(|| {}),
        search: Rc::new(|_| {}),
        select: Rc::new(|_| {}),
        tarragon_status: Rc::new(|| {}),
        tarragon_reload: Rc::new(|| {}),
        load_preview: Rc::new(|_, _| {}),
        media_play_pause: Rc::new(|_| {}),
        media_next: Rc::new(|_| {}),
        media_previous: Rc::new(|_| {}),
        notification_expired: Rc::new(|_, _| {}),
        notification_dismiss: Rc::new(|_| {}),
        notification_invoke: Rc::new(|_, _| {}),
        notification_clear_all: Rc::new(|| {}),
        notification_inhibit: Rc::new(|_| {}),
        tray_activate: Rc::new(|_, _, _, _| {}),
        tray_secondary_activate: Rc::new(|_, _, _, _| {}),
        tray_context_menu: Rc::new(|_, _, _, _| {}),
        tray_scroll: Rc::new(|_, _, _, _| {}),
        tray_menu_event: Rc::new(|_, _, _| {}),
    };
    let pump = |duration: Duration| {
        let main_loop = gtk::glib::MainLoop::new(None, false);
        let quit = main_loop.clone();
        gtk::glib::timeout_add_local_once(duration, move || quit.quit());
        main_loop.run();
        while gtk::glib::MainContext::default().pending() {
            gtk::glib::MainContext::default().iteration(false);
        }
    };
    let await_allocation = |ready: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready() && Instant::now() < deadline {
            pump(Duration::from_millis(16));
        }
    };

    // Disabled motion still resolves the date, hardware panel, and input
    // regions synchronously. Use the same scale as the user's live shell.
    for scale in [1.0, 1.9] {
        let mut config = AppConfig::default();
        config.shell.scale = scale;
        config.shell.animation_ms = 0;
        config.launcher.presentation = LauncherPresentation::Integrated;
        config.battery.wave = true;
        config.theme.engine = crate::config::PaletteEngine::Gtk;
        let island = IslandWindow::new_for_test(
            &app,
            &monitor,
            "broadway-island".into(),
            &config,
            actions.clone(),
            false,
        );
        app.activate();
        pump(Duration::from_millis(40));
        island.update_system(&SystemSnapshot {
            battery: Some(crate::state::BatteryState {
                percent: 63,
                status: "Discharging".into(),
            }),
            hardware: HardwareSnapshot {
                cpu_percent: Some(38.0),
                cpu_temperature_celsius: Some(61.0),
                memory_used_bytes: Some(9_395_240_960),
                memory_total_bytes: Some(34_359_738_368),
                network_receive_bytes_per_second: Some(12_400_000.0),
                network_transmit_bytes_per_second: Some(1_800_000.0),
            },
            ..SystemSnapshot::default()
        });
        assert_eq!(island.battery_waves.area.opacity(), 1.0);
        assert!(
            island
                .compact
                .parent()
                .is_some_and(|parent| parent == island.surface_shell)
        );

        island.set_pointer_in_hover_region(true);
        assert!(island.dashboard.is_visible());
        assert!(island.dashboard.can_target());
        assert_eq!(island.hardware.cpu.label(), "38%");
        assert_eq!(island.hardware.temperature.label(), "61°C");
        assert_eq!(island.hardware.memory.label(), "8.8");
        assert_eq!(island.hardware.memory_total.label(), "/ 32.0 GiB");
        assert_eq!(island.hardware.receive.label(), "11.8 MiB/s");
        assert_eq!(island.hardware.transmit.label(), "1.7 MiB/s");
        assert!((island.hardware.cpu_progress.fraction() - 0.38).abs() < 0.01);
        assert!((island.hardware.memory_progress.fraction() - 0.273).abs() < 0.01);
        assert!(island.compact_date.is_visible());
        assert!(!island.compact_date_day.label().is_empty());
        assert!(!island.compact_date_rest.label().is_empty());
        let peek = island.geometry.get();
        assert!(
            peek.width
                >= f64::from(
                    island.dashboard.measure(gtk::Orientation::Horizontal, -1).0
                        + island.metrics.spacing(44)
                ),
            "peek must contain the hardware minimum width"
        );
        assert!(peek.height > f64::from(island.metrics.compact_height * 2));
        let peek_region = island.island_input_region(peek);
        assert!(
            peek_region.contains_point(island.metrics.window_width / 2, 2),
            "the hover lift must retain the original resting hit area"
        );
        let inset = island.metrics.spacing(22);
        let panel_x = ((island.metrics.window_width - peek.width.round() as i32) / 2) + inset;
        let panel_y = peek.y.round() as i32 + island.metrics.compact_height / 2;
        let panel_height = peek.height.round() as i32 - island.metrics.compact_height / 2;
        assert!(peek_region.contains_point(panel_x + inset, panel_y + panel_height / 2));
        assert!(!peek_region.contains_point(panel_x, panel_y + panel_height - 1));
        let circle_rest = island.central_circle_rect();
        assert_eq!(circle_rest.y, 0.0);
        assert_eq!(circle_rest.height, f64::from(island.metrics.compact_height));
        pump(Duration::from_millis(40));

        island.open();
        assert_eq!(island.current_view.get(), View::Dashboard);
        pump(Duration::from_millis(40));
        assert!(island.compact.is_visible() && island.compact.can_target());
        assert!(island.dashboard.is_visible() && island.dashboard.can_target());
        assert!(island.battery_waves.area.opacity() > 0.0);
        assert_eq!(
            island.battery_waves.header_height,
            island.metrics.compact_height
        );
        assert!(island.geometry.get().width > peek.width);
        assert_eq!(island.hardware.cpu.label(), "38%");
        assert_eq!(island.hardware.temperature.label(), "61°C");
        assert_eq!(island.hardware.memory.label(), "8.8");
        assert_eq!(island.hardware.memory_total.label(), "/ 32.0 GiB");
        assert_eq!(island.hardware.receive.label(), "11.8 MiB/s");
        assert_eq!(island.hardware.transmit.label(), "1.7 MiB/s");
        assert!((island.hardware.cpu_progress.fraction() - 0.38).abs() < 0.01);
        assert!((island.hardware.memory_progress.fraction() - 0.273).abs() < 0.01);
        let panel_content_width = island.geometry.get().width.round() as i32 - inset * 2;
        assert!(
            island
                .workspace_row
                .measure(gtk::Orientation::Horizontal, -1)
                .1
                <= panel_content_width
        );
        assert!(
            island
                .volume_value
                .measure(gtk::Orientation::Horizontal, -1)
                .1
                > 0
        );
        assert!(
            island
                .notification_list
                .measure(gtk::Orientation::Horizontal, -1)
                .1
                <= panel_content_width
        );
        assert!(
            island
                .compact
                .pick(
                    f64::from(island.compact.width()) / 2.0,
                    f64::from(island.compact.height()) / 2.0,
                    gtk::PickFlags::DEFAULT,
                )
                .is_some()
        );
        let open_region = island.island_input_region(island.geometry.get());
        let open_panel_x =
            ((island.metrics.window_width - island.geometry.get().width.round() as i32) / 2)
                + inset;
        let open_panel_y =
            island.geometry.get().y.round() as i32 + island.metrics.compact_height / 2;
        assert!(open_region.contains_point(
            open_panel_x + island.metrics.spacing(50),
            open_panel_y + island.metrics.spacing(100),
        ));
        assert!(island.compact_date.is_visible());

        // A hardware refresh or pointer leave must never reset the pinned
        // header to its content-driven idle allocation.
        let open_width = island.geometry.get().width;
        island.set_pointer_in_hover_region(false);
        island.resize_compact();
        // Broadway's ordinary windows cannot express the layer-shell
        // ordering of the dismiss catcher. Present the actual fixture
        // root so an empty test window cannot occlude its frame clock.
        let fixture_window = island
            .focus_root
            .borrow()
            .clone()
            .downcast::<gtk::Window>()
            .unwrap();
        fixture_window.present();
        await_allocation(&|| (f64::from(island.compact.width()) - open_width).abs() <= 4.0);
        assert_eq!(island.current_view.get(), View::Dashboard);
        assert!(
            (f64::from(island.compact.width()) - open_width).abs() <= 6.0,
            "pinned header width={} requested={} open={} shell={} scale={}",
            island.compact.width(),
            island.compact.width_request(),
            open_width,
            island.surface_shell.width(),
            island.metrics.scale
        );
        let scroll = island.notification_scroll.vadjustment();
        scroll.set_value(scroll.upper());
        pump(Duration::from_millis(40));
        let last = island
            .notification_list
            .last_child()
            .expect("notification content");
        let bounds = last
            .compute_bounds(&island.notification_scroll)
            .expect("notification allocation");
        assert!(
            bounds.y() + bounds.height() <= island.notification_scroll.height() as f32 + 2.0,
            "last notification must be reachable inside its own scroller"
        );
        let history: Vec<_> = (0..8).map(|id| crate::state::Notification {
            id,
            received_at_unix_seconds: 0,
            app_name: "GTK fixture".into(),
            app_icon: None,
            summary: "Saved screenshot".into(),
            body: "A wrapped notification body with a long /home/test/Pictures/Screenshots/file.png path. ".repeat(12),
            urgency: crate::state::Urgency::Normal,
            actions: vec![],
            timeout: crate::state::NotificationTimeout::Never,
        }).collect();
        let mut representative = history[..2].to_vec();
        for notification in &mut representative {
            notification.received_at_unix_seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .saturating_sub(120);
        }
        representative[0].body =
            "Saved to /home/test/Pictures/Screenshots/2026-09-25_19-45-00.png".into();
        representative[1].app_name = "Build runner".into();
        representative[1].summary = "Build finished".into();
        representative[1].body = "Release candidate compiled successfully.".into();
        island.update_notification_history(&representative);
        fixture_window.present();
        let row = island.notification_list.first_child().unwrap();
        let mut pending = vec![row];
        let mut rendered_text = Vec::new();
        while let Some(widget) = pending.pop() {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                rendered_text.push(label.label().to_string());
            }
            let mut child = widget.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                pending.push(widget);
            }
        }
        for expected in [
            &representative[0].app_name,
            &representative[0].summary,
            &representative[0].body,
        ] {
            assert!(
                rendered_text.contains(expected),
                "notification lost {expected:?}"
            );
        }
        pump(Duration::from_millis(150));
        capture_island_fixture(&island, "notifications");

        island.update_notification_history(&history);
        fixture_window.present();
        let scroll = island.notification_scroll.vadjustment();
        await_allocation(&|| scroll.upper() > scroll.page_size());
        assert!(
            scroll.upper() > scroll.page_size(),
            "long history must scroll locally: scale={scale} upper={} page={} geometry={:?} panel={} dashboard={} list={} visible={}",
            scroll.upper(),
            scroll.page_size(),
            island.geometry.get(),
            island.panel_scroll.height(),
            island.dashboard.height(),
            island.notification_list.height(),
            island.notification_list.is_visible()
        );
        let hardware_before = island.hardware.root.compute_bounds(&island.fixed).unwrap();
        let notification_section = island.dashboard_sections.last().unwrap();
        let header = notification_section.content.first_child().unwrap();
        let header_before = header.compute_bounds(&island.fixed).unwrap();
        assert!(
            notification_section.viewport.height() <= island.metrics.monitor_height / 3 + 2,
            "notification card must be capped at a third of the display: card={} monitor={} scale={scale}",
            notification_section.viewport.height(),
            island.metrics.monitor_height,
        );
        let panel = island.panel_scroll.vadjustment();
        assert!(
            panel.upper() <= panel.page_size() + 4.0,
            "dashboard should fit without scrolling: upper={} page={} scale={scale}",
            panel.upper(),
            panel.page_size()
        );
        panel.set_value(panel.upper());
        assert_eq!(panel.value(), 0.0, "dashboard clip must stay pinned");
        for section in &island.dashboard_sections {
            let adjustment = section.viewport.vadjustment();
            adjustment.set_value(adjustment.upper());
            assert_eq!(adjustment.value(), 0.0, "section clips must stay pinned");
        }
        scroll.set_value(scroll.upper());
        let last = island.notification_list.last_child().unwrap();
        await_allocation(&|| {
            last.compute_bounds(&island.notification_scroll)
                .is_some_and(|bounds| {
                    bounds.y() + bounds.height() <= island.notification_scroll.height() as f32 + 4.0
                })
        });
        let bounds = last.compute_bounds(&island.notification_scroll).unwrap();
        assert!(
            bounds.y() + bounds.height() <= island.notification_scroll.height() as f32 + 4.0,
            "wrapped history remains reachable at scale {scale}"
        );
        assert!(scroll.value() > 0.0);
        assert_eq!(
            island.hardware.root.compute_bounds(&island.fixed).unwrap(),
            hardware_before
        );
        assert_eq!(header.compute_bounds(&island.fixed).unwrap(), header_before);
        let capped_height = island.geometry.get().height;
        island.update_notification_history(&[]);
        await_allocation(&|| island.notification_scroll.vadjustment().value() == 0.0);
        assert!(
            island.geometry.get().height < capped_height,
            "empty history should shrink the dashboard rather than reserve the cap"
        );

        // Existing alternate views temporarily replace the center, then
        // return to the persistent compact header through the production path.
        island.open_weather();
        assert_eq!(island.current_view.get(), View::Weather);
        island.close();
        assert_eq!(island.current_view.get(), View::Compact);
        island.open_search();
        assert_eq!(island.current_view.get(), View::Search);
        island.close();
        assert_eq!(island.current_view.get(), View::Compact);
        assert!(island.compact.is_visible());
        island
            .compact_visualizer_revealer
            .set_transition_duration(0);
        island.compact_visualizer_revealer.set_reveal_child(true);
        island.compact_battery.set_visible(true);
        island.compact_battery.set_label("100%");
        island.resize_compact();
        island.reconcile_pill_geometry();
        fixture_window.present();
        await_allocation(&|| (island.compact.width() - island.compact_width.get()).abs() <= 4);
        let battery = island
            .compact_battery
            .compute_bounds(&island.compact)
            .unwrap();
        assert!(
            battery.x() + battery.width() <= island.compact.width() as f32 - 10.0,
            "idle battery and visualizer must fit with right padding at scale {scale}: {battery:?}, width={}",
            island.compact.width()
        );
        island.destroy();
    }

    // Real frame-clock sampling and reversal: interrupt both the hover
    // expand and its collapse, then promote that same rendered geometry to
    // the click-open path without a one-frame jump.
    let mut animated_config = AppConfig::default();
    animated_config.shell.scale = 1.0;
    animated_config.shell.animation_ms = 280;
    let animated = IslandWindow::new_for_test(
        &app,
        &monitor,
        "broadway-island-motion".into(),
        &animated_config,
        actions,
        true,
    );
    pump(Duration::from_millis(520));
    let resting = animated.geometry.get();
    animated.set_pointer_in_hover_region(true);
    assert!(animated.dashboard.is_visible());
    assert_eq!(
        animated.dashboard.opacity(),
        1.0,
        "Peek content must be ready before expansion"
    );
    assert!(animated.compact_date.is_visible());
    assert_eq!(animated.compact_date.opacity(), 0.0);
    await_allocation(&|| animated.geometry.get().height > resting.height + 1.0);
    pump(Duration::from_millis(150));
    let date_midway = animated.compact_date.opacity();
    assert!(
        date_midway > 0.0 && date_midway < 1.0,
        "date must fade in, got {date_midway}"
    );
    let expanded_part = animated.geometry.get();
    assert_eq!(animated.dashboard.opacity(), 1.0);
    assert!(
        expanded_part.height > resting.height,
        "resting={resting:?} sampled={expanded_part:?} target={:?} active={} mapped={} hovered={} enabled={} ms={} generation={}",
        animated.presentation_target_geometry(View::Compact),
        animated.view_transition_active.get(),
        animated.surface.is_mapped(),
        animated.island_hovered.get(),
        animated.animations_enabled.get(),
        animated.animation_ms.get(),
        animated.pill_animation_generation.get()
    );
    assert!(expanded_part.height < animated.presentation_target_geometry(View::Compact).height);
    animated.set_pointer_in_hover_region(false);
    assert_eq!(animated.geometry.get(), expanded_part);
    assert_eq!(animated.compact_date.opacity(), date_midway);
    assert!(animated.compact_date.is_visible());
    pump(Duration::from_millis(72));
    let collapsing_part = animated.geometry.get();
    assert!(collapsing_part.height < expanded_part.height);
    animated.set_pointer_in_hover_region(true);
    assert_eq!(animated.geometry.get(), collapsing_part);
    pump(Duration::from_millis(72));
    let expanding_again = animated.geometry.get();
    animated.open();
    assert_eq!(animated.geometry.get(), expanding_again);
    assert!(animated.view_transition_active.get());
    animated.close();
    assert_eq!(animated.geometry.get(), expanding_again);
    pump(Duration::from_millis(520));
    animated.set_pointer_in_hover_region(false);
    pump(Duration::from_millis(520));
    assert!(!animated.compact_date.is_visible());
    assert_eq!(animated.compact_date.opacity(), 0.0);
    assert!(!animated.panel_scroll.is_visible());
    // Only the actual capsule draws a border at idle or during collapse.
    assert!(animated.surface_shell.has_css_class("island-persistent"));

    // Frequent unchanged telemetry/media refreshes must not reset the
    // hover clock. Both directions still finish within their own duration.
    for hovered in [true, false] {
        animated.set_pointer_in_hover_region(hovered);
        let destination = animated.presentation_target_geometry(View::Compact);
        for _ in 0..12 {
            pump(Duration::from_millis(50));
            animated.update_system(&SystemSnapshot::default());
            animated.reconcile_view();
        }
        assert!(
            (animated.geometry.get().height - destination.height).abs() < 1.0,
            "refreshes extended the hover transition: {:?} -> {destination:?}",
            animated.geometry.get()
        );
        assert!(!animated.view_transition_active.get());
    }

    // Direct IPC/click opening from Idle must measure the hidden panel
    // before starting, rather than grow only the header then pop the page
    // to full height in finish_view.
    let open_target = animated.presentation_target_geometry(View::Dashboard);
    assert!(!animated.dashboard.is_visible());
    assert!(open_target.height > f64::from(animated.metrics.compact_height * 4));
    animated.open();
    assert_eq!(animated.dashboard.opacity(), 1.0);
    assert_eq!(animated.dashboard_section_opacity.get(), 1.0);
    assert!(
        animated
            .dashboard_sections
            .iter()
            .all(|section| section.viewport.is_visible())
    );
    pump(Duration::from_millis(300));
    assert!(animated.geometry.get().height > open_target.height * 0.7);
    pump(Duration::from_millis(260));
    assert!((animated.geometry.get().height - open_target.height).abs() < 1.0);

    animated.set_pointer_in_hover_region(true);
    animated.close();
    assert!(
        animated
            .dashboard_sections
            .iter()
            .all(|section| section.viewport.is_visible())
    );
    assert_eq!(animated.dashboard_section_opacity.get(), 1.0);
    pump(Duration::from_millis(80));
    let fading = animated.dashboard_section_opacity.get();
    let contracting = animated.dashboard_expansion.get();
    assert!(
        fading > 0.0 && fading < 1.0,
        "Open-only content must fade, got {fading}"
    );
    assert!(contracting > 0.0 && contracting < 1.0);
    assert!(
        animated
            .dashboard_sections
            .iter()
            .all(|section| section.viewport.is_visible())
    );
    animated.open();
    assert_eq!(animated.dashboard_section_opacity.get(), fading);
    assert_eq!(animated.dashboard_expansion.get(), contracting);
    pump(Duration::from_millis(560));
    animated.close();
    pump(Duration::from_millis(460));
    assert!(
        animated
            .dashboard_sections
            .iter()
            .all(|section| !section.viewport.is_visible())
    );
    assert!(animated.hardware.root.is_visible());
    assert!(animated.dashboard.is_visible());

    // Exercise the new settings through real widget entry points. A zero
    // scale must settle synchronously, including date opacity and picking,
    // even when a legacy positive duration remains in the config.
    let legacy_peek = animated.geometry.get();
    let mut shell = animated_config.shell.clone();
    let mut motion = crate::config::MotionConfig {
        duration_scale: 0.0,
        ..crate::config::MotionConfig::default()
    };
    motion.peek.lift = 0.0;
    motion.peek.width_scale = 0.9;
    shell.motion = Some(motion);
    animated.update_shell_config(&shell, true);
    animated.set_pointer_in_hover_region(false);
    assert!(animated.pill_animation_target.get().is_none());
    animated.set_pointer_in_hover_region(true);
    let configured_peek = animated.geometry.get();
    assert_eq!(
        configured_peek,
        animated.presentation_target_geometry(View::Compact)
    );
    assert_eq!(configured_peek.y, 0.0);
    assert!(configured_peek.width <= legacy_peek.width);
    assert_eq!(animated.compact_date.opacity(), 1.0);
    assert!(animated.dashboard.can_target());
    assert_eq!(animated.search_stack.transition_duration(), 0);
    assert_eq!(
        animated.compact_visualizer_revealer.transition_duration(),
        0
    );
    animated.open();
    assert!(!animated.view_transition_active.get());
    animated.close();
    assert!(!animated.view_transition_active.get());
    assert_eq!(animated.compact_date.opacity(), 1.0);
    animated.open_search();
    assert_eq!(animated.search.opacity(), 1.0);
    animated.close();
    assert!(!animated.search_window.is_visible());
    animated.set_pointer_in_hover_region(false);

    // The new table overrides legacy zero, and its scaled delay stays in
    // sync with its scaled transform. Reversing preserves the rendered frame.
    shell.animation_ms = 0;
    motion.duration_scale = 2.0;
    shell.motion = Some(motion);
    animated.update_shell_config(&shell, true);
    assert_eq!(animated.search_stack.transition_duration(), 320);
    assert_eq!(
        animated.compact_visualizer_revealer.transition_duration(),
        560
    );
    let duration = animated
        .motion_profile(crate::ui::motion::Transition::PeekEnter)
        .duration;
    assert_eq!(duration, Duration::from_millis(800));
    assert_eq!(
        animated.island_fade_progress(true, Duration::from_millis(100), duration),
        0.0
    );
    assert!(animated.island_fade_progress(true, Duration::from_millis(300), duration) > 0.0);
    let rest = animated.geometry.get();
    animated.set_pointer_in_hover_region(true);
    assert_eq!(animated.geometry.get(), rest);
    pump(Duration::from_millis(100));
    let midway = animated.geometry.get();
    assert!(midway.height > rest.height);
    assert!(midway.height < animated.presentation_target_geometry(View::Compact).height);
    animated.set_pointer_in_hover_region(false);
    assert_eq!(animated.geometry.get(), midway);
    motion.duration_scale = 0.0;
    shell.motion = Some(motion);
    animated.update_shell_config(&shell, true);
    animated.set_pointer_in_hover_region(true);
    assert_eq!(
        animated.geometry.get(),
        animated.presentation_target_geometry(View::Compact)
    );
    assert_eq!(animated.compact_date.opacity(), 1.0);
    animated.destroy();
}

/// Optional artifact from the mapped GTK widget tree, using the installed
/// production stylesheet and its actual allocations rather than a mock UI.
fn capture_island_fixture(island: &IslandWindow, state: &str) {
    let Ok(directory) = std::env::var("MITHSHELL_UI_CAPTURE_DIR") else {
        return;
    };
    let paintable = gtk::WidgetPaintable::new(Some(&island.surface_shell));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(
        &snapshot,
        f64::from(island.surface_shell.width()),
        f64::from(island.surface_shell.height()),
    );
    let node = snapshot.to_node().expect("mapped island render node");
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer
        .realize(None::<&gtk::gdk::Surface>)
        .expect("offscreen GTK renderer");
    let texture = renderer.render_texture(&node, None);
    std::fs::create_dir_all(&directory).expect("capture directory");
    texture
        .save_to_png(
            std::path::Path::new(&directory)
                .join(format!("island-{}-{state}.png", island.metrics.scale)),
        )
        .expect("save GTK fixture");
    renderer.unrealize();
}
