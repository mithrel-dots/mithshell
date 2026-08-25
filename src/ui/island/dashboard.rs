//! The expanded dashboard: a compact header band, the output/workspace
//! strip, the player panel, thin system control rows, and the notification
//! history panel that takes whatever vertical space is left over.

use super::*;

use std::rc::Rc;

use gtk::{Align, Orientation};

use super::{IslandWindow, Metrics};
use crate::state::{HyprlandSnapshot, SystemSnapshot};

pub(super) struct DashboardWidgets {
    pub(super) root: gtk::Box,
    pub(super) hero_time: gtk::Label,
    pub(super) hero_date: gtk::Label,
    pub(super) battery_chip: gtk::Box,
    pub(super) battery_icon: gtk::Image,
    pub(super) battery_label: gtk::Label,
    pub(super) player_card: gtk::Box,
    pub(super) player_icon: gtk::Image,
    pub(super) player_title: gtk::Label,
    pub(super) player_artist: gtk::Label,
    pub(super) player_progress: gtk::ProgressBar,
    pub(super) player_elapsed_label: gtk::Label,
    pub(super) player_duration_label: gtk::Label,
    pub(super) player_prev_button: gtk::Button,
    pub(super) player_play_pause_button: gtk::Button,
    pub(super) player_next_button: gtk::Button,
    pub(super) player_switch_row: gtk::Box,
    pub(super) player_switch_label: gtk::Label,
    pub(super) player_switch_prev: gtk::Button,
    pub(super) player_switch_next: gtk::Button,
    pub(super) active_eyebrow: gtk::Label,
    pub(super) active_title: gtk::Label,
    pub(super) workspace_row: gtk::FlowBox,
    pub(super) volume_scale: gtk::Scale,
    pub(super) volume_value: gtk::Label,
    pub(super) brightness_row: gtk::Box,
    pub(super) brightness_scale: gtk::Scale,
    pub(super) brightness_value: gtk::Label,
    pub(super) notification_count: gtk::Label,
    pub(super) notification_list: gtk::Box,
    pub(super) weather_button: gtk::Button,
    pub(super) search_button: gtk::Button,
    pub(super) close_button: gtk::Button,
}

