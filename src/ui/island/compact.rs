//! The resting compact pill: workspace dots, clock, battery, and the
//! hover-revealed tray segment, plus its content-driven width solver.

use super::*;

use std::rc::Rc;

use gtk::{Align, Orientation};

use super::{IslandWindow, Metrics, View, measure_clamped};
use crate::config::{BatteryConfig, BatteryOrientation};

pub(super) fn compact_view(
    metrics: Metrics,
    animations_enabled: bool,
    config: BatteryConfig,
) -> (
    gtk::Overlay,
    gtk::Box,
    gtk::Label,
    gtk::Label,
    gtk::Box,
    gtk::DrawingArea,
    Rc<Cell<u8>>,
) {
    let root = gtk::Overlay::new();
    root.set_size_request(metrics.compact_width, metrics.compact_height);
    // Padding belongs to the foreground row only. Putting compact-content on
    // this overlay also inset the background at medium/large density tiers.
    root.add_css_class("compact-pill");
    root.set_valign(Align::Start);

    let wave_percent = Rc::new(Cell::new(0_u8));
    let draw_percent = wave_percent.clone();
    let phase = Rc::new(Cell::new(0.0_f64));
    let draw_phase = phase.clone();
    let wave = gtk::DrawingArea::new();
    wave.add_css_class("compact-battery-wave");
    wave.set_hexpand(true);
    wave.set_vexpand(true);
    wave.set_halign(Align::Fill);
    wave.set_valign(Align::Fill);
    wave.set_can_target(false);
    wave.set_visible(false);
    wave.set_draw_func(move |area, context, width, height| {
        let accent = area.color();
        let _ = draw_battery_wave(
            context,
            (f64::from(width), f64::from(height)),
            draw_percent.get(),
            draw_phase.get(),
            config,
            (
                f64::from(accent.red()),
                f64::from(accent.green()),
                f64::from(accent.blue()),
            ),
        );
    });
    // The no-animations daemon flag also makes the battery indicator static.
    // An enabled wave otherwise keeps its own lightweight frame clock so the
    // boundary moves even when no battery snapshot is arriving.
    if animations_enabled && config.wave {
        let animate_phase = phase;
        wave.add_tick_callback(move |area, frame_clock| {
            if area.is_mapped() {
                animate_phase.set(frame_clock.frame_time() as f64 / 1_000_000.0 * 1.4);
                area.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }

    let workspaces = gtk::Box::new(Orientation::Horizontal, metrics.spacing(5));
    workspaces.set_hexpand(false);
    workspaces.set_halign(Align::Start);
    workspaces.set_valign(Align::Center);

    let clock = gtk::Label::new(Some("--:--"));
    clock.add_css_class("compact-clock");
    clock.set_hexpand(true);
    clock.set_halign(Align::Center);
    clock.set_valign(Align::Center);

    let battery = gtk::Label::new(None);
    battery.add_css_class("compact-battery");
    battery.set_hexpand(false);
    battery.set_halign(Align::End);
    battery.set_valign(Align::Center);
    battery.set_visible(false);

    // Hidden by default (`update_tray`/`set_tray_hovered`): only shown while
    // the pointer is over the pill and at least one tray item exists, per
    // `resize_compact`.
    let tray = gtk::Box::new(Orientation::Horizontal, metrics.spacing(3));
    tray.add_css_class("compact-tray");
    tray.set_hexpand(false);
    tray.set_halign(Align::End);
    tray.set_valign(Align::Center);
    tray.set_visible(false);

    let content = gtk::Box::new(Orientation::Horizontal, metrics.spacing(10));
    content.add_css_class("compact-content");
    content.set_hexpand(true);
    content.set_vexpand(true);
    content.set_halign(Align::Fill);
    content.set_valign(Align::Fill);
    content.append(&workspaces);
    content.append(&clock);
    content.append(&battery);
    content.append(&tray);
    root.set_child(Some(&wave));
    root.add_overlay(&content);
    (root, workspaces, clock, battery, tray, wave, wave_percent)
}

/// Paint behind the foreground row, across the entire pill. The enclosing
/// island surface supplies the rounded clip, so there is no second inset
/// capsule and the wave meets the actual themed border at every scale.
fn draw_battery_wave(
    context: &gtk::cairo::Context,
    (width, height): (f64, f64),
    percent: u8,
    phase: f64,
    config: BatteryConfig,
    accent: (f64, f64, f64),
) -> Result<(), gtk::cairo::Error> {
    if width <= 0.0 || height <= 0.0 || percent == 0 {
        return Ok(());
    }

    let hue = battery_hue(percent);
    // A translucent wash preserves even custom CSS surface backgrounds. The
    // subtle accent-to-status gradient gives the liquid some depth without
    // painting an opaque green slab over the theme.
    let fill_start = mix(accent, hue, 0.55);
    let line = mix(accent, hue, 0.65);
    let (length, depth) = match config.orientation {
        BatteryOrientation::Horizontal => (width, height),
        BatteryOrientation::Vertical => (height, width),
    };
    let point = |along: f64, across: f64| match config.orientation {
        BatteryOrientation::Horizontal => (along, across),
        BatteryOrientation::Vertical => (width - across, along),
    };
    let boundary = |along: f64| point(along, wave_boundary(along, (length, depth), percent, phase));
    let trace_boundary = || {
        let (x, y) = boundary(0.0);
        context.move_to(x, y);
        for step in 1..=length.ceil() as i32 {
            let (x, y) = boundary(f64::from(step).min(length));
            context.line_to(x, y);
        }
    };

    // This save/restore also makes the renderer usable on an offscreen Cairo
    // surface for geometry/color regression tests without a GTK display.
    context.save()?;
    context.rectangle(0.0, 0.0, width, height);
    context.clip();
    let result = (|| {
        context.new_path();
        trace_boundary();
        let (x, y) = point(length, depth);
        context.line_to(x, y);
        let (x, y) = point(0.0, depth);
        context.line_to(x, y);
        context.close_path();
        if config.tint {
            let gradient = gtk::cairo::LinearGradient::new(0.0, 0.0, width, height);
            gradient.add_color_stop_rgba(0.0, fill_start.0, fill_start.1, fill_start.2, 0.16);
            gradient.add_color_stop_rgba(1.0, hue.0, hue.1, hue.2, 0.23);
            context.set_source(&gradient)?;
        } else {
            context.set_source_rgb(hue.0, hue.1, hue.2);
        }
        context.fill()?;

        if percent < 100 {
            context.new_path();
            trace_boundary();
            context.set_line_width(height / 32.0);
            // Fade the crest as it reaches the edge: at 99% it is barely
            // visible, at full charge it disappears entirely.
            let edge_fade = (f64::from(percent.min(100 - percent)) / 5.0).min(1.0);
            if config.tint {
                context.set_source_rgba(line.0, line.1, line.2, 0.65 * edge_fade);
            } else {
                let line = mix(hue, (1.0, 1.0, 1.0), 0.25);
                context.set_source_rgba(line.0, line.1, line.2, edge_fade);
            }
            context.stroke()?;
        }
        Ok(())
    })();
    context.restore()?;
    result
}

fn wave_boundary(along: f64, (length, depth): (f64, f64), percent: u8, phase: f64) -> f64 {
    let baseline = depth * (1.0 - f64::from(percent.min(100)) / 100.0);
    let amplitude = (length.min(depth) * 0.04)
        .min(baseline * 0.65)
        .min((depth - baseline) * 0.65);
    let cycles = if length >= depth { 3.4 } else { 1.0 };
    baseline + amplitude * (along / length * std::f64::consts::TAU * cycles + phase).sin()
}

fn mix(base: (f64, f64, f64), color: (f64, f64, f64), amount: f64) -> (f64, f64, f64) {
    (
        base.0 + (color.0 - base.0) * amount,
        base.1 + (color.1 - base.1) * amount,
        base.2 + (color.2 - base.2) * amount,
    )
}

/// Battery level moves from red at empty through orange and yellow to green
/// at full, with smooth transitions between the four status colors.
fn battery_hue(percent: u8) -> (f64, f64, f64) {
    let level = f64::from(percent.min(100)) / 100.0;
    if level < 1.0 / 3.0 {
        mix((0.95, 0.16, 0.12), (1.0, 0.55, 0.0), level * 3.0)
    } else if level < 2.0 / 3.0 {
        mix(
            (1.0, 0.55, 0.0),
            (1.0, 0.86, 0.12),
            (level - 1.0 / 3.0) * 3.0,
        )
    } else {
        mix(
            (1.0, 0.86, 0.12),
            (0.20, 0.75, 0.34),
            (level - 2.0 / 3.0) * 3.0,
        )
    }
}

impl IslandWindow {
    pub(super) fn update_battery_wave(&self, percent: Option<u8>) {
        self.compact_battery_percent
            .set(percent.unwrap_or_default().min(100));
        self.compact_battery_wave
            .set_visible(self.compact_battery_wave_enabled && percent.is_some());
        self.compact_battery_wave.queue_draw();
    }
}

impl IslandWindow {
    /// Recomputes the compact pill's width from the combined (individually
    /// capped) natural width of its children, and repositions it within
    /// `content` to match, the same way `resize_media` does for the media
    /// pill. Called whenever a child's content changes (workspaces,
    /// battery, tray) or the tray's hover-visibility toggles.
    pub(super) fn resize_compact(self: &Rc<Self>) {
        let workspaces_width = measure_clamped(
            &self.compact_workspaces,
            self.metrics.compact_workspaces_max_width,
        );
        let clock_width =
            measure_clamped(&self.compact_clock, self.metrics.compact_clock_max_width);
        let battery_width = if self.compact_battery.is_visible() {
            measure_clamped(
                &self.compact_battery,
                self.metrics.compact_battery_max_width,
            )
        } else {
            0
        };

        let tray_visible = self.tray_visible();
        self.compact_tray.set_visible(tray_visible);
        let tray_width = if tray_visible {
            measure_clamped(&self.compact_tray, self.metrics.compact_tray_max_width)
        } else {
            0
        };

        let segments = 2 + i32::from(battery_width > 0) + i32::from(tray_width > 0);
        let spacing = self.metrics.spacing(10) * (segments - 1).max(0);
        // Matches the compact content row's horizontal CSS padding.
        let padding = self.metrics.spacing(30);
        let natural =
            workspaces_width + clock_width + battery_width + tray_width + spacing + padding;
        let width = natural.clamp(self.metrics.compact_min_width, self.metrics.media_max_width);

        self.compact
            .set_size_request(width, self.metrics.compact_height);
        self.content.move_(
            &self.compact,
            f64::from((self.metrics.window_width - width) / 2),
            0.0,
        );
        self.compact_width.set(width);

        if self.current_view.get() == View::Compact {
            self.set_view(View::Compact);
        }
    }

    /// Whether the tray row should be shown: at least one item exists, and
    /// either the pill (compact or media, whichever is active) is hovered
    /// or one of the items' menus is currently open.
    pub(super) fn tray_visible(&self) -> bool {
        (self.tray_hovered.get() || self.tray_menu_open.get()) && self.tray_item_count.get() > 0
    }

    /// Hides/reveals the tray row on hover, and resizes both pills (only
    /// the currently active one animates; the other silently follows so
    /// it's already correct if the view switches while hovered).
    pub(super) fn set_tray_hovered(self: &Rc<Self>, hovered: bool) {
        if self.tray_hovered.get() == hovered {
            return;
        }
        self.tray_hovered.set(hovered);
        self.resize_compact();
        self.resize_media();
    }
}

#[cfg(test)]
mod tests {
    use super::{battery_hue, draw_battery_wave, mix, wave_boundary};
    use crate::config::{BatteryConfig, BatteryOrientation};

    #[test]
    fn divider_tracks_charge_and_recedes_to_the_edge_in_both_orientations() {
        for size in [(224.0, 32.0), (32.0, 224.0), (448.0, 64.0)] {
            for phase in [0.0, 1.0, 3.0, 5.0] {
                for step in 0..=100 {
                    let along = size.0 * f64::from(step) / 100.0;
                    assert_eq!(wave_boundary(along, size, 0, phase), size.1);
                    assert_eq!(wave_boundary(along, size, 100, phase), 0.0);
                    let near_full = wave_boundary(along, size, 99, phase);
                    assert!(near_full > 0.0 && near_full < size.1 * 0.02);
                    let half = wave_boundary(along, size, 50, phase);
                    assert!((half - size.1 * 0.5).abs() <= size.1 * 0.041);
                }
            }
        }
    }

    fn render(percent: u8, tint: bool, orientation: BatteryOrientation) -> Vec<u32> {
        let mut surface =
            gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, 224, 32).unwrap();
        {
            let context = gtk::cairo::Context::new(&surface).unwrap();
            draw_battery_wave(
                &context,
                (224.0, 32.0),
                percent,
                0.0,
                BatteryConfig {
                    wave: true,
                    tint,
                    orientation,
                },
                (0.6, 0.65, 1.0),
            )
            .unwrap();
        }
        surface
            .data()
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_ne_bytes(*bytes))
            .collect()
    }

    #[test]
    fn horizontal_wave_reaches_both_edges_and_tints_only_below_the_divider() {
        let pixels = render(50, true, BatteryOrientation::Horizontal);
        for x in [0, 112, 223] {
            assert_eq!(pixels[8 * 224 + x], 0);
            let alpha = pixels[24 * 224 + x] >> 24;
            assert!(
                (40..=60).contains(&alpha),
                "tint must stay translucent at x={x}"
            );
        }
        let flat = render(50, false, BatteryOrientation::Horizontal);
        assert_eq!(flat[24 * 224 + 112] >> 24, 255);
    }

    #[test]
    fn vertical_wave_fills_left_to_right_and_endpoints_are_exact() {
        let pixels = render(50, true, BatteryOrientation::Vertical);
        assert!(pixels[16 * 224 + 40] >> 24 > 0);
        assert_eq!(pixels[16 * 224 + 180], 0);
        for orientation in [BatteryOrientation::Horizontal, BatteryOrientation::Vertical] {
            assert!(render(0, true, orientation).iter().all(|pixel| *pixel == 0));
            let full = render(100, false, orientation);
            assert!(
                full.iter()
                    .all(|pixel| *pixel == full[0] && *pixel >> 24 == 255)
            );
        }
    }

    #[test]
    fn battery_hue_uses_the_expected_endpoints() {
        assert_color_close(battery_hue(0), (0.95, 0.16, 0.12));
        assert_color_close(battery_hue(100), (0.20, 0.75, 0.34));
    }

    #[test]
    fn battery_hue_interpolates_through_orange_and_yellow() {
        let orange = battery_hue(33);
        let yellow = battery_hue(66);
        assert!(orange.0 > orange.1);
        assert!(yellow.1 > orange.1);
        assert!(yellow.1 > yellow.0 * 0.7);
        assert_color_close(
            mix((0.0, 0.0, 0.0), (1.0, 0.5, 0.25), 0.5),
            (0.5, 0.25, 0.125),
        );
    }

    fn assert_color_close(actual: (f64, f64, f64), expected: (f64, f64, f64)) {
        assert!((actual.0 - expected.0).abs() < f64::EPSILON);
        assert!((actual.1 - expected.1).abs() < f64::EPSILON);
        assert!((actual.2 - expected.2).abs() < f64::EPSILON);
    }
}
