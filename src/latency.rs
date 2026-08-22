//! Env-gated end-to-end latency tracing for the TarraGon search path.
//!
//! Enable with `MITHSHELL_TRACE_LATENCY=1`. When disabled every `mark_*` call
//! is a single relaxed atomic load and an early return, so the hooks can stay
//! compiled into release builds permanently.
//!
//! The pipeline is measured as five consecutive spans plus a total:
//!
//! | span      | from -> to                        | measures                        |
//! |-----------|-----------------------------------|---------------------------------|
//! | `debounce`| keystroke -> query dispatched     | GTK search-delay + own debounce |
//! | `write`   | dispatched -> socket flushed      | command channel + writer thread |
//! | `backend` | socket flushed -> snapshot parsed | TarraGon round trip             |
//! | `build`   | snapshot -> widgets updated       | main-thread rendering work      |
//! | `paint`   | widgets updated -> frame on screen| GTK/compositor paint            |
//! | `total`   | keystroke -> frame on screen      | true perceived latency          |
//!
//! Only the *first* snapshot of a query is timed, because TarraGon streams one
//! update per plugin completion and the user perceives the first paint.

use std::{
    env,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// Upper bound on retained samples per span, keeping the tracker's footprint
/// bounded during long benchmark runs.
const MAX_SAMPLES: usize = 20_000;

static ENABLED: AtomicBool = AtomicBool::new(false);
static INIT: OnceLock<()> = OnceLock::new();

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

/// Reads `MITHSHELL_TRACE_LATENCY` once and reports whether tracing is active.
pub fn init() -> bool {
    INIT.get_or_init(|| {
        let enabled = env::var("MITHSHELL_TRACE_LATENCY")
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        ENABLED.store(enabled, Ordering::Relaxed);
    });
    enabled()
}

#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

#[derive(Debug, Default)]
struct State {
    keystroke: Option<Instant>,
    dispatch: Option<Instant>,
    write: Option<Instant>,
    arrival: Option<Instant>,
    build: Option<Instant>,
    /// Set once the first frame of a query is painted so later streamed
    /// snapshots for the same query are not counted again.
    settled: bool,
    spans: Spans,
}

#[derive(Debug, Default)]
struct Spans {
    debounce: Vec<u64>,
    write: Vec<u64>,
    backend: Vec<u64>,
    build: Vec<u64>,
    paint: Vec<u64>,
    total: Vec<u64>,
}

fn push(samples: &mut Vec<u64>, micros: u64) {
    if samples.len() < MAX_SAMPLES {
        samples.push(micros);
    }
}

fn elapsed_micros(from: Instant, to: Instant) -> u64 {
    to.saturating_duration_since(from).as_micros() as u64
}

impl State {
    fn keystroke(&mut self, now: Instant) {
        // Deliberately does not clear `settled` or `build`: doing so would let
        // an unrelated frame painted before the next dispatch be timed against
        // the previous query's build timestamp.
        self.keystroke = Some(now);
    }

    fn dispatch(&mut self, now: Instant) {
        if let Some(keystroke) = self.keystroke {
            let micros = elapsed_micros(keystroke, now);
            push(&mut self.spans.debounce, micros);
        }
        self.dispatch = Some(now);
        self.write = None;
        self.arrival = None;
        self.build = None;
        self.settled = false;
    }

    fn write(&mut self, now: Instant) {
        if self.write.is_some() {
            return;
        }
        if let Some(dispatch) = self.dispatch {
            let micros = elapsed_micros(dispatch, now);
            push(&mut self.spans.write, micros);
        }
        self.write = Some(now);
    }

    fn results(&mut self, now: Instant) {
        if self.settled || self.arrival.is_some() {
            return;
        }
        if let Some(write) = self.write {
            let micros = elapsed_micros(write, now);
            push(&mut self.spans.backend, micros);
        }
        self.arrival = Some(now);
    }

    fn build(&mut self, now: Instant) {
        if self.settled || self.build.is_some() {
            return;
        }
        let Some(arrival) = self.arrival else {
            return;
        };
        let micros = elapsed_micros(arrival, now);
        push(&mut self.spans.build, micros);
        self.build = Some(now);
    }

    fn paint(&mut self, now: Instant) {
        if self.settled {
            return;
        }
        let Some(build) = self.build else {
            return;
        };
        let micros = elapsed_micros(build, now);
        push(&mut self.spans.paint, micros);
        if let Some(keystroke) = self.keystroke {
            let total = elapsed_micros(keystroke, now);
            push(&mut self.spans.total, total);
        }
        // Consume the build timestamp so later frames for the same query, and
        // frames painted for unrelated reasons, are not timed again.
        self.build = None;
        self.settled = true;
    }
}

/// t0: a key that mutates the search text reached the window controller.
pub fn mark_keystroke() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut state) = state().lock() {
        state.keystroke(now);
    }
}

/// t1: the debounce elapsed and the query is handed to the TarraGon client.
pub fn mark_dispatch() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut state) = state().lock() {
        state.dispatch(now);
    }
}

/// t2: the query was written and flushed to the TarraGon socket.
///
/// Called from the TarraGon client thread.
pub fn mark_write() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut state) = state().lock() {
        state.write(now);
    }
}

/// t3: the first snapshot for the in-flight query arrived on the main thread.
pub fn mark_results() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut state) = state().lock() {
        state.results(now);
    }
}

