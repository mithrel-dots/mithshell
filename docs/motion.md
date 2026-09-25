# Transition motion contract

`src/ui/motion.rs` provides clock-independent transition profiles for island,
launcher, and circle integration. All durations below are milliseconds.

## Tokens and provenance

The definitions below are legacy Material Design 3 duration/easing tokens for
these transitions, not the newer Material Expressive physics system. Numerical
values were verified against the first-party M3 documentation (content version
`2026-09-16_06-10-03`) on 2026-09-22:
The shell uses them as named profiles; the use-case mapping is a project choice,
not a claim that Google prescribes these timings for this custom surface.

- https://m3.material.io/styles/motion/easing-and-duration/tokens-specs
- https://m3.material.io/styles/motion/easing-and-duration/applying-easing-and-duration

Duration subset: `short2 = 100`, `short3 = 150`, `short4 = 200`,
`medium4 = 400`, `long2 = 500`.

| Easing token | CSS cubic-bezier control points |
| --- | --- |
| standard | `(0.2, 0, 0, 1)` |
| standard-decelerate | `(0, 0, 0, 1)` |
| standard-accelerate | `(0.3, 0, 1, 1)` |
| emphasized-accelerate | `(0.3, 0, 0.8, 0.15)` |

The distinct **emphasized** path is sampled as two cubic segments joined at
`(0.166666, 0.4)`, rather than approximated by a single cubic-bezier.
The emphasized-accelerate token is a cubic-bezier. These profiles introduce
no springs or overshoot.

## Chosen shell profiles

These use-case assignments are project design choices using the tokens above;
they are **not Google's prescribed component mappings**. Persistent container
expansion and collapse are undirected movement, so they use `standard`, the
documented fallback when an emphasized direction cannot be selected. The 500 ms
and 200 ms container durations are explicit shell choices. Likewise, 150 ms
hover timing is a shell choice rather than a universal Google rule.

| `Profile` constant | Duration token | Duration | Easing |
| --- | --- | ---: | --- |
| `HOVER_ENTER` | short3 | 150 | standard |
| `CONTAINER_EXPAND` | long2 | 500 | standard (undirected emphasized fallback) |
| `CONTAINER_COLLAPSE` | short4 | 200 | standard (undirected emphasized fallback) |
| `CONTENT_IN` | short3 | 150 | standard-decelerate |
| `CONTENT_OUT` | short2 | 100 | standard-accelerate |
| `ISLAND_EXPAND` | long2 | 500 | emphasized (two-segment path) |
| `ISLAND_COLLAPSE` | medium4 | 400 | emphasized-accelerate |
| `ISLAND_FADE_IN` | short4 | 200 | standard |
| `ISLAND_FADE_OUT` | short4 | 200 | standard |

Hover is a small, quickly reversible depth change. Container profiles give a
larger expansion room to settle while making collapse quicker. Content profiles
are for opacity tracks.
Replacement content fades out before the replacement fades in by default; use a
brief intentional crossfade only when the transition calls for it. The caller
owns sequencing, overlap, visibility, and hit testing. The profile API does not
infer which direction a scalar is moving: a collapse uses the collapse profile
even when a particular coordinate increases.

## Integration and compatibility

```rust,ignore
use std::time::Duration;
use crate::ui::motion::Profile;

// Presentation resolves the historical default 280 to the named profile;
// non-default values (including zero) remain exact overrides.
let geometry = Profile::CONTAINER_EXPAND
    .with_timing(animations_enabled, (shell.animation_ms != 280).then_some(shell.animation_ms));

// Use the profile's Material duration when token timing is explicitly selected.
let content = Profile::CONTENT_IN.with_timing(animations_enabled, None);

// Convert a GTK frame-clock delta defensively, keeping outputs in this clock.
let elapsed = Duration::from_micros(now_us.saturating_sub(start_us).max(0) as u64);
let eased = geometry.progress(elapsed); // share this across geometry coordinates
let opacity = start_opacity + (1.0 - start_opacity) * content.progress(elapsed);
let geometry_finished = geometry.is_complete(elapsed);
```

