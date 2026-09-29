//! Material 3 transition tokens and shell-specific profiles.
//!
//! Token definitions are verified against the first-party documentation cited
//! in `docs/motion.md`. Profile assignments are shell design choices, not
//! prescribed Google component timings.

use std::time::Duration;

use crate::config::{MotionConfig, MotionEasing, MotionOverride};

/// Shell transition identities, independent of their default tokens.
#[derive(Clone, Copy, Debug)]
pub enum Transition {
    PeekEnter,
    PeekExit,
    IslandOpen,
    IslandClose,
    LauncherOpen,
    LauncherClose,
    CircleEnter,
    CircleExit,
    TrayEnter,
    TrayExit,
    MediaEnter,
    MediaExit,
    HoverEnter,
    ContainerExpand,
    ContainerCollapse,
    ContentIn,
    ContentOut,
    IslandFadeIn,
    IslandFadeOut,
}

impl Transition {
    fn defaults(self, modern: bool) -> Profile {
        match self {
            Self::MediaEnter | Self::MediaExit => Profile::PEEK_ENTER,
            Self::PeekEnter | Self::TrayEnter if modern => Profile::PEEK_ENTER,
            Self::PeekExit | Self::TrayExit if modern => Profile::PEEK_EXIT,
            Self::PeekEnter | Self::TrayEnter | Self::IslandOpen => Profile::ISLAND_EXPAND,
            Self::PeekExit | Self::TrayExit | Self::IslandClose => Profile::ISLAND_COLLAPSE,
            Self::LauncherOpen | Self::CircleEnter | Self::ContainerExpand => {
                Profile::CONTAINER_EXPAND
            }
            Self::LauncherClose | Self::CircleExit | Self::ContainerCollapse => {
                Profile::CONTAINER_COLLAPSE
            }
            Self::HoverEnter => Profile::HOVER_ENTER,
            Self::ContentIn => Profile::CONTENT_IN,
            Self::ContentOut => Profile::CONTENT_OUT,
            Self::IslandFadeIn => Profile::ISLAND_FADE_IN,
            Self::IslandFadeOut => Profile::ISLAND_FADE_OUT,
        }
    }

    fn settings(self, config: MotionConfig) -> MotionOverride {
        match self {
            Self::PeekEnter => config.peek_enter,
            Self::PeekExit => config.peek_exit,
            Self::IslandOpen => config.island_open,
            Self::IslandClose => config.island_close,
            Self::LauncherOpen => config.launcher_open,
            Self::LauncherClose => config.launcher_close,
            Self::CircleEnter | Self::TrayEnter | Self::MediaEnter => config.circle_enter,
            Self::CircleExit | Self::TrayExit | Self::MediaExit => config.circle_exit,
            Self::HoverEnter => config.hover_enter,
            Self::ContainerExpand => config.container_expand,
            Self::ContainerCollapse => config.container_collapse,
            Self::ContentIn => config.content_in,
            Self::ContentOut => config.content_out,
            Self::IslandFadeIn => config.island_fade_in,
            Self::IslandFadeOut => config.island_fade_out,
        }
    }

    pub fn resolve(self, config: Option<MotionConfig>, enabled: bool, legacy_ms: u32) -> Profile {
        let profile = self.defaults(config.is_some());
        let Some(config) = config else {
            return profile.with_timing(enabled, (legacy_ms != 280).then_some(legacy_ms));
        };
        let settings = self.settings(config);
        let profile = profile.with_timing(enabled, settings.duration_ms);
        Profile {
            duration: scale_duration(profile.duration, config.duration_scale),
            easing: settings.easing.map_or(profile.easing, Easing::from),
        }
    }
}

impl From<MotionEasing> for Easing {
    fn from(value: MotionEasing) -> Self {
        match value {
            MotionEasing::Standard => Self::Standard,
            MotionEasing::StandardDecelerate => Self::StandardDecelerate,
            MotionEasing::StandardAccelerate => Self::StandardAccelerate,
            MotionEasing::Emphasized => Self::Emphasized,
            MotionEasing::EmphasizedAccelerate => Self::EmphasizedAccelerate,
        }
    }
}

