//! The TarraGon launcher view: results, plugin inventory, previews, and
//! the dispatch/throttle pipeline feeding them.

use super::*;

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use gtk::{Align, Orientation, glib};

use super::{IslandWindow, Metrics};
use crate::preview::{HIGHLIGHT_NAMES, PreviewContent, PreviewData};
use crate::tarragon::{
    TarragonAction, TarragonPlugin, TarragonPluginState, TarragonSelection, TarragonSnapshot,
    TarragonStatus,
};

const SEARCH_RESULTS_MIN_WIDTH: i32 = 340;
const SEARCH_PREVIEW_MIN_WIDTH: i32 = 260;

/// Minimum spacing between dispatched queries.
///
/// This throttles on the leading edge: the first keystroke after an idle
/// period is sent immediately, and only a burst faster than this window (held
/// keys repeating, or a very fast typist) is coalesced. A trailing debounce
/// would instead tax every keystroke, which is pure cost given TarraGon
/// answers in well under a millisecond.
const SEARCH_THROTTLE: Duration = Duration::from_millis(16);
/// How long a selection must hold still before its file preview is loaded.
/// Previews can spawn ffprobe or ffmpegthumbnailer, so unlike a query they are
/// far too expensive to issue for every intermediate row.
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(80);

pub(super) struct SearchWidgets {
    pub(super) root: gtk::Box,
    pub(super) entry: gtk::SearchEntry,
    pub(super) results: gtk::ListBox,
    pub(super) status: gtk::Label,
    pub(super) back_button: gtk::Button,
    pub(super) reload_button: gtk::Button,
    pub(super) plugin_toggle: gtk::ToggleButton,
    pub(super) stack: gtk::Stack,
    pub(super) plugins: gtk::ListBox,
    pub(super) preview_stack: gtk::Stack,
    pub(super) preview_picture: gtk::Picture,
    pub(super) preview_icon: gtk::Box,
    pub(super) preview_title: gtk::Label,
    pub(super) preview_description: gtk::Label,
    pub(super) preview_file_meta: gtk::Label,
    pub(super) preview_meta: gtk::Label,
    pub(super) preview_text: gtk::TextView,
    pub(super) preview_text_scroll: gtk::ScrolledWindow,
    pub(super) preview_error: gtk::Label,
}

