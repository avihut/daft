//! Rail-native presenter for multi-worktree `daft exec` (#533).
//!
//! Translates the [`JobPresenter`] events `run_pipeline_streaming` emits into
//! plan-row stage events on the timeline. Each worktree is a plan row (or a
//! `├─` group of command rows for a pipeline); this presenter maps the
//! stream of `on_job_*` callbacks — arriving out of order across parallel
//! workers, and from the stream-reader threads for output — onto the right
//! [`StepKey`] via a per-name command cursor.
//!
//! Row identity: the presenter and the plan builder must agree on the keys, so
//! both go through [`command_key`]. `run_fleet` is driven with
//! `NameStyle::Label`, so the `name` in every callback is the target's full
//! `repo:branch` label — distinct even when two related repos share a branch.

use crate::core::stage::{StageEvent, StageId, StepKey};
use crate::executor::JobResult;
use crate::executor::presenter::JobPresenter;
use crate::output::timeline::TimelineHandle;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// Normalize a captured child-output line to a single, control-free line
/// before it rides a rail bar or lands in the receipt — a raw line from a
/// chatty command (`cargo`/`npm`/`docker`, which repaint with `\r` and ANSI)
/// would otherwise return the cursor to column 0 or inject escapes mid-row and
/// corrupt the live rail and its scrollback. The rail row/window enforce
/// single-line *width* (`{wide_msg}`) but not content, so the caller owns this.
fn sanitize(line: &str) -> String {
    // Progress output overwrites its line with `\r`; keep only the final
    // segment the terminal would have shown.
    let visible = line.rsplit('\r').next().unwrap_or(line);
    // Drop ANSI escape sequences, then any remaining control character (tabs
    // become spaces so words don't fuse).
    console::strip_ansi_codes(visible)
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// The [`StepKey`] for target `label`'s command `idx` of `count`. A single
/// command keys by the bare label (the row's identity is the worktree); a
/// pipeline disambiguates each command by index. The plan builder and the
/// presenter both call this so their keys line up.
pub fn command_key(label: &str, idx: usize, count: usize) -> StepKey {
    if count <= 1 {
        StepKey::scoped(StageId::ExecCommand, label.to_string())
    } else {
        StepKey::scoped(StageId::ExecCommand, format!("{label}\u{1f}{idx}"))
    }
}

/// Presenter that renders `daft exec` workers as timeline plan rows.
pub struct RailExecPresenter {
    handle: TimelineHandle,
    /// Target label → its command rows' keys, in pipeline order.
    rows: HashMap<String, Vec<StepKey>>,
    /// Target label → the index of its current (most recently started or
    /// skipped) command. `on_job_start`/`on_job_skipped` advance it; the
    /// output/finish callbacks read it.
    cursor: Mutex<HashMap<String, usize>>,
}

impl RailExecPresenter {
    pub fn new(handle: TimelineHandle, rows: HashMap<String, Vec<StepKey>>) -> Self {
        Self {
            handle,
            rows,
            cursor: Mutex::new(HashMap::new()),
        }
    }

    /// Advance `name`'s cursor to its next command and return that key.
    fn advance(&self, name: &str) -> Option<StepKey> {
        let next = {
            let mut cur = self.cursor.lock().expect("cursor mutex poisoned");
            let next = cur.get(name).map_or(0, |i| i + 1);
            cur.insert(name.to_string(), next);
            next
        };
        self.rows.get(name)?.get(next).cloned()
    }

    /// The key of `name`'s current command (most recently started).
    fn current(&self, name: &str) -> Option<StepKey> {
        let idx = *self
            .cursor
            .lock()
            .expect("cursor mutex poisoned")
            .get(name)?;
        self.rows.get(name)?.get(idx).cloned()
    }
}

impl JobPresenter for RailExecPresenter {
    fn on_phase_start(&self, _phase_name: &str, _target: Option<&str>) {}

    fn on_job_start(&self, name: &str, _description: Option<&str>, _command_preview: Option<&str>) {
        if let Some(key) = self.advance(name) {
            self.handle.on_stage(&key, StageEvent::Started);
        }
    }

    fn on_job_output(&self, name: &str, line: &str) {
        if let Some(key) = self.current(name) {
            self.handle.push_row_output(&key, &sanitize(line));
        }
    }

    fn on_job_success(&self, name: &str, _duration: Duration) {
        if let Some(key) = self.current(name) {
            self.handle
                .on_stage(&key, StageEvent::Completed { annotation: None });
        }
    }

    fn on_job_failure(&self, name: &str, duration: Duration) {
        self.on_job_failure_with_exit(name, duration, None);
    }

    fn on_job_failure_with_exit(&self, name: &str, _duration: Duration, exit_code: Option<i32>) {
        if let Some(key) = self.current(name) {
            let detail = exit_code.map_or_else(|| "failed".to_string(), |c| format!("exit {c}"));
            self.handle.on_stage(&key, StageEvent::Failed { detail });
        }
    }

    fn on_job_skipped(
        &self,
        name: &str,
        _reason: &str,
        _duration: Duration,
        _show_duration: bool,
        _command_preview: Option<&str>,
    ) {
        // A command that never ran (fail-fast, cancellation, or a
        // never-dispatched worktree): exec's reason is empty, so render the
        // rail's own `(not run)` — the same annotation the region's NotReached
        // teardown face uses, shared so the two never drift.
        if let Some(key) = self.advance(name) {
            self.handle.on_stage(
                &key,
                StageEvent::SkippedExpected {
                    reason: crate::output::timeline::NOT_RUN.to_string(),
                },
            );
        }
    }

    fn on_job_cancelled(&self, name: &str, _duration: Duration) {
        if let Some(key) = self.current(name) {
            self.handle.on_stage(&key, StageEvent::Cancelled);
        }
    }

    fn on_job_background(&self, _name: &str, _description: Option<&str>) {}

    fn on_message(&self, _msg: &str) {}

    fn on_phase_complete(&self, _total_duration: Duration) {}

    /// Exec aggregates outcomes through its own `ExecReport`, not the
    /// presenter's `JobResult` accumulator.
    fn take_results(&self) -> Vec<JobResult> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stage::{PlanCommit, Row, StepSpec};
    use crate::output::timeline::{RowOutputConfig, Timeline, TimelineMode};
    use indicatif::{InMemoryTerm, ProgressDrawTarget};

    /// Build a committed rail timeline plus a presenter over `targets`, each a
    /// single-command row, wired to the same `InMemoryTerm`.
    fn harness(header: &str, labels: &[&str]) -> (Timeline, RailExecPresenter, InMemoryTerm) {
        let term = InMemoryTerm::new(40, 120);
        let tl = Timeline::new(TimelineMode::Interactive { color: false }, false, header);
        tl.set_test_draw_target(ProgressDrawTarget::term_like(Box::new(term.clone())));
        tl.set_ordered_receipts(true);
        tl.set_row_output(RowOutputConfig {
            verbose: false,
            tail_lines: 6,
            buffer_cap: None,
        });
        let rows: HashMap<String, Vec<StepKey>> = labels
            .iter()
            .map(|l| (l.to_string(), vec![command_key(l, 0, 1)]))
            .collect();
        let plan = PlanCommit::new(
            labels
                .iter()
                .map(|l| Row::Step(StepSpec::new(command_key(l, 0, 1)).with_label(*l)))
                .collect(),
        );
        let mut tl = tl;
        tl.commit_plan(plan);
        let presenter = RailExecPresenter::new(tl.handle(), rows);
        (tl, presenter, term)
    }

    #[test]
    fn sanitize_strips_ansi_carriage_returns_and_controls() {
        // A cargo/npm-style progress line: ANSI color + `\r` repaint. Only the
        // final segment survives, control-free — nothing that could return the
        // cursor or inject escapes into a rail row.
        assert_eq!(
            sanitize("\x1b[32mBuilding [==>  ] 40%\rBuilding [====] 100%\x1b[0m"),
            "Building [====] 100%"
        );
        assert_eq!(sanitize("tab\there"), "tab here");
        assert_eq!(sanitize("plain output"), "plain output");
        // A bare carriage return leaves no residue.
        assert_eq!(sanitize("gone\rkept"), "kept");
    }

    #[test]
    fn parallel_workers_render_in_plan_order() {
        // Two workers finish out of order (b before a); the receipt stays in
        // plan order, and the failure threads its output while the success
        // stays compact.
        let (mut tl, p, term) = harness("Running mise clean in 2 worktrees", &["a", "b"]);
        p.on_job_start("b", None, Some("mise clean"));
        p.on_job_output("b", "boom");
        p.on_job_failure_with_exit("b", Duration::from_millis(10), Some(1));
        p.on_job_start("a", None, Some("mise clean"));
        p.on_job_success("a", Duration::from_millis(10));
        tl.finish("Finished with failures in 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running mise clean in 2 worktrees\n\
             \u{2502}\n\
             \u{2713}  a\n\
             \u{2717}  b  exit 1\n\
             \u{2502}    boom\n\
             \u{2502}\n\
             \u{2514}  Finished with failures in 0.1s"
        );
    }

    #[test]
    fn fan_out_to_one_worktree_rails_orphans_then_the_live_row() {
        // #533 [3]: a glob matching several branches where only one has a
        // worktree renders the rail (not single-target passthrough) — leading
        // `↓ … no worktree` orphan rows in plan order, then the one live
        // worktree's row, under a singular "in 1 worktree" header. This
        // one-real-row-after-orphans shape only became reachable once the
        // command layer stopped collapsing a fan-out-of-one to passthrough.
        use crate::core::stage::{PlanCommit, Row, StageEvent, StageId, StepSpec};
        let term = InMemoryTerm::new(40, 120);
        let mut tl = Timeline::new(
            TimelineMode::Interactive { color: false },
            false,
            // Exactly what run_rail's rail_header() builds for this shape.
            "Running echo hi in 1 worktree",
        );
        tl.set_test_draw_target(ProgressDrawTarget::term_like(Box::new(term.clone())));
        tl.set_ordered_receipts(true);
        tl.set_row_output(RowOutputConfig {
            verbose: false,
            tail_lines: 6,
            buffer_cap: None,
        });

        // Orphan rows lead the plan (as run_rail builds them), then the live
        // worktree's row.
        let orphan_b = StepKey::scoped(StageId::ExecCommand, "orphan\u{1f}feat/b".to_string());
        let orphan_c = StepKey::scoped(StageId::ExecCommand, "orphan\u{1f}feat/c".to_string());
        let live = command_key("feat/a", 0, 1);
        tl.commit_plan(PlanCommit::new(vec![
            Row::Step(StepSpec::new(orphan_b.clone()).with_label("feat/b")),
            Row::Step(StepSpec::new(orphan_c.clone()).with_label("feat/c")),
            Row::Step(StepSpec::new(live.clone()).with_label("feat/a")),
        ]));

        // Orphans never run — resolved to the yellow `↓ … no worktree`.
        for key in [&orphan_b, &orphan_c] {
            tl.on_stage(
                key,
                StageEvent::SkippedAttention {
                    reason: "no worktree".to_string(),
                },
            );
        }

        // The one live worktree runs and succeeds.
        let mut rows = HashMap::new();
        rows.insert("feat/a".to_string(), vec![live]);
        let p = RailExecPresenter::new(tl.handle(), rows);
        p.on_job_start("feat/a", None, Some("echo hi"));
        p.on_job_success("feat/a", Duration::from_millis(10));
        tl.finish("Done in 0.1s");

        assert_eq!(
            term.contents(),
            "\u{250c}  Running echo hi in 1 worktree\n\
             \u{2502}\n\
             \u{2193}  feat/b  no worktree\n\
             \u{2193}  feat/c  no worktree\n\
             \u{2713}  feat/a\n\
             \u{2502}\n\
             \u{2514}  Done in 0.1s"
        );
    }

    #[test]
    fn pipeline_commands_key_by_index_and_fail_fast_marks_not_run() {
        // One worktree, two commands: the first fails, the second is skipped
        // and renders `(not run)` — keyed by command index, not name.
        let term = InMemoryTerm::new(40, 120);
        let mut tl = Timeline::new(
            TimelineMode::Interactive { color: false },
            false,
            "Running 2 commands in 1 worktree",
        );
        tl.set_test_draw_target(ProgressDrawTarget::term_like(Box::new(term.clone())));
        tl.set_ordered_receipts(true);
        tl.set_row_output(RowOutputConfig {
            verbose: false,
            tail_lines: 6,
            buffer_cap: None,
        });
        let keys = vec![command_key("wt", 0, 2), command_key("wt", 1, 2)];
        tl.commit_plan(PlanCommit::new(vec![
            Row::Group { label: "wt".into() },
            Row::Step(StepSpec::new(keys[0].clone()).with_label("mise clean")),
            Row::Step(StepSpec::new(keys[1].clone()).with_label("mise dev")),
        ]));
        let mut rows = HashMap::new();
        rows.insert("wt".to_string(), keys);
        let p = RailExecPresenter::new(tl.handle(), rows);
        p.on_job_start("wt", None, Some("mise clean"));
        p.on_job_output("wt", "boom");
        p.on_job_failure_with_exit("wt", Duration::from_millis(10), Some(1));
        p.on_job_skipped("wt", "", Duration::ZERO, false, Some("mise dev"));
        tl.abort("Failed after 0.1s");
        assert_eq!(
            term.contents(),
            "\u{250c}  Running 2 commands in 1 worktree\n\
             \u{2502}\n\
             \u{251c}\u{2500} wt\n\
             \u{2502}  \u{2717}  mise clean  exit 1\n\
             \u{2502}  \u{2502}    boom\n\
             \u{2502}  \u{2502}\n\
             \u{2502}  \u{25cb}  mise dev    (not run)\n\
             \u{2502}\n\
             \u{2514}  Failed after 0.1s"
        );
    }
}
