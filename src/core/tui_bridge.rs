//! TUI-mode bridge that executes hooks and forwards events to the DAG channel.
//!
//! `TuiBridge` implements both `ProgressSink` and `HookRunner`, satisfying
//! core operation requirements while keeping the TUI renderer decoupled from
//! hook execution details.

use crate::core::worktree::sync_dag::DagEvent;
use crate::core::{HookOutcome, HookRunner, ProgressSink};
use crate::hooks::{HookContext, HookExecutor};
use crate::output::tui::TuiPresenter;
use crate::output::BufferingOutput;
use anyhow::Result;
use std::sync::mpsc;

/// A combined `ProgressSink` + `HookRunner` for TUI mode.
///
/// Progress messages are discarded (the TUI handles all display). Hooks are
/// executed via `HookExecutor` and the results are forwarded as `DagEvent`s
/// through the given channel so the renderer can show hook status.
pub struct TuiBridge {
    executor: HookExecutor,
    output: BufferingOutput,
    sender: mpsc::Sender<DagEvent>,
    branch_name: String,
}

impl TuiBridge {
    /// Create a new `TuiBridge`.
    ///
    /// * `executor` — configured hook executor (trust DB, callbacks, etc.)
    /// * `sender` — channel to the TUI renderer
    /// * `branch_name` — branch this bridge is associated with (for events)
    pub fn new(
        executor: HookExecutor,
        sender: mpsc::Sender<DagEvent>,
        branch_name: impl Into<String>,
    ) -> Self {
        Self {
            executor,
            output: BufferingOutput::new(),
            sender,
            branch_name: branch_name.into(),
        }
    }

    /// Take any warnings that were buffered during hook execution.
    ///
    /// These should be displayed to the user after the TUI exits.
    pub fn take_warnings(&mut self) -> Vec<String> {
        self.output.take_warnings()
    }
}

impl ProgressSink for TuiBridge {
    fn on_step(&mut self, _msg: &str) {}
    fn on_warning(&mut self, _msg: &str) {}
    fn on_debug(&mut self, _msg: &str) {}
}

impl HookRunner for TuiBridge {
    fn run_hook(&mut self, ctx: &HookContext) -> Result<HookOutcome> {
        let hook_type = ctx.hook_type;

        let presenter = TuiPresenter::new(self.sender.clone(), self.branch_name.clone(), hook_type);
        match self.executor.execute(ctx, &mut self.output, presenter) {
            Ok(result) => {
                if result.skipped {
                    // TODO: When hooks are skipped due to TrustLevel::Prompt, surface a
                    // post-TUI notice suggesting `git daft hooks trust`. Currently the
                    // skip_reason is captured in HookOutcome but not surfaced to the user
                    // in TUI mode. See spec: "Prompt Callbacks" section.
                    // Skipped hooks (disabled, not trusted, etc.) produce no events —
                    // the executor returns early before calling any presenter methods.
                    return Ok(HookOutcome {
                        success: result.success,
                        skipped: true,
                        skip_reason: result.skip_reason,
                    });
                }

                // Events (HookStarted + HookCompleted) were already sent by the
                // presenter's on_phase_start and on_phase_complete callbacks.
                Ok(HookOutcome {
                    success: result.success,
                    skipped: false,
                    skip_reason: None,
                })
            }
            Err(e) => {
                // An Err from execute() means FailMode::Abort triggered bail!().
                // The presenter's on_phase_start already sent HookStarted, but
                // on_phase_complete was NOT called (executor bailed before reaching
                // it). Manually send HookCompleted so the TUI can show the failure.
                let _ = self.sender.send(DagEvent::HookCompleted {
                    branch_name: self.branch_name.clone(),
                    hook_type,
                    success: false,
                    warned: false,
                    duration: std::time::Duration::ZERO,
                    exit_code: None,
                    output: Some(format!("{e:#}")),
                });

                Ok(HookOutcome {
                    success: false,
                    skipped: false,
                    skip_reason: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::{HookExecutor, HookType, HooksConfig, TrustDatabase};

    fn make_executor_disabled() -> HookExecutor {
        let config = HooksConfig {
            enabled: false,
            ..Default::default()
        };
        HookExecutor::with_trust_db(config, TrustDatabase::default())
    }

    fn make_context(hook_type: HookType) -> HookContext {
        HookContext::new(
            hook_type,
            "sync",
            "/tmp/project",
            "/tmp/project/.git",
            "origin",
            "/tmp/project/main",
            "/tmp/project/main",
            "main",
        )
    }

    #[test]
    fn tui_bridge_creation() {
        let executor = make_executor_disabled();
        let (tx, _rx) = mpsc::channel();
        let bridge = TuiBridge::new(executor, tx, "main");
        // Just verify construction does not panic.
        let _ = bridge;
    }

    #[test]
    fn skipped_hooks_send_no_events() {
        let executor = make_executor_disabled();
        let (tx, rx) = mpsc::channel();
        let mut bridge = TuiBridge::new(executor, tx, "main");

        let ctx = make_context(HookType::PostCreate);
        let outcome = bridge.run_hook(&ctx).unwrap();

        // Hooks are disabled, so the result should be skipped.
        assert!(outcome.skipped);
        assert!(outcome.success);

        // Drop the sender so we can collect events.
        drop(bridge);

        let events: Vec<DagEvent> = rx.try_iter().collect();
        let hook_events: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DagEvent::HookStarted { .. } | DagEvent::HookCompleted { .. }
                )
            })
            .collect();

        assert!(
            hook_events.is_empty(),
            "Skipped hooks must not send HookStarted or HookCompleted events"
        );
    }

    #[test]
    fn progress_sink_is_noop() {
        let executor = make_executor_disabled();
        let (tx, _rx) = mpsc::channel();
        let mut bridge = TuiBridge::new(executor, tx, "main");

        // These must not panic.
        bridge.on_step("step message");
        bridge.on_warning("warning message");
        bridge.on_debug("debug message");
    }
}
