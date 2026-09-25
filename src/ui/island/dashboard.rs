//! The expanded dashboard: system statistics, output/workspace, volume, and
//! notification history.

use super::*;

use std::rc::Rc;

use gtk::{Align, Orientation};

use super::{IslandWindow, Metrics};
use crate::state::{HyprlandSnapshot, SystemSnapshot};

pub(super) struct DashboardWidgets {
    pub(super) root: gtk::Box,
    pub(super) sections: Vec<DashboardSection>,
    pub(super) hardware: HardwarePanel,
    pub(super) active_eyebrow: gtk::Label,
    pub(super) active_title: gtk::Label,
    pub(super) workspace_row: gtk::FlowBox,
    pub(super) volume_scale: gtk::Scale,
    pub(super) volume_value: gtk::Label,
    pub(super) notification_count: gtk::Label,
    pub(super) notification_inhibit_remaining: gtk::Label,
    pub(super) notification_clear_button: gtk::Button,
    pub(super) notification_inhibit_button: gtk::ToggleButton,
    pub(super) notification_list: gtk::Box,
    pub(super) weather_button: gtk::Button,
    pub(super) search_button: gtk::Button,
    pub(super) close_button: gtk::Button,
    pub(super) mute_button: gtk::Button,
}

pub(super) struct DashboardSection {
    pub(super) viewport: gtk::ScrolledWindow,
    pub(super) content: gtk::Box,
    pub(super) trailing_gap: bool,
}

fn dashboard_section(root: &gtk::Box, content: &gtk::Box, trailing_gap: bool) -> DashboardSection {
    let viewport = gtk::ScrolledWindow::new();
    viewport.add_css_class("island-dashboard-section");
    viewport.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
    viewport.set_has_frame(false);
    viewport.set_overflow(gtk::Overflow::Hidden);
    viewport.set_valign(Align::Start);
    viewport.set_child(Some(content));
    viewport.set_visible(false);
    root.append(&viewport);
    DashboardSection {
        viewport,
        content: content.clone(),
        trailing_gap,
    }
}

pub(super) struct HardwarePanel {
    pub(super) root: gtk::Box,
    pub(super) cpu: gtk::Label,
    pub(super) temperature: gtk::Label,
    pub(super) memory: gtk::Label,
    pub(super) memory_total: gtk::Label,
    pub(super) receive: gtk::Label,
    pub(super) transmit: gtk::Label,
    pub(super) cpu_progress: gtk::ProgressBar,
    pub(super) memory_progress: gtk::ProgressBar,
}

impl HardwarePanel {
    pub(super) fn update(&self, hardware: &crate::state::HardwareSnapshot) {
        self.cpu.set_label(
            &hardware
                .cpu_percent
                .map_or_else(|| "--%".to_owned(), |value| format!("{value:.0}%")),
        );
        self.temperature.set_label(
            &hardware
                .cpu_temperature_celsius
                .map_or_else(|| "--°C".to_owned(), |value| format!("{value:.0}°C")),
        );
        self.cpu_progress
            .set_fraction(hardware.cpu_percent.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0);
        match (hardware.memory_used_bytes, hardware.memory_total_bytes) {
            (Some(used), Some(total)) if total > 0 => {
                self.memory.set_label(&format_gib(used));
                self.memory_total
                    .set_label(&format!("/ {} GiB", format_gib(total)));
                self.memory_progress
                    .set_fraction((used as f64 / total as f64).clamp(0.0, 1.0));
            }
            _ => {
                self.memory.set_label("-- GiB");
                self.memory_total.set_label("/ -- GiB");
                self.memory_progress.set_fraction(0.0);
            }
        }
        self.receive
            .set_label(&format_rate(hardware.network_receive_bytes_per_second));
        self.transmit
            .set_label(&format_rate(hardware.network_transmit_bytes_per_second));
    }
}

fn format_gib(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1_073_741_824.0)
}