pub(super) fn search_view(metrics: Metrics) -> SearchWidgets {
    let root = gtk::Box::new(Orientation::Vertical, metrics.spacing(10));
    root.set_size_request(metrics.search_width, metrics.search_height);
    root.add_css_class("search-content");
    root.set_valign(Align::Start);

    let header = gtk::Box::new(Orientation::Horizontal, metrics.spacing(9));
    let back_button = icon::icon_button(Icon::Back, metrics.icons);
    back_button.add_css_class("close-button");
    back_button.set_tooltip_text(Some("Back to dashboard"));
    let entry = gtk::SearchEntry::new();
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("Search apps, files, commands, and plugins"));
    entry.add_css_class("tarragon-search");
    let plugin_toggle = gtk::ToggleButton::with_label("PLUGINS");
    plugin_toggle.add_css_class("search-header-button");
    plugin_toggle.set_tooltip_text(Some("Show loaded TarraGon plugins"));
    let reload_button = icon::icon_button(Icon::Refresh, metrics.icons);
    reload_button.add_css_class("close-button");
    reload_button.set_tooltip_text(Some("Reload TarraGon configuration and plugins"));
    header.append(&back_button);
    header.append(&entry);
    header.append(&plugin_toggle);
    header.append(&reload_button);
    root.append(&header);

    let status = gtk::Label::new(Some("TARRAGON OFFLINE"));
    status.add_css_class("search-status");
    status.set_halign(Align::Start);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    root.append(&status);

    let results = gtk::ListBox::new();
    results.add_css_class("search-results");
    results.set_selection_mode(gtk::SelectionMode::Single);
    results.set_activate_on_single_click(true);
    results.set_vexpand(true);
    let results_scroller = gtk::ScrolledWindow::new();
    results_scroller.add_css_class("search-results-scroll");
    results_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    results_scroller.set_vexpand(true);
    results_scroller.set_size_request(metrics.spacing(SEARCH_RESULTS_MIN_WIDTH), -1);
    results_scroller.set_child(Some(&results));

    let preview = gtk::Box::new(Orientation::Vertical, metrics.spacing(9));
    preview.add_css_class("search-preview");
    preview.set_size_request(metrics.spacing(SEARCH_PREVIEW_MIN_WIDTH), -1);
    let preview_title = gtk::Label::new(Some("Select a result"));
    preview_title.add_css_class("search-preview-title");
    preview_title.set_halign(Align::Start);
    preview_title.set_wrap(true);
    preview_title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    let preview_description = gtk::Label::new(None);
    preview_description.add_css_class("search-preview-description");
    preview_description.set_halign(Align::Start);
    preview_description.set_wrap(true);
    preview_description.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    preview_description.set_lines(3);
    let preview_file_meta = gtk::Label::new(None);
    preview_file_meta.add_css_class("search-preview-file-meta");
    preview_file_meta.set_halign(Align::Start);
    preview_file_meta.set_wrap(true);
    let preview_meta = gtk::Label::new(Some("TarraGon aggregate results"));
    preview_meta.add_css_class("search-preview-meta");
    preview_meta.set_halign(Align::Start);
    preview_meta.set_wrap(true);
    preview.append(&preview_title);
    preview.append(&preview_description);
    preview.append(&preview_file_meta);
    preview.append(&preview_meta);

    let preview_stack = gtk::Stack::new();
    preview_stack.set_vhomogeneous(false);
    preview_stack.set_vexpand(true);
    preview_stack.set_size_request(-1, metrics.spacing(190));
    let preview_picture = gtk::Picture::new();
    preview_picture.set_content_fit(gtk::ContentFit::Contain);
    preview_picture.add_css_class("search-preview-picture");
    // A slot rather than a widget: the preview alternates between one of our
    // own glyphs and an image named by TarraGon, and those cannot be the same
    // widget type. The CSS class lives on the slot so the size and color rules
    // inherit down to whichever child is currently in it.
    let preview_icon = gtk::Box::new(Orientation::Vertical, 0);
    preview_icon.add_css_class("search-preview-icon");
    preview_icon.set_halign(Align::Center);
    preview_icon.set_valign(Align::Center);
    set_preview_chrome_icon(&preview_icon, Icon::Search, metrics.icons);
    let preview_text = gtk::TextView::new();
    preview_text.add_css_class("search-preview-text");
    preview_text.set_editable(false);
    preview_text.set_cursor_visible(false);
    preview_text.set_monospace(true);
    preview_text.set_wrap_mode(gtk::WrapMode::None);
    preview_text.set_left_margin(metrics.spacing(9));
    preview_text.set_right_margin(metrics.spacing(9));
    preview_text.set_top_margin(metrics.spacing(8));
    preview_text.set_bottom_margin(metrics.spacing(8));
    let preview_text_scroll = gtk::ScrolledWindow::new();
    preview_text_scroll.add_css_class("search-preview-text-scroll");
    preview_text_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    preview_text_scroll.set_child(Some(&preview_text));
    let preview_error = gtk::Label::new(None);
    preview_error.add_css_class("search-preview-error");
    preview_error.set_wrap(true);
    preview_error.set_halign(Align::Center);
    preview_error.set_valign(Align::Center);
    preview_stack.add_named(&preview_picture, Some("picture"));
    preview_stack.add_named(&preview_icon, Some("icon"));
    preview_stack.add_named(&preview_text_scroll, Some("text"));
    preview_stack.add_named(&preview_error, Some("error"));
    preview_stack.set_visible_child_name("icon");
    preview.append(&preview_stack);

    let result_page = gtk::Box::new(Orientation::Horizontal, metrics.spacing(12));
    results_scroller.set_hexpand(true);
    result_page.append(&results_scroller);
    result_page.append(&preview);

    let plugins = gtk::ListBox::new();
    plugins.add_css_class("plugin-list");
    plugins.set_selection_mode(gtk::SelectionMode::None);
    let plugin_scroller = gtk::ScrolledWindow::new();
    plugin_scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    plugin_scroller.set_vexpand(true);
    plugin_scroller.set_child(Some(&plugins));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(160);
    stack.set_vexpand(true);
    stack.add_named(&result_page, Some("results"));
    stack.add_named(&plugin_scroller, Some("plugins"));
    stack.set_visible_child_name("results");
    root.append(&stack);

    SearchWidgets {
        root,
        entry,
        results,
        status,
        back_button,
        reload_button,
        plugin_toggle,
        stack,
        plugins,
        preview_stack,
        preview_picture,
        preview_icon,
        preview_title,
        preview_description,
        preview_file_meta,
        preview_meta,
        preview_text,
        preview_text_scroll,
        preview_error,
    }
}

