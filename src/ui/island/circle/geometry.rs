//! Collision policy: reserve room for each present module's largest view, even
//! while compact. If both side corridors can fit those widths, use them. If not,
//! place the present slots in non-overlapping lanes below the central island.
//! Two slots share the available width equally; one gets the entire width.
//! Side lanes keep their inner edge and top anchor. Below-island lanes expand
//! around the compact center, clamped to the lane, with the same top anchor.
//! Thus hover itself never triggers a lane change or moves the compact footprint.
//! Expanded content scrolls when constrained; a slot is temporarily suppressed
//! (`None`) if its lane cannot fit even its compact diameter. We never overlap
//! the central island, the other slot, or monitor edges to force a circle in.
//! A nearly full-screen central view can therefore temporarily hide circles.
//! Changing central geometry or module presence can relocate lanes; recompute on
//! every frame, rather than interpolating positions through the central surface.

use gtk::cairo::RectangleInt;

use super::Mode;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn right(self) -> f64 {
        self.x + self.width
    }

    fn bottom(self) -> f64 {
        self.y + self.height
    }

    fn valid(self) -> bool {
        [self.x, self.y, self.right(), self.bottom()]
            .into_iter()
            .all(f64::is_finite)
            && self.width >= 0.0
            && self.height >= 0.0
    }
}

/// Sizes in unscaled design units. Full is only supplied for notifications.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CircleSpec {
    pub diameter: f64,
    pub hover: Size,
    pub full: Option<Size>,
}

impl CircleSpec {
    pub(crate) fn visual(self, mode: Mode) -> Option<Visual> {
        let size = match mode {
            Mode::Absent => return None,
            Mode::Compact => Size {
                width: self.diameter,
                height: self.diameter,
            },
            Mode::HoverExpanded => self.hover,
            Mode::FullExpanded => self.full.unwrap_or(self.hover),
        };
        Some(Visual {
            size,
            // Never cut away any part of the compact footprint on expansion.
            radius: if mode == Mode::Compact {
                self.diameter / 2.0
            } else {
                16.0_f64.min(self.diameter / 2.0)
            },
        })
    }

    fn max_width(self) -> f64 {
        self.diameter
            .max(self.hover.width)
            .max(self.full.map_or(0.0, |s| s.width))
    }

