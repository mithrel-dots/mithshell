use std::f64::consts::{FRAC_PI_2, TAU};

pub(super) const EXPANDED_WIDTH: f64 = 320.0;
pub(super) const EXPANDED_HEIGHT: f64 = 136.0;
pub(super) const PADDING: i32 = 12;
pub(super) const ART_SCALE: f64 = 3.0;
const PROGRESS_GAP: f64 = 6.0;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ArtworkRect {
    pub x: f64,
    pub y: f64,
    pub size: f64,
}

/// Counterclockwise curls at each end of a straight baseline. Splitting the
/// ring at its bottom tangent lets its top seam open into a left-to-right bar
/// without reversing playback direction or introducing a kink at either join.
/// All three pieces share one playback fraction by path length.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ProgressTrack {
    pub x: f64,
    pub y: f64,
    pub straight: f64,
    pub radius: f64,
    pub sweep: f64,
}

impl ProgressTrack {
    pub fn length(self) -> f64 {
        self.straight + self.radius * self.sweep
    }

    pub fn point(self, fraction: f64) -> (f64, f64) {
        let distance = fraction.clamp(0.0, 1.0) * self.length();
        let curl = self.radius * self.sweep / 2.0;
        if distance < curl && self.radius > f64::EPSILON {
            let angle = (curl - distance) / self.radius;
            (
                self.x - self.radius * angle.sin(),
                self.y - self.radius * (1.0 - angle.cos()),
            )
        } else if distance <= curl + self.straight || self.radius <= f64::EPSILON {
            (self.x + distance - curl, self.y)
        } else {
            let angle = (distance - curl - self.straight) / self.radius;
            (
                self.x + self.straight + self.radius * angle.sin(),
                self.y - self.radius * (1.0 - angle.cos()),
            )
        }
    }

    fn bounds(self) -> (f64, f64, f64) {
        let half_sweep = self.sweep / 2.0;
        let reach = self.radius * half_sweep.min(FRAC_PI_2).sin();
        let rise = self.radius * (1.0 - half_sweep.cos());
        (-reach, self.straight + reach, rise)
    }

