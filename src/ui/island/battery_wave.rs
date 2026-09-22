//! Synchronized battery backgrounds for the compact and playing-media pills.

use super::*;

use std::rc::Rc;

use gtk::Align;

use crate::config::{BatteryConfig, BatteryOrientation};

pub(super) struct BatteryWaves {
    pub(super) compact: gtk::DrawingArea,
    pub(super) media: gtk::DrawingArea,
    percent: Rc<Cell<u8>>,
    phase: Rc<Cell<f64>>,
    enabled: bool,
    motion: Cell<bool>,
    ticks: RefCell<Vec<gtk::TickCallbackId>>,
}

impl BatteryWaves {
    pub(super) fn new(config: BatteryConfig, animations_enabled: bool, animation_ms: u32) -> Self {
        let percent = Rc::new(Cell::new(0));
        let phase = Rc::new(Cell::new(0.0));
        Self {
            compact: wave_widget(config, percent.clone(), phase.clone()),
            media: wave_widget(config, percent.clone(), phase.clone()),
            percent,
            phase,
            enabled: config.wave,
            motion: Cell::new(animations_enabled && animation_ms > 0),
            ticks: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn update(&self, percent: Option<u8>) {
        self.percent.set(percent.unwrap_or_default().min(100));
        for wave in [&self.compact, &self.media] {
            wave.set_visible(self.enabled && percent.is_some());
        }
        self.refresh_ticks();
        self.queue_draw();
    }

    pub(super) fn queue_draw(&self) {
        self.compact.queue_draw();
        self.media.queue_draw();
    }

    pub(super) fn set_motion(&self, animations_enabled: bool, animation_ms: u32) {
        self.motion.set(animations_enabled && animation_ms > 0);
        if !self.motion.get() {
            self.phase.set(0.0);
        }
        self.refresh_ticks();
        self.queue_draw();
    }

    fn refresh_ticks(&self) {
        // Empty and full batteries have no moving boundary. Removing callbacks
        // also lets a static/disabled indicator stop requesting frame clocks.
        let animate = self.enabled && self.motion.get() && (1..100).contains(&self.percent.get());
        let mut ticks = self.ticks.borrow_mut();
        if animate != ticks.is_empty() {
            return;
        }
        for tick in ticks.drain(..) {
            tick.remove();
        }
        if animate {
            for wave in [&self.compact, &self.media] {
                let phase = self.phase.clone();
                ticks.push(wave.add_tick_callback(move |area, frame_clock| {
                    if area.is_mapped() {
                        // Absolute frame time keeps the two instances in phase
                        // through view switches, independent of snapshots/Cava.
                        phase.set(frame_clock.frame_time() as f64 / 1_000_000.0 * 1.4);
                        area.queue_draw();
                    }
                    glib::ControlFlow::Continue
                }));
            }
        }
    }
}

impl Drop for BatteryWaves {
    fn drop(&mut self) {
        for tick in self.ticks.get_mut().drain(..) {
            tick.remove();
        }
    }
}

fn wave_widget(
    config: BatteryConfig,
    percent: Rc<Cell<u8>>,
    phase: Rc<Cell<f64>>,
) -> gtk::DrawingArea {
    let wave = gtk::DrawingArea::new();
    // Keep the existing theme selector for both presentations.
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
            percent.get(),
            phase.get(),
            config,
            (
                f64::from(accent.red()),
                f64::from(accent.green()),
                f64::from(accent.blue()),
            ),
        );
    });
    wave
}

impl IslandWindow {
    pub(super) fn update_battery_wave(&self, percent: Option<u8>) {
        self.battery_waves.update(percent);
    }
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

#[cfg(test)]
mod tests {
    use super::{battery_hue, draw_battery_wave, mix, wave_boundary};
    use crate::config::{BatteryConfig, BatteryOrientation};

