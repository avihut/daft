//! ProgressSink port: live lifecycle events for the runner.
//!
//! Parallel to [`super::reporter::Reporter`]. While `Reporter` owns the
//! per-scenario byte stream (header, step lines, footer) and is called by
//! workers writing to their own `Vec<u8>` buffers, `ProgressSink` owns the
//! out-of-band live state — what scenarios are in flight, what step each is
//! on, how many have completed, how many failed — and is called for its
//! side effects.
//!
//! Two impls ship: [`NoopProgressSink`] for non-TTY runs (CI logs,
//! redirected output, the runner-output YAML smoke that goes through
//! `cargo run`), and [`IndicatifProgressSink`] for TTY runs with a
//! pinned multi-row progress region at the bottom of the terminal.
//!
//! Both ports stay narrow on purpose. Mixing event-stream and byte-stream
//! concerns into one trait would muddy the seam and make the non-TTY path
//! (byte-identical to the pre-PR behavior) hard to preserve.

mod indicatif_sink;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::reporter::ScenarioStatus;

pub use indicatif_sink::IndicatifProgressSink;

/// Cooperative cancellation flag shared between the SIGINT handler and the
/// runner / sink.
///
/// On first Ctrl+C the handler calls [`Self::set`]; the runner checks
/// [`Self::is_set`] between scenario steps and bails with
/// [`ScenarioStatus::Cancelled`] when it sees the flag flip. The flag is also
/// passed to [`IndicatifProgressSink`] so the live region's `M cancelled`
/// segment can render from the same source of truth as the runner's bail
/// logic.
///
/// Cheap to clone — wraps a single `Arc<AtomicBool>`. All loads / stores use
/// `Ordering::Relaxed`; the flag is one-way (`false` → `true`) and no other
/// memory effects are ordered around it.
#[derive(Clone, Default)]
pub struct InterruptFlag(Arc<AtomicBool>);

impl InterruptFlag {
    pub fn new() -> Self {
        Self::default()
    }

    /// `true` once a SIGINT has been observed.
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Mark the run as cancelled. Idempotent; further calls are no-ops.
    /// Returns the prior value so the SIGINT handler can distinguish first
    /// (soft cancel) from subsequent (hard exit) presses.
    pub fn set(&self) -> bool {
        self.0.swap(true, Ordering::Relaxed)
    }
}

/// Receives lifecycle events from the runner.
///
/// Implementations must be `Send + Sync` because parallel rayon workers
/// call into the same sink from multiple threads. They must also be
/// **cheap when called in a hot loop** — `step_started` fires once per
/// step across every scenario, so anything past a couple of map lookups
/// belongs behind an interior `Mutex` rather than in the calling thread.
pub trait ProgressSink: Send + Sync {
    /// Called once before any scenarios start, with the total to expect.
    fn run_started(&self, total_scenarios: usize);

    /// Called when a worker picks up a scenario.
    fn scenario_started(&self, name: &str, total_steps: usize);

    /// Called before each step's command runs. `idx` is zero-based.
    fn step_started(&self, scenario_name: &str, idx: usize, total: usize, step_name: &str);

    /// Called when a worker finishes a scenario, with the full buffered
    /// output and the determined status.
    ///
    /// Replaces the earlier `scenario_finished` + `println` split, which
    /// raced: `scenario_finished` ran on the worker thread (removed the
    /// row from the multi) while `multi.println` for the footer ran on
    /// the main thread (cleared lines based on the multi's current count).
    /// Concurrent reordering left in-flight rows stranded in scrollback.
    ///
    /// The sink owns the atomic sequence:
    ///   1. Print `buf` line-by-line above the live region.
    ///   2. Remove the scenario's row from the live region.
    ///   3. Update the running/failed/cancelled counters and tick the
    ///      summary forward.
    ///
    /// All three steps land on the calling thread (the worker), so the
    /// remove happens after the print — no cross-thread reordering, no
    /// stranded ghost rows.
    ///
    /// `buf` is the scenario's accumulated stderr-style bytes (header +
    /// step lines + footer + any cleanup note). On `NoopProgressSink` the
    /// buf is *ignored* — the orchestrator drains buffers in input order
    /// at end-of-run for the non-TTY path, so printing here would
    /// duplicate output.
    fn complete_scenario(&self, name: &str, status: ScenarioStatus, duration: Duration, buf: &[u8]);

    /// Called once at the end of the run, after the summary block.
    fn run_finished(&self);

