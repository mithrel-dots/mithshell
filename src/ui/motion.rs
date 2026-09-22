//! Material 3 transition tokens and shell-specific profiles.
//!
//! Token definitions are from the legacy Material 3 motion guidance, verified
//! against the first-party documentation cited in `docs/motion.md`. Profile
//! assignments are shell design choices, not prescribed Google component
//! timings.

use std::time::Duration;

/// The subset of Material 3 duration tokens used by the shell profiles.
pub mod duration {
    use std::time::Duration;

    pub const SHORT2: Duration = Duration::from_millis(100);
    pub const SHORT3: Duration = Duration::from_millis(150);
    pub const SHORT4: Duration = Duration::from_millis(200);
    pub const MEDIUM2: Duration = Duration::from_millis(300);
    pub const LONG2: Duration = Duration::from_millis(500);
}

/// Monotone, non-overshooting Material 3 cubic-bezier easing tokens.
///
/// The multi-segment `emphasized` token is intentionally not represented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    /// cubic-bezier(0.2, 0, 0, 1)
    Standard,
    /// cubic-bezier(0, 0, 0, 1)
    StandardDecelerate,
    /// cubic-bezier(0.3, 0, 1, 1)
    StandardAccelerate,
    /// cubic-bezier(0.05, 0.7, 0.1, 1)
    EmphasizedDecelerate,
    /// cubic-bezier(0.3, 0, 0.8, 0.15)
    EmphasizedAccelerate,
}

impl Easing {
    /// Maps normalized elapsed time to normalized displacement.
    ///
    /// Endpoints are exact. Out-of-range times (including infinities) clamp to
    /// the endpoints; NaN is treated as the start. Output is always in [0, 1].
    pub fn sample(self, progress: f64) -> f64 {
        if progress.is_nan() || progress <= 0.0 {
            return 0.0;
        }
        if progress >= 1.0 {
            return 1.0;
        }
        let (x1, y1, x2, y2) = match self {
            Self::Standard => (0.2, 0.0, 0.0, 1.0),
            Self::StandardDecelerate => (0.0, 0.0, 0.0, 1.0),
            Self::StandardAccelerate => (0.3, 0.0, 1.0, 1.0),
            Self::EmphasizedDecelerate => (0.05, 0.7, 0.1, 1.0),
            Self::EmphasizedAccelerate => (0.3, 0.0, 0.8, 0.15),
        };

        // Time is the x coordinate, not the Bezier parameter. Invert x using
        // bisection: unlike Newton iteration this also handles flat derivatives
        // at the endpoints without division by zero or special fallback paths.
        // 48 steps narrow the parameter interval to 2^-48 before floating-point
        // rounding, providing ample resolution for pixel/opacity sampling.
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..48 {
            let parameter = (low + high) * 0.5;
            if coordinate(parameter, x1, x2) < progress {
                low = parameter;
            } else {
                high = parameter;
            }
        }
        coordinate((low + high) * 0.5, y1, y2).clamp(0.0, 1.0)
    }
}

fn coordinate(parameter: f64, first: f64, second: f64) -> f64 {
    let inverse = 1.0 - parameter;
    3.0 * inverse * inverse * parameter * first
        + 3.0 * inverse * parameter * parameter * second
        + parameter * parameter * parameter
}

/// A reusable transition definition with no clock, widget, or callback ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub duration: Duration,
    pub easing: Easing,
}

impl Profile {
    pub const HOVER_ENTER: Self = Self {
        duration: duration::SHORT3,
        easing: Easing::Standard,
    };
    pub const HOVER_EXIT: Self = Self::HOVER_ENTER;
    pub const CONTAINER_EXPAND: Self = Self {
        duration: duration::LONG2,
        // Persistent container movement is undirected; Standard is Google's
        // documented fallback when the emphasized direction cannot be chosen.
        easing: Easing::Standard,
    };
    pub const CONTAINER_COLLAPSE: Self = Self {
        duration: duration::SHORT4,
        // See CONTAINER_EXPAND: this is not an enter/exit direction.
        easing: Easing::Standard,
    };
    pub const ENTER: Self = Self {
        duration: duration::MEDIUM2,
        easing: Easing::StandardDecelerate,
    };
    pub const EXIT: Self = Self {
        duration: duration::SHORT4,
        easing: Easing::StandardAccelerate,
    };
    pub const CONTENT_IN: Self = Self {
        duration: duration::SHORT3,
        easing: Easing::StandardDecelerate,
    };
    pub const CONTENT_OUT: Self = Self {
        duration: duration::SHORT2,
        easing: Easing::StandardAccelerate,
    };