    /// Exercises the actual view builders, widget mapping, draw closures, and
    /// frame clocks. Run explicitly against a private GTK display (Broadway
    /// works); this does not construct layer-shell windows or start the daemon.
    #[test]
    #[ignore = "requires an isolated GTK display; run with --ignored --test-threads=1"]
    fn playing_media_keeps_a_live_full_width_battery_background() {
        use super::BatteryWaves;
        use crate::config::{IconStyle, ThemeConfig};
        use crate::ui::island::{Metrics, compact::compact_view, media::media_view};
        use gtk::{gdk, prelude::*};

        gtk::init().expect("initialize a private GTK display for this test");
        let display = gdk::Display::default().unwrap();
        let monitor = display
            .monitors()
            .item(0)
            .unwrap()
            .downcast::<gdk::Monitor>()
            .unwrap();
        let palette = crate::theme::generate(&ThemeConfig::default()).unwrap();
        let styles = crate::ui::install_styles(&palette);

        for scale in [1.0, 1.4, 1.75] {
            for orientation in [BatteryOrientation::Horizontal, BatteryOrientation::Vertical] {
                for tint in [true, false] {
                    let config = BatteryConfig {
                        wave: true,
                        tint,
                        orientation,
                    };
                    // Zero-duration starts static even with the daemon flag enabled.
                    let waves = BatteryWaves::new(config, true, 0);
                    let metrics = Metrics::new(&monitor, scale, 1.5, IconStyle::default());
                    let (compact, workspaces, clock, _, _) = compact_view(metrics, &waves.compact);
                    let media = media_view(metrics, &waves.media);
                    media.title.set_label("Playing track");
                    media
                        .root
                        .set_size_request(metrics.media_max_width, metrics.media_height);

                    let host = gtk::Fixed::new();
                    host.put(&compact, 0.0, 0.0);
                    host.put(&media.root, 0.0, 0.0);
                    media.root.set_visible(false);
                    let window = gtk::Window::new();
                    window.set_default_size(metrics.media_max_width, metrics.media_height);
                    if let Some(class) = metrics.css_class() {
                        window.add_css_class(class);
                    }
                    window.set_child(Some(&host));
                    window.present();
                    waves.update(Some(25));
                    frames();
                    assert!(waves.compact.is_mapped());
                    assert!(
                        compact.width() > 0,
                        "compact pill must be allocated by the test display"
                    );
                    assert!(!waves.media.is_mapped());
                    assert_eq!(waves.compact.width(), compact.width());
                    assert_eq!(waves.compact.height(), compact.height());

                    // The playing view hides the compact parent, just as
                    // finish_view does. Foreground remains exclusive to media.
                    compact.set_visible(false);
                    media.root.set_visible(true);
                    frames();
                    assert!(!workspaces.is_mapped());
                    assert!(!clock.is_mapped());
                    assert!(!waves.compact.is_mapped());
                    assert!(waves.media.is_mapped());
                    assert!(media.title.is_mapped());
                    assert_eq!(waves.media.width(), media.root.width());
                    assert_eq!(waves.media.height(), media.root.height());
                    assert!(!waves.media.can_target());
                    assert!(!media.root.has_css_class("media-content"));
                    let foreground = waves.media.next_sibling().unwrap();
                    assert!(foreground.has_css_class("media-content"));
                    assert!(
                        foreground.width() < waves.media.width(),
                        "CSS padding must inset only the foreground"
                    );

                    let quarter = widget_pixels(&waves.media);
                    assert!(quarter.iter().any(|pixel| *pixel != 0));
                    assert!(waves.ticks.borrow().is_empty());
                    assert_eq!(waves.phase.get(), 0.0);
                    waves.update(Some(75));
                    frames();
                    let three_quarters = widget_pixels(&waves.media);
                    assert_ne!(
                        quarter, three_quarters,
                        "playing background must receive battery updates"
                    );

                    // Theme reload must redraw even a static media background.
                    let mut changed = palette.clone();
                    changed.primary = "#ff0000".to_owned();
                    crate::ui::update_styles(&styles, &changed);
                    waves.queue_draw();
                    frames();
                    if tint {
                        assert_ne!(three_quarters, widget_pixels(&waves.media));
                    }
                    crate::ui::update_styles(&styles, &palette);
                    waves.queue_draw();

                    waves.set_motion(true, 280);
                    frames();
                    let phase = waves.phase.get();
                    let animated = widget_pixels(&waves.media);
                    frames();
                    assert_ne!(
                        phase,
                        waves.phase.get(),
                        "media wave must advance without battery or Cava updates"
                    );
                    assert_ne!(
                        animated,
                        widget_pixels(&waves.media),
                        "frame ticks must redraw the playing background"
                    );
                    waves.set_motion(false, 280);
                    frames();
                    assert_eq!(waves.phase.get(), 0.0);
                    assert!(waves.ticks.borrow().is_empty());
                    waves.set_motion(true, 280);
                    frames();
                    waves.set_motion(true, 0);
                    frames();
                    assert_eq!(waves.phase.get(), 0.0);
                    assert!(waves.ticks.borrow().is_empty());

                    // Missing battery hides both instances; reappearance while
                    // playing restores the media instance without a view switch.
                    waves.update(None);
                    assert!(!waves.compact.is_visible());
                    assert!(!waves.media.is_mapped());
                    waves.update(Some(0));
                    frames();
                    assert!(waves.media.is_mapped());
                    assert!(widget_pixels(&waves.media).iter().all(|pixel| *pixel == 0));
                    waves.set_motion(true, 280);
                    assert!(waves.ticks.borrow().is_empty());
                    waves.update(Some(255));
                    frames();
                    assert_eq!(waves.percent.get(), 100);
                    assert!(waves.ticks.borrow().is_empty());
                    assert!(widget_pixels(&waves.media).iter().all(|pixel| *pixel != 0));

                    media.root.set_visible(false);
                    compact.set_visible(true);
                    frames();
                    assert!(waves.compact.is_mapped());
                    assert!(!waves.media.is_mapped());
                    assert!(
                        widget_pixels(&waves.compact)
                            .iter()
                            .all(|pixel| *pixel != 0)
                    );
                    window.close();
                }
            }
        }

        for (flag, duration) in [(false, 280), (true, 0)] {
            let static_waves = BatteryWaves::new(
                BatteryConfig {
                    wave: true,
                    ..BatteryConfig::default()
                },
                flag,
                duration,
            );
            static_waves.update(Some(50));
            assert!(static_waves.ticks.borrow().is_empty());
        }
        let disabled = BatteryWaves::new(
            BatteryConfig {
                wave: false,
                ..BatteryConfig::default()
            },
            true,
            280,
        );
        for percent in [None, Some(0), Some(50), Some(100)] {
            disabled.update(percent);
            assert!(!disabled.compact.is_visible());
            assert!(!disabled.media.is_visible());
            assert!(disabled.ticks.borrow().is_empty());
        }
        gtk::style_context_remove_provider_for_display(&display, &styles);
    }

    fn frames() {
        let main_loop = gtk::glib::MainLoop::new(None, false);
        let stop = main_loop.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(80), move || {
            stop.quit()
        });
        main_loop.run();
    }

    /// Snapshot the GTK widget, so this catches stale state in the draw closure
    /// rather than merely invoking the Cairo renderer with handpicked inputs.
    fn widget_pixels(widget: &gtk::DrawingArea) -> Vec<u32> {
        use gtk::prelude::*;
        let (width, height) = (widget.width(), widget.height());
        assert!(width > 0 && height > 0);
        let snapshot = gtk::Snapshot::new();
        gtk::WidgetPaintable::new(Some(widget)).snapshot(
            &snapshot,
            f64::from(width),
            f64::from(height),
        );
        let mut surface =
            gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, height).unwrap();
        if let Some(node) = snapshot.to_node() {
            node.draw(&gtk::cairo::Context::new(&surface).unwrap());
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