    /// Force a refresh of the summary's "running / failed / cancelled"
    /// message so the `(cancelling)` suffix appears immediately after
    /// Ctrl+C instead of waiting for the first worker to bail
    /// (potentially seconds on a slow step). No-op for the noop sink.
    fn notify_cancelling(&self);
}

/// No-op sink for non-TTY runs.
///
/// Every method is a no-op so the runner can call into it from the same
/// code path as the live sink without branching. The compiler optimizes
/// the calls away in release builds.
pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn run_started(&self, _total_scenarios: usize) {}
    fn scenario_started(&self, _name: &str, _total_steps: usize) {}
    fn step_started(&self, _scenario_name: &str, _idx: usize, _total: usize, _step_name: &str) {}
    fn complete_scenario(
        &self,
        _name: &str,
        _status: ScenarioStatus,
        _duration: Duration,
        _buf: &[u8],
    ) {
        // Intentional no-op. The orchestrator drains buffered outputs in
        // input order at end-of-run for the non-TTY path; printing here
        // would either duplicate or scramble the CI log output.
    }
    fn run_finished(&self) {}
    fn notify_cancelling(&self) {}
}

/// Pick a sink based on whether the runner should show live progress.
///
/// `show_progress` is the orchestrator's already-resolved decision (TTY
/// detection + `NO_PROGRESS` / `CI` env-var overrides); this function
/// doesn't re-probe the environment.
///
/// `name_col_width` and `step_col_width` are the widest scenario name
/// and the widest `[N/M] step_name` label across the discovered scenario
/// set. The live sink pads each in-flight row's columns to these widths
/// so spinner+name, step label, and elapsed line up across rows. Pass
/// `0` for either to disable that column's padding.
///
/// Returns `Box<dyn ProgressSink>` because the live impl carries indicatif
/// state that the no-op impl doesn't, so the two can't share a single
/// concrete type.
pub fn progress_sink_for(
    show_progress: bool,
    name_col_width: usize,
    step_col_width: usize,
    interrupt: InterruptFlag,
) -> Box<dyn ProgressSink> {
    if show_progress {
        Box::new(IndicatifProgressSink::new(
            name_col_width,
            step_col_width,
            interrupt,
        ))
    } else {
        Box::new(NoopProgressSink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_sink_swallows_every_call() {
        let sink = NoopProgressSink;
        sink.run_started(10);
        sink.scenario_started("example", 3);
        sink.step_started("example", 0, 3, "first step");
        sink.step_started("example", 1, 3, "second step");
        sink.complete_scenario(
            "example",
            ScenarioStatus::Pass,
            Duration::from_millis(120),
            b"",
        );
        sink.run_finished();
        // The contract is "no panic, no observable effect" — reaching this
        // line satisfies it.
    }

    #[test]
    fn progress_sink_for_picks_noop_when_disabled() {
        // We can't easily assert "this is concretely a NoopProgressSink"
        // through a dyn pointer; exercise every method instead and rely on
        // the IndicatifProgressSink unit tests (in indicatif_sink.rs) to
        // cover the live path.
        let sink = progress_sink_for(false, 0, 0, InterruptFlag::new());
        sink.run_started(0);
        sink.scenario_started("x", 1);
        sink.step_started("x", 0, 1, "s");
        sink.complete_scenario("x", ScenarioStatus::Fail, Duration::ZERO, b"");
        sink.run_finished();
    }

    #[test]
    fn interrupt_flag_is_set_after_set() {
        let flag = InterruptFlag::new();
        assert!(!flag.is_set());
        let prior = flag.set();
        assert!(!prior);
        assert!(flag.is_set());
        // Second set is a no-op; returns the prior value (true).
        assert!(flag.set());
        assert!(flag.is_set());
    }

    #[test]
    fn interrupt_flag_clone_shares_state() {
        let a = InterruptFlag::new();
        let b = a.clone();
        assert!(!a.is_set());
        assert!(!b.is_set());
        a.set();
        assert!(a.is_set());
        assert!(b.is_set());
    }

    #[test]
    fn noop_complete_scenario_ignores_buf() {
        // NoopProgressSink intentionally drops the buf — the orchestrator
        // drains buffers in input order at end-of-run for the non-TTY
        // path, so a sink that printed here would duplicate output. The
        // contract is "no panic, no observable effect".
        let sink = NoopProgressSink;
        sink.complete_scenario("x", ScenarioStatus::Pass, Duration::ZERO, b"some content\n");
    }
}