/// Swaps the single child of a preview icon slot.
fn replace_slot_child(slot: &gtk::Box, child: &impl IsA<gtk::Widget>) {
    while let Some(existing) = slot.first_child() {
        slot.remove(&existing);
    }
    slot.append(child);
}

/// Shows one of mithshell's own icons in the preview slot.
fn set_preview_chrome_icon(slot: &gtk::Box, icon: Icon, style: IconStyle) {
    replace_slot_child(slot, &icon::icon_widget(icon, style));
}

/// Shows an icon named by TarraGon in the preview slot.
fn set_preview_foreign_icon(slot: &gtk::Box, name: &str) {
    replace_slot_child(slot, &icon::foreign_image(Some(name), Icon::Loading));
}

fn search_result_row(
    result: &crate::tarragon::TarragonResult,
    metrics: Metrics,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("search-result");
    let content = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));

    let icon = icon::foreign_image(Some(result.icon.as_str()), Icon::Search);
    icon.add_css_class("search-result-icon");
    icon.set_valign(Align::Center);
    content.append(&icon);

    let text = gtk::Box::new(Orientation::Vertical, 0);
    text.set_hexpand(true);
    let label = gtk::Label::new(Some(if result.label.is_empty() {
        &result.id
    } else {
        &result.label
    }));
    label.add_css_class("search-result-title");
    label.set_halign(Align::Start);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let description = gtk::Label::new(Some(&result.description));
    description.add_css_class("search-result-description");
    description.set_halign(Align::Start);
    description.set_ellipsize(gtk::pango::EllipsizeMode::End);
    description.set_visible(!result.description.is_empty());
    text.append(&label);
    text.append(&description);
    content.append(&text);

    let source = if result.category.is_empty() {
        result.plugin.as_str()
    } else {
        result.category.as_str()
    };
    let plugin = gtk::Label::new(Some(source));
    plugin.add_css_class("search-result-plugin");
    plugin.set_valign(Align::Center);
    content.append(&plugin);
    row.set_child(Some(&content));
    row
}

