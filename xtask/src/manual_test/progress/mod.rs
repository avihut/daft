//! ProgressSink port: live lifecycle events for the runner.
//!
//! Parallel to [`super::reporter::Reporter`]. While `Reporter` owns the
//! per-scenario byte stream (header, step lines, footer) and is called by
//! workers writing to their own `Vec<u8>` buffers, `ProgressSink` owns the
//! out-of-band live state — what scenarios are in flight, what step each is
//! on, how many have completed, how many failed — and is called for its
//! side effects.
//!
//! Two impls ship: `NoopProgressSink` for non-TTY runs (CI logs, redirected
//! output, the runner-output YAML smoke that goes through `cargo run`), and
//! `IndicatifProgressSink` (added in a later step) for TTY runs with a
//! pinned multi-row progress region at the bottom of the terminal.
//!
//! Both ports stay narrow on purpose. Mixing event-stream and byte-stream
//! concerns into one trait would muddy the seam and make the non-TTY path
//! (byte-identical to the pre-PR behavior) hard to preserve.

use std::time::Duration;

use super::reporter::ScenarioStatus;

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

    /// Called when a scenario reaches its footer (pass or fail), with the
    /// wall-clock duration of its step phase.
    fn scenario_finished(&self, name: &str, status: ScenarioStatus, duration: Duration);

    /// Called once at the end of the run, after the summary block.
    fn run_finished(&self);
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
    fn scenario_finished(&self, _name: &str, _status: ScenarioStatus, _duration: Duration) {}
    fn run_finished(&self) {}
}

/// Pick a sink based on whether the runner should show live progress.
///
/// `show_progress` is the orchestrator's already-resolved decision (TTY
/// detection + `NO_PROGRESS` / `CI` env-var overrides); this function
/// doesn't re-probe the environment.
///
/// Returns `Box<dyn ProgressSink>` because the live impl (added in a later
/// step) carries indicatif state that the no-op impl doesn't, so the two
/// can't share a single concrete type.
pub fn progress_sink_for(show_progress: bool) -> Box<dyn ProgressSink> {
    // Live sink lands in a follow-up commit. Until then both branches return
    // the no-op — the call site is wired but behavior is unchanged.
    let _ = show_progress;
    Box::new(NoopProgressSink)
}

/// Run `f` with the sink's live region suspended so a multi-line block can
/// be written to stderr without bar tearing.
///
/// Used by the orchestrator to print the final summary block. On
/// `NoopProgressSink` this just calls `f` directly; on the live sink it
/// hides the bars, runs `f`, redraws.
///
/// Returns the closure's result so callers can `?`-propagate errors from
/// the writes inside it. Default impl on the trait would be the natural
/// shape, but trait methods can't take `FnOnce` closures generic in the
/// impl without extra plumbing — a free function specializing on the
/// concrete sink behind the trait object keeps the surface flat.
pub fn suspend_for_summary<R>(_sink: &dyn ProgressSink, f: impl FnOnce() -> R) -> R {
    // Live sink lands in a follow-up commit; for now Noop is the only impl
    // and there's nothing to suspend.
    f()
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
        sink.scenario_finished("example", ScenarioStatus::Pass, Duration::from_millis(120));
        sink.run_finished();
        // The contract is "no panic, no observable effect" — reaching this
        // line satisfies it.
    }

    #[test]
    fn progress_sink_for_returns_noop_when_disabled() {
        // Both branches currently return Noop; assert by exercising every
        // method. Once the live sink lands, this test will be updated to
        // distinguish the two impls.
        let sink = progress_sink_for(false);
        sink.run_started(0);
        sink.scenario_started("x", 1);
        sink.step_started("x", 0, 1, "s");
        sink.scenario_finished("x", ScenarioStatus::Fail, Duration::ZERO);
        sink.run_finished();
    }

    #[test]
    fn suspend_for_summary_invokes_closure_on_noop() {
        let sink = NoopProgressSink;
        let result = suspend_for_summary(&sink, || 42);
        assert_eq!(result, 42);
    }
}