pub(super) fn hardware_panel(metrics: Metrics) -> HardwarePanel {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(8));
    root.add_css_class("hardware-panel");
    let stats = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    stats.set_homogeneous(true);
    stats.add_css_class("hardware-stats");
    let cpu_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(7));
    cpu_card.add_css_class("hardware-stat");
    cpu_card.set_hexpand(true);
    let ram_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(7));
    ram_card.add_css_class("hardware-stat");
    ram_card.set_hexpand(true);

    let cpu_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    let cpu_icon = hardware_icon("cpu");
    let cpu_name = hardware_name("CPU");
    let cpu = hardware_value("--%");
    let temperature = hardware_secondary("--°C");
    cpu_row.append(&cpu_icon);
    let cpu_text = gtk::Box::new(Orientation::Vertical, metrics.spacing(4));
    cpu_name.set_xalign(0.0);
    cpu_text.append(&cpu_name);
    let cpu_values = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    cpu_values.append(&cpu);
    cpu_values.append(&gtk::Separator::new(Orientation::Vertical));
    cpu_values.append(&temperature);
    cpu_text.append(&cpu_values);
    cpu_row.append(&cpu_text);
    let cpu_progress = hardware_progress();
    cpu_card.append(&cpu_row);
    cpu_card.append(&cpu_progress);

    let ram_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    let ram_icon = hardware_icon("ram");
    let ram_name = hardware_name("RAM");
    let memory = hardware_value("-- GiB");
    let memory_total = hardware_secondary("/ -- GiB");
    ram_row.append(&ram_icon);
    let ram_text = gtk::Box::new(Orientation::Vertical, metrics.spacing(4));
    ram_name.set_xalign(0.0);
    ram_text.append(&ram_name);
    let ram_values = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    ram_values.append(&memory);
    ram_values.append(&memory_total);
    ram_text.append(&ram_values);
    ram_row.append(&ram_text);
    let memory_progress = hardware_progress();
    ram_card.append(&ram_row);
    ram_card.append(&memory_progress);
    stats.append(&cpu_card);
    stats.append(&ram_card);

    let network = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    network.add_css_class("hardware-network");
    let net_name = hardware_name("NET");
    net_name.set_hexpand(true);
    let receive = hardware_value("--/s");
    let transmit = hardware_value("--/s");
    let down = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    down.set_hexpand(true);
    down.set_halign(Align::Center);
    down.add_css_class("hardware-rate");
    down.append(&hardware_icon("↓"));
    down.append(&receive);
    let up = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    up.set_hexpand(true);
    up.set_halign(Align::Center);
    up.add_css_class("hardware-rate");
    up.append(&hardware_icon("↑"));
    up.append(&transmit);
    network.append(&hardware_icon("⇅"));
    network.append(&net_name);
    network.append(&gtk::Separator::new(Orientation::Vertical));
    network.append(&down);
    network.append(&gtk::Separator::new(Orientation::Vertical));
    network.append(&up);
    root.append(&stats);
    root.append(&network);
    HardwarePanel {
        root,
        cpu,
        temperature,
        memory,
        memory_total,
        receive,
        transmit,
        cpu_progress,
        memory_progress,
    }
}

fn hardware_icon(kind: &str) -> gtk::Widget {
    if !matches!(kind, "cpu" | "ram") {
        let label = gtk::Label::new(Some(kind));
        label.add_css_class("hardware-icon");
        return label.upcast();
    }
    let cpu = kind == "cpu";
    let area = gtk::DrawingArea::new();
    area.add_css_class("hardware-icon");
    area.set_content_width(28);
    area.set_content_height(28);
    area.set_valign(Align::Center);
    area.set_draw_func(move |area, cr, w, h| {
        cr.scale(f64::from(w) / 24.0, f64::from(h) / 24.0);
        let color = area.color();
        cr.set_source_rgba(
            color.red().into(),
            color.green().into(),
            color.blue().into(),
            1.0,
        );
        cr.set_line_width(1.6);
        if cpu {
            cr.rectangle(6.0, 6.0, 12.0, 12.0);
            cr.rectangle(9.0, 9.0, 6.0, 6.0);
            for p in [9.0, 12.0, 15.0] {
                for (x1, y1, x2, y2) in [
                    (p, 2.0, p, 6.0),
                    (p, 18.0, p, 22.0),
                    (2.0, p, 6.0, p),
                    (18.0, p, 22.0, p),
                ] {
                    cr.move_to(x1, y1);
                    cr.line_to(x2, y2);
                }
            }
        } else {
            cr.rectangle(2.0, 7.0, 20.0, 10.0);
            for x in [6.0, 10.0, 14.0, 18.0] {
                cr.move_to(x, 9.0);
                cr.line_to(x, 12.0);
                cr.move_to(x, 15.0);
                cr.line_to(x, 19.0);
            }
        }
        let _ = cr.stroke();
    });
    area.upcast()
}