fn plugin_status_row(
    plugin: &TarragonPlugin,
    query: Option<&TarragonPluginState>,
    metrics: Metrics,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("plugin-row");
    let content = gtk::Box::new(Orientation::Horizontal, metrics.spacing(11));
    let icon = icon::foreign_image(Some(plugin.icon.as_str()), Icon::Addon);
    icon.add_css_class("plugin-icon");
    content.append(&icon);

    let details = gtk::Box::new(Orientation::Vertical, metrics.spacing(2));
    details.set_hexpand(true);
    let name = gtk::Label::new(Some(&plugin.name));
    name.add_css_class("plugin-name");
    name.set_halign(Align::Start);
    let description = gtk::Label::new(Some(&plugin.description));
    description.add_css_class("plugin-description");
    description.set_halign(Align::Start);
    description.set_ellipsize(gtk::pango::EllipsizeMode::End);
    description.set_visible(!plugin.description.is_empty());
    let mut metadata = vec![plugin.lifecycle.clone()];
    if !plugin.prefix.is_empty() {
        metadata.push(format!("prefix {}", plugin.prefix));
    }
    if plugin.require_prefix {
        metadata.push("prefix required".into());
    }
    if plugin.provides_general_suggestions {
        metadata.push("general".into());
    }
    if !plugin.source.is_empty() {
        metadata.push(plugin.source.clone());
    }
    if !plugin.capabilities.is_empty() {
        metadata.push(plugin.capabilities.join(", "));
    }
    let metadata = gtk::Label::new(Some(&metadata.join("  //  ")));
    metadata.add_css_class("plugin-metadata");
    metadata.set_halign(Align::Start);
    metadata.set_ellipsize(gtk::pango::EllipsizeMode::End);
    details.append(&name);
    details.append(&description);
    details.append(&metadata);
    content.append(&details);

    let availability = if !plugin.enabled {
        "DISABLED".to_owned()
    } else if let Some(query) = query {
        match query.state.as_str() {
            "pending" => "PENDING".into(),
            "done" => format!("{} RESULTS  //  {:.1} MS", query.count, query.elapsed_ms),
            "empty" => format!("EMPTY  //  {:.1} MS", query.elapsed_ms),
            "error" => {
                if query.error.is_empty() {
                    "ERROR".into()
                } else {
                    format!("ERROR  //  {}", query.error)
                }
            }
            state => state.to_uppercase(),
        }
    } else if plugin.lifecycle == "on_call" {
        "ON CALL".into()
    } else if plugin.connected {
        "CONNECTED".into()
    } else if plugin.lifecycle == "on_demand_persistent" {
        "IDLE".into()
    } else {
        "DISCONNECTED".into()
    };
    let state = gtk::Label::new(Some(&availability));
    state.add_css_class("plugin-state");
    if query.is_some_and(|query| query.state == "error") || !plugin.enabled {
        state.add_css_class("error");
    } else if plugin.connected || plugin.lifecycle == "on_call" {
        state.add_css_class("available");
    }
    state.set_valign(Align::Center);
    state.set_max_width_chars(30);
    state.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&state);
    row.set_child(Some(&content));
    row
}