    pub fn append_path(self, cr: &gtk::cairo::Context, fraction: f64) {
        let distance = fraction.clamp(0.0, 1.0) * self.length();
        cr.new_path();
        let start = self.point(0.0);
        cr.move_to(start.0, start.1);
        let curl = self.radius * self.sweep / 2.0;
        if distance > 0.0 && self.radius > f64::EPSILON {
            cr.arc_negative(
                self.x,
                self.y - self.radius,
                self.radius,
                FRAC_PI_2 + self.sweep / 2.0,
                FRAC_PI_2 + self.sweep / 2.0 - distance.min(curl) / self.radius,
            );
        }
        if distance > curl {
            cr.line_to(self.x + (distance - curl).min(self.straight), self.y);
        }
        if distance > curl + self.straight && self.radius > f64::EPSILON {
            cr.arc_negative(
                self.x + self.straight,
                self.y - self.radius,
                self.radius,
                FRAC_PI_2,
                FRAC_PI_2 - (distance - curl - self.straight) / self.radius,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MediaGeometry {
    pub art: ArtworkRect,
    pub track: ProgressTrack,
    pub stroke: f64,
    pub expansion: f64,
}

impl MediaGeometry {
    pub fn new(width: f64, height: f64, compact_art: f64, scale: f64, expansion: f64) -> Self {
        let expansion = expansion.clamp(0.0, 1.0);
        let padding = (f64::from(PADDING) * scale).round() * expansion;
        let art = ArtworkRect {
            x: padding,
            y: padding,
            size: (compact_art * (1.0 + (ART_SCALE - 1.0) * expansion))
                .min((width - 2.0 * padding).max(1.0))
                .min((height - 2.0 * padding).max(1.0)),
        };
        let stroke = ((1.25 + 1.25 * expansion) * scale).max(1.0);
        let inset = padding.max(stroke / 2.0);
        let available = (width - 2.0 * inset - stroke).max(0.0);
        let mut track = ProgressTrack {
            straight: available * expansion,
            // Release the curls as the artwork grows, so the remaining ring
            // settles below the content instead of sweeping across controls.
            radius: ((art.size - stroke) / 2.0).max(0.0) * (1.0 - expansion),
            sweep: TAU * (1.0 - expansion),
            ..Default::default()
        };
        let (left, right, rise) = track.bounds();
        let fit = (width - 2.0 * inset).max(0.0) / (right - left).max(1.0);
        let fit = fit
            .min((height - 2.0 * inset).max(0.0) / rise.max(1.0))
            .min(1.0);
        track.straight *= fit;
        track.radius *= fit;
        let (left, right, rise) = track.bounds();
        let mix = |a: f64, b: f64| a + (b - a) * expansion;
        let center_x = mix(art.x + art.size / 2.0, width / 2.0);
        let bar_y = (art.y + art.size + PROGRESS_GAP * scale + stroke / 2.0)
            .min(height - inset - stroke / 2.0);
        let center_y = mix(art.y + art.size / 2.0, bar_y);
        track.x = (center_x - (left + right) / 2.0)
            .clamp(inset - left, (width - inset - right).max(inset - left));
        track.y = (center_y + rise / 2.0).clamp(inset + rise, (height - inset).max(inset + rise));
        Self {
            art,
            track,
            stroke,
            expansion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_unrolls_to_a_padded_full_width_bar_with_threefold_artwork() {
        for scale in [0.75, 1.0, 1.45, 1.9] {
            let art = 30.0 * scale;
            let compact = MediaGeometry::new(art, art, art, scale, 0.0);
            assert_eq!(compact.track.straight, 0.0);
            assert!((compact.track.sweep - TAU).abs() < 1e-9);
            let start = compact.track.point(0.0);
            let end = compact.track.point(1.0);
            assert!((start.0 - end.0).abs() < 1e-9 && (start.1 - end.1).abs() < 1e-9);
            assert!((start.0 - art / 2.0).abs() < 1e-9);
            assert!(
                (start.1 - compact.stroke / 2.0).abs() < 1e-9,
                "the ring's outer stroke touches the artwork's top edge"
            );
            let quarter = compact.track.point(0.25);
            let three_quarters = compact.track.point(0.75);
            assert!(
                quarter.0 < start.0 && quarter.1 > start.1,
                "progress runs down the left side first"
            );
            assert!(three_quarters.0 > start.0 && three_quarters.1 > start.1);
            let full = MediaGeometry::new(318.0 * scale, 134.0 * scale, art, scale, 1.0);
            assert!((full.art.size - art * 3.0).abs() < 1e-9);
            assert_eq!(full.track.sweep, 0.0);
            let padding = (f64::from(PADDING) * scale).round();
            let first = full.track.point(0.0);
            let last = full.track.point(1.0);
            assert!((first.0 - padding - full.stroke / 2.0).abs() < 1e-9);
            assert!((last.0 - (318.0 * scale - padding - full.stroke / 2.0)).abs() < 1e-9);
            assert_eq!(first.1, last.1);
            assert!(
                (first.1 - full.stroke / 2.0 - full.art.y - full.art.size - 6.0 * scale).abs()
                    < 1e-9
            );
            let quarter = full.track.point(0.25);
            assert!((quarter.0 - first.0 - (last.0 - first.0) * 0.25).abs() < 1e-9);
        }
    }

    #[test]
    fn curling_track_stays_inside_every_intermediate_and_constrained_frame() {
        for scale in [0.75, 1.0, 1.9] {
            for width_limit in [80.0, 210.0, 320.0] {
                for step in 0..=100 {
                    let p = f64::from(step) / 100.0;
                    let width = (30.0 + 288.0 * p).min(width_limit) * scale;
                    let height = (30.0 + 104.0 * p) * scale;
                    let geometry = MediaGeometry::new(width, height, 30.0 * scale, scale, p);
                    for point in 0..=100 {
                        let (x, y) = geometry.track.point(f64::from(point) / 100.0);
                        assert!(x.is_finite() && y.is_finite());
                        assert!(x >= 0.0 && x <= width + 1e-9 && y >= 0.0 && y <= height + 1e-9);
                    }
                }
            }
        }
    }
}
