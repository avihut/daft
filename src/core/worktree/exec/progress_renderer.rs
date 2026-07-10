//! Live progress renderer for multi-worktree `daft exec`.
//!
//! Reuses the `JobPresenter` / `CliPresenter` plumbing — the same mechanism
//! that powers hook output — to drive one live panel per target worktree.
//! Each command in the pipeline produces a short-lived window that tears
//! down when the command finishes; output lines stream into each window in
//! real time. The presenter is driven entirely from [`run_with_progress`];
//! the per-target runtime lives in
//! [`super::run_pipeline_streaming`](crate::core::worktree::exec::run_pipeline_streaming).

use super::{
    CancelFlag, CommandSpec, ExecMode, ExecReport, ResolvedTarget, alias_cache::AliasCache,
    run_pipeline_streaming,
};
use crate::executor::cli_presenter::CliPresenter;
use crate::executor::presenter::JobPresenter;
use crate::settings::HookOutputConfig;
use std::sync::Arc;
use std::thread;

use crate::output::term_guard::EchoCtlGuard;

/// Run the pipeline across all targets, rendering a live per-worktree
/// progress UI (spinner + rolling tail, finalized to a compact one-line row
/// per worktree). Returns the aggregated [`ExecReport`] so the command layer
/// can still render a scrollback-friendly failure dump.
pub fn run_with_progress(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    mode: ExecMode,
    cancel: &CancelFlag,
    alias_cache: Option<&AliasCache>,
) -> anyhow::Result<ExecReport> {
    // Suppress the TTY's `^C` echo for the duration of the live render so
    // Ctrl-C cancellation doesn't corrupt indicatif's terminal tracking.
    let _echoctl_guard = EchoCtlGuard::new();

    // Print the scope-summary header directly — the presenter's
    // on_phase_start would otherwise print a hook-branded box we don't
    // want here. Skipped under `cfg!(test)`: unit tests drive
    // `run_with_progress` directly and assert on the returned report, so the
    // header is pure noise in the test log (the indicatif presenter is
    // already silent on the non-tty test stderr). Mirrors the `!cfg!(test)`
    // gate on the coordinator's background-job banner. Integration tests run
    // the real binary (where `cfg!(test)` is false) so they keep the header.
    if !cfg!(test) {
        let stderr = std::io::stderr();
        let mut sink = stderr.lock();
        super::list_renderer::render_header(&mut sink, targets.len(), pipeline)?;
    }

    let cfg = HookOutputConfig {
        compact_finalization: true,
        ..HookOutputConfig::default()
    };
    let presenter_concrete = CliPresenter::auto(&cfg);
    let max_name = targets
        .iter()
        .map(|t| t.label().len())
        .max()
        .unwrap_or(crate::output::hook_progress::DEFAULT_NAME_COLUMN_WIDTH);
    presenter_concrete.set_name_column_width(max_name);
    let presenter: Arc<dyn JobPresenter> = presenter_concrete;

    // Deliberately skip presenter.on_phase_start — it prints the hook
    // header. The list-mode header above replaces it.

    let outcomes = match mode {
        ExecMode::Parallel => run_parallel(targets, pipeline, &presenter, cancel, alias_cache)?,
        ExecMode::Sequential => {
            run_sequential(targets, pipeline, false, &presenter, cancel, alias_cache)?
        }
        ExecMode::KeepGoing => {
            run_sequential(targets, pipeline, true, &presenter, cancel, alias_cache)?
        }
    };

    // Deliberately skip presenter.on_phase_complete — it prints the hook
    // summary block. Compact per-row finalization + the caller's failed-
    // output dump already cover the user's needs.

    // Emit skip rows for targets that never launched (e.g. cancelled before
    // dispatch). This ensures every target gets a visible finalization row.
    let dispatched: std::collections::HashSet<_> = outcomes
        .iter()
        .map(|o| o.target.worktree_path.clone())
        .collect();
    for target in targets {
        if dispatched.contains(&target.worktree_path) {
            continue;
        }
        for step in pipeline {
            presenter.on_job_skipped(
                target.label(),
                "",
                std::time::Duration::ZERO,
                false,
                Some(&step.display()),
            );
        }
    }

    Ok(ExecReport {
        outcomes,
        orphan_branches_skipped: Vec::new(),
    })
}