/// t4: the result widgets finished updating on the main thread.
pub fn mark_build() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut state) = state().lock() {
        state.build(now);
    }
}

/// t5: the frame containing those widgets was painted.
pub fn mark_paint() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    if let Ok(mut state) = state().lock() {
        state.paint(now);
    }
}

/// Clears every recorded sample, so a benchmark run starts from a clean slate.
pub fn reset() {
    if let Ok(mut state) = state().lock() {
        *state = State::default();
    }
}

/// Percentile summary of one span, with all values in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub avg_ms: f64,
}

impl Summary {
    fn from_samples(samples: &[u64]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let total: u64 = sorted.iter().sum();
        Some(Self {
            count: sorted.len(),
            min_ms: to_ms(sorted[0]),
            p50_ms: to_ms(percentile(&sorted, 0.50)),
            p95_ms: to_ms(percentile(&sorted, 0.95)),
            p99_ms: to_ms(percentile(&sorted, 0.99)),
            max_ms: to_ms(sorted[sorted.len() - 1]),
            avg_ms: to_ms(total / sorted.len() as u64),
        })
    }

    fn to_json(self) -> serde_json::Value {
        serde_json::json!({
            "count": self.count,
            "min_ms": round2(self.min_ms),
            "avg_ms": round2(self.avg_ms),
            "p50_ms": round2(self.p50_ms),
            "p95_ms": round2(self.p95_ms),
            "p99_ms": round2(self.p99_ms),
            "max_ms": round2(self.max_ms),
        })
    }
}

/// Nearest-rank percentile over an ascending slice.
fn percentile(sorted: &[u64], quantile: f64) -> u64 {
    debug_assert!(!sorted.is_empty());
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[index]
}

fn to_ms(micros: u64) -> f64 {
    micros as f64 / 1000.0
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Machine-readable report for `mithshell latency --json`.
pub fn report() -> serde_json::Value {
    let Ok(state) = state().lock() else {
        return serde_json::json!({ "enabled": enabled(), "error": "tracker poisoned" });
    };
    let spans = &state.spans;
    let mut map = serde_json::Map::new();
    for (name, samples) in [
        ("debounce", &spans.debounce),
        ("write", &spans.write),
        ("backend", &spans.backend),
        ("build", &spans.build),
        ("paint", &spans.paint),
        ("total", &spans.total),
    ] {
        if let Some(summary) = Summary::from_samples(samples) {
            map.insert(name.to_owned(), summary.to_json());
        }
    }
    serde_json::json!({
        "enabled": enabled(),
        "spans": serde_json::Value::Object(map),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_match_expected_positions() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&sorted, 0.50), 50);
        assert_eq!(percentile(&sorted, 0.95), 95);
        assert_eq!(percentile(&sorted, 0.99), 99);
    }

    #[test]
    fn percentiles_are_clamped_for_tiny_samples() {
        let sorted = vec![7];
        assert_eq!(percentile(&sorted, 0.50), 7);
        assert_eq!(percentile(&sorted, 0.99), 7);
    }

    #[test]
    fn summary_reports_milliseconds() {
        let summary = Summary::from_samples(&[1_000, 3_000, 2_000]).unwrap();
        assert_eq!(summary.count, 3);
        assert_eq!(summary.min_ms, 1.0);
        assert_eq!(summary.max_ms, 3.0);
        assert_eq!(summary.p50_ms, 2.0);
        assert_eq!(summary.avg_ms, 2.0);
    }

    #[test]
    fn empty_span_has_no_summary() {
        assert!(Summary::from_samples(&[]).is_none());
    }

    #[test]
    fn samples_are_capped() {
        let mut samples = Vec::new();
        for _ in 0..(MAX_SAMPLES + 10) {
            push(&mut samples, 1);
        }
        assert_eq!(samples.len(), MAX_SAMPLES);
    }

    /// A keystroke arriving after a completed query must not leave the previous
    /// `build` timestamp armed, or stray frames get timed against it and the
    /// paint/total spans collect far more samples than there were queries.
    #[test]
    fn keystroke_does_not_rearm_a_settled_query() {
        let mut state = State::default();
        let base = Instant::now();
        let ms = |n: u64| base + std::time::Duration::from_millis(n);
        state.keystroke(ms(0));
        state.dispatch(ms(1));
        state.write(ms(2));
        state.results(ms(3));
        state.build(ms(4));
        state.paint(ms(5));
        assert!(state.settled);
        assert_eq!(state.spans.paint.len(), 1);
        assert_eq!(state.spans.total.len(), 1);

        // A new keystroke must not clear the settled flag nor resurrect the
        // consumed build timestamp, or stray frames get timed against the
        // previous query.
        state.keystroke(ms(10));
        assert!(state.settled);
        assert!(state.build.is_none());

        // And a paint with no armed build records nothing more.
        let totals_before = state.spans.total.len();
        state.paint(ms(11));
        assert_eq!(state.spans.paint.len(), 1);
        assert_eq!(state.spans.total.len(), totals_before);
    }

    #[test]
    fn disabled_tracker_records_nothing() {
        // `ENABLED` defaults to false and these calls must be inert.
        assert!(!enabled());
        mark_keystroke();
        mark_dispatch();
        mark_paint();
        let report = report();
        assert_eq!(report["enabled"], serde_json::Value::Bool(false));
        assert!(report["spans"].as_object().unwrap().is_empty());
    }
}