pub(super) fn dashboard_view(metrics: Metrics) -> DashboardWidgets {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(8));
    root.set_size_request(metrics.dashboard_width, metrics.dashboard_height);
    root.add_css_class("dashboard-content");
    root.set_valign(Align::Start);

    // The clock leads the header band and the identity/date pair stacks
    // beside it rather than under it, so the window controls share one row
    // with the time instead of facing an empty gap across the panel.
    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(9));
    let time = gtk::Label::new(Some("--:--"));
    time.add_css_class("hero-time");
    time.set_halign(Align::Start);
    time.set_valign(Align::Center);
    let heading = gtk::Box::new(Orientation::Vertical, 0);
    heading.set_hexpand(true);
    heading.set_valign(Align::Center);
    // Filled rather than sized to the text: `.eyebrow`'s letter-spacing is
    // not counted in the natural width GTK measures, so an ellipsizing
    // label pinned to `halign: start` gets allocated a hair less than it
    // needs and drops its last character.
    let eyebrow = gtk::Label::new(Some("MITHSHELL  //  LOCAL"));
    eyebrow.add_css_class("eyebrow");
    eyebrow.set_ellipsize(gtk::pango::EllipsizeMode::End);
    eyebrow.set_xalign(0.0);
    let date = gtk::Label::new(None);
    date.add_css_class("hero-date");
    date.set_ellipsize(gtk::pango::EllipsizeMode::End);
    date.set_xalign(0.0);
    heading.append(&eyebrow);
    heading.append(&date);

    let battery_chip = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    battery_chip.add_css_class("battery-chip");
    battery_chip.set_valign(Align::Center);
    battery_chip.set_visible(false);
    let battery_icon = gtk::Image::from_icon_name("xsi-battery-symbolic");
    battery_chip.append(&battery_icon);
    let battery_label = gtk::Label::new(None);
    battery_chip.append(&battery_label);

    let close_button = gtk::Button::from_icon_name("window-close-symbolic");
    close_button.add_css_class("close-button");
    close_button.set_valign(Align::Center);
    let search_button = gtk::Button::from_icon_name("system-search-symbolic");
    search_button.add_css_class("close-button");
    search_button.set_tooltip_text(Some("Search with TarraGon"));
    search_button.set_valign(Align::Center);
    let weather_button = gtk::Button::from_icon_name("weather-clear-symbolic");
    weather_button.add_css_class("close-button");
    weather_button.set_tooltip_text(Some("Weather forecast"));
    weather_button.set_valign(Align::Center);
    header.append(&time);
    header.append(&heading);
    header.append(&battery_chip);
    header.append(&weather_button);
    header.append(&search_button);
    header.append(&close_button);
    root.append(&header);

    // Output identity and the workspace grid share a single panel. The grid
    // asks for exactly the width its buttons need and the window title
    // takes the remainder, rather than the near-empty column claiming the
    // wider half.
    let status_card = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    status_card.add_css_class("status-card");

    let active_column = gtk::Box::new(Orientation::Vertical, metrics.spacing(1));
    active_column.set_hexpand(true);
    active_column.set_valign(Align::Center);
    let active_eyebrow = gtk::Label::new(Some("OUTPUT  //  WORKSPACE --"));
    active_eyebrow.add_css_class("eyebrow");
    active_eyebrow.set_ellipsize(gtk::pango::EllipsizeMode::End);
    active_eyebrow.set_xalign(0.0);
    active_eyebrow.set_max_width_chars(16);
    let active_title = gtk::Label::new(Some("Quiet desktop"));
    active_title.add_css_class("active-title");
    active_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // Fill + a small natural width, rather than `halign: start` at the
    // label's own width: the dashboard is laid out at its natural size
    // inside a fixed-width surface, so a long title left unbounded pushes
    // the workspace grid out past the visible edge. Filling still lets it
    // use every pixel the strip actually has at any scale.
    active_title.set_xalign(0.0);
    active_title.set_max_width_chars(16);
    active_column.append(&active_eyebrow);
    active_column.append(&active_title);

    let workspace_row = gtk::FlowBox::new();
    workspace_row.add_css_class("workspace-grid");
    workspace_row.set_column_spacing(metrics.spacing(4) as u32);
    workspace_row.set_row_spacing(metrics.spacing(4) as u32);
    // Rewritten per snapshot by `update_hyprland` so the grid never
    // reserves slots it has no workspace for.
    workspace_row.set_max_children_per_line(5);
    workspace_row.set_min_children_per_line(5);
    workspace_row.set_selection_mode(gtk::SelectionMode::None);
    workspace_row.set_halign(Align::End);
    workspace_row.set_valign(Align::Center);

    status_card.append(&active_column);
    status_card.append(&workspace_row);
    root.append(&status_card);

    let player_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(6));
    player_card.add_css_class("player-card");
    player_card.add_css_class("unavailable");

    let player_top = gtk::Box::new(Orientation::Horizontal, metrics.spacing(9));
    let player_icon = gtk::Image::new();
    player_icon.add_css_class("player-icon");
    player_icon.set_valign(Align::Center);
    player_icon.set_visible(false);

    let player_text = gtk::Box::new(Orientation::Vertical, 0);
    player_text.set_hexpand(true);
    player_text.set_valign(Align::Center);
    // Same bounded-natural-width treatment as the status strip, so a long
    // track title ellipsizes inside the panel instead of shoving the
    // transport buttons off the edge of the surface.
    let player_title = gtk::Label::new(Some("Nothing playing"));
    player_title.add_css_class("player-title");
    player_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    player_title.set_xalign(0.0);
    player_title.set_max_width_chars(18);
    let player_artist = gtk::Label::new(None);
    player_artist.add_css_class("player-artist");
    player_artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
    player_artist.set_xalign(0.0);
    player_artist.set_max_width_chars(18);
    player_artist.set_visible(false);
    player_text.append(&player_title);
    player_text.append(&player_artist);

    let player_prev_button = gtk::Button::from_icon_name("media-skip-backward-symbolic");
    player_prev_button.add_css_class("player-button");
    player_prev_button.set_tooltip_text(Some("Previous track"));
    player_prev_button.set_valign(Align::Center);
    player_prev_button.set_sensitive(false);
    let player_play_pause_button = gtk::Button::from_icon_name("media-playback-pause-symbolic");
    player_play_pause_button.add_css_class("player-button");
    player_play_pause_button.add_css_class("player-play");
    player_play_pause_button.set_tooltip_text(Some("Play/Pause"));
    player_play_pause_button.set_valign(Align::Center);
    player_play_pause_button.set_sensitive(false);
    let player_next_button = gtk::Button::from_icon_name("media-skip-forward-symbolic");
    player_next_button.add_css_class("player-button");
    player_next_button.set_tooltip_text(Some("Next track"));
    player_next_button.set_valign(Align::Center);
    player_next_button.set_sensitive(false);

    player_top.append(&player_icon);
    player_top.append(&player_text);
    player_top.append(&player_prev_button);
    player_top.append(&player_play_pause_button);
    player_top.append(&player_next_button);
    player_card.append(&player_top);

    // Elapsed/remaining sit on the progress line instead of below it, which
    // drops a whole row from the panel and reads as one scrubber.
    let player_time_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    let player_elapsed_label = gtk::Label::new(Some("--:--"));
    player_elapsed_label.add_css_class("player-time");
    player_elapsed_label.set_halign(Align::Start);
    player_elapsed_label.set_valign(Align::Center);
    let player_progress = gtk::ProgressBar::new();
    player_progress.set_hexpand(true);
    player_progress.set_valign(Align::Center);
    player_progress.add_css_class("player-progress");
    let player_duration_label = gtk::Label::new(Some("--:--"));
    player_duration_label.add_css_class("player-time");
    player_duration_label.set_halign(Align::End);
    player_duration_label.set_valign(Align::Center);
    player_time_row.append(&player_elapsed_label);
    player_time_row.append(&player_progress);
    player_time_row.append(&player_duration_label);
    player_card.append(&player_time_row);
    let player_switch_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(4));
    player_switch_row.add_css_class("player-switch-row");
    let player_switch_prev = gtk::Button::from_icon_name("go-previous-symbolic");
    player_switch_prev.add_css_class("player-switch-button");
    let player_switch_label = gtk::Label::new(Some("1 player"));
    player_switch_label.add_css_class("player-switch-label");
    player_switch_label.set_hexpand(true);
    player_switch_label.set_xalign(0.5);
    let player_switch_next = gtk::Button::from_icon_name("go-next-symbolic");
    player_switch_next.add_css_class("player-switch-button");
    player_switch_row.append(&player_switch_prev);
    player_switch_row.append(&player_switch_label);
    player_switch_row.append(&player_switch_next);
    player_card.append(&player_switch_row);
    root.append(&player_card);

    // Volume and brightness are single sliders: they ride directly on the
    // dashboard as thin rows instead of inside a panel that would give them
    // the same weight as the content above and below.
    let controls = gtk::Box::new(Orientation::Vertical, 0);
    controls.add_css_class("control-stack");
    let (volume_row, volume_scale, volume_value) =
        control_row("audio-volume-high-symbolic", "Volume", metrics);
    let (brightness_row, brightness_scale, brightness_value) =
        control_row("display-brightness-symbolic", "Brightness", metrics);
    controls.append(&volume_row);
    controls.append(&brightness_row);
    root.append(&controls);

    // The densest section, and the only one that grows: it absorbs whatever
    // the fixed-height dashboard has left after the rows above.
    let notification_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(6));
    notification_card.add_css_class("notification-card");
    notification_card.set_vexpand(true);

    let notification_header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    let notification_title = gtk::Label::new(Some("NOTIFICATIONS"));
    notification_title.add_css_class("eyebrow");
    notification_title.set_halign(Align::Start);
    notification_title.set_hexpand(true);
    let notification_count = gtk::Label::new(Some("0"));
    notification_count.add_css_class("notification-count");
    notification_header.append(&notification_title);
    notification_header.append(&notification_count);
    notification_card.append(&notification_header);

    let notification_list = gtk::Box::new(Orientation::Vertical, metrics.spacing(4));
    notification_list.add_css_class("notification-list");
    notification_list.set_vexpand(true);
    // Replaced by `update_notification_history` as soon as the controller
    // pushes its first (possibly empty) history snapshot.
    let notification_placeholder = gtk::Label::new(Some("No notifications yet"));
    notification_placeholder.add_css_class("muted-label");
    notification_placeholder.add_css_class("notification-empty");
    notification_placeholder.set_halign(Align::Start);
    notification_placeholder.set_wrap(true);
    notification_list.append(&notification_placeholder);
    notification_card.append(&notification_list);
    root.append(&notification_card);

    DashboardWidgets {
        root,
        hero_time: time,
        hero_date: date,
        battery_chip,
        battery_icon,
        battery_label,
        player_card,
        player_icon,
        player_title,
        player_artist,
        player_progress,
        player_elapsed_label,
        player_duration_label,
        player_prev_button,
        player_play_pause_button,
        player_next_button,
        player_switch_row,
        player_switch_label,
        player_switch_prev,
        player_switch_next,
        active_eyebrow,
        active_title,
        workspace_row,
        volume_scale,
        volume_value,
        brightness_row,
        brightness_scale,
        brightness_value,
        notification_count,
        notification_list,
        weather_button,
        search_button,
        close_button,
    }
}