fn format_preview_metadata(metadata: &[(String, String)]) -> String {
    metadata
        .chunks(2)
        .map(|fields| {
            fields
                .iter()
                .map(|(name, value)| format!("{} {}", name.to_uppercase(), value))
                .collect::<Vec<_>>()
                .join("  //  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn highlight_color(name: &str) -> &'static str {
    match name.split('.').next().unwrap_or(name) {
        "comment" => "#7f849c",
        "keyword" | "conditional" | "exception" => "#cba6f7",
        "string" => "#a6e3a1",
        "number" | "boolean" | "constant" => "#fab387",
        "function" | "method" | "constructor" => "#89b4fa",
        "type" | "module" | "namespace" => "#f9e2af",
        "property" | "attribute" => "#94e2d5",
        "tag" | "label" => "#f38ba8",
        "operator" | "punctuation" => "#bac2de",
        _ => "#cdd6f4",
    }
}

impl IslandWindow {
    pub fn update_tarragon_connection(&self, connected: bool, message: Option<&str>) {
        self.search_connected.set(connected);
        self.search_entry.set_sensitive(connected);
        self.search_plugin_toggle.set_sensitive(connected);
        if connected {
            self.search_status.set_label("READY  //  TYPE TO SEARCH");
        } else {
            self.search_selection_pending.set(false);
            self.search_action_generation
                .set(self.search_action_generation.get().wrapping_add(1));
            self.search_status
                .set_label(message.unwrap_or("TARRAGON OFFLINE"));
        }
    }

    pub fn update_tarragon_results(self: &Rc<Self>, snapshot: &TarragonSnapshot) {
        // Match the query this island actually asked for. Comparing against the
        // live entry text instead would drop every snapshot whenever the user
        // typed while results were in flight, pinning the UI on "SEARCHING".
        if self.search_dispatched.borrow().as_deref() != Some(snapshot.input.as_str()) {
            return;
        }
        let selected_index = self
            .search_results
            .selected_row()
            .map_or(0, |row| row.index());
        *self.search_snapshot.borrow_mut() = Some(snapshot.clone());
        while let Some(child) = self.search_results.first_child() {
            self.search_results.remove(&child);
        }

        for result in &snapshot.list {
            let row = search_result_row(result, self.metrics);
            let click = gtk::GestureClick::new();
            click.set_button(gtk::gdk::BUTTON_SECONDARY);
            let weak = Rc::downgrade(self);
            let row_for_handler = row.clone();
            click.connect_pressed(move |_, _, _, _| {
                if let Some(island) = weak.upgrade() {
                    island.search_results.select_row(Some(&row_for_handler));
                    island.open_search_actions(row_for_handler.index(), &row_for_handler);
                }
            });
            row.add_controller(click);
            self.search_results.append(&row);
        }
        let selected_index = selected_index.min(snapshot.list.len().saturating_sub(1) as i32);
        if let Some(row) = self.search_results.row_at_index(selected_index) {
            self.search_results.select_row(Some(&row));
        } else {
            self.clear_search_preview();
        }

        let pending = snapshot
            .plugins
            .values()
            .filter(|plugin| plugin.state == "pending")
            .count();
        let errors = snapshot
            .plugins
            .values()
            .filter(|plugin| plugin.state == "error")
            .count();
        let completed = snapshot.plugins.len().saturating_sub(pending);
        let elapsed = snapshot
            .plugins
            .values()
            .map(|plugin| plugin.elapsed_ms)
            .fold(0.0, f64::max);
        let mut status = format!(
            "{} RESULTS  //  {completed}/{} PLUGINS",
            snapshot.list.len(),
            snapshot.plugins.len()
        );
        if pending > 0 {
            status.push_str(&format!("  //  {pending} PENDING"));
        }
        if errors > 0 {
            status.push_str(&format!("  //  {errors} ERRORS"));
        }
        if pending == 0 && elapsed > 0.0 {
            status.push_str(&format!("  //  {elapsed:.1} MS"));
        }
        if snapshot.list.is_empty() && pending == 0 {
            status = format!("NO RESULTS  //  {completed} PLUGINS COMPLETE");
        }
        self.search_status.set_label(&status);
        self.render_plugin_list();
        // t4: main-thread widget work for this snapshot is complete.
        crate::latency::mark_build();
    }

    pub fn update_tarragon_status(self: &Rc<Self>, status: &TarragonStatus) {
        *self.search_backend_status.borrow_mut() = Some(status.clone());
        self.render_plugin_list();
        if self.search_plugin_toggle.is_active() {
            self.show_plugin_summary();
        } else if self.search_open.get() {
            let snapshot = self.search_snapshot.borrow().clone();
            if let Some(snapshot) = snapshot {
                self.update_tarragon_results(&snapshot);
            } else {
                self.search_status.set_label("READY  //  TYPE TO SEARCH");
            }
        }
    }

    pub fn update_tarragon_reload(&self, success: bool, message: &str) {
        if self.search_open.get() {
            self.search_status.set_label(if success {
                "TARRAGON RELOADED  //  REFRESHING STATUS"
            } else {
                message
            });
        }
    }

    pub fn update_tarragon_selection(self: &Rc<Self>, success: bool, message: &str) {
        self.search_selection_pending.set(false);
        if success && self.search_open.get() {
            self.close();
        } else if !success && self.search_open.get() {
            self.search_status.set_label(message);
        }
    }

    pub(super) fn render_plugin_list(&self) {
        // The plugin pane is a separate stack page. Rebuilding roughly seven
        // widgets per plugin on every streamed snapshot is invisible work when
        // the pane is not on screen, and TarraGon sends one snapshot per plugin
        // completion.
        if !self.search_plugin_toggle.is_active() {
            return;
        }
        clear_list_box(&self.search_plugins);
        let status = self.search_backend_status.borrow();
        let Some(status) = status.as_ref() else {
            return;
        };
        let snapshot = self.search_snapshot.borrow();
        for plugin in &status.plugins {
            let query_state = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.plugins.get(&plugin.name));
            self.search_plugins
                .append(&plugin_status_row(plugin, query_state, self.metrics));
        }
    }

    pub(super) fn show_plugin_summary(&self) {
        let status = self.search_backend_status.borrow();
        let Some(status) = status.as_ref() else {
            self.search_status.set_label("LOADING PLUGIN STATUS");
            return;
        };
        let enabled = status
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled)
            .count();
        let on_call = status
            .plugins
            .iter()
            .filter(|plugin| plugin.enabled && plugin.lifecycle == "on_call")
            .count();
        self.search_status.set_label(&format!(
            "{} DISCOVERED  //  {enabled} ENABLED  //  {} CONNECTED  //  {on_call} ON CALL",
            status.plugins.len(),
            status.connected.len()
        ));
    }

    pub(super) fn clear_search_preview(&self) {
        self.search_preview_key.borrow_mut().take();
        self.preview_generation
            .set(self.preview_generation.get().wrapping_add(1));
        self.search_preview_stack.set_visible_child_name("icon");
        set_preview_chrome_icon(&self.search_preview_icon, Icon::Search, self.metrics.icons);
        self.search_preview_picture
            .set_filename(None::<&std::path::Path>);
        self.search_preview_text.buffer().set_text("");
        self.search_preview_file_meta.set_label("");
        self.search_preview_error.set_label("");
        self.search_preview_title.set_label("Select a result");
        self.search_preview_description.set_label("");
        self.search_preview_meta
            .set_label("TarraGon aggregate results");
    }

    pub(super) fn update_search_preview(self: &Rc<Self>, index: i32) {
        let Some(result) = self
            .search_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.list.get(index.max(0) as usize))
            .cloned()
        else {
            self.clear_search_preview();
            return;
        };

        let title = if result.label.is_empty() {
            &result.id
        } else {
            &result.label
        };
        self.search_preview_title.set_label(title);
        self.search_preview_description
            .set_label(&result.description);
        self.search_preview_description
            .set_visible(!result.description.is_empty());

        let preview_key = format!("{}\0{}\0{}", result.plugin, result.id, result.preview_path);
        let old_key = self.search_preview_key.replace(Some(preview_key.clone()));
        let preview_changed = old_key.as_deref() != Some(preview_key.as_str());

        let category = if result.category.is_empty() {
            "uncategorized"
        } else {
            &result.category
        };
        let mut meta = format!(
            "{}  //  {}\nSCORE {:.3}  //  FRECENCY {:.3}",
            result.plugin, category, result.score, result.frecency_score
        );
        if !result.preview_path.is_empty() {
            meta.push_str(&format!("\n{}", result.preview_path));
        }
        self.search_preview_meta.set_label(&meta);
        if preview_changed {
            self.search_preview_picture
                .set_filename(None::<&std::path::Path>);
            self.search_preview_text.buffer().set_text("");
            self.search_preview_error.set_label("");
            if result.icon.is_empty() {
                set_preview_chrome_icon(
                    &self.search_preview_icon,
                    Icon::Loading,
                    self.metrics.icons,
                );
            } else {
                set_preview_foreign_icon(&self.search_preview_icon, &result.icon);
            }
            self.search_preview_stack.set_visible_child_name("icon");
            let generation = self.preview_generation.get().wrapping_add(1);
            self.preview_generation.set(generation);
            if result.preview_path.is_empty() {
                self.search_preview_file_meta.set_label("NO FILE PREVIEW");
            } else {
                self.search_preview_file_meta
                    .set_label("LOADING FILE METADATA");
                // Held arrow keys walk the list faster than a preview can be
                // produced, so wait out the burst before touching the disk.
                let weak = Rc::downgrade(self);
                let path = result.preview_path.clone();
                glib::timeout_add_local_once(PREVIEW_DEBOUNCE, move || {
                    if let Some(island) = weak.upgrade()
                        && island.preview_generation.get() == generation
                    {
                        (island.actions.load_preview)(generation, path);
                    }
                });
            }
        }
    }

    /// Shows the selected result's non-default actions in a transient menu.
    /// Keeping the popover outside the preview layout leaves the preview's
    /// vertical space available for the actual file content.
    pub(super) fn open_search_actions(self: &Rc<Self>, index: i32, anchor: &impl IsA<gtk::Widget>) {
        let Some(result) = self
            .search_snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.list.get(index.max(0) as usize))
            .cloned()
        else {
            return;
        };
        let default_name = result.default_action().map(|action| action.name.as_str());
        let alternatives: Vec<_> = result
            .actions
            .iter()
            .filter(|action| !action.name.is_empty() && Some(action.name.as_str()) != default_name)
            .cloned()
            .collect();
        if alternatives.is_empty() {
            return;
        }

        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        popover.add_css_class("search-action-menu");
        popover.set_position(gtk::PositionType::Bottom);
        popover.set_has_arrow(false);
        popover.set_autohide(true);

        let menu = gtk::Box::new(Orientation::Vertical, 2);
        menu.add_css_class("search-action-menu-list");
        for action in alternatives {
            let label = if action.description.is_empty() {
                action.name.clone()
            } else {
                action.description.clone()
            };
            let button = gtk::Button::new();
            button.add_css_class("search-action-menu-item");
            button.set_has_frame(false);
            let text = gtk::Label::new(Some(&label));
            text.set_halign(Align::Start);
            text.set_xalign(0.0);
            button.set_child(Some(&text));
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(island) = weak.upgrade() {
                    island.execute_search_action(index, &action);
                }
            });
            menu.append(&button);
        }
        popover.set_child(Some(&menu));

        popover.connect_closed(|popover| {
            popover.unparent();
        });
        popover.popup();
    }

    pub fn apply_file_preview(&self, generation: u64, result: Result<PreviewData, String>) {
        if self.preview_generation.get() != generation {
            return;
        }
        let data = match result {
            Ok(data) => data,
            Err(error) => {
                self.search_preview_file_meta.set_label("PREVIEW ERROR");
                self.search_preview_error.set_label(&error);
                self.search_preview_stack.set_visible_child_name("error");
                return;
            }
        };
        self.search_preview_file_meta
            .set_label(&format_preview_metadata(&data.metadata));
        match data.content {
            PreviewContent::Text { text, highlights } => {
                let buffer = self.search_preview_text.buffer();
                buffer.set_text(&text);
                for span in highlights {
                    let Some(name) = HIGHLIGHT_NAMES.get(span.style) else {
                        continue;
                    };
                    let tag_name = format!("mithshell-highlight-{}", name.replace('.', "-"));
                    let table = buffer.tag_table();
                    let tag = table.lookup(&tag_name).unwrap_or_else(|| {
                        let tag = gtk::TextTag::new(Some(&tag_name));
                        tag.set_foreground(Some(highlight_color(name)));
                        table.add(&tag);
                        tag
                    });
                    let start = buffer.iter_at_offset(span.start);
                    let end = buffer.iter_at_offset(span.end);
                    buffer.apply_tag(&tag, &start, &end);
                }
                let scroller = self.search_preview_text_scroll.clone();
                glib::idle_add_local_once(move || {
                    scroller.hadjustment().set_value(0.0);
                    scroller.vadjustment().set_value(0.0);
                });
                self.search_preview_stack.set_visible_child_name("text");
            }
            PreviewContent::Image(path) | PreviewContent::VideoThumbnail(path) => {
                self.search_preview_picture.set_filename(Some(path));
                self.search_preview_stack.set_visible_child_name("picture");
            }
            PreviewContent::Generic => {
                self.search_preview_stack.set_visible_child_name("icon");
            }
        }
    }

    pub fn open_search(self: &Rc<Self>) {
        self.clear_osd();
        self.dashboard_open.set(false);
        self.weather_open.set(false);
        self.search_open.set(true);
        self.search_plugin_toggle.set_active(false);
        self.search_stack.set_visible_child_name("results");
        self.search_entry.set_text("");
        while let Some(child) = self.search_results.first_child() {
            self.search_results.remove(&child);
        }
        *self.search_snapshot.borrow_mut() = None;
        self.clear_search_preview();
        if self.search_connected.get() {
            self.search_status
                .set_label(if self.search_selection_pending.get() {
                    "ACTION STILL PENDING"
                } else {
                    "READY  //  TYPE TO SEARCH"
                });
            (self.actions.tarragon_status)();
        }
        self.reconcile_view();
        let entry = self.search_entry.clone();
        glib::idle_add_local_once(move || {
            entry.grab_focus();
        });
    }

    pub(super) fn schedule_search(self: &Rc<Self>, text: String) {
        let generation = self.search_generation.get().wrapping_add(1);
        self.search_generation.set(generation);
        if !self.search_connected.get() {
            return;
        }
        if text.trim().is_empty() {
            *self.search_dispatched.borrow_mut() = None;
            *self.search_snapshot.borrow_mut() = None;
            clear_list_box(&self.search_results);
            self.clear_search_preview();
            self.search_status.set_label("READY  //  TYPE TO SEARCH");
            return;
        }
        self.search_status.set_label("SEARCHING");

        // Leading edge: nothing dispatched recently, so send immediately.
        let now = Instant::now();
        let ready = self
            .last_search_dispatch
            .get()
            .is_none_or(|last| now.duration_since(last) >= SEARCH_THROTTLE);
        if ready {
            self.dispatch_search(text);
            return;
        }

        // Inside the window: coalesce until it closes. The generation check
        // means only the final keystroke of the burst is actually sent.
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(SEARCH_THROTTLE, move || {
            if let Some(island) = weak.upgrade()
                && island.search_open.get()
                && island.search_generation.get() == generation
            {
                island.dispatch_search(text);
            }
        });
    }

    fn dispatch_search(self: &Rc<Self>, text: String) {
        crate::latency::mark_dispatch();
        self.last_search_dispatch.set(Some(Instant::now()));
        *self.search_dispatched.borrow_mut() = Some(text.clone());
        (self.actions.search)(text);
    }

    pub(super) fn move_search_selection(&self, offset: i32) {
        let count = self
            .search_snapshot
            .borrow()
            .as_ref()
            .map_or(0, |snapshot| snapshot.list.len() as i32);
        if count == 0 {
            return;
        }
        let current = self
            .search_results
            .selected_row()
            .map_or(0, |row| row.index());
        let target = (current + offset).clamp(0, count - 1);
        if let Some(row) = self.search_results.row_at_index(target) {
            self.search_results.select_row(Some(&row));
            row.grab_focus();
            self.search_entry.grab_focus();
        }
    }

    pub(super) fn activate_search_result(self: &Rc<Self>, index: i32) {
        let Some(snapshot) = self.search_snapshot.borrow().clone() else {
            return;
        };
        let Some(result) = snapshot.list.get(index.max(0) as usize) else {
            return;
        };
        let Some(action) = result.default_action() else {
            self.search_status.set_label("RESULT HAS NO ACTION");
            return;
        };
        self.execute_search_action(index, action);
    }

    pub(super) fn execute_search_action(self: &Rc<Self>, index: i32, action: &TarragonAction) {
        if action.action_type.as_deref() == Some("query_replace")
            && let Some(query) = action.query.as_deref()
        {
            self.search_entry.set_text(query);
            self.search_entry.grab_focus();
            return;
        }
        if self.search_selection_pending.replace(true) {
            return;
        }
        let Some(snapshot) = self.search_snapshot.borrow().clone() else {
            self.search_selection_pending.set(false);
            return;
        };
        let Some(result) = snapshot.list.get(index.max(0) as usize) else {
            self.search_selection_pending.set(false);
            return;
        };
        (self.actions.select)(TarragonSelection {
            query_id: snapshot.query_id,
            plugin: result.plugin.clone(),
            result_id: result.id.clone(),
            action: action.name.clone(),
        });
        self.search_status.set_label("RUNNING ACTION");
        let generation = self.search_action_generation.get().wrapping_add(1);
        self.search_action_generation.set(generation);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            if let Some(island) = weak.upgrade()
                && island.search_action_generation.get() == generation
                && island.search_selection_pending.get()
                && island.search_open.get()
            {
                island.search_status.set_label("ACTION STILL PENDING");
            }
        });
    }
}
