//! CLI presenter wrapping [`HookRenderer`] for thread-safe progress output.
//!
//! Adapts the existing `HookRenderer` (which uses `&mut self`) into the
//! [`JobPresenter`] trait (which uses `&self` with interior mutability).

use super::presenter::JobPresenter;
use super::{JobResult, NodeStatus};
use crate::core::stage::{StageId, StepKey};
use crate::output::hook_progress::{HookRenderer, JobOutcome, JobResultEntry};
use crate::output::timeline::{RailHookRenderer, TimelineHandle};
use crate::settings::HookOutputConfig;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The renderer behind the presenter: the rail rows (#651) for embedded
/// phases — succinct by default, threading each job's log when
/// `HookOutputConfig::verbose` — or the standalone hook block for every
/// non-embedded path (and the degraded no-region fallback).
enum EmbedRenderer {
    Block(HookRenderer),
    /// Boxed: the rail renderer carries three `ProgressStyle`s and dwarfs
    /// the block variant (clippy::large_enum_variant).
    Rail(Box<RailHookRenderer>),
}

impl EmbedRenderer {
    fn print_header(&mut self, hook_name: &str, target: Option<&str>) {
        match self {
            Self::Block(r) => r.print_header(hook_name, target),
            Self::Rail(r) => r.print_header(hook_name, target),
        }
    }

    fn start_job_with_description(
        &mut self,
        name: &str,
        description: Option<&str>,
        command_preview: Option<&str>,
    ) {
        match self {
            Self::Block(r) => r.start_job_with_description(name, description, command_preview),
            Self::Rail(r) => r.start_job_with_description(name, description, command_preview),
        }
    }

    fn update_job_output(&mut self, name: &str, line: &str) {
        match self {
            Self::Block(r) => r.update_job_output(name, line),
            Self::Rail(r) => r.update_job_output(name, line),
        }
    }

    fn finish_job_success(&mut self, name: &str, duration: Duration) {
        match self {
            Self::Block(r) => r.finish_job_success(name, duration),
            Self::Rail(r) => r.finish_job_success(name, duration),
        }
    }

    fn finish_job_failure(&mut self, name: &str, duration: Duration) {
        match self {
            Self::Block(r) => r.finish_job_failure(name, duration),
            Self::Rail(r) => r.finish_job_failure(name, duration),
        }
    }

    fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        match self {
            Self::Block(r) => {
                r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
            }
            Self::Rail(r) => {
                r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
            }
        }
    }

    fn finish_job_cancelled(&mut self, name: &str, duration: Duration) {
        match self {
            Self::Block(r) => r.finish_job_cancelled(name, duration),
            Self::Rail(r) => r.finish_job_cancelled(name, duration),
        }
    }

    fn show_background_job(&mut self, name: &str, description: Option<&str>) {
        match self {
            Self::Block(r) => r.show_background_job(name, description),
            Self::Rail(r) => r.show_background_job(name, description),
        }
    }

    fn record_background_job(&mut self, name: &str, description: Option<&str>) {
        match self {
            Self::Block(r) => r.record_background_job(name, description),
            Self::Rail(r) => r.push_finished_job(JobResultEntry {
                name: name.to_string(),
                outcome: JobOutcome::Background {
                    description: description.map(String::from),
                },
                duration: Duration::ZERO,
            }),
        }
    }

    fn print_summary(&self, total_duration: Duration) {
        match self {
            Self::Block(r) => r.print_summary(total_duration),
            Self::Rail(r) => r.print_summary(total_duration),
        }
    }

    fn take_finished_jobs(&mut self) -> Vec<JobResultEntry> {
        match self {
            Self::Block(r) => r.take_finished_jobs(),
            Self::Rail(r) => r.take_finished_jobs(),
        }
    }

    fn println(&self, msg: &str) {
        match self {
            Self::Block(r) => r.println(msg),
            Self::Rail(r) => r.println(msg),
        }
    }

    /// Compact-finalization column width — a block-only concern
    /// (`daft exec`); the rail derives its own alignment from job names.
    fn set_name_column_width(&mut self, width: usize) {
        match self {
            Self::Block(r) => r.set_name_column_width(width),
            Self::Rail(_) => {}
        }
    }
}

/// Which plan row an embedded presenter renders into.
enum EmbedTarget {
    /// One exact step, resolved by the caller (`TimelineBridge::run_hook`).
    Exact(StepKey),
    /// A stage whose scope is resolved per phase from the phase's `target`
    /// (the pre-push presenter: one presenter serves every branch's push,
    /// re-embedding — with a fresh, live anchor — at each `on_phase_start`).
    Stage(StageId),
}