fn control_row(icon: &str, label: &str, metrics: Metrics) -> (gtk::Box, gtk::Scale, gtk::Label) {
    let row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    row.add_css_class("control-row");
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("control-icon");
    image.set_tooltip_text(Some(label));
    let scale = gtk::Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.add_css_class("control-scale");
    let value = gtk::Label::new(Some("--"));
    value.add_css_class("control-value");
    value.set_xalign(1.0);
    row.append(&image);
    row.append(&scale);
    row.append(&value);
    (row, scale, value)
}

/// Nearest-decile battery icon, e.g. `xsi-battery-level-40-charging-symbolic`.
pub(super) fn battery_icon_name(percent: u8, status: &str) -> String {
    let level = ((f64::from(percent.min(100)) / 10.0).round() * 10.0) as u8;
    let charging = if status.eq_ignore_ascii_case("charging") {
        "-charging"
    } else {
        ""
    };
    format!("xsi-battery-level-{level}{charging}-symbolic")
}

impl IslandWindow {
    pub fn update_hyprland(self: &Rc<Self>, snapshot: &HyprlandSnapshot) {
        *self.latest_hyprland.borrow_mut() = snapshot.clone();
        self.reconcile_notification_toasts();
        let monitor = snapshot.monitor(&self.monitor_name);
        let active_workspace = monitor.map(|monitor| monitor.active_workspace.id);
        self.active_eyebrow.set_label(&format!(
            "{}  //  WORKSPACE {}",
            self.monitor_name,
            active_workspace
                .map(|id| id.to_string())
                .unwrap_or_else(|| "--".into())
        ));

        let active_window = snapshot
            .active_window
            .as_ref()
            .filter(|window| monitor.is_some_and(|monitor| window.monitor == monitor.id));
        self.active_title.set_label(
            active_window
                .map(|window| window.title.as_str())
                .filter(|title| !title.is_empty())
                .unwrap_or("Quiet desktop"),
        );

        clear_box(&self.compact_workspaces);
        clear_box(&self.media_workspaces);
        while let Some(child) = self.workspace_row.first_child() {
            let child = child
                .downcast::<gtk::FlowBoxChild>()
                .expect("flow box children are wrapped by GTK");
            self.workspace_row.remove(&child);
        }
        let workspaces = snapshot.workspaces_for(&self.monitor_name);
        for workspace in workspaces
            .iter()
            .filter(|workspace| workspace.windows > 0 || Some(workspace.id) == active_workspace)
            .take(7)
        {
            for container in [&self.compact_workspaces, &self.media_workspaces] {
                let dot = gtk::Button::new();
                dot.add_css_class("workspace-dot");
                dot.set_tooltip_text(Some(&format!("Workspace {}", workspace.name)));
                if workspace.windows > 0 {
                    dot.add_css_class("occupied");
                }
                if Some(workspace.id) == active_workspace {
                    dot.add_css_class("active");
                }
                let actions = self.actions.clone();
                let monitor_name = self.monitor_name.clone();
                let workspace_id = workspace.id;
                dot.connect_clicked(move |_| {
                    (actions.switch_workspace)(&monitor_name, workspace_id)
                });
                container.append(&dot);
            }
        }

        // Fit the grid to the workspaces that actually exist: up to five in
        // a single row, then balanced over two rows. A fixed five-per-line
        // grid reserves (and leaves blank) slots it never fills.
        let shown = workspaces.len().min(10);
        let per_line = if shown <= 5 { shown } else { shown.div_ceil(2) };
        let per_line = per_line.clamp(1, 5) as u32;
        self.workspace_row.set_min_children_per_line(per_line);
        self.workspace_row.set_max_children_per_line(per_line);

        for workspace in workspaces.into_iter().take(10) {
            let button = gtk::Button::with_label(&workspace.name);
            button.add_css_class("workspace-button");
            if workspace.windows > 0 {
                button.add_css_class("occupied");
            }
            if Some(workspace.id) == active_workspace {
                button.add_css_class("active");
            }
            let actions = self.actions.clone();
            let monitor_name = self.monitor_name.clone();
            let workspace_id = workspace.id;
            button
                .connect_clicked(move |_| (actions.switch_workspace)(&monitor_name, workspace_id));
            self.workspace_row.insert(&button, -1);
        }
        self.resize_compact();
    }