    /// Resolves timing without interpreting config defaults as user intent.
    ///
    /// `None` selects this profile's duration; `Some(ms)` is an exact duration
    /// override compatible with `shell.animation_ms: u32`, including zero.
    /// Disabled animations always resolve to zero, regardless of the override.
    pub fn with_timing(self, animations_enabled: bool, override_ms: Option<u32>) -> Self {
        let duration = if !animations_enabled {
            Duration::ZERO
        } else {
            override_ms.map_or(self.duration, |ms| Duration::from_millis(u64::from(ms)))
        };
        Self { duration, ..self }
    }

    /// Whether the clock has reached the end, including instant transitions.
    /// Use this rather than comparing eased floating-point progress to one.
    pub fn is_complete(self, elapsed: Duration) -> bool {
        elapsed >= self.duration
    }

    /// Eased progress for a nonnegative elapsed time. Zero duration returns one
    /// even at elapsed zero: the destination must be applied immediately.
    pub fn progress(self, elapsed: Duration) -> f64 {
        if self.is_complete(elapsed) {
            return 1.0;
        }
        self.easing
            .sample(elapsed.as_secs_f64() / self.duration.as_secs_f64())
    }

    /// Interpolates finite scalar endpoints with exact start/end values.
    ///
    /// For interruption, capture the currently rendered value as `from`, choose
    /// the new `to` and profile, then restart elapsed at zero. This preserves
    /// position continuity, not velocity. Invalidate the previous tick callback
    /// at the call site. Geometry can instead share one `progress` sample.
    pub fn sample(self, from: f64, to: f64, elapsed: Duration) -> f64 {
        let progress = self.progress(elapsed);
        if progress <= 0.0 {
            from
        } else if progress >= 1.0 {
            to
        } else {
            // Convex form avoids overflow in `to - from` for opposite signs.
            (1.0 - progress) * from + progress * to
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EASINGS: [Easing; 5] = [
        Easing::Standard,
        Easing::StandardDecelerate,
        Easing::StandardAccelerate,
        Easing::EmphasizedDecelerate,
        Easing::EmphasizedAccelerate,
    ];
    const PROFILES: [Profile; 8] = [
        Profile::HOVER_ENTER,
        Profile::HOVER_EXIT,
        Profile::CONTAINER_EXPAND,
        Profile::CONTAINER_COLLAPSE,
        Profile::ENTER,
        Profile::EXIT,
        Profile::CONTENT_IN,
        Profile::CONTENT_OUT,
    ];

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-12,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn easing_endpoints_and_invalid_times_are_defined() {
        for easing in EASINGS {
            for time in [f64::NEG_INFINITY, -1.0, -0.0, 0.0, f64::NAN] {
                assert_eq!(easing.sample(time), 0.0);
            }
            for time in [1.0, 2.0, f64::INFINITY] {
                assert_eq!(easing.sample(time), 1.0);
            }
        }
    }

    #[test]
    fn easing_inverts_time_instead_of_treating_it_as_the_parameter() {
        // Analytic points at Bezier parameter 1/2, independently evaluated:
        // (x, y) = (3*x1/8 + 3*x2/8 + 1/8, 3*y1/8 + 3*y2/8 + 1/8).
        for (easing, time, displacement) in [
            (Easing::Standard, 0.2, 0.5),
            (Easing::StandardDecelerate, 0.125, 0.5),
            (Easing::StandardAccelerate, 0.6125, 0.5),
            (Easing::EmphasizedDecelerate, 0.18125, 0.7625),
            (Easing::EmphasizedAccelerate, 0.5375, 0.18125),
        ] {
            close(easing.sample(time), displacement);
        }
        // Decelerate has x=t^3 and y=3t^2-2t^3. These points also exercise
        // the flat initial x derivative, which is troublesome for Newton-only.
        close(Easing::StandardDecelerate.sample(0.001), 0.028);
        close(Easing::StandardDecelerate.sample(0.729), 0.972);
    }

    #[test]
    fn all_easings_are_bounded_and_monotone() {
        for easing in EASINGS {
            let mut previous = 0.0;
            for step in 0..=10_000 {
                let value = easing.sample(f64::from(step) / 10_000.0);
                assert!((0.0..=1.0).contains(&value));
                assert!(value >= previous, "non-monotone {easing:?}");
                previous = value;
            }
            for time in [f64::MIN_POSITIVE, 1e-15, 1.0 - f64::EPSILON] {
                assert!((0.0..=1.0).contains(&easing.sample(time)));
            }
        }
    }

    #[test]
    fn token_and_legacy_timing_are_explicit_and_zero_is_immediate() {
        for profile in PROFILES {
            assert_eq!(profile.with_timing(true, None), profile);
            let legacy = profile.with_timing(true, Some(280));
            assert_eq!(legacy.duration, Duration::from_millis(280));
            assert_eq!(legacy.easing, profile.easing);
            for instant in [
                profile.with_timing(true, Some(0)),
                profile.with_timing(false, None),
                profile.with_timing(false, Some(280)),
            ] {
                assert_eq!(instant.duration, Duration::ZERO);
                assert_eq!(instant.progress(Duration::ZERO), 1.0);
                assert_eq!(instant.sample(23.0, -8.0, Duration::ZERO), -8.0);
                assert!(instant.is_complete(Duration::ZERO));
            }
            let maximum = profile.with_timing(true, Some(u32::MAX));
            assert_eq!(maximum.duration.as_millis(), u128::from(u32::MAX));
            assert_eq!(maximum.progress(Duration::MAX), 1.0);
        }
    }

    #[test]
    fn persistent_container_profiles_use_undirected_standard_fallback() {
        assert_eq!(Profile::CONTAINER_EXPAND.easing, Easing::Standard);
        assert_eq!(Profile::CONTAINER_COLLAPSE.easing, Easing::Standard);
        assert_eq!(Profile::CONTAINER_EXPAND.duration, duration::LONG2);
        assert_eq!(Profile::CONTAINER_COLLAPSE.duration, duration::SHORT4);
    }

    #[test]
    fn profile_sampling_finishes_exactly_and_preserves_scalar_endpoints() {
        for profile in PROFILES {
            assert!(!profile.is_complete(Duration::ZERO));
            assert!(!profile.is_complete(profile.duration - Duration::from_nanos(1)));
            assert!(profile.is_complete(profile.duration));
            assert_eq!(profile.progress(Duration::ZERO), 0.0);
            assert_eq!(profile.progress(profile.duration), 1.0);
            assert_eq!(
                profile.sample(-0.0, 83.0, Duration::ZERO).to_bits(),
                (-0.0_f64).to_bits()
            );
            assert_eq!(profile.sample(-20.0, 83.0, profile.duration), 83.0);
            assert_eq!(profile.sample(-20.0, 83.0, Duration::MAX), 83.0);
            assert!(
                profile
                    .sample(-f64::MAX, f64::MAX, profile.duration / 2)
                    .is_finite()
            );
        }
    }

    #[test]
    fn reversal_starts_at_the_rendered_value_and_converges_without_overshoot() {
        let opening = Profile::CONTAINER_EXPAND;
        let closing = Profile::CONTAINER_COLLAPSE;
        let rendered = opening.sample(40.0, 400.0, Duration::from_millis(120));
        assert!(rendered > 40.0 && rendered < 400.0);
        assert_eq!(closing.sample(rendered, 40.0, Duration::ZERO), rendered);
        let mut previous = rendered;
        for elapsed_ms in 0..=200 {
            let value = closing.sample(rendered, 40.0, Duration::from_millis(elapsed_ms));
            assert!((40.0..=rendered).contains(&value));
            assert!(value <= previous);
            previous = value;
        }
        assert_eq!(previous, 40.0);
    }
}