/// Renderer state behind the presenter mutex.
///
/// The embedded variant (#651) must resolve lazily: the hook step's rail row
/// is consumed the moment the block starts rendering, and that must happen at
/// `on_phase_start` — not at presenter construction, which the commands do
/// eagerly (e.g. the pre-push presenter exists before the plan even commits).
enum PresenterState {
    Ready(EmbedRenderer),
    Embed {
        config: HookOutputConfig,
        handle: TimelineHandle,
        target: EmbedTarget,
        /// The renderer for the phase currently (or last) rendering.
        /// Replaced with a fresh embed at each phase start so the insertion
        /// anchor is always a live bar.
        renderer: Option<EmbedRenderer>,
    },
}

/// Thread-safe CLI presenter backed by [`HookRenderer`].
///
/// Wraps a `HookRenderer` in a `Mutex` so it can be shared across threads
/// via `Arc<CliPresenter>`. All trait methods lock the mutex briefly.
pub struct CliPresenter {
    renderer: Mutex<PresenterState>,
}

impl CliPresenter {
    /// Create a presenter that auto-detects TTY vs plain output.
    pub fn auto(config: &HookOutputConfig) -> Arc<Self> {
        Arc::new(Self {
            renderer: Mutex::new(PresenterState::Ready(EmbedRenderer::Block(
                HookRenderer::auto(config),
            ))),
        })
    }

    /// Create a presenter that renders the hook block inside a plan-execute
    /// timeline (#651). Lazy: each `on_phase_start` expands the `key` step's
    /// rail row into the block (via `begin_hook_embed`) and builds the
    /// embedded renderer; if the region is gone by then, it degrades to
    /// `auto`.
    pub fn embedded(config: &HookOutputConfig, handle: TimelineHandle, key: StepKey) -> Arc<Self> {
        Arc::new(Self {
            renderer: Mutex::new(PresenterState::Embed {
                config: config.clone(),
                handle,
                target: EmbedTarget::Exact(key),
                renderer: None,
            }),
        })
    }

    /// Like [`CliPresenter::embedded`], but the step is resolved per phase:
    /// `stage` plus the phase's `target` (the branch name for pre-push
    /// phases) pick the plan row, so one presenter can serve repeated
    /// phases across a multi-branch plan.
    pub fn embedded_for_stage(
        config: &HookOutputConfig,
        handle: TimelineHandle,
        stage: StageId,
    ) -> Arc<Self> {
        Arc::new(Self {
            renderer: Mutex::new(PresenterState::Embed {
                config: config.clone(),
                handle,
                target: EmbedTarget::Stage(stage),
                renderer: None,
            }),
        })
    }

    /// Create from an existing `HookRenderer` (useful for tests).
    #[cfg(test)]
    pub fn from_renderer(renderer: HookRenderer) -> Arc<Self> {
        Arc::new(Self {
            renderer: Mutex::new(PresenterState::Ready(EmbedRenderer::Block(renderer))),
        })
    }