    pub fn update_system(self: &Rc<Self>, snapshot: &SystemSnapshot) {
        self.updating_controls.set(true);
        if let Some(audio) = snapshot.audio {
            self.volume_scale.set_value(f64::from(audio.percent));
            self.volume_value.set_label(&format!("{}%", audio.percent));
            self.volume_scale.set_sensitive(true);
        } else {
            self.volume_value.set_label("--");
            self.volume_scale.set_sensitive(false);
        }

        if let Some(brightness) = &snapshot.brightness {
            self.brightness_scale
                .set_value(f64::from(brightness.percent));
            self.brightness_value
                .set_label(&format!("{}%", brightness.percent));
            self.brightness_row.set_visible(true);
            self.brightness_row.remove_css_class("unavailable");
            self.brightness_scale.set_sensitive(true);
        } else {
            self.brightness_value.set_label("--");
            self.brightness_row.set_visible(false);
            self.brightness_scale.set_sensitive(false);
        }

        if let Some(battery) = &snapshot.battery {
            let name = battery_icon_name(battery.percent, &battery.status);
            self.battery_icon.set_icon_name(Some(&name));
            self.battery_label
                .set_label(&format!("{}%", battery.percent));
            self.compact_battery
                .set_label(&format!("{}%", battery.percent));
            self.battery_chip.set_visible(true);
            self.compact_battery.set_visible(true);
        } else {
            self.battery_chip.set_visible(false);
            self.compact_battery.set_visible(false);
        }
        self.updating_controls.set(false);
        self.resize_compact();
    }
}
