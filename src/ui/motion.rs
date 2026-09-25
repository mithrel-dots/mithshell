//! Material 3 transition tokens and shell-specific profiles.
//!
//! Token definitions are verified against the first-party documentation cited
//! in `docs/motion.md`. Profile assignments are shell design choices, not
//! prescribed Google component timings.

use std::time::Duration;

/// The subset of Material 3 duration tokens used by the shell profiles.
pub mod duration {
    use std::time::Duration;

    pub const SHORT2: Duration = Duration::from_millis(100);
    pub const SHORT3: Duration = Duration::from_millis(150);
    pub const SHORT4: Duration = Duration::from_millis(200);
    pub const MEDIUM4: Duration = Duration::from_millis(400);
    pub const LONG2: Duration = Duration::from_millis(500);

    /// Delay before island content fades in after its container starts expanding.
    pub const ISLAND_ENTER_FADE_DELAY: Duration = SHORT2;
}

/// Monotone, non-overshooting Material 3 cubic-bezier easing tokens.
///
/// Includes the two-segment emphasized path for persistent container transforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    /// Material's two-segment emphasized path, not the single-bezier fallback.
    Emphasized,
    /// cubic-bezier(0.2, 0, 0, 1)
    Standard,
    /// cubic-bezier(0, 0, 0, 1)
    StandardDecelerate,
    /// cubic-bezier(0.3, 0, 1, 1)
    StandardAccelerate,
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
        if self == Self::Emphasized {
            let (start, a, b, end) = if progress < 0.166666 {
                ((0.0, 0.0), (0.05, 0.0), (0.133333, 0.06), (0.166666, 0.4))
            } else {
                ((0.166666, 0.4), (0.208333, 0.82), (0.25, 1.0), (1.0, 1.0))
            };
            let bezier = |t: f64, s: f64, a: f64, b: f64, e: f64| {
                let u = 1.0 - t;
                u * u * u * s + 3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t * e
            };
            let (mut low, mut high) = (0.0, 1.0);
            for _ in 0..48 {
                let t = (low + high) * 0.5;
                if bezier(t, start.0, a.0, b.0, end.0) < progress {
                    low = t;
                } else {
                    high = t;
                }
            }
            return bezier((low + high) * 0.5, start.1, a.1, b.1, end.1);
        }
        let (x1, y1, x2, y2) = match self {
            Self::Emphasized => unreachable!("sampled using the two-segment path"),
            Self::Standard => (0.2, 0.0, 0.0, 1.0),
            Self::StandardDecelerate => (0.0, 0.0, 0.0, 1.0),
            Self::StandardAccelerate => (0.3, 0.0, 1.0, 1.0),
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
    pub const CONTENT_IN: Self = Self {
        duration: duration::SHORT3,
        easing: Easing::StandardDecelerate,
    };
    pub const CONTENT_OUT: Self = Self {
        duration: duration::SHORT2,
        easing: Easing::StandardAccelerate,
    };
    pub const ISLAND_EXPAND: Self = Self {
        duration: duration::LONG2,
        easing: Easing::Emphasized,
    };
    pub const ISLAND_COLLAPSE: Self = Self {
        duration: duration::MEDIUM4,
        easing: Easing::EmphasizedAccelerate,
    };
    pub const ISLAND_FADE_IN: Self = Self {
        duration: duration::SHORT4,
        easing: Easing::Standard,
    };
    pub const ISLAND_FADE_OUT: Self = Self {
        duration: duration::SHORT4,
        easing: Easing::Standard,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const EASINGS: [Easing; 5] = [
        Easing::Emphasized,
        Easing::Standard,
        Easing::StandardDecelerate,
        Easing::StandardAccelerate,
        Easing::EmphasizedAccelerate,
    ];
    const PROFILES: [Profile; 9] = [
        Profile::HOVER_ENTER,
        Profile::CONTAINER_EXPAND,
        Profile::CONTAINER_COLLAPSE,
        Profile::CONTENT_IN,
        Profile::CONTENT_OUT,
        Profile::ISLAND_EXPAND,
        Profile::ISLAND_COLLAPSE,
        Profile::ISLAND_FADE_IN,
        Profile::ISLAND_FADE_OUT,
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
    fn island_profiles_use_directional_material_tokens_and_fade_delay() {
        assert_eq!(duration::MEDIUM4, Duration::from_millis(400));
        assert_eq!(duration::ISLAND_ENTER_FADE_DELAY, duration::SHORT2);
        assert_eq!(
            Profile::ISLAND_EXPAND,
            Profile {
                duration: duration::LONG2,
                easing: Easing::Emphasized,
            }
        );
        assert_eq!(
            Profile::ISLAND_COLLAPSE,
            Profile {
                duration: duration::MEDIUM4,
                easing: Easing::EmphasizedAccelerate,
            }
        );
        assert_eq!(
            Profile::ISLAND_FADE_IN,
            Profile {
                duration: duration::SHORT4,
                easing: Easing::Standard,
            }
        );
        assert_eq!(
            Profile::ISLAND_FADE_OUT,
            Profile {
                duration: duration::SHORT4,
                easing: Easing::Standard,
            }
        );
    }

    #[test]
    fn profile_progress_finishes_exactly() {
        for profile in PROFILES {
            assert!(!profile.is_complete(Duration::ZERO));
            assert!(!profile.is_complete(profile.duration - Duration::from_nanos(1)));
            assert!(profile.is_complete(profile.duration));
            assert_eq!(profile.progress(Duration::ZERO), 0.0);
            assert_eq!(profile.progress(profile.duration), 1.0);
            assert_eq!(profile.progress(Duration::MAX), 1.0);
        }
    }
}