    /// Test-only: which renderer currently backs the presenter.
    #[cfg(test)]
    fn renderer_kind(&self) -> Option<&'static str> {
        let kind = |r: &EmbedRenderer| match r {
            EmbedRenderer::Block(_) => "block",
            EmbedRenderer::Rail(_) => "rail",
        };
        match &*self.lock() {
            PresenterState::Ready(r) => Some(kind(r)),
            PresenterState::Embed { renderer, .. } => renderer.as_ref().map(kind),
        }
    }

    fn lock(&self) -> MutexGuard<'_, PresenterState> {
        self.renderer.lock().expect("CliPresenter mutex poisoned")
    }

    /// Set the name-column width used when rendering compact finalization rows.
    pub fn set_name_column_width(&self, width: usize) {
        if let Some(r) = ready(&mut self.lock()) {
            r.set_name_column_width(width);
        }
    }

    /// Convert a `JobResultEntry` (from `HookRenderer`) into our generic `JobResult`.
    fn entry_to_job_result(entry: JobResultEntry) -> JobResult {
        let status = match &entry.outcome {
            JobOutcome::Success => NodeStatus::Succeeded,
            JobOutcome::Failed => NodeStatus::Failed,
            JobOutcome::Skipped { .. } => NodeStatus::Skipped,
            JobOutcome::Background { .. } => NodeStatus::Pending,
        };

        JobResult {
            name: entry.name,
            status,
            duration: entry.duration,
            // HookRenderer does not track exit codes or output.
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

/// The active renderer behind the state, if any.
fn ready(state: &mut PresenterState) -> Option<&mut EmbedRenderer> {
    match state {
        PresenterState::Ready(r) => Some(r),
        PresenterState::Embed { renderer, .. } => renderer.as_mut(),
    }
}

impl JobPresenter for CliPresenter {
    fn on_phase_start(&self, phase_name: &str, target: Option<&str>) {
        let mut guard = self.lock();
        if let PresenterState::Embed {
            config,
            handle,
            target: embed_target,
            renderer,
        } = &mut *guard
        {
            let key = match embed_target {
                EmbedTarget::Exact(key) => Some(key.clone()),
                EmbedTarget::Stage(id) => handle.resolve_key(*id, target),
            };
            *renderer = match key.and_then(|k| handle.begin_hook_embed(&k)) {
                // Embedded phases always render rail-native (#651) — the
                // rail renderer reads `config.verbose` itself and threads
                // each job's log under its row.
                Some(embed) => Some(EmbedRenderer::Rail(Box::new(RailHookRenderer::new(
                    embed,
                    handle.clone(),
                    config,
                )))),
                // Step unknown while the region still owns the terminal —
                // render nothing rather than tear the rail: the standalone
                // renderer's own `MultiProgress` would fight the region for
                // the cursor. (A `None` renderer no-ops every callback;
                // executor failures still surface through the routed
                // output.)
                None if handle.region_live() => None,
                // Region gone (error paths, post-rail phases) — degrade to
                // the standalone renderer rather than losing the block.
                // Rail rows make no sense without a region.
                None => Some(EmbedRenderer::Block(HookRenderer::auto(config))),
            };
        }
        if let Some(r) = ready(&mut guard) {
            r.print_header(phase_name, target);
        }
    }

    fn on_job_start(&self, name: &str, description: Option<&str>, command_preview: Option<&str>) {
        if let Some(r) = ready(&mut self.lock()) {
            r.start_job_with_description(name, description, command_preview);
        }
    }

    fn on_job_output(&self, name: &str, line: &str) {
        if let Some(r) = ready(&mut self.lock()) {
            r.update_job_output(name, line);
        }
    }

    fn on_job_success(&self, name: &str, duration: Duration) {
        if let Some(r) = ready(&mut self.lock()) {
            r.finish_job_success(name, duration);
        }
    }

    fn on_job_failure(&self, name: &str, duration: Duration) {
        if let Some(r) = ready(&mut self.lock()) {
            r.finish_job_failure(name, duration);
        }
    }

    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        if let Some(r) = ready(&mut self.lock()) {
            r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
        }
    }

    fn on_job_cancelled(&self, name: &str, duration: Duration) {
        if let Some(r) = ready(&mut self.lock()) {
            r.finish_job_cancelled(name, duration);
        }
    }

    fn on_job_background(&self, name: &str, description: Option<&str>) {
        if let Some(r) = ready(&mut self.lock()) {
            r.show_background_job(name, description);
            r.record_background_job(name, description);
        }
    }

    fn on_message(&self, msg: &str) {
        if let Some(r) = ready(&mut self.lock()) {
            r.println(msg);
        }
    }

    fn on_phase_complete(&self, total_duration: Duration) {
        if let Some(r) = ready(&mut self.lock()) {
            r.print_summary(total_duration);
        }
    }

    fn take_results(&self) -> Vec<JobResult> {
        match ready(&mut self.lock()) {
            Some(r) => r
                .take_finished_jobs()
                .into_iter()
                .map(Self::entry_to_job_result)
                .collect(),
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stage::{PlanCommit, Row, StepSpec};
    use crate::output::timeline::{Timeline, TimelineMode};

    #[test]
    fn cli_presenter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CliPresenter>();
    }

    /// A live timeline whose plan carries a post-create hook row.
    fn hook_timeline() -> Timeline {
        let mut tl = Timeline::new(TimelineMode::Interactive { color: false }, false, "t");
        tl.commit_plan(PlanCommit::new(vec![Row::Step(StepSpec::new(
            StepKey::new(StageId::PostCreateHooks),
        ))]));
        tl
    }

    #[test]
    fn embedded_phase_defaults_to_rail_rows() {
        let tl = hook_timeline();
        let presenter = CliPresenter::embedded(
            &HookOutputConfig::default(),
            tl.handle(),
            StepKey::new(StageId::PostCreateHooks),
        );
        presenter.on_phase_start("worktree-post-create", Some("feat/x"));
        assert_eq!(presenter.renderer_kind(), Some("rail"));

        // The rail renderer feeds take_results exactly like the block.
        presenter.on_job_start("install", None, None);
        presenter.on_job_success("install", Duration::from_secs(2));
        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Succeeded);
    }

    #[test]
    fn verbose_embed_stays_on_the_rail() {
        // Verbose no longer swaps in the welded block — the rail renderer
        // itself threads the log (#651).
        let tl = hook_timeline();
        let config = HookOutputConfig {
            verbose: true,
            ..Default::default()
        };
        let presenter =
            CliPresenter::embedded(&config, tl.handle(), StepKey::new(StageId::PostCreateHooks));
        presenter.on_phase_start("worktree-post-create", None);
        assert_eq!(presenter.renderer_kind(), Some("rail"));
    }

    #[test]
    fn dead_region_degrades_to_the_standalone_block() {
        // No commit_plan: the region never materializes, so the embed fails
        // and the presenter must fall back to the auto block — rail rows
        // make no sense without a region.
        let tl = Timeline::new(TimelineMode::Interactive { color: false }, false, "t");
        let presenter = CliPresenter::embedded(
            &HookOutputConfig::default(),
            tl.handle(),
            StepKey::new(StageId::PostCreateHooks),
        );
        presenter.on_phase_start("worktree-post-create", None);
        assert_eq!(presenter.renderer_kind(), Some("block"));
    }

    #[test]
    fn auto_returns_arc() {
        let config = HookOutputConfig::default();
        let presenter = CliPresenter::auto(&config);
        // Should be usable as a trait object.
        let _: Arc<dyn JobPresenter> = presenter;
    }

    #[test]
    fn from_renderer_creates_presenter() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);
        let _: Arc<dyn JobPresenter> = presenter;
    }

    #[test]
    fn lifecycle_through_trait() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter: Arc<dyn JobPresenter> = CliPresenter::from_renderer(renderer);

        presenter.on_phase_start("post-clone", None);
        presenter.on_job_start("install", Some("Install dependencies"), None);
        presenter.on_job_output("install", "fetching packages...");
        presenter.on_job_success("install", Duration::from_secs(2));
        presenter.on_phase_complete(Duration::from_secs(3));

        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "install");
        assert_eq!(results[0].status, NodeStatus::Succeeded);
    }

    #[test]
    fn failure_maps_to_failed_status() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);

        presenter.on_job_start("build", None, None);
        presenter.on_job_failure("build", Duration::from_secs(1));

        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Failed);
    }

    #[test]
    fn skipped_maps_to_skipped_status() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);

        presenter.on_job_start("lint", None, None);
        presenter.on_job_skipped(
            "lint",
            "no files changed",
            Duration::from_millis(10),
            false,
            None,
        );

        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Skipped);
    }

    #[test]
    fn take_results_drains() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);

        presenter.on_job_start("a", None, None);
        presenter.on_job_success("a", Duration::from_secs(1));

        let first = presenter.take_results();
        assert_eq!(first.len(), 1);

        let second = presenter.take_results();
        assert!(second.is_empty());
    }

    #[test]
    fn entry_to_job_result_success() {
        let entry = JobResultEntry {
            name: "job".into(),
            outcome: JobOutcome::Success,
            duration: Duration::from_secs(1),
        };
        let result = CliPresenter::entry_to_job_result(entry);
        assert_eq!(result.status, NodeStatus::Succeeded);
        assert!(result.exit_code.is_none());
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn entry_to_job_result_failed() {
        let entry = JobResultEntry {
            name: "job".into(),
            outcome: JobOutcome::Failed,
            duration: Duration::from_secs(2),
        };
        let result = CliPresenter::entry_to_job_result(entry);
        assert_eq!(result.status, NodeStatus::Failed);
    }

    #[test]
    fn entry_to_job_result_skipped() {
        let entry = JobResultEntry {
            name: "job".into(),
            outcome: JobOutcome::Skipped {
                reason: "condition".into(),
                show_duration: true,
            },
            duration: Duration::from_millis(50),
        };
        let result = CliPresenter::entry_to_job_result(entry);
        assert_eq!(result.status, NodeStatus::Skipped);
    }

    #[test]
    fn on_message_does_not_panic() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);
        presenter.on_message("informational message");
    }

    #[test]
    fn usable_across_threads() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter: Arc<dyn JobPresenter> = CliPresenter::from_renderer(renderer);
        let p = Arc::clone(&presenter);

        let handle = std::thread::spawn(move || {
            p.on_job_start("threaded", None, None);
            p.on_job_success("threaded", Duration::from_millis(50));
        });

        handle.join().unwrap();
        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "threaded");
    }
}