/// Round once to milliseconds and saturate to GTK's duration range. This also
/// keeps huge (but finite) user multipliers from overflowing clock arithmetic.
pub fn scale_duration(duration: Duration, scale: f64) -> Duration {
    let milliseconds = (duration.as_secs_f64() * 1000.0 * scale).round();
    Duration::from_millis(u64::from(milliseconds as u32))
}

/// Global timing for GTK-owned transitions with their own fixed easing.
pub fn auxiliary_duration_ms(
    config: Option<MotionConfig>,
    enabled: bool,
    legacy_ms: u32,
    default_ms: u32,
) -> u32 {
    if !enabled {
        return 0;
    }
    config.map_or(legacy_ms, |config| {
        scale_duration(
            Duration::from_millis(u64::from(default_ms)),
            config.duration_scale,
        )
        .as_millis() as u32
    })
}

/// The subset of Material 3 duration tokens used by the shell profiles.
pub mod duration {
    use std::time::Duration;

    pub const SHORT2: Duration = Duration::from_millis(100);
    pub const SHORT3: Duration = Duration::from_millis(150);
    pub const SHORT4: Duration = Duration::from_millis(200);
    pub const MEDIUM2: Duration = Duration::from_millis(300);
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
    pub const PEEK_ENTER: Self = Self {
        duration: duration::MEDIUM4,
        easing: Easing::Standard,
    };
    pub const PEEK_EXIT: Self = Self {
        duration: duration::MEDIUM2,
        easing: Easing::Standard,
    };
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
    #[test]
    fn configured_motion_preserves_ratios_and_scales_overrides_last() {
        use super::{Transition, scale_duration};
        use crate::config::{MotionConfig, MotionEasing, MotionOverride};
        let motion = MotionConfig {
            duration_scale: 1.25,
            peek_enter: MotionOverride {
                duration_ms: Some(600),
                easing: Some(MotionEasing::StandardAccelerate),
            },
            ..MotionConfig::default()
        };
        let enter = Transition::PeekEnter.resolve(Some(motion), true, 320);
        assert_eq!(enter.duration, Duration::from_millis(750));
        assert_eq!(enter.easing, Easing::StandardAccelerate);
        assert_eq!(
            Transition::IslandOpen
                .resolve(Some(motion), true, 320)
                .duration,
            Duration::from_millis(625)
        );
        assert_eq!(
            Transition::IslandClose
                .resolve(Some(motion), true, 320)
                .duration,
            Duration::from_millis(500)
        );
        assert_eq!(
            Transition::PeekExit
                .resolve(Some(motion), true, 320)
                .duration,
            Duration::from_millis(375)
        );
        assert_eq!(
            scale_duration(duration::ISLAND_ENTER_FADE_DELAY, motion.duration_scale),
            Duration::from_millis(125)
        );
        // Legacy zero is ignored once the new table is present; the real
        // animation-disable switch still takes precedence over every override.
        assert_eq!(Transition::PeekEnter.resolve(Some(motion), true, 0), enter);
        assert!(
            Transition::PeekEnter
                .resolve(Some(motion), false, 320)
                .duration
                .is_zero()
        );
        assert!(
            Transition::PeekEnter
                .resolve(
                    Some(MotionConfig {
                        duration_scale: 0.0,
                        ..motion
                    }),
                    true,
                    320
                )
                .duration
                .is_zero()
        );
        assert!(
            Transition::PeekExit
                .resolve(
                    Some(MotionConfig {
                        peek_exit: MotionOverride {
                            duration_ms: Some(0),
                            easing: None
                        },
                        ..motion
                    }),
                    true,
                    320
                )
                .duration
                .is_zero()
        );
        assert_eq!(
            Transition::PeekEnter.resolve(None, true, 280),
            Profile::ISLAND_EXPAND
        );
        assert_eq!(
            Transition::PeekEnter.resolve(None, true, 320).duration,
            Duration::from_millis(320)
        );
        assert!(
            Transition::PeekEnter
                .resolve(None, true, 0)
                .duration
                .is_zero()
        );
        assert_eq!(
            scale_duration(Duration::from_millis(u64::from(u32::MAX)), f64::MAX),
            Duration::from_millis(u64::from(u32::MAX))
        );
    }