    fn valid(self) -> bool {
        let valid_size = |s: Size| {
            s.width.is_finite()
                && s.height.is_finite()
                && s.width >= self.diameter
                && s.height >= self.diameter
        };
        self.diameter.is_finite()
            && self.diameter > 0.0
            && valid_size(self.hover)
            && self.full.is_none_or(valid_size)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Visual {
    pub size: Size,
    pub radius: f64,
}

impl Visual {
    /// `progress` is already eased by the caller's Material profile. Starting
    /// from the last rendered visual makes interrupted/reversed motion continuous.
    pub(crate) fn interpolate(self, target: Self, progress: f64) -> Self {
        let t = if progress.is_finite() {
            progress.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let lerp = |a, b| a + (b - a) * t;
        Self {
            size: Size {
                width: lerp(self.size.width, target.size.width),
                height: lerp(self.size.height, target.size.height),
            },
            radius: lerp(self.radius, target.radius),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CircleRequest {
    pub spec: CircleSpec,
    /// Current animated dimensions, in design units, not the final target.
    pub visual: Visual,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub rect: Rect,
    pub radius: f64,
}

impl Frame {
    /// Window-local logical integer rectangles suitable for a Cairo/GDK region
    /// union. Scanline bands cover the rounded shape, never its transparent
    /// bounding-box corners or the inter-surface gap (apart from <1px rounding).
    /// GDK input regions use logical units, not physical device pixels.
    pub(crate) fn input_rectangles(self) -> Vec<RectangleInt> {
        let mut bands = Vec::new();
        let r = self
            .radius
            .clamp(0.0, self.rect.width.min(self.rect.height) / 2.0);
        for y in self.rect.y.floor() as i32..self.rect.bottom().ceil() as i32 {
            let top = f64::from(y).max(self.rect.y);
            let bottom = (f64::from(y) + 1.0).min(self.rect.bottom());
            let dy = (self.rect.y + r - bottom)
                .max(top - (self.rect.bottom() - r))
                .max(0.0);
            let inset = r - (r * r - dy * dy).max(0.0).sqrt();
            let x = (self.rect.x + inset).floor() as i32;
            let end = (self.rect.right() - inset).ceil() as i32;
            if end > x {
                bands.push(RectangleInt::new(x, y, end - x, 1));
            }
        }
        bands
    }

    pub(crate) fn contains(self, x: f64, y: f64) -> bool {
        let rect = self.rect;
        if x < rect.x || x >= rect.right() || y < rect.y || y >= rect.bottom() {
            return false;
        }
        let r = self.radius.clamp(0.0, rect.width.min(rect.height) / 2.0);
        let dx = x - x.clamp(rect.x + r, rect.right() - r);
        let dy = y - y.clamp(rect.y + r, rect.bottom() - r);
        dx * dx + dy * dy <= r * r
    }
}

/// Exactly one optional request per side: index 0 is left, index 1 right.
/// `None` means absent and reserves neither a lane nor a gap nor an input region.
/// `monitor` should be the usable monitor/canvas intersection in window-local
/// logical coordinates, with reserved areas already removed by the integrator.
pub(crate) fn layout(
    central: Rect,
    monitor: Rect,
    scale: f64,
    slots: [Option<CircleRequest>; 2],
) -> [Option<Frame>; 2] {
    if !central.valid() || !monitor.valid() || !scale.is_finite() || scale <= 0.0 {
        return [None; 2];
    }
    let slots = slots.map(|slot| {
        slot.filter(|s| {
            s.spec.valid()
                && s.visual.size.width.is_finite()
                && s.visual.size.height.is_finite()
                && s.visual.radius.is_finite()
        })
    });
    let count = slots.iter().flatten().count();
    if count == 0 {
        return [None; 2];
    }
    let gap = (8.0 * scale).ceil();
    let left_edge = (central.x - gap).min(monitor.right());
    let right_edge = (central.right() + gap).max(monitor.x);
    let side_widths = [left_edge - monitor.x, monitor.right() - right_edge];
    let beside = slots
        .iter()
        .enumerate()
        .all(|(i, slot)| slot.is_none_or(|s| side_widths[i] >= s.spec.max_width() * scale));
    let y = if beside {
        central.y.max(monitor.y).ceil()
    } else {
        (central.bottom() + gap).max(monitor.y).ceil()
    };
    let mid = monitor.x + monitor.width / 2.0;
    std::array::from_fn(|i| {
        let slot = slots[i]?;
        let (start, end) = if beside {
            if i == 0 {
                (monitor.x, left_edge)
            } else {
                (right_edge, monitor.right())
            }
        } else if count == 1 {
            (monitor.x, monitor.right())
        } else if i == 0 {
            (monitor.x, mid - gap / 2.0)
        } else {
            (mid + gap / 2.0, monitor.right())
        };
        let start = start.ceil();
        let end = end.floor();
        let available_width = end - start;
        let available_height = monitor.bottom().floor() - y;
        let diameter = (slot.spec.diameter * scale).ceil();
        if available_width < diameter || available_height < diameter {
            return None;
        }
        let width = (slot.visual.size.width * scale)
            .max(diameter)
            .min(available_width)
            .floor();
        let height = (slot.visual.size.height * scale)
            .max(diameter)
            .min(available_height)
            .floor();
        let x = if beside {
            if i == 0 { end - width } else { start }
        } else {
            let anchor = if i == 0 {
                central.x - gap - diameter / 2.0
            } else {
                central.right() + gap + diameter / 2.0
            }
            .clamp(start + diameter / 2.0, end - diameter / 2.0);
            (anchor - width / 2.0).clamp(start, end - width).floor()
        };
        Some(Frame {
            rect: Rect {
                x,
                y,
                width,
                height,
            },
            radius: (slot.visual.radius * scale).clamp(0.0, diameter.min(width).min(height) / 2.0),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> CircleSpec {
        CircleSpec {
            diameter: 32.0,
            hover: Size {
                width: 240.0,
                height: 160.0,
            },
            full: Some(Size {
                width: 360.0,
                height: 400.0,
            }),
        }
    }

    fn request(mode: Mode) -> Option<CircleRequest> {
        spec().visual(mode).map(|visual| CircleRequest {
            spec: spec(),
            visual,
        })
    }

    fn monitor(width: f64, height: f64) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        }
    }

    fn central(width: f64) -> Rect {
        Rect {
            x: (width - 224.0) / 2.0,
            y: 10.0,
            width: 224.0,
            height: 32.0,
        }
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
    }

    #[test]
    fn absent_slots_reserve_no_lane_gap_or_hits() {
        assert_eq!(
            layout(
                central(400.0),
                monitor(400.0, 600.0),
                1.0,
                [request(Mode::Absent); 2]
            ),
            [None; 2]
        );
        let single = layout(
            central(400.0),
            monitor(400.0, 600.0),
            1.0,
            [request(Mode::FullExpanded), None],
        );
        assert!(single[1].is_none());
        assert_eq!(single[0].unwrap().rect.width, 360.0); // no phantom half lane
        let pair = layout(
            central(400.0),
            monitor(400.0, 600.0),
            1.0,
            [request(Mode::FullExpanded); 2],
        );
        assert_eq!(pair[0].unwrap().rect.width, 196.0);
        assert!(
            !pair[0]
                .unwrap()
                .input_rectangles()
                .iter()
                .any(|r| r.x() + r.width() > 196)
        );
    }

    #[test]
    fn compact_footprint_stays_inside_hover_and_full_on_both_sides() {
        for width in [400.0, 1920.0] {
            let compact = layout(
                central(width),
                monitor(width, 900.0),
                1.0,
                [request(Mode::Compact); 2],
            );
            for mode in [Mode::HoverExpanded, Mode::FullExpanded] {
                let expanded = layout(
                    central(width),
                    monitor(width, 900.0),
                    1.0,
                    [request(mode); 2],
                );
                for i in 0..2 {
                    let c = compact[i].unwrap();
                    let e = expanded[i].unwrap();
                    for x in 0..32 {
                        for y in 0..32 {
                            let (x, y) =
                                (c.rect.x + f64::from(x) + 0.5, c.rect.y + f64::from(y) + 0.5);
                            if c.contains(x, y) {
                                assert!(e.contains(x, y));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn animated_sizes_stay_bounded_and_disjoint_on_narrow_scaled_monitors() {
        for width in [40.0, 100.0, 320.0, 640.0, 1920.0] {
            for height in [40.0, 120.0, 800.0] {
                for scale in [0.75, 1.0, 1.5, 2.4] {
                    for step in 0..=10 {
                        // The central island is also growing/moving, independently
                        // of the circle. Lanes must follow its rendered bounds.
                        let mut c = central(width);
                        c.width += f64::from(step) * 20.0;
                        c.x -= f64::from(step) * 10.0;
                        c.height += f64::from(step) * 30.0;
                        c.y += f64::from(step);
                        let visual = spec().visual(Mode::Compact).unwrap().interpolate(
                            spec().visual(Mode::FullExpanded).unwrap(),
                            f64::from(step) / 10.0,
                        );
                        let frames = layout(
                            c,
                            monitor(width, height),
                            scale,
                            [Some(CircleRequest {
                                spec: spec(),
                                visual,
                            }); 2],
                        );
                        for frame in frames.into_iter().flatten() {
                            assert!(frame.rect.x >= 0.0 && frame.rect.y >= 0.0);
                            assert!(frame.rect.right() <= width && frame.rect.bottom() <= height);
                            assert!(!overlaps(frame.rect, c));
                            for band in frame.input_rectangles() {
                                assert!(band.x() >= 0 && band.y() >= 0);
                                assert!(f64::from(band.x() + band.width()) <= width);
                                assert!(f64::from(band.y() + band.height()) <= height);
                            }
                        }
                        if let [Some(a), Some(b)] = frames {
                            assert!(!overlaps(a.rect, b.rect));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn translated_fractional_bounds_and_rounded_hits_use_logical_coordinates() {
        let bounds = Rect {
            x: -200.5,
            y: -10.5,
            width: 640.5,
            height: 500.5,
        };
        let frames = layout(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 32.0,
            },
            bounds,
            1.25,
            [request(Mode::Compact); 2],
        );
        for frame in frames.into_iter().flatten() {
            assert_eq!(frame.rect.width, 40.0);
            assert!(frame.rect.x >= bounds.x && frame.rect.right() <= bounds.right());
            assert!(!frame.contains(frame.rect.x, frame.rect.y));
            assert!(frame.contains(frame.rect.x + 20.0, frame.rect.y + 20.0));
            assert!(frame.input_rectangles()[0].width() < 40);
        }
    }

    #[test]
    fn a_single_fallback_circle_keeps_its_side_and_footprint_during_every_frame() {
        for side in 0..2 {
            let mut slots = [None; 2];
            slots[side] = request(Mode::Compact);
            let compact = layout(central(400.0), monitor(400.0, 700.0), 1.0, slots)[side].unwrap();
            if side == 0 {
                assert!(compact.rect.right() < 200.0);
            } else {
                assert!(compact.rect.x > 200.0);
            }
            for step in 0..=20 {
                let visual = spec().visual(Mode::Compact).unwrap().interpolate(
                    spec().visual(Mode::FullExpanded).unwrap(),
                    f64::from(step) / 20.0,
                );
                slots[side] = Some(CircleRequest {
                    spec: spec(),
                    visual,
                });
                let frame =
                    layout(central(400.0), monitor(400.0, 700.0), 1.0, slots)[side].unwrap();
                for x in 0..32 {
                    for y in 0..32 {
                        let (x, y) = (
                            compact.rect.x + f64::from(x) + 0.5,
                            compact.rect.y + f64::from(y) + 0.5,
                        );
                        if compact.contains(x, y) {
                            assert!(frame.contains(x, y));
                        }
                    }
                }
            }
        }
    }
}
