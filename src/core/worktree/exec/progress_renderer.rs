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
    run_pipeline_streaming, CancelFlag, CommandSpec, ExecMode, ExecReport, ResolvedTarget,
};
use crate::executor::cli_presenter::CliPresenter;
use crate::executor::presenter::JobPresenter;
use crate::settings::HookOutputConfig;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

/// Run the pipeline across all targets, driving a live multi-panel progress
/// TUI via [`CliPresenter`]. Returns the aggregated [`ExecReport`] after the
/// TUI has torn down so the command layer can still render a scrollback-friendly
/// failure dump.
pub fn run_with_progress(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    mode: ExecMode,
    cancel: &CancelFlag,
) -> anyhow::Result<ExecReport> {
    // Enable command previews under each panel; other knobs match the
    // defaults used by the hooks command.
    let cfg = HookOutputConfig {
        verbose: true,
        ..HookOutputConfig::default()
    };
    let presenter: Arc<dyn JobPresenter> = CliPresenter::auto(&cfg);

    presenter.on_phase_start("exec");
    let phase_start = Instant::now();

    let outcomes = match mode {
        ExecMode::Parallel => run_parallel(targets, pipeline, &presenter, cancel)?,
        ExecMode::Sequential => run_sequential(targets, pipeline, false, &presenter, cancel)?,
        ExecMode::KeepGoing => run_sequential(targets, pipeline, true, &presenter, cancel)?,
    };

    presenter.on_phase_complete(phase_start.elapsed());

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
) -> anyhow::Result<Vec<super::WorktreeOutcome>> {
    // `thread::scope` lets worker threads borrow `cancel`, `pipeline`, and
    // `presenter` for their entire lifetime without `'static`, which keeps
    // the cancellation flag observable across every worker.
    let mut outcomes = thread::scope(|scope| -> anyhow::Result<Vec<super::WorktreeOutcome>> {
        let handles: Vec<_> = targets
            .iter()
            .map(|t| {
                let pres = Arc::clone(presenter);
                scope.spawn(move || run_pipeline_streaming(t, pipeline, "", &pres, cancel))
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
) -> anyhow::Result<Vec<super::WorktreeOutcome>> {
    let mut outcomes = Vec::with_capacity(targets.len());
    for t in targets {
        if cancel.is_cancelled() {
            break;
        }
        let outcome = run_pipeline_streaming(t, pipeline, "", presenter, cancel)?;
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
        }];
        let pipeline = vec![CommandSpec::Argv(vec!["echo".into(), "hi".into()])];
        let report =
            run_with_progress(&targets, &pipeline, ExecMode::Parallel, &CancelFlag::new()).unwrap();
        assert_eq!(report.outcomes.len(), 1);
        assert_eq!(report.aggregate_exit_code(), 0);
        assert!(report.outcomes[0].succeeded());
    }
}