`with_timing(enabled, None)` retains the profile duration;
`with_timing(enabled, Some(ms))` replaces it exactly without changing easing.
`enabled = false` always produces zero duration. A zero-duration profile returns
progress **1 even at elapsed zero**. Apply
final geometry/content/visibility synchronously in that case instead of waiting
for a tick. Use `is_complete(elapsed)` for lifecycle completion; floating
point eased progress can round to one just before the clock's endpoint.

The existing `shell.animation_ms` is a defaulted `u32` (280), so the runtime
value cannot distinguish an omitted default from an explicitly configured 280.
Presentation call sites use 280 as the compatibility default for named profiles;
any other positive value is an exact override, and zero/disabled animation is
always immediate. This convention is documented rather than hidden, and an
explicit token-timing config can remove the ambiguity later.

For interruption, cancel/invalidate the old tick generation, capture the current
rendered geometry and opacities, and restart elapsed at zero with those values
as the new start. Samples preserve position continuity, including reversal;
they do not preserve velocity or automatically shorten a partial reversal.
When tracks have different durations, complete the transition only when all
required tracks finish. The caller retains ownership of callbacks, clocks,
generation checks, widget visibility, and input regions.

`Easing::sample` accepts normalized elapsed time: NaN and negative time map to
zero, positive infinity and time at/above one map to one. Interior samples invert
the Bezier x coordinate with bounded bisection; they do not confuse the curve
parameter with elapsed time. `Duration` avoids negative/NaN durations and
elapsed times.

## Verification scope

Unit tests cover analytic Bezier points, exact endpoints, boundedness,
monotonicity, nonfinite normalized time, duration overrides, immediate completion,
and large durations. The island presentation additionally tests
position-continuous reversal and reversible hover geometry, and uses
generation-cancelled GTK frame tracks. Clipping, focus, and compositor input
regions still need live desktop review.
Continuous battery-wave motion is not transition easing; it is outside this API.

## Island presentation notes

The compact/media pill uses one cancellable geometry track for content width,
tray visibility, and hover depth. A fixed neutral GTK hover region sits behind
the moving pill; its allocation is stable while the compositor input region is
kept in the same scaled bounds. This avoids relying on padding alone to keep
GTK motion events stable. Integrated launcher presentation uses `View::Search`
on the fixed island canvas, while independent presentation keeps its separate
search surface and persistent pill.

Transition choreography guidance: https://m3.material.io/styles/motion/transitions/applying-transitions

## Native GTK island choreography

Use `Profile::ISLAND_EXPAND` for the island container's expanding geometry and
`Profile::ISLAND_COLLAPSE` for collapsing geometry. These are explicit
Material easing tokens: the full emphasized path for entry and
emphasized-accelerate for exit. The 500/400 ms durations are shell-specific
choices tuned after native visual feedback; collapse is intentionally slower
than the original 200 ms exit so the shared header and panel remain legible.
There are no springs. Animate the container transform
and any coupled bounds from a shared profile progress sample.

Peek hardware is painted at full opacity before expansion starts; the rolling
viewport reveals and conceals it. It does not wait for an opacity entrance.
New Open-only content is also opaque before expansion begins; its clipped
allocation reveals it. Date opacity uses `Profile::ISLAND_FADE_IN` and
`Profile::ISLAND_FADE_OUT`. Open-only sections retain clipped allocations while
their opacity, height, and spacing contract, so closing to Peek never abruptly
unmounts a card or jumps the hardware row. On an enabled date enter, begin the fade-in after
`duration::ISLAND_ENTER_FADE_DELAY` (the `short2`, 100 ms token), measured from
the start of the container expansion; sequence the fade so it can finish with
the longer transform. Do not apply that delay for reduced motion, when
animations are disabled, or when the resolved transition duration is zero:
apply final opacity and geometry synchronously. `with_timing(false, ...)`
resolves each profile to zero duration, so callers should also suppress the
enter delay in that case. The delay is a choreography token, not part of the
profile duration.

Geometry advances on the mapped root's GTK frame clock. Reconciliation compares
active destinations as well as the rendered geometry: unchanged telemetry/media
updates keep the current clock, while actual reversals start from the rendered
geometry and opacity. The input bridge above a lifted Peek remains active for
the whole rollout so the original hover point cannot trigger spurious exits.