fn hardware_name(name: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(name));
    label.add_css_class("hardware-name");
    label
}

fn hardware_value(value: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(value));
    label.add_css_class("hardware-value");
    label
}

fn hardware_secondary(value: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(value));
    label.add_css_class("hardware-secondary");
    label
}

fn hardware_progress() -> gtk::ProgressBar {
    let progress = gtk::ProgressBar::new();
    progress.add_css_class("hardware-progress");
    progress
}

pub(super) fn dashboard_view(metrics: Metrics) -> DashboardWidgets {
    // Section gaps roll away with their clipped content, rather than remaining
    // fixed until a disappearing child is abruptly removed from the box.
    let root = gtk::Box::new(Orientation::Vertical, 0);
    root.set_size_request(-1, -1);
    root.add_css_class("dashboard-content");
    root.set_valign(Align::Start);

    // The identity label fills the header beside its navigation controls.
    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(9));
    header.add_css_class("island-drop-header");
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
    heading.append(&eyebrow);

    let close_button = icon::icon_button(Icon::Close, metrics.icons);
    close_button.add_css_class("close-button");
    close_button.set_valign(Align::Center);
    let search_button = icon::icon_button(Icon::Search, metrics.icons);
    search_button.add_css_class("close-button");
    search_button.set_tooltip_text(Some("Search with TarraGon"));
    search_button.set_valign(Align::Center);
    let weather_button = icon::icon_button(Icon::WeatherClear, metrics.icons);
    weather_button.add_css_class("close-button");
    weather_button.set_tooltip_text(Some("Weather forecast"));
    weather_button.set_valign(Align::Center);
    header.append(&heading);
    header.append(&weather_button);
    header.append(&search_button);
    header.append(&close_button);
    let header_section = dashboard_section(&root, &header, true);

    let hardware = hardware_panel(metrics);
    root.append(&hardware.root);

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
    let status_section = dashboard_section(&root, &status_card, true);

    // Volume rides directly on the dashboard as a thin row.
    let controls_stack = gtk::Box::new(Orientation::Vertical, 0);
    controls_stack.add_css_class("control-stack");
    let volume_row = gtk::Box::new(Orientation::Horizontal, metrics.spacing(8));
    volume_row.add_css_class("control-row");
    let mute_button = icon::icon_button(Icon::VolumeHigh, metrics.icons);
    mute_button.add_css_class("close-button");
    mute_button.set_tooltip_text(Some("Mute or unmute audio"));
    let volume_scale = gtk::Scale::with_range(Orientation::Horizontal, 0.0, 100.0, 1.0);
    volume_scale.set_draw_value(false);
    volume_scale.set_hexpand(true);
    volume_scale.add_css_class("control-scale");
    let volume_value = gtk::Label::new(Some("--"));
    volume_value.add_css_class("control-value");
    volume_row.append(&mute_button);
    volume_row.append(&volume_scale);
    volume_row.append(&volume_value);
    controls_stack.append(&volume_row);
    let controls_section = dashboard_section(&root, &controls_stack, true);

    // The densest section, and the only one that grows: it absorbs whatever
    // the fixed-height dashboard has left after the rows above.
    let notification_card = gtk::Box::new(Orientation::Vertical, metrics.spacing(6));
    notification_card.add_css_class("notification-card");

    let notification_header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(6));
    let notification_title = gtk::Label::new(Some("NOTIFICATIONS"));
    notification_title.add_css_class("eyebrow");
    notification_title.set_halign(Align::Start);
    notification_title.set_hexpand(true);
    let notification_count = gtk::Label::new(Some("0"));
    notification_count.add_css_class("notification-count");
    notification_count.set_valign(Align::Center);
    let notification_inhibit_remaining = gtk::Label::new(None);
    notification_inhibit_remaining.add_css_class("notification-inhibit-remaining");
    notification_inhibit_remaining.set_valign(Align::Center);
    notification_inhibit_remaining.set_visible(false);
    let notification_clear_button = icon::icon_button(Icon::ClearAll, metrics.icons);
    notification_clear_button.add_css_class("close-button");
    notification_clear_button.set_tooltip_text(Some("Clear notification history"));
    notification_clear_button.set_sensitive(false);
    let notification_inhibit_button = gtk::ToggleButton::new();
    icon::set_button_icon(&notification_inhibit_button, Icon::BellOff, metrics.icons);
    notification_inhibit_button.add_css_class("close-button");
    notification_inhibit_button.set_tooltip_text(Some("Inhibit notifications"));
    notification_header.append(&notification_title);
    notification_header.append(&notification_clear_button);
    notification_header.append(&notification_inhibit_remaining);
    notification_header.append(&notification_inhibit_button);
    notification_header.append(&notification_count);
    notification_card.append(&notification_header);

    let notification_list = gtk::Box::new(Orientation::Vertical, metrics.spacing(4));
    notification_list.add_css_class("notification-list");
    notification_list.set_vexpand(false);
    // Replaced by `update_notification_history` as soon as the controller
    // pushes its first (possibly empty) history snapshot.
    let notification_placeholder = gtk::Label::new(Some("All caught up"));
    notification_placeholder.add_css_class("muted-label");
    notification_placeholder.add_css_class("notification-empty");
    notification_placeholder.set_halign(Align::Center);
    notification_placeholder.set_wrap(true);
    notification_list.append(&notification_placeholder);
    notification_card.append(&notification_list);
    let notification_section = dashboard_section(&root, &notification_card, false);

    DashboardWidgets {
        root,
        sections: vec![
            header_section,
            status_section,
            controls_section,
            notification_section,
        ],
        hardware,
        active_eyebrow,
        active_title,
        workspace_row,
        volume_scale,
        volume_value,
        notification_count,
        notification_inhibit_remaining,
        notification_clear_button,
        notification_inhibit_button,
        notification_list,
        weather_button,
        search_button,
        close_button,
        mute_button,
    }
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

        for (index, workspace) in workspaces.into_iter().take(10).enumerate() {
            if index == 5 && shown == 9 {
                let spacer = gtk::Button::new();
                spacer.add_css_class("workspace-button");
                spacer.set_opacity(0.0);
                spacer.set_can_target(false);
                spacer.set_focusable(false);
                self.workspace_row.insert(&spacer, -1);
            }
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
        self.reconcile_pill_geometry();
    }

    pub fn update_system(self: &Rc<Self>, snapshot: &SystemSnapshot) {
        self.updating_controls.set(true);
        let hardware = &snapshot.hardware;
        self.hardware.update(hardware);
        if let Some(audio) = snapshot.audio {
            self.volume_scale.set_value(f64::from(audio.percent));
            self.volume_value.set_label(&if audio.muted {
                "Muted".into()
            } else {
                format!("{}%", audio.percent)
            });
            icon::set_button_icon(
                &self.mute_button,
                if audio.muted {
                    Icon::VolumeMuted
                } else {
                    Icon::VolumeHigh
                },
                self.metrics.icons,
            );
            self.mute_button.set_tooltip_text(Some(if audio.muted {
                "Unmute audio"
            } else {
                "Mute audio"
            }));
            self.mute_button.set_sensitive(true);
            self.volume_scale.set_sensitive(true);
        } else {
            self.volume_value.set_label("--");
            self.mute_button.set_sensitive(false);
            self.volume_scale.set_sensitive(false);
        }

        if let Some(battery) = &snapshot.battery {
            self.compact_battery
                .set_label(&format!("{}%", battery.percent));
            self.compact_battery.set_visible(true);
            self.update_battery_wave(Some(battery.percent));
        } else {
            self.compact_battery.set_visible(false);
            self.update_battery_wave(None);
        }
        self.updating_controls.set(false);
        self.resize_compact();
        self.reconcile_pill_geometry();
    }
}

fn format_rate(rate: Option<f64>) -> String {
    match rate {
        Some(rate) if rate >= 1_048_576.0 => format!("{:.1} MiB/s", rate / 1_048_576.0),
        Some(rate) if rate >= 1024.0 => format!("{:.0} KiB/s", rate / 1024.0),
        Some(rate) => format!("{rate:.0} B/s"),
        None => "--".to_owned(),
    }
}