fn run_parallel(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    presenter: &Arc<dyn JobPresenter>,
    cancel: &CancelFlag,
    alias_cache: Option<&AliasCache>,
) -> anyhow::Result<Vec<super::WorktreeOutcome>> {
    // `thread::scope` lets worker threads borrow `cancel`, `pipeline`, and
    // `presenter` for their entire lifetime without `'static`, which keeps
    // the cancellation flag observable across every worker.
    let mut outcomes = thread::scope(|scope| -> anyhow::Result<Vec<super::WorktreeOutcome>> {
        let handles: Vec<_> = targets
            .iter()
            .map(|t| {
                let pres = Arc::clone(presenter);
                scope.spawn(move || {
                    run_pipeline_streaming(t, pipeline, "", &pres, cancel, alias_cache)
                })
            })
            .collect();

        let mut out = Vec::with_capacity(targets.len());
        for h in handles {
            match h.join() {
                Ok(Ok(o)) => out.push(o),
                Ok(Err(e)) => return Err(e),
                Err(panic) => return Err(anyhow::anyhow!("worker thread panicked: {:?}", panic)),
            }
        }
        Ok(out)
    })?;
    outcomes.sort_by_key(|o| {
        targets
            .iter()
            .position(|t| t.worktree_path == o.target.worktree_path)
            .unwrap_or(usize::MAX)
    });
    Ok(outcomes)
}

fn run_sequential(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    keep_going: bool,
    presenter: &Arc<dyn JobPresenter>,
    cancel: &CancelFlag,
    alias_cache: Option<&AliasCache>,
) -> anyhow::Result<Vec<super::WorktreeOutcome>> {
    let mut outcomes = Vec::with_capacity(targets.len());
    for t in targets {
        if cancel.is_cancelled() {
            break;
        }
        let outcome = run_pipeline_streaming(t, pipeline, "", presenter, cancel, alias_cache)?;
        let succeeded = outcome.succeeded();
        outcomes.push(outcome);
        if !succeeded && !keep_going {
            break;
        }
    }
    Ok(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn run_with_progress_single_target_success() {
        let dir = TempDir::new().unwrap();
        let targets = vec![ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: "master".into(),
            display: None,
        }];
        let pipeline = vec![CommandSpec::Argv(vec!["echo".into(), "hi".into()])];
        let report = run_with_progress(
            &targets,
            &pipeline,
            ExecMode::Parallel,
            &CancelFlag::new(),
            None,
        )
        .unwrap();
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.aggregate_exit_code(), 0);
        assert!(report.outcomes[0].succeeded());
    }

    #[test]
    fn pre_cancelled_sequential_run_returns_empty_outcomes_without_panicking() {
        // Exercises the post-scheduler skip-emission branch: when the
        // sequential scheduler sees a pre-escalated cancel flag, no targets
        // launch, `outcomes` is empty, and `run_with_progress` must emit
        // skip rows (via the presenter) for every target × step and return
        // cleanly. This test asserts the no-panic path; the event-level
        // skip emission is covered by the presenter's internal recorders
        // and the streaming_skip_emission_tests module in exec/mod.rs.
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget {
                worktree_path: dir1.path().to_path_buf(),
                branch_name: "a".into(),
                display: None,
            },
            ResolvedTarget {
                worktree_path: dir2.path().to_path_buf(),
                branch_name: "b".into(),
                display: None,
            },
        ];
        let pipeline = vec![
            CommandSpec::Argv(vec!["echo".into(), "one".into()]),
            CommandSpec::Argv(vec!["echo".into(), "two".into()]),
        ];
        let cancel = CancelFlag::new();
        cancel.escalate();

        let report =
            run_with_progress(&targets, &pipeline, ExecMode::Sequential, &cancel, None).unwrap();

        assert!(
            report.outcomes.is_empty(),
            "pre-cancelled sequential run should produce no outcomes: {:?}",
            report.outcomes
        );
        assert_eq!(report.aggregate_exit_code(), 0);
    }
}
