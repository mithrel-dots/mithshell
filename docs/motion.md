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

Duration subset: `short2 = 100`, `short3 = 150`, `short4 = 200`, `medium2 = 300`,
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
| `PEEK_ENTER` | medium4 | 400 | standard |
| `PEEK_EXIT` | medium2 | 300 | standard |
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

## Configuration

`shell.motion` is optional. Its presence opts into named defaults; its absence
preserves legacy `shell.animation_ms` behavior. A nested override alone also
counts as opting in. Serialization preserves this distinction.

```toml
[shell.motion]
duration_scale = 1.25

[shell.motion.peek_enter]
duration_ms = 400
easing = "standard"

[shell.motion.peek]
lift = 8.0
width_scale = 1.0
```

Resolution order: select the named default, apply optional `duration_ms` and
`easing`, then multiply the duration by `duration_scale`. The animation-disable
switch wins over every override. A zero scale makes all tracks and delays
instant; a zero per-track duration makes that track instant. Replacement
transitions still wait for their other required tracks to finish.

`duration_scale` defaults to 1, accepts finite nonnegative numbers, and means
**longer/slower** above 1. Durations are rounded to milliseconds after scaling
and saturate at `u32::MAX` milliseconds. `duration_ms` accepts a nonnegative
32-bit integer. Unknown fields, invalid easings, and invalid geometry fail
configuration parsing before reload tears down any windows.

Each table below accepts `duration_ms` and/or `easing`. Unspecified properties
retain their defaults; overriding one transition never changes another.

| Table under `shell.motion` | Default ms | Default easing | Applies to |
| --- | ---: | --- | --- |
| `peek_enter` | 400 | standard | Idle → hover peek |
| `peek_exit` | 300 | standard | Hover peek → idle |
| `island_open` | 500 | emphasized | Open dashboard |
| `island_close` | 400 | emphasized-accelerate | Close dashboard |
| `launcher_open` | 500 | standard | Independent and integrated launcher |
| `launcher_close` | 200 | standard | Independent and integrated launcher |
| `circle_enter` | 500 | standard | Circle expansion, including full pages |
| `circle_exit` | 200 | standard | Circle contraction |
| `hover_enter` | 150 | standard | Small tray/media hover changes |
| `container_expand` | 500 | standard | Other container expansions |
| `container_collapse` | 200 | standard | Other container collapses |
| `content_in` | 150 | standard-decelerate | Shared incoming content |
| `content_out` | 100 | standard-accelerate | Shared outgoing content |
| `island_fade_in` | 200 | standard | Island/date entrance opacity |
| `island_fade_out` | 200 | standard | Island/date exit opacity |

The tray circle derives its default geometry profiles from Peek (400/300 ms,
standard with `shell.motion`, or the legacy pill profiles). Explicit
`circle_enter`/`circle_exit` overrides still apply to it. Its backdrop stays
opaque while the compact count and expanded icons crossfade inside it; geometry
runs on the same GTK frame clock as the pill. Repeated tray snapshots preserve
the active transition, and reversals start from the last painted bounds.

The media circle uses symmetric 400 ms standard-eased expansion and collapse,
also configurable through `circle_enter`/`circle_exit`. One persistent artwork
tile grows to three times its compact size inside a 320 × 136 design-unit card.
Its circular progress track unrolls into a horizontal bar beneath the artwork
and controls, spanning the card minus its 12-unit side padding. Progress starts
at the top and runs counterclockwise. Two curls open around a growing horizontal
segment, sharing one playback fraction by path length and joining tangentially
into the final left-to-right bar.
Artwork, track shape, and container use the same reversible geometry progress.
The compact ring hugs the artwork's outer edge. Its stroke thickens from 1.25
to 2.5 design units as it unrolls, then thins back on collapse.
The expanded bar sits 6 design units below the artwork, with an elapsed / total
time readout beneath it. The readout fades in near the end of expansion and uses
the same playback clock as the ring; unknown durations display `--:--` while
elapsed time continues advancing during playback.
The controls stay mounted until the container has closed; media snapshots and
reversals preserve the current artwork position and progress-track shape.

For a slower unravel and wrap-back, set longer circle durations:

```toml
[shell.motion.circle_enter]
duration_ms = 700

[shell.motion.circle_exit]
duration_ms = 700
```

These overrides also apply to tray and notification circles. The media card,
artwork, and progress track stay synchronized; `duration_scale` multiplies these
durations afterward. Save the config and run `mithshell reload` to apply timing
changes.

Allowed easing strings are `standard`, `standard-decelerate`,
`standard-accelerate`, `emphasized`, and `emphasized-accelerate`.

The global scale also applies to the lock fade (280 ms base), audio visualizer
reveal (280 ms), launcher result/plugin crossfade (160 ms), and compact-pill
CSS color/shadow transitions (150 ms); these retain their existing easing.
Continuous audio/battery motion and interaction/notification timeouts are not
transition durations. Zero scale freezes the battery wave, like disabled motion.

### Peek geometry

`shell.motion.peek` controls distance independently of time:

- `lift`: default 8, range 0–128, in logical pixels multiplied by UI density.
  Zero places the header at its resting vertical origin. The open dashboard
  retains this lift so its persistent header does not jump between states.
- `width_scale`: default 1, greater than 0 and at most 4. Multiplies the 500px
  reference peek width after density normalization. Content minimums and screen
  bounds still apply, so reducing it cannot force hardware content to fit in an
  arbitrarily narrow panel. It does not scale text or change open-dashboard width.

Legacy configs retain a lift of 16 and a width multiplier of 1.1. The new
defaults intentionally soften peek while keeping the emphasized open animation.

## Integration and compatibility

```rust,ignore
use std::time::Duration;
use crate::ui::motion::Profile;

use crate::ui::motion::Transition;

let geometry = Transition::PeekEnter
    .resolve(shell.motion, animations_enabled, shell.animation_ms);

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

When `shell.motion` is absent, `shell.animation_ms` is a defaulted `u32` (280), so the runtime
value cannot distinguish an omitted default from an explicitly configured 280.
Presentation call sites use 280 as the compatibility default for named profiles;
any other positive value is an exact override, and zero/disabled animation is
always immediate. This convention is documented rather than hidden, and an
new `shell.motion` table removes this ambiguity by ignoring the legacy scalar.

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

Resolve `Transition::PeekEnter`/`PeekExit` for hover and
`Transition::IslandOpen`/`IslandClose` for the pinned dashboard. The latter
default to `Profile::ISLAND_EXPAND` and `Profile::ISLAND_COLLAPSE`. These are explicit
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
the start of the container expansion, multiplied by the global duration scale;
sequence the fade so it can finish with
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