    #[test]
    fn named_overrides_route_independently() {
        use super::Transition;
        let config: crate::config::AppConfig = toml::from_str(
            r#"
            [shell.motion]
            duration_scale = 2.0
            [shell.motion.peek_enter]
            duration_ms = 111
            [shell.motion.peek_exit]
            duration_ms = 112
            [shell.motion.island_open]
            duration_ms = 113
            [shell.motion.island_close]
            duration_ms = 114
            [shell.motion.launcher_open]
            duration_ms = 115
            [shell.motion.launcher_close]
            duration_ms = 116
            [shell.motion.circle_enter]
            duration_ms = 117
            [shell.motion.circle_exit]
            duration_ms = 118
        "#,
        )
        .unwrap();
        for (transition, expected) in [
            (Transition::PeekEnter, 222),
            (Transition::PeekExit, 224),
            (Transition::IslandOpen, 226),
            (Transition::IslandClose, 228),
            (Transition::LauncherOpen, 230),
            (Transition::LauncherClose, 232),
            (Transition::CircleEnter, 234),
            (Transition::CircleExit, 236),
            (Transition::TrayEnter, 234),
            (Transition::TrayExit, 236),
            (Transition::MediaEnter, 234),
            (Transition::MediaExit, 236),
        ] {
            assert_eq!(
                transition.resolve(config.shell.motion, true, 320).duration,
                Duration::from_millis(expected)
            );
        }
        // Overriding a surface does not flatten the shared content tracks.
        assert_eq!(
            Transition::ContentIn
                .resolve(config.shell.motion, true, 320)
                .duration,
            Duration::from_millis(300)
        );
        assert_eq!(
            Transition::ContentOut
                .resolve(config.shell.motion, true, 320)
                .duration,
            Duration::from_millis(200)
        );
    }

    #[test]
    fn tray_defaults_follow_peek_and_respect_disabled_motion() {
        for config in [None, Some(crate::config::MotionConfig::default())] {
            for (tray, peek) in [
                (Transition::TrayEnter, Transition::PeekEnter),
                (Transition::TrayExit, Transition::PeekExit),
            ] {
                assert_eq!(
                    tray.resolve(config, true, 280),
                    peek.resolve(config, true, 280)
                );
                assert!(tray.resolve(config, false, 280).duration.is_zero());
            }
        }
    }

    #[test]
    fn media_motion_is_symmetric_and_can_be_disabled() {
        for config in [None, Some(crate::config::MotionConfig::default())] {
            for transition in [Transition::MediaEnter, Transition::MediaExit] {
                assert_eq!(transition.resolve(config, true, 280), Profile::PEEK_ENTER);
                assert!(transition.resolve(config, false, 280).duration.is_zero());
            }
        }
    }

    #[test]
    fn auxiliary_transitions_honor_global_scale_and_disable() {
        use super::auxiliary_duration_ms;
        use crate::config::MotionConfig;
        let motion = Some(MotionConfig {
            duration_scale: 1.5,
            ..MotionConfig::default()
        });
        assert_eq!(auxiliary_duration_ms(motion, true, 320, 280), 420);
        assert_eq!(auxiliary_duration_ms(motion, false, 320, 280), 0);
        assert_eq!(auxiliary_duration_ms(None, true, 320, 280), 320);
    }

    use super::*;

    const EASINGS: [Easing; 5] = [
        Easing::Emphasized,
        Easing::Standard,
        Easing::StandardDecelerate,
        Easing::StandardAccelerate,
        Easing::EmphasizedAccelerate,
    ];
    const PROFILES: [Profile; 11] = [
        Profile::PEEK_ENTER,
        Profile::PEEK_EXIT,
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
