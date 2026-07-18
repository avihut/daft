//! Dependency graph for parallelized sync/prune operations.
//!
//! Defines task types, status tracking, and operation phases used by
//! the sync and prune TUI renderers.

use crate::core::ownership::BranchOwner;
use crate::core::worktree::info_field::FieldSet;
use crate::hooks::HookType;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Semantic outcomes that a task can produce. Downstream tasks may
/// inspect these as preconditions for whether to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskOutcome {
    /// Rebase had conflicts and was aborted.
    Conflict,
}

/// Identifies a single executable task in the sync DAG.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TaskId {
    /// Fetch from remote with --prune.
    Fetch,
    /// Prune a single gone branch (and its worktree if present).
    Prune(String),
    /// Update (pull) a single worktree.
    Update(String),
    /// Rebase a worktree onto a base branch.
    Rebase(String),
    /// Push a worktree branch to its remote.
    Push(String),
    /// Push every pushable owned branch in one `git push` — the pre-push
    /// hook fires once with all refs (#678, `pushHookStrategy: batched`).
    PushBatch,
    /// Set up a worktree during clone.
    Setup(String),
    /// Remove a single worktree (path is the unique key).
    RemoveWorktree(std::path::PathBuf),
    /// Remove the bare git directory after all worktrees are gone.
    RemoveBare,
}

/// Execution status of a single task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    /// Skipped because a dependency failed.
    DepFailed,
    /// Task checked preconditions and chose not to run.
    PreconditionFailed,
    /// The run was cancelled (Ctrl+C / SIGTERM) before or during this
    /// task. Distinct from `Failed` so cancellation never trips the
    /// failure exit path.
    Cancelled,
    /// Killed by the resource governor under memory pressure (#678); the
    /// executor resets it to Pending and requeues it (bounded retries).
    /// Transient — never carried by a `TaskCompleted` event.
    Evicted,
}

impl TaskStatus {
    /// Whether this status is a terminal state (no further transitions).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::Skipped
                | Self::DepFailed
                | Self::PreconditionFailed
                | Self::Cancelled
        )
    }
}

/// Typed task completion message, replacing string sentinels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskMessage {
    /// Generic success with displayable git output.
    Ok(String),
    /// Git pull/rebase reported "Already up to date".
    UpToDate,
    /// Rebase had conflicts and was aborted.
    Conflict,
    /// Worktree/branch was removed by prune.
    Removed,
    /// Prune deferred (current worktree — handled post-TUI).
    Deferred,
    /// Prune found nothing to remove.
    NoActionNeeded,
    /// Prune skipped because worktree has uncommitted changes.
    SkippedDirty,
    /// Prune kept the worktree: refined untracked daft files need
    /// consolidation (`daft file merge`) or --force.
    SkippedRefined,
    /// Prune kept the branch: remote is gone but the local branch is not
    /// merged into the default branch.
    SkippedUnmerged,
    /// Update couldn't fast-forward (branch diverged from upstream).
    Diverged,
    /// Push completed successfully.
    Pushed,
    /// Branch has no upstream tracking branch (push skipped).
    NoPushUpstream,
    /// Task failed with error message.
    Failed(String),
    /// The run was cancelled before/during this task.
    Cancelled,
    /// Worktree was created during clone.
    Created,
    /// Base worktree was created during clone.
    BaseCreated,
    /// Branch was not found on remote.
    NotFound,
}

/// A high-level operation phase shown in the operation header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationPhase {
    Fetch,
    Prune,
    Update,
    Rebase(String),
    Push,
    Setup,
    /// Removing a repo's worktrees (and finally its bare git dir).
    RemoveRepo,
}

impl OperationPhase {
    /// Human-readable label for the operation header.
    pub fn label(&self) -> String {
        match self {
            Self::Fetch => "Fetching remote branches".into(),
            Self::Prune => "Pruning stale branches".into(),
            Self::Update => "Updating worktrees".into(),
            Self::Rebase(branch) => format!("Rebasing onto {branch}"),
            Self::Push => "Pushing to remote".into(),
            Self::Setup => "Setting up worktrees".to_string(),
            Self::RemoveRepo => "Removing repository".to_string(),
        }
    }
}

/// A node in the sync dependency graph.
#[derive(Debug)]
pub struct SyncTask {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Which operation phase this task belongs to.
    pub phase: OperationPhase,
    /// Worktree path (None for Fetch tasks).
    pub worktree_path: Option<PathBuf>,
    /// Branch name associated with this task.
    pub branch_name: String,
}

/// The dependency graph for a sync or prune operation.
#[derive(Debug)]
pub struct SyncDag {
    /// All tasks in topological-ish order (fetch first).
    pub tasks: Vec<SyncTask>,
    /// For each task index, the indices of tasks it depends on.
    dependencies: Vec<Vec<usize>>,
    /// For each task index, the indices of tasks that depend on it.
    pub(crate) dependents: Vec<Vec<usize>>,
    /// The rebase base branch, if rebase was requested.
    rebase_branch: Option<String>,
    /// Whether push tasks are included.
    push: bool,
}

impl SyncDag {
    /// Build a DAG for `daft sync` (with optional rebase and push).
    ///
    /// `owned_worktrees` get Update + Rebase + Push tasks.
    /// `unowned_worktrees` get Update tasks only (no rebase/push).
    /// Branches with `None` paths are local-only (no persistent worktree).
    pub fn build_sync(
        owned_worktrees: Vec<(String, Option<PathBuf>)>,
        unowned_worktrees: Vec<(String, Option<PathBuf>)>,
        gone_branches: Vec<String>,
        rebase_branch: Option<String>,
        push: bool,
    ) -> Self {
        Self::build_sync_with_strategy(
            owned_worktrees,
            unowned_worktrees,
            gone_branches,
            rebase_branch,
            push,
            false,
        )
    }

    /// [`Self::build_sync`] with an explicit push strategy: `batched_push`
    /// replaces the per-branch Push tasks with one `PushBatch` barrier task
    /// depending on every pushable branch's last task (#678). The barrier
    /// cost is documented: the batch waits for the slowest rebase.
    pub fn build_sync_with_strategy(
        owned_worktrees: Vec<(String, Option<PathBuf>)>,
        unowned_worktrees: Vec<(String, Option<PathBuf>)>,
        gone_branches: Vec<String>,
        rebase_branch: Option<String>,
        push: bool,
        batched_push: bool,
    ) -> Self {
        let stored_rebase_branch = rebase_branch.clone();
        let mut tasks = Vec::new();
        let mut dependencies: Vec<Vec<usize>> = Vec::new();
        let mut dependents: Vec<Vec<usize>> = Vec::new();

        // Collect all worktrees for Update tasks.
        let all_worktrees: Vec<(String, Option<PathBuf>)> = owned_worktrees
            .iter()
            .cloned()
            .chain(unowned_worktrees.iter().cloned())
            .collect();

        // Set of owned branch names for filtering rebase/push.
        let owned_set: HashSet<String> = owned_worktrees.iter().map(|(b, _)| b.clone()).collect();

        // Helper to push a task and its deps, returning the new index.
        let mut push_task = |task: SyncTask, deps: Vec<usize>| -> usize {
            let idx = tasks.len();
            tasks.push(task);
            // For each dependency, record that this new task depends on it.
            for &dep in &deps {
                // Grow dependents vec if needed.
                if dependents.len() <= dep {
                    dependents.resize_with(dep + 1, Vec::new);
                }
                dependents[dep].push(idx);
            }
            dependencies.push(deps);
            // Ensure dependents vec covers the new index too.
            if dependents.len() <= idx {
                dependents.resize_with(idx + 1, Vec::new);
            }
            idx
        };

        // 1. Fetch task (index 0, no deps).
        let fetch_idx = push_task(
            SyncTask {
                id: TaskId::Fetch,
                phase: OperationPhase::Fetch,
                worktree_path: None,
                branch_name: String::new(),
            },
            vec![],
        );

        // 2. Prune tasks (each depends on fetch).
        for branch in &gone_branches {
            push_task(
                SyncTask {
                    id: TaskId::Prune(branch.clone()),
                    phase: OperationPhase::Prune,
                    worktree_path: None,
                    branch_name: branch.clone(),
                },
                vec![fetch_idx],
            );
        }

        // 3. Update tasks for ALL worktrees (each depends on fetch).
        let mut update_indices: Vec<(String, usize)> = Vec::new();
        for (branch, path) in &all_worktrees {
            let idx = push_task(
                SyncTask {
                    id: TaskId::Update(branch.clone()),
                    phase: OperationPhase::Update,
                    worktree_path: path.clone(),
                    branch_name: branch.clone(),
                },
                vec![fetch_idx],
            );
            update_indices.push((branch.clone(), idx));
        }

        // 4. Rebase tasks ONLY for owned worktrees (if rebase_branch is specified).
        // Track the last task index per owned branch for push dependencies.
        let mut last_task_indices: Vec<(String, Option<PathBuf>, usize)> = update_indices
            .iter()
            .filter(|(branch, _)| owned_set.contains(branch))
            .map(|(branch, idx)| {
                let path = all_worktrees
                    .iter()
                    .find(|(b, _)| b == branch)
                    .map(|(_, p)| p.clone())
                    .unwrap_or_default();
                (branch.clone(), path, *idx)
            })
            .collect();

        if let Some(ref base_branch) = rebase_branch {
            // Find the update task for the base branch.
            let base_update_idx = update_indices
                .iter()
                .find(|(b, _)| b == base_branch)
                .map(|(_, idx)| *idx);

            for (branch, path) in &owned_worktrees {
                // Don't rebase the base branch onto itself.
                if branch == base_branch {
                    continue;
                }

                // Find the update task for this branch.
                let this_update_idx = update_indices
                    .iter()
                    .find(|(b, _)| b == branch)
                    .map(|(_, idx)| *idx);

                let mut deps = Vec::new();
                if let Some(idx) = base_update_idx {
                    deps.push(idx);
                }
                if let Some(idx) = this_update_idx {
                    deps.push(idx);
                }

                let rebase_idx = push_task(
                    SyncTask {
                        id: TaskId::Rebase(branch.clone()),
                        phase: OperationPhase::Rebase(base_branch.clone()),
                        worktree_path: path.clone(),
                        branch_name: branch.clone(),
                    },
                    deps,
                );

                // Update last task index for this branch to the rebase task.
                if let Some(entry) = last_task_indices.iter_mut().find(|(b, _, _)| b == branch) {
                    entry.2 = rebase_idx;
                }
            }
        }

        // 5. Push tasks ONLY for owned worktrees (if push is enabled).
        // The rebase base branch is excluded: daft only used its locally-fetched
        // tip as a rebase target, so pushing it could overwrite commits other
        // contributors landed between fetch and sync completion.
        if push {
            if batched_push {
                let deps: Vec<usize> = last_task_indices
                    .iter()
                    .filter(|(branch, _, _)| rebase_branch.as_ref() != Some(branch))
                    .map(|(_, _, idx)| *idx)
                    .collect();
                // Empty branch_name keeps the TUI's auto-create guard from
                // inventing a row for the barrier node; per-branch rows are
                // driven by the batch executor's synthetic events.
                if !deps.is_empty() {
                    push_task(
                        SyncTask {
                            id: TaskId::PushBatch,
                            phase: OperationPhase::Push,
                            worktree_path: None,
                            branch_name: String::new(),
                        },
                        deps,
                    );
                }
            } else {
                for (branch, path, last_idx) in &last_task_indices {
                    if rebase_branch.as_ref() == Some(branch) {
                        continue;
                    }
                    push_task(
                        SyncTask {
                            id: TaskId::Push(branch.clone()),
                            phase: OperationPhase::Push,
                            worktree_path: path.clone(),
                            branch_name: branch.clone(),
                        },
                        vec![*last_idx],
                    );
                }
            }
        }

        Self {
            tasks,
            dependencies,
            dependents,
            rebase_branch: stored_rebase_branch,
            push,
        }
    }

    /// Build a DAG for `daft prune`.
    pub fn build_prune(gone_branches: Vec<String>) -> Self {
        Self::build_sync(vec![], vec![], gone_branches, None, false)
    }

    /// Build a DAG for `daft repo remove`.
    ///
    /// Each `(branch_name, worktree_path)` becomes a `RemoveWorktree` task with no
    /// inter-task dependencies (parallel removal). A terminal `RemoveBare` task
    /// depends on every worktree task; if `worktrees` is empty, `RemoveBare` is
    /// the sole task with no dependencies.
    pub fn build_remove_repo(
        worktrees: Vec<(String, std::path::PathBuf)>,
        bare_git_dir: std::path::PathBuf,
    ) -> Self {
        let mut tasks = Vec::new();
        let mut dependencies: Vec<Vec<usize>> = Vec::new();
        let mut dependents: Vec<Vec<usize>> = Vec::new();

        let mut push_task = |task: SyncTask, deps: Vec<usize>| -> usize {
            let idx = tasks.len();
            tasks.push(task);
            for &dep in &deps {
                if dependents.len() <= dep {
                    dependents.resize_with(dep + 1, Vec::new);
                }
                dependents[dep].push(idx);
            }
            dependencies.push(deps);
            if dependents.len() <= idx {
                dependents.resize_with(idx + 1, Vec::new);
            }
            idx
        };

        let mut worktree_indices = Vec::new();
        for (branch, path) in &worktrees {
            let idx = push_task(
                SyncTask {
                    id: TaskId::RemoveWorktree(path.clone()),
                    phase: OperationPhase::RemoveRepo,
                    worktree_path: Some(path.clone()),
                    branch_name: branch.clone(),
                },
                vec![],
            );
            worktree_indices.push(idx);
        }

        push_task(
            SyncTask {
                id: TaskId::RemoveBare,
                phase: OperationPhase::RemoveRepo,
                worktree_path: Some(bare_git_dir),
                // Intentionally empty: the TUI's TaskStarted handler
                // auto-creates a row for any non-empty branch_name. A
                // sentinel like "(bare)" would surface as a synthetic row
                // even though no row exists for the bare git dir. The
                // OperationPhase header ("Removing repository") already
                // covers progress for this task visually.
                branch_name: String::new(),
            },
            worktree_indices,
        );

        Self {
            tasks,
            dependencies,
            dependents,
            rebase_branch: None,
            push: false,
        }
    }

    /// Get the dependency indices for a task.
    pub fn dependencies_of(&self, task_idx: usize) -> &[usize] {
        &self.dependencies[task_idx]
    }

    /// Get the dependent indices for a task.
    pub fn dependents_of(&self, task_idx: usize) -> &[usize] {
        &self.dependents[task_idx]
    }

    /// Get the ordered list of operation phases for the header display.
    ///
    /// For sync, always includes Fetch, Prune, Update even if no tasks exist
    /// for those phases. Only includes Rebase if a rebase branch was specified.
    pub fn phases(&self) -> Vec<OperationPhase> {
        let mut phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
        ];

        if let Some(ref branch) = self.rebase_branch {
            phases.push(OperationPhase::Rebase(branch.clone()));
        }

        if self.push {
            phases.push(OperationPhase::Push);
        }

        phases
    }
}

/// A typed delta over `WorktreeInfo`. Each variant maps 1:1 to one
/// underlying git/FS call cluster in the streaming collector.
#[derive(Debug, Clone)]
pub enum WorktreeInfoPatch {
    BaseAheadBehind(Option<(usize, usize)>),
    RemoteAheadBehind(Option<(usize, usize)>),
    Changes {
        staged: usize,
        unstaged: usize,
        untracked: usize,
    },
    LastCommit {
        timestamp: Option<i64>,
        hash: Option<String>,
        subject: String,
    },
    BranchAge(Option<i64>),
    Owner(Option<BranchOwner>),
    BaseLines(Option<(usize, usize)>),
    ChangesLines {
        staged: (usize, usize),
        unstaged: (usize, usize),
    },
    RemoteLines(Option<(usize, usize)>),
    Size(Option<u64>),
    Mtime(Option<i64>),
    ForgeRef(Option<super::forge_ref::ForgeBranchRef>),
}

/// Why a patch was emitted. `LiveTable` uses this to suppress stale
/// patches: a `Collector` patch arriving after a `PostFetch` patch covering
/// the same field on the same branch is dropped. Priority order:
/// `PostTask > PostFetch > Collector`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSource {
    Collector,
    PostFetch,
    PostTask(OperationPhase),
}

impl PatchSource {
    /// Higher = more authoritative. Used for staleness suppression.
    pub fn priority(&self) -> u8 {
        match self {
            Self::Collector => 0,
            Self::PostFetch => 1,
            Self::PostTask(_) => 2,
        }
    }
}

/// The hook phase a `DagEvent` hook/job event belongs to: a lifecycle hook
/// run by daft, or the synthetic `pre-push` stage reported around a hooked
/// git push (#599). `HookType` stays closed — the pre-push stage is not a
/// daft lifecycle hook, just a reported phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DagHookPhase {
    Lifecycle(HookType),
    PrePush,
}

impl DagHookPhase {
    /// Short status-column label (kept narrow for the table layout).
    pub fn label(&self) -> &'static str {
        match self {
            DagHookPhase::Lifecycle(HookType::PreRemove) => "pre-remove",
            DagHookPhase::Lifecycle(HookType::PostRemove) => "post-remove",
            DagHookPhase::Lifecycle(HookType::PreCreate) => "pre-create",
            DagHookPhase::Lifecycle(HookType::PostCreate) => "post-create",
            DagHookPhase::Lifecycle(HookType::PostClone) => "post-clone",
            DagHookPhase::Lifecycle(HookType::PreMerge) => "pre-merge",
            DagHookPhase::Lifecycle(HookType::PostMerge) => "post-merge",
            DagHookPhase::PrePush => "pre-push",
        }
    }

    /// Canonical hook name for summaries (matches `HookType::filename`).
    pub fn hook_name(&self) -> &'static str {
        match self {
            DagHookPhase::Lifecycle(hook_type) => hook_type.filename(),
            DagHookPhase::PrePush => "pre-push",
        }
    }
}

impl From<HookType> for DagHookPhase {
    fn from(hook_type: HookType) -> Self {
        DagHookPhase::Lifecycle(hook_type)
    }
}

/// Message sent from worker threads to the renderer.
#[derive(Debug, Clone)]
pub enum DagEvent {
    /// A task started running.
    TaskStarted {
        phase: OperationPhase,
        branch_name: String,
    },
    /// A task completed.
    TaskCompleted {
        phase: OperationPhase,
        branch_name: String,
        status: TaskStatus,
        /// Typed result message.
        message: TaskMessage,
    },
    /// A task was held back by the resource governor (#678): deferred
    /// before launch (emitted on the not-throttled → throttled transition
    /// only; the matching `TaskStarted` announces admission), frozen
    /// mid-run (`TaskResumed` announces the thaw), or evicted and
    /// requeued.
    TaskThrottled {
        phase: OperationPhase,
        branch_name: String,
        reason: ThrottleReason,
    },
    /// A frozen task's processes were thawed and it is running again.
    TaskResumed {
        phase: OperationPhase,
        branch_name: String,
    },
    /// All tasks are done.
    AllDone,
    /// A hook started running for a branch.
    HookStarted {
        branch_name: String,
        hook_type: DagHookPhase,
    },
    /// A hook completed for a branch.
    HookCompleted {
        branch_name: String,
        hook_type: DagHookPhase,
        success: bool,
        /// Non-zero exit with FailMode::Warn.
        warned: bool,
        duration: Duration,
        /// Exit code from the hook process, if available.
        exit_code: Option<i32>,
        /// Captured stdout+stderr, only stored on failure/warning.
        output: Option<String>,
    },
    /// A job started running within a hook.
    JobStarted {
        branch_name: String,
        hook_type: DagHookPhase,
        job_name: String,
    },
    /// A job completed within a hook.
    JobCompleted {
        branch_name: String,
        hook_type: DagHookPhase,
        job_name: String,
        status: JobCompletionStatus,
        duration: Duration,
        skip_reason: Option<String>,
    },

    /// A patch landed for `branch_name` from `source`. Carries one cluster
    /// of cells produced by a single underlying git/FS call.
    WorktreeInfoUpdated {
        branch_name: String,
        patch: WorktreeInfoPatch,
        source: PatchSource,
    },

    /// The forge-PR cache finished a background refresh while the table is
    /// live: swap in the fresh PR-column lookup so rows re-decorate without
    /// waiting for the next invocation. Emitted by `daft list`'s cache poll
    /// (command layer — renderers never read the store), not by collectors.
    ForgePrsRefreshed(super::forge_ref::ForgePrLookup),

    /// The initial `source=Collector` run completed. Subset re-runs
    /// (`PostFetch`, `PostTask`) do not emit this — they end silently.
    WorktreeInfoCollectionDone,
}

/// Terminal status for a job within a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCompletionStatus {
    Succeeded,
    Failed,
    Skipped,
}

/// Admission decision returned by [`DagGovernor::try_admit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmitDecision {
    /// Run the task now. The governor reserved a slot; the executor pairs
    /// this with exactly one [`DagGovernor::release`] when the task leaves
    /// the running set.
    Admit,
    /// Keep the task in the ready queue and re-check admission later.
    Defer(DeferReason),
}

/// Why the governor deferred a ready task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferReason {
    /// The concurrency cap for the task's class is reached.
    ClassCap,
    /// Not enough memory headroom to admit another hook-bearing push.
    MemoryPressure,
    /// A governor kill just happened; waiting out the post-kill cooldown.
    KillCooldown,
}

/// How the governor is holding a task back (payload of
/// [`DagEvent::TaskThrottled`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleReason {
    /// Held in the ready queue before launch.
    Deferred(DeferReason),
    /// Launched, then SIGSTOP'd mid-run to relieve memory pressure.
    Frozen,
    /// Killed under memory pressure and requeued; `attempt` counts the
    /// retries consumed so far.
    Evicted { attempt: u8 },
}

/// Admission gate consulted by [`DagExecutor`] before it runs a ready task.
///
/// Contract (the executor relies on every point):
/// - `try_admit` returning [`AdmitDecision::Admit`] reserves one slot and is
///   paired with exactly one [`DagGovernor::release`]; returning
///   [`AdmitDecision::Defer`] must be side-effect-free.
/// - Task classes the governor does not manage are always admitted.
/// - A governor that currently tracks zero admitted units must admit — this
///   is the executor's liveness guarantee (workers re-check admission on a
///   timeout, so an all-deferred ready queue with nothing running would
///   otherwise never make progress).
/// - Both methods are called with the executor's internal lock held: they
///   must return promptly and never call back into the executor.
pub trait DagGovernor: Send + Sync {
    /// Decide whether `task` may start now.
    fn try_admit(&self, task: &SyncTask) -> AdmitDecision;
    /// Return the slot reserved by a successful `try_admit`.
    fn release(&self, task: &SyncTask);
}

/// Stage-0 resource governor: a fixed cap on concurrent push tasks.
///
/// Sync constructs this only when the repo has an executable pre-push hook
/// and hooks are honored — pushes are the one task class whose subprocess
/// (git + hook) multiplies memory use with parallelism (#678). All other
/// task classes are always admitted.
#[derive(Debug)]
pub struct StaticCapGovernor {
    cap: usize,
    push_active: std::sync::atomic::AtomicUsize,
}

impl StaticCapGovernor {
    /// Cap concurrent push tasks at `cap` (clamped to at least 1).
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            push_active: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl DagGovernor for StaticCapGovernor {
    fn try_admit(&self, task: &SyncTask) -> AdmitDecision {
        use std::sync::atomic::Ordering;
        if !matches!(task.id, TaskId::Push(_) | TaskId::PushBatch) {
            return AdmitDecision::Admit;
        }
        let reserved = self
            .push_active
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |active| {
                (active < self.cap).then_some(active + 1)
            })
            .is_ok();
        if reserved {
            AdmitDecision::Admit
        } else {
            AdmitDecision::Defer(DeferReason::ClassCap)
        }
    }

    fn release(&self, task: &SyncTask) {
        if matches!(task.id, TaskId::Push(_) | TaskId::PushBatch) {
            self.push_active
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// How often a worker re-checks admission when every ready task is deferred
/// by the governor. Pressure can clear without any task completing, so a
/// plain `Condvar::wait` could sleep long past the moment a deferred push
/// becomes admissible.
const ADMISSION_RECHECK: Duration = Duration::from_millis(200);

/// Retries a governor-evicted push gets before it fails terminally
/// (so 3 attempts total). Pre-push hooks are re-runnable checks by
/// convention, and a killed `git push` is atomic server-side.
const MAX_PUSH_RETRIES: u8 = 2;

/// Shared mutable state for the worker pool.
struct DagState {
    ready: Vec<usize>,
    status: Vec<TaskStatus>,
    in_degree: Vec<usize>,
    active: usize,
    done: usize,
    total: usize,
    branch_outcomes: HashMap<String, HashSet<TaskOutcome>>,
    /// Per-task "currently deferred by the governor" flag, so
    /// `TaskThrottled` fires once per transition instead of once per scan.
    throttled: Vec<bool>,
    /// Per-task governor-eviction retry counter (#678 stage 3).
    attempts: Vec<u8>,
}

/// Executes a DAG of sync tasks in parallel.
pub struct DagExecutor {
    dag: SyncDag,
    sender: mpsc::Sender<DagEvent>,
    cancel: Option<std::sync::Arc<crate::git::cancel::CancelFlag>>,
    governor: Option<Arc<dyn DagGovernor>>,
}

impl DagExecutor {
    /// Create a new executor for the given DAG, sending events through `sender`.
    pub fn new(dag: SyncDag, sender: mpsc::Sender<DagEvent>) -> Self {
        Self {
            dag,
            sender,
            cancel: None,
            governor: None,
        }
    }

    /// Gate task admission through a resource governor (#678). Deferred
    /// tasks stay in the ready queue — workers skip over them to whatever
    /// else is runnable and re-check on completions or a short timeout.
    pub fn with_governor(mut self, governor: Arc<dyn DagGovernor>) -> Self {
        self.governor = Some(governor);
        self
    }

    /// Observe a shared cancel flag: once it goes active, workers stop
    /// popping new tasks and every still-pending task resolves as
    /// `Cancelled` (with a `TaskCompleted` event, so UI rows converge).
    /// In-flight tasks finish on their own — their subprocess seams
    /// observe the same flag.
    pub fn with_cancel(mut self, cancel: std::sync::Arc<crate::git::cancel::CancelFlag>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Execute all tasks in the DAG, calling `task_fn` for each task.
    ///
    /// Tasks are executed in parallel (respecting dependencies) using a thread pool.
    /// The closure receives a reference to the `SyncTask` and the current set of
    /// outcome tags for that branch, and must return a
    /// `(TaskStatus, TaskMessage, HashSet<TaskOutcome>)` tuple indicating the
    /// result status, a typed message, and updated outcome tags.
    ///
    /// Refreshed worktree info is now propagated out-of-band as
    /// `WorktreeInfoUpdated` patches with `PatchSource::PostTask(phase)`.
    /// Callers wire this up via `list_stream::spawn` in their task closures.
    ///
    /// Consumes `self` so that the sender is dropped after `AllDone` is sent,
    /// allowing the receiver to detect channel closure.
    pub fn run<F>(self, task_fn: F)
    where
        F: Fn(&SyncTask, &HashSet<TaskOutcome>) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>)
            + Send
            + Sync,
    {
        let n = self.dag.tasks.len();

        // Compute initial in-degrees from the DAG's dependencies.
        let mut in_degree = vec![0usize; n];
        for (i, deps) in self.dag.dependencies.iter().enumerate() {
            in_degree[i] = deps.len();
        }

        // Find initially ready tasks (in-degree == 0).
        let ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

        let state = Arc::new((
            Mutex::new(DagState {
                ready,
                status: vec![TaskStatus::Pending; n],
                in_degree,
                active: 0,
                done: 0,
                total: n,
                branch_outcomes: HashMap::new(),
                throttled: vec![false; n],
                attempts: vec![0; n],
            }),
            Condvar::new(),
        ));

        let max_workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);

        let task_fn = &task_fn;
        let dag = &self.dag;
        let sender = &self.sender;
        let cancel = self.cancel.as_deref();
        let governor = self.governor.as_deref();

        std::thread::scope(|scope| {
            for _ in 0..max_workers {
                let state = Arc::clone(&state);

                scope.spawn(move || {
                    loop {
                        let task_idx;
                        {
                            let (lock, cvar) = &*state;
                            let mut s = lock.lock().unwrap();

                            loop {
                                // A live cancel resolves every still-pending
                                // task in place of popping it. Each swept task
                                // still gets its TaskCompleted event — UI rows
                                // must converge or the render loop never ends.
                                // Idempotent by construction: the sweep leaves
                                // no Pending tasks for a second pass to find.
                                if let Some(flag) = cancel
                                    && flag.is_cancelled()
                                {
                                    let mut swept = false;
                                    for idx in 0..n {
                                        if s.status[idx] == TaskStatus::Pending {
                                            s.status[idx] = TaskStatus::Cancelled;
                                            s.done += 1;
                                            swept = true;
                                            let _ = sender.send(DagEvent::TaskCompleted {
                                                phase: dag.tasks[idx].phase.clone(),
                                                branch_name: dag.tasks[idx].branch_name.clone(),
                                                status: TaskStatus::Cancelled,
                                                message: TaskMessage::Cancelled,
                                            });
                                        }
                                    }
                                    s.ready.clear();
                                    if swept {
                                        cvar.notify_all();
                                    }
                                }

                                // Try to pop a ready task the governor admits.
                                // Scanning from the end preserves `pop()`'s
                                // LIFO bias; deferred tasks stay in `ready` so
                                // this worker can pick up anything else
                                // runnable instead of blocking on them.
                                let popped = match governor {
                                    None => s.ready.pop(),
                                    Some(gov) => {
                                        let mut admitted = None;
                                        for pos in (0..s.ready.len()).rev() {
                                            let idx = s.ready[pos];
                                            match gov.try_admit(&dag.tasks[idx]) {
                                                AdmitDecision::Admit => {
                                                    admitted = Some(pos);
                                                    break;
                                                }
                                                AdmitDecision::Defer(reason) => {
                                                    if !s.throttled[idx] {
                                                        s.throttled[idx] = true;
                                                        let _ =
                                                            sender.send(DagEvent::TaskThrottled {
                                                                phase: dag.tasks[idx].phase.clone(),
                                                                branch_name: dag.tasks[idx]
                                                                    .branch_name
                                                                    .clone(),
                                                                reason: ThrottleReason::Deferred(
                                                                    reason,
                                                                ),
                                                            });
                                                    }
                                                }
                                            }
                                        }
                                        admitted.map(|pos| s.ready.remove(pos))
                                    }
                                };
                                if let Some(idx) = popped {
                                    task_idx = idx;
                                    s.throttled[task_idx] = false;
                                    s.status[task_idx] = TaskStatus::Running;
                                    s.active += 1;
                                    break;
                                }

                                // No ready tasks: check if we're done.
                                if s.done == s.total {
                                    return;
                                }

                                // Nothing ready but work still in flight — wait.
                                if s.active == 0 && s.ready.is_empty() {
                                    // All tasks are either done or dep-failed; no more work.
                                    return;
                                }

                                s = if governor.is_some() && !s.ready.is_empty() {
                                    // Every ready task is currently deferred by
                                    // the governor — re-check admission on a
                                    // short timeout; no completion may arrive
                                    // to wake us.
                                    cvar.wait_timeout(s, ADMISSION_RECHECK).unwrap().0
                                } else {
                                    cvar.wait(s).unwrap()
                                };
                            }
                        }

                        // Send TaskStarted event.
                        let _ = sender.send(DagEvent::TaskStarted {
                            phase: dag.tasks[task_idx].phase.clone(),
                            branch_name: dag.tasks[task_idx].branch_name.clone(),
                        });

                        // Snapshot branch outcomes before calling task_fn.
                        let branch_outcomes = {
                            let (lock, _) = &*state;
                            let s = lock.lock().unwrap();
                            s.branch_outcomes
                                .get(&dag.tasks[task_idx].branch_name)
                                .cloned()
                                .unwrap_or_default()
                        };

                        // Execute the task outside the lock.
                        let task = &dag.tasks[task_idx];
                        let (result_status, message, returned_outcomes) =
                            task_fn(task, &branch_outcomes);

                        // Update DAG state.
                        {
                            let (lock, cvar) = &*state;
                            let mut s = lock.lock().unwrap();
                            // Return the governor slot before anything else so
                            // admission accounting never lags the running set —
                            // and strictly before a requeue re-enters `ready`.
                            if let Some(gov) = governor {
                                gov.release(&dag.tasks[task_idx]);
                            }
                            s.active -= 1;

                            // Governor eviction (#678): not a completion.
                            // Reset to Pending and requeue — no `done`
                            // increment, no outcome write, no dependent
                            // bookkeeping, no TaskCompleted. Admission holds
                            // the retry until pressure clears (post-kill
                            // cooldown). Retries exhausted → terminal Failed
                            // through the normal path below.
                            let mut result_status = result_status;
                            let mut message = message;
                            if result_status == TaskStatus::Evicted {
                                if s.attempts[task_idx] < MAX_PUSH_RETRIES {
                                    s.attempts[task_idx] += 1;
                                    s.status[task_idx] = TaskStatus::Pending;
                                    s.throttled[task_idx] = true;
                                    s.ready.push(task_idx);
                                    let _ = sender.send(DagEvent::TaskThrottled {
                                        phase: dag.tasks[task_idx].phase.clone(),
                                        branch_name: dag.tasks[task_idx].branch_name.clone(),
                                        reason: ThrottleReason::Evicted {
                                            attempt: s.attempts[task_idx],
                                        },
                                    });
                                    cvar.notify_all();
                                    continue;
                                }
                                result_status = TaskStatus::Failed;
                                message = TaskMessage::Failed(format!(
                                    "push killed under memory pressure ({} attempts)",
                                    s.attempts[task_idx] + 1
                                ));
                            }

                            s.status[task_idx] = result_status;
                            s.done += 1;

                            let branch = &dag.tasks[task_idx].branch_name;
                            if !branch.is_empty() {
                                s.branch_outcomes.insert(branch.clone(), returned_outcomes);
                            }

                            if result_status == TaskStatus::Succeeded
                                || result_status == TaskStatus::Skipped
                                || result_status == TaskStatus::PreconditionFailed
                            {
                                // Decrement in-degrees of dependents.
                                for &dep_idx in &dag.dependents[task_idx] {
                                    if s.status[dep_idx] == TaskStatus::Pending {
                                        s.in_degree[dep_idx] -= 1;
                                        if s.in_degree[dep_idx] == 0 {
                                            s.ready.push(dep_idx);
                                        }
                                    }
                                }
                            } else if result_status == TaskStatus::Failed {
                                // Cascade DepFailed to all transitive dependents —
                                // EXCEPT the batched push barrier (#678). That node
                                // fans over every owned branch, so one branch's
                                // failed update/rebase must not sink the push of all
                                // the others. Treat the failed dep as resolved
                                // (decrement its in-degree like a success) and let
                                // execute_push_batch_task skip that branch via
                                // shared_push_skip while pushing the healthy ones.
                                // Per-branch Push nodes keep the normal cascade, so
                                // only that one branch's push is dropped.
                                let mut stack = vec![task_idx];
                                while let Some(idx) = stack.pop() {
                                    for &dep_idx in &dag.dependents[idx] {
                                        if s.status[dep_idx] != TaskStatus::Pending {
                                            continue;
                                        }
                                        if matches!(dag.tasks[dep_idx].id, TaskId::PushBatch) {
                                            s.in_degree[dep_idx] -= 1;
                                            if s.in_degree[dep_idx] == 0 {
                                                s.ready.push(dep_idx);
                                            }
                                            // The barrier has no further dependents;
                                            // don't cascade past it.
                                        } else {
                                            s.status[dep_idx] = TaskStatus::DepFailed;
                                            s.done += 1;
                                            stack.push(dep_idx);
                                        }
                                    }
                                }
                            }

                            // Send TaskCompleted event.
                            let _ = sender.send(DagEvent::TaskCompleted {
                                phase: dag.tasks[task_idx].phase.clone(),
                                branch_name: dag.tasks[task_idx].branch_name.clone(),
                                status: result_status,
                                message: message.clone(),
                            });

                            // Also send TaskCompleted for any dep-failed dependents.
                            if result_status == TaskStatus::Failed {
                                let mut stack = vec![task_idx];
                                let mut visited = std::collections::HashSet::new();
                                while let Some(idx) = stack.pop() {
                                    for &dep_idx in &dag.dependents[idx] {
                                        if s.status[dep_idx] == TaskStatus::DepFailed
                                            && visited.insert(dep_idx)
                                        {
                                            let _ = sender.send(DagEvent::TaskCompleted {
                                                phase: dag.tasks[dep_idx].phase.clone(),
                                                branch_name: dag.tasks[dep_idx].branch_name.clone(),
                                                status: TaskStatus::DepFailed,
                                                message: TaskMessage::Failed(format!(
                                                    "dependency {:?} failed",
                                                    dag.tasks[task_idx].id
                                                )),
                                            });
                                            stack.push(dep_idx);
                                        }
                                    }
                                }
                            }

                            cvar.notify_all();
                        }
                    }
                });
            }
        });

        // All workers are done. Send AllDone and drop sender.
        let _ = self.sender.send(DagEvent::AllDone);
        // self.sender is dropped here when self goes out of scope.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_source_priority_ordering() {
        assert!(
            PatchSource::PostTask(OperationPhase::Push).priority()
                > PatchSource::PostFetch.priority()
        );
        assert!(PatchSource::PostFetch.priority() > PatchSource::Collector.priority());
    }

    #[test]
    fn task_id_display() {
        let id = TaskId::Fetch;
        assert_eq!(format!("{id:?}"), "Fetch");

        let id = TaskId::Prune("feat/old".into());
        assert_eq!(format!("{id:?}"), "Prune(\"feat/old\")");
    }

    #[test]
    fn task_status_is_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Succeeded.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Skipped.is_terminal());
        assert!(TaskStatus::DepFailed.is_terminal());
    }

    #[test]
    fn operation_phase_label() {
        assert_eq!(OperationPhase::Fetch.label(), "Fetching remote branches");
        assert_eq!(
            OperationPhase::Rebase("master".into()).label(),
            "Rebasing onto master"
        );
    }

    #[test]
    fn remove_repo_phase_label() {
        assert_eq!(OperationPhase::RemoveRepo.label(), "Removing repository");
    }

    #[test]
    fn remove_repo_task_ids_are_distinct() {
        use std::path::PathBuf;
        let a = TaskId::RemoveWorktree(PathBuf::from("/tmp/wt-a"));
        let b = TaskId::RemoveWorktree(PathBuf::from("/tmp/wt-b"));
        let bare = TaskId::RemoveBare;
        assert_ne!(a, b);
        assert_ne!(a, bare);
        let mut set = std::collections::HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&a));
        assert!(!set.contains(&b));
    }

    #[test]
    fn build_remove_repo_no_worktrees() {
        use std::path::PathBuf;
        let dag = SyncDag::build_remove_repo(vec![], PathBuf::from("/repo/.git"));
        assert_eq!(dag.tasks.len(), 1);
        assert_eq!(dag.tasks[0].id, TaskId::RemoveBare);
        assert!(dag.dependencies_of(0).is_empty());
    }

    #[test]
    fn build_remove_repo_bare_task_has_empty_branch_name() {
        // The TUI auto-create guard at state.rs:225 keys on a non-empty
        // branch_name; a non-empty sentinel like "(bare)" causes a synthetic
        // row to appear when the bare-removal task fires its TaskStarted
        // event. Keeping branch_name empty suppresses auto-creation while
        // phase activation still works.
        use std::path::PathBuf;
        let dag = SyncDag::build_remove_repo(vec![], PathBuf::from("/repo/.git"));
        assert_eq!(dag.tasks[0].id, TaskId::RemoveBare);
        assert_eq!(
            dag.tasks[0].branch_name, "",
            "RemoveBare branch_name must be empty so the TUI auto-create guard skips it",
        );
    }

    #[test]
    fn build_remove_repo_terminal_bare_depends_on_all_worktrees() {
        use std::path::PathBuf;
        let worktrees = vec![
            ("main".to_string(), PathBuf::from("/repo/main")),
            ("feat/a".to_string(), PathBuf::from("/repo/feat-a")),
            ("feat/b".to_string(), PathBuf::from("/repo/feat-b")),
        ];
        let dag = SyncDag::build_remove_repo(worktrees, PathBuf::from("/repo/.git"));
        assert_eq!(dag.tasks.len(), 4);
        for i in 0..3 {
            assert!(
                dag.dependencies_of(i).is_empty(),
                "worktree task {i} should have no deps",
            );
        }
        let bare_idx = dag.tasks.len() - 1;
        assert_eq!(dag.tasks[bare_idx].id, TaskId::RemoveBare);
        let mut deps = dag.dependencies_of(bare_idx).to_vec();
        deps.sort();
        assert_eq!(deps, vec![0, 1, 2]);
    }

    #[test]
    fn build_sync_dag_no_rebase() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
        ];
        let gone: Vec<String> = vec!["feat/old".into()];

        let dag = SyncDag::build_sync(worktrees, vec![], gone, None, false);

        // 1 fetch + 1 prune + 2 updates = 4 tasks
        assert_eq!(dag.tasks.len(), 4);
        // Fetch has no dependencies
        assert!(dag.dependencies_of(0).is_empty());
        // All others depend on fetch (index 0)
        for i in 1..dag.tasks.len() {
            assert!(dag.dependencies_of(i).contains(&0));
        }
    }

    #[test]
    fn build_sync_dag_with_rebase() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
            ("feat/b".into(), Some(PathBuf::from("/p/feat-b"))),
        ];
        let gone: Vec<String> = vec![];

        let dag = SyncDag::build_sync(worktrees, vec![], gone, Some("master".into()), false);

        // 1 fetch + 3 updates + 2 rebases = 6 tasks
        assert_eq!(dag.tasks.len(), 6);

        // Find the update(master) task index
        let master_update_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();

        // Rebase tasks depend on update(master)
        for (i, task) in dag.tasks.iter().enumerate() {
            if matches!(&task.id, TaskId::Rebase(_)) {
                assert!(
                    dag.dependencies_of(i).contains(&master_update_idx),
                    "Rebase task should depend on update(master)"
                );
            }
        }
    }

    #[test]
    fn build_prune_dag() {
        let gone = vec!["feat/old".into(), "feat/stale".into()];
        let dag = SyncDag::build_prune(gone);

        // 1 fetch + 2 prunes = 3 tasks
        assert_eq!(dag.tasks.len(), 3);
    }

    #[test]
    fn dag_phases_sync() {
        let worktrees = vec![("master".into(), Some(PathBuf::from("/p/master")))];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], None, false);
        let phases = dag.phases();
        assert_eq!(phases.len(), 3); // Fetch, Prune, Update
    }

    #[test]
    fn dag_phases_sync_with_rebase() {
        let worktrees = vec![("master".into(), Some(PathBuf::from("/p/master")))];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], Some("master".into()), false);
        let phases = dag.phases();
        assert_eq!(phases.len(), 4); // Fetch, Prune, Update, Rebase
    }

    #[test]
    fn executor_runs_all_tasks() {
        let dag = SyncDag::build_prune(vec!["feat/a".into()]);
        let (tx, rx) = mpsc::channel();

        let executor = DagExecutor::new(dag, tx);
        executor.run(|_task, outcomes| {
            (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            )
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        let starts = events
            .iter()
            .filter(|e| matches!(e, DagEvent::TaskStarted { .. }))
            .count();
        let completes = events
            .iter()
            .filter(|e| matches!(e, DagEvent::TaskCompleted { .. }))
            .count();
        let dones = events
            .iter()
            .filter(|e| matches!(e, DagEvent::AllDone))
            .count();
        assert_eq!(starts, 2);
        assert_eq!(completes, 2);
        assert_eq!(dones, 1);
    }

    #[test]
    fn executor_respects_dependencies() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
        ];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], Some("master".into()), false);
        let (tx, rx) = mpsc::channel();

        let order = Arc::new(Mutex::new(Vec::new()));
        let order_clone = Arc::clone(&order);

        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task, outcomes| {
            order_clone.lock().unwrap().push(task.id.clone());
            (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            )
        });

        let _events: Vec<DagEvent> = rx.iter().collect();
        let execution_order = order.lock().unwrap();
        // Fetch must come first
        assert_eq!(execution_order[0], TaskId::Fetch);
        // Rebase(feat/a) must come after Update(master)
        let master_pos = execution_order
            .iter()
            .position(|t| *t == TaskId::Update("master".into()))
            .unwrap();
        let rebase_pos = execution_order
            .iter()
            .position(|t| *t == TaskId::Rebase("feat/a".into()))
            .unwrap();
        assert!(master_pos < rebase_pos);
    }

    fn push_task(branch: &str) -> SyncTask {
        SyncTask {
            id: TaskId::Push(branch.into()),
            phase: OperationPhase::Push,
            worktree_path: None,
            branch_name: branch.into(),
        }
    }

    #[test]
    fn static_cap_governor_only_governs_pushes() {
        let gov = StaticCapGovernor::new(1);
        let push = push_task("feat/a");
        let fetch = SyncTask {
            id: TaskId::Fetch,
            phase: OperationPhase::Fetch,
            worktree_path: None,
            branch_name: String::new(),
        };
        assert_eq!(gov.try_admit(&push), AdmitDecision::Admit);
        assert_eq!(
            gov.try_admit(&push),
            AdmitDecision::Defer(DeferReason::ClassCap)
        );
        // Non-push classes are never deferred, even at cap.
        assert_eq!(gov.try_admit(&fetch), AdmitDecision::Admit);
        // Releasing a non-push task must not free a push slot.
        gov.release(&fetch);
        assert_eq!(
            gov.try_admit(&push),
            AdmitDecision::Defer(DeferReason::ClassCap)
        );
        gov.release(&push);
        assert_eq!(gov.try_admit(&push), AdmitDecision::Admit);
    }

    #[test]
    fn static_cap_governor_clamps_cap_to_one() {
        // A zero cap would violate the min-one-runner contract.
        let gov = StaticCapGovernor::new(0);
        let push = push_task("feat/a");
        assert_eq!(gov.try_admit(&push), AdmitDecision::Admit);
        assert_eq!(
            gov.try_admit(&push),
            AdmitDecision::Defer(DeferReason::ClassCap)
        );
    }

    #[test]
    fn static_cap_bounds_concurrent_pushes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let worktrees: Vec<(String, Option<PathBuf>)> = (0..6)
            .map(|i| {
                (
                    format!("feat/b{i}"),
                    Some(PathBuf::from(format!("/p/b{i}"))),
                )
            })
            .collect();
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], None, true);
        let (tx, rx) = mpsc::channel();

        let push_running = Arc::new(AtomicUsize::new(0));
        let push_peak = Arc::new(AtomicUsize::new(0));
        let running = Arc::clone(&push_running);
        let peak = Arc::clone(&push_peak);

        let executor = DagExecutor::new(dag, tx).with_governor(Arc::new(StaticCapGovernor::new(2)));
        executor.run(move |task, outcomes| {
            if matches!(task.id, TaskId::Push(_)) {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(20));
                running.fetch_sub(1, Ordering::SeqCst);
            }
            (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            )
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        let pushed = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DagEvent::TaskCompleted {
                        phase: OperationPhase::Push,
                        status: TaskStatus::Succeeded,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(pushed, 6, "every push must still complete under the cap");
        assert!(
            push_peak.load(Ordering::SeqCst) <= 2,
            "cap of 2 exceeded: peak {}",
            push_peak.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn deferred_pushes_do_not_block_other_tasks() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Needs a second worker to observe overlap; single-core runners
        // serialize everything and prove nothing.
        if std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1)
            < 2
        {
            return;
        }

        let worktrees: Vec<(String, Option<PathBuf>)> = (0..6)
            .map(|i| {
                (
                    format!("feat/b{i}"),
                    Some(PathBuf::from(format!("/p/b{i}"))),
                )
            })
            .collect();
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], None, true);
        let (tx, rx) = mpsc::channel();

        let overall_running = Arc::new(AtomicUsize::new(0));
        let overall_peak = Arc::new(AtomicUsize::new(0));
        let push_running = Arc::new(AtomicUsize::new(0));
        let push_peak = Arc::new(AtomicUsize::new(0));
        let o_run = Arc::clone(&overall_running);
        let o_peak = Arc::clone(&overall_peak);
        let p_run = Arc::clone(&push_running);
        let p_peak = Arc::clone(&push_peak);

        let executor = DagExecutor::new(dag, tx).with_governor(Arc::new(StaticCapGovernor::new(1)));
        executor.run(move |task, outcomes| {
            let now = o_run.fetch_add(1, Ordering::SeqCst) + 1;
            o_peak.fetch_max(now, Ordering::SeqCst);
            if matches!(task.id, TaskId::Push(_)) {
                let now = p_run.fetch_add(1, Ordering::SeqCst) + 1;
                p_peak.fetch_max(now, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(20));
            if matches!(task.id, TaskId::Push(_)) {
                p_run.fetch_sub(1, Ordering::SeqCst);
            }
            o_run.fetch_sub(1, Ordering::SeqCst);
            (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            )
        });

        let _events: Vec<DagEvent> = rx.iter().collect();
        assert!(
            push_peak.load(Ordering::SeqCst) <= 1,
            "cap of 1 exceeded: peak {}",
            push_peak.load(Ordering::SeqCst)
        );
        assert!(
            overall_peak.load(Ordering::SeqCst) >= 2,
            "deferred pushes must not pin workers: overall peak stayed at {}",
            overall_peak.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn build_sync_batched_emits_one_barrier_push_node() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
            ("feat/b".into(), Some(PathBuf::from("/p/feat-b"))),
        ];
        let dag = SyncDag::build_sync_with_strategy(worktrees, vec![], vec![], None, true, true);
        // 1 fetch + 3 updates + 1 batch = 5 tasks (no per-branch pushes).
        assert_eq!(dag.tasks.len(), 5);
        let batch_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::PushBatch)
            .expect("one PushBatch node");
        assert!(
            !dag.tasks.iter().any(|t| matches!(t.id, TaskId::Push(_))),
            "batched mode must not emit per-branch push tasks"
        );
        assert_eq!(
            dag.tasks[batch_idx].branch_name, "",
            "empty branch_name keeps the TUI auto-create guard off"
        );
        // The barrier depends on every branch's last task (3 updates).
        assert_eq!(dag.dependencies_of(batch_idx).len(), 3);
    }

    #[test]
    fn build_sync_batched_excludes_rebase_base_and_skips_empty() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
        ];
        let dag = SyncDag::build_sync_with_strategy(
            worktrees,
            vec![],
            vec![],
            Some("master".into()),
            true,
            true,
        );
        let batch_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::PushBatch)
            .expect("PushBatch node");
        // Only feat/a's rebase feeds the barrier — the base is excluded.
        assert_eq!(dag.dependencies_of(batch_idx).len(), 1);

        // No pushable branches → no barrier node at all.
        let empty = SyncDag::build_sync_with_strategy(
            vec![("master".into(), Some(PathBuf::from("/p/master")))],
            vec![],
            vec![],
            Some("master".into()),
            true,
            true,
        );
        assert!(!empty.tasks.iter().any(|t| t.id == TaskId::PushBatch));
    }

    #[test]
    fn evicted_push_requeues_then_succeeds() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let worktrees = vec![("feat/a".into(), Some(PathBuf::from("/p/a")))];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], None, true);
        let (tx, rx) = mpsc::channel();

        let push_calls = Arc::new(AtomicUsize::new(0));
        let calls = Arc::clone(&push_calls);
        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task, outcomes| {
            if matches!(task.id, TaskId::Push(_)) {
                // First attempt dies "under memory pressure"; the retry lands.
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return (
                        TaskStatus::Evicted,
                        TaskMessage::Failed("killed under memory pressure".into()),
                        outcomes.clone(),
                    );
                }
            }
            (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            )
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        assert_eq!(push_calls.load(Ordering::SeqCst), 2, "one retry");
        let requeues: Vec<u8> = events
            .iter()
            .filter_map(|e| match e {
                DagEvent::TaskThrottled {
                    reason: ThrottleReason::Evicted { attempt },
                    ..
                } => Some(*attempt),
                _ => None,
            })
            .collect();
        assert_eq!(requeues, vec![1]);
        // The eviction itself never surfaces as a completion; the retry's
        // success does, exactly once.
        let push_completions: Vec<TaskStatus> = events
            .iter()
            .filter_map(|e| match e {
                DagEvent::TaskCompleted {
                    phase: OperationPhase::Push,
                    status,
                    ..
                } => Some(*status),
                _ => None,
            })
            .collect();
        assert_eq!(push_completions, vec![TaskStatus::Succeeded]);
        // Both attempts announced a start.
        let push_starts = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DagEvent::TaskStarted {
                        phase: OperationPhase::Push,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(push_starts, 2);
    }

    #[test]
    fn batched_push_barrier_survives_one_failed_dependency() {
        use std::sync::atomic::{AtomicBool, Ordering};

        // Batched mode: three branches, one whose update hard-fails. The single
        // PushBatch barrier must still run (carrying the two healthy branches)
        // instead of cascade-DepFailing — otherwise one failed update silently
        // drops every other branch's push (the #678 batched regression).
        let worktrees = vec![
            ("feat/a".into(), Some(PathBuf::from("/p/a"))),
            ("feat/b".into(), Some(PathBuf::from("/p/b"))),
            ("feat/c".into(), Some(PathBuf::from("/p/c"))),
        ];
        let dag = SyncDag::build_sync_with_strategy(worktrees, vec![], vec![], None, true, true);
        let (tx, rx) = mpsc::channel();

        let batch_ran = Arc::new(AtomicBool::new(false));
        let ran = Arc::clone(&batch_ran);
        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task, outcomes| match &task.id {
            TaskId::Update(b) if b == "feat/a" => (
                TaskStatus::Failed,
                TaskMessage::Failed("pull failed".into()),
                outcomes.clone(),
            ),
            TaskId::PushBatch => {
                ran.store(true, Ordering::SeqCst);
                (TaskStatus::Succeeded, TaskMessage::Pushed, outcomes.clone())
            }
            _ => (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            ),
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        assert!(
            batch_ran.load(Ordering::SeqCst),
            "PushBatch must run despite feat/a's failed update"
        );
        // The barrier completed as Succeeded (empty branch_name), never DepFailed.
        let batch_completions: Vec<TaskStatus> = events
            .iter()
            .filter_map(|e| match e {
                DagEvent::TaskCompleted {
                    phase: OperationPhase::Push,
                    status,
                    branch_name,
                    ..
                } if branch_name.is_empty() => Some(*status),
                _ => None,
            })
            .collect();
        assert_eq!(batch_completions, vec![TaskStatus::Succeeded]);
        // feat/a's failed update is still reported (row shows the failure).
        assert!(events.iter().any(|e| matches!(
            e,
            DagEvent::TaskCompleted {
                phase: OperationPhase::Update,
                status: TaskStatus::Failed,
                branch_name,
                ..
            } if branch_name == "feat/a"
        )));
    }

    #[test]
    fn eviction_retries_exhaust_into_terminal_failure() {
        let worktrees = vec![("feat/a".into(), Some(PathBuf::from("/p/a")))];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], None, true);
        let (tx, rx) = mpsc::channel();

        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task, outcomes| {
            if matches!(task.id, TaskId::Push(_)) {
                return (
                    TaskStatus::Evicted,
                    TaskMessage::Failed("killed under memory pressure".into()),
                    outcomes.clone(),
                );
            }
            (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            )
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        let requeues = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DagEvent::TaskThrottled {
                        reason: ThrottleReason::Evicted { .. },
                        ..
                    }
                )
            })
            .count();
        assert_eq!(requeues, usize::from(MAX_PUSH_RETRIES));
        let failure = events.iter().find_map(|e| match e {
            DagEvent::TaskCompleted {
                phase: OperationPhase::Push,
                status: TaskStatus::Failed,
                message,
                ..
            } => Some(message.clone()),
            _ => None,
        });
        assert_eq!(
            failure,
            Some(TaskMessage::Failed(
                "push killed under memory pressure (3 attempts)".into()
            ))
        );
    }

    /// Defers pushes until a wall-clock instant. After the last non-push
    /// task completes nothing else wakes the workers, so only the periodic
    /// admission re-check can admit the push — the run completing at all
    /// proves the `wait_timeout` path works.
    struct NotBeforeGovernor {
        admit_after: std::time::Instant,
    }

    impl DagGovernor for NotBeforeGovernor {
        fn try_admit(&self, task: &SyncTask) -> AdmitDecision {
            if !matches!(task.id, TaskId::Push(_)) {
                return AdmitDecision::Admit;
            }
            if std::time::Instant::now() >= self.admit_after {
                AdmitDecision::Admit
            } else {
                AdmitDecision::Defer(DeferReason::MemoryPressure)
            }
        }
        fn release(&self, _task: &SyncTask) {}
    }

    #[test]
    fn all_deferred_ready_queue_recovers_via_recheck() {
        let worktrees = vec![("feat/a".into(), Some(PathBuf::from("/p/a")))];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], None, true);
        let (tx, rx) = mpsc::channel();

        let hold = Duration::from_millis(250);
        let start = std::time::Instant::now();
        let executor = DagExecutor::new(dag, tx).with_governor(Arc::new(NotBeforeGovernor {
            admit_after: start + hold,
        }));
        executor.run(|_task, outcomes| {
            (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            )
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        let pushed = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    DagEvent::TaskCompleted {
                        phase: OperationPhase::Push,
                        status: TaskStatus::Succeeded,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(pushed, 1, "deferred push must eventually run");
        assert!(
            start.elapsed() >= hold,
            "push admitted before the governor allowed it"
        );
        // Workers re-scan the deferred push many times while it waits, but
        // the throttle event fires only on the state transition.
        let throttled = events
            .iter()
            .filter(|e| matches!(e, DagEvent::TaskThrottled { .. }))
            .count();
        assert_eq!(throttled, 1, "TaskThrottled must be transition-edge only");
    }

    #[test]
    fn build_sync_dag_with_push_no_rebase() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
        ];
        let gone: Vec<String> = vec![];

        let dag = SyncDag::build_sync(worktrees, vec![], gone, None, true);

        // 1 fetch + 2 updates + 2 pushes = 5 tasks
        assert_eq!(dag.tasks.len(), 5);

        // Push tasks should depend on their corresponding Update tasks
        for (i, task) in dag.tasks.iter().enumerate() {
            if let TaskId::Push(ref branch) = task.id {
                let update_idx = dag
                    .tasks
                    .iter()
                    .position(|t| t.id == TaskId::Update(branch.clone()))
                    .unwrap();
                assert!(
                    dag.dependencies_of(i).contains(&update_idx),
                    "Push({branch}) should depend on Update({branch})"
                );
            }
        }
    }

    #[test]
    fn build_sync_dag_with_push_and_rebase() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
            ("feat/b".into(), Some(PathBuf::from("/p/feat-b"))),
        ];
        let gone: Vec<String> = vec![];

        let dag = SyncDag::build_sync(worktrees, vec![], gone, Some("master".into()), true);

        // 1 fetch + 3 updates + 2 rebases + 2 pushes = 8 tasks.
        // master is the rebase base, so it gets neither a Rebase nor a Push task —
        // pushing the base branch could clobber commits landed by other devs
        // between the initial fetch and sync completion.
        assert_eq!(dag.tasks.len(), 8);

        // No Push(master) task — base branch is excluded from push.
        assert!(
            !dag.tasks
                .iter()
                .any(|t| t.id == TaskId::Push("master".into())),
            "rebase base branch must not be pushed"
        );

        // Push(feat/a) depends on Rebase(feat/a)
        let push_feat_a_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Push("feat/a".into()))
            .unwrap();
        let rebase_feat_a_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Rebase("feat/a".into()))
            .unwrap();
        assert!(
            dag.dependencies_of(push_feat_a_idx)
                .contains(&rebase_feat_a_idx),
            "Push(feat/a) should depend on Rebase(feat/a)"
        );
    }

    #[test]
    fn rebase_base_branch_excluded_from_push_even_when_only_owned_branch() {
        // Edge case: if the base branch is the only owned worktree, no push tasks
        // should be emitted at all — there is nothing else to push, and the base
        // branch itself must not be pushed.
        let worktrees = vec![("master".into(), Some(PathBuf::from("/p/master")))];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], Some("master".into()), true);

        assert!(
            !dag.tasks.iter().any(|t| matches!(t.id, TaskId::Push(_))),
            "no push tasks expected when the only owned branch is the rebase base"
        );
    }

    #[test]
    fn dag_phases_sync_with_push() {
        let worktrees = vec![("master".into(), Some(PathBuf::from("/p/master")))];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], None, true);
        let phases = dag.phases();
        assert_eq!(phases.len(), 4); // Fetch, Prune, Update, Push
        assert!(phases.contains(&OperationPhase::Push));
    }

    #[test]
    fn executor_cascades_failure() {
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
        ];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], Some("master".into()), false);
        let (tx, rx) = mpsc::channel();

        let executor = DagExecutor::new(dag, tx);
        executor.run(|task, outcomes| match &task.id {
            TaskId::Update(name) if name == "master" => (
                TaskStatus::Failed,
                TaskMessage::Failed("pull failed".into()),
                outcomes.clone(),
            ),
            _ => (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
            ),
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        let rebase_event = events.iter().find(|e| {
            matches!(
                e,
                DagEvent::TaskCompleted {
                    status: TaskStatus::DepFailed,
                    ..
                }
            )
        });
        assert!(rebase_event.is_some());
    }

    #[test]
    fn precondition_failed_is_terminal() {
        assert!(TaskStatus::PreconditionFailed.is_terminal());
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn executor_propagates_outcomes_to_dependent_tasks() {
        use std::collections::HashSet;
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
        ];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], Some("master".into()), true);
        let (tx, rx) = mpsc::channel();

        let received_outcomes: Arc<Mutex<Vec<(TaskId, HashSet<TaskOutcome>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let outcomes_clone = Arc::clone(&received_outcomes);

        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task, outcomes| {
            outcomes_clone
                .lock()
                .unwrap()
                .push((task.id.clone(), outcomes.clone()));
            match &task.id {
                TaskId::Rebase(name) if name == "feat/a" => {
                    let mut out = outcomes.clone();
                    out.insert(TaskOutcome::Conflict);
                    (TaskStatus::Succeeded, TaskMessage::Conflict, out)
                }
                TaskId::Push(name) if name == "feat/a" => {
                    if outcomes.contains(&TaskOutcome::Conflict) {
                        (
                            TaskStatus::PreconditionFailed,
                            TaskMessage::Failed("rebase conflict".into()),
                            outcomes.clone(),
                        )
                    } else {
                        (TaskStatus::Succeeded, TaskMessage::Pushed, outcomes.clone())
                    }
                }
                _ => (
                    TaskStatus::Succeeded,
                    TaskMessage::Ok("ok".into()),
                    outcomes.clone(),
                ),
            }
        });

        let _events: Vec<DagEvent> = rx.iter().collect();
        let recorded = received_outcomes.lock().unwrap();

        let push_entry = recorded
            .iter()
            .find(|(id, _)| *id == TaskId::Push("feat/a".into()));
        assert!(push_entry.is_some(), "Push(feat/a) should have been called");
        let (_, push_outcomes) = push_entry.unwrap();
        assert!(
            push_outcomes.contains(&TaskOutcome::Conflict),
            "Push(feat/a) should receive Conflict outcome from Rebase(feat/a)"
        );
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn outcomes_do_not_leak_across_branches() {
        use std::collections::HashSet;
        let worktrees = vec![
            ("master".into(), Some(PathBuf::from("/p/master"))),
            ("feat/a".into(), Some(PathBuf::from("/p/feat-a"))),
            ("feat/b".into(), Some(PathBuf::from("/p/feat-b"))),
        ];
        let dag = SyncDag::build_sync(worktrees, vec![], vec![], Some("master".into()), true);
        let (tx, rx) = mpsc::channel();

        let received_outcomes: Arc<Mutex<Vec<(TaskId, HashSet<TaskOutcome>)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let outcomes_clone = Arc::clone(&received_outcomes);

        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task, outcomes| {
            outcomes_clone
                .lock()
                .unwrap()
                .push((task.id.clone(), outcomes.clone()));
            match &task.id {
                TaskId::Rebase(name) if name == "feat/a" => {
                    let mut out = outcomes.clone();
                    out.insert(TaskOutcome::Conflict);
                    (TaskStatus::Succeeded, TaskMessage::Conflict, out)
                }
                TaskId::Rebase(name) if name == "feat/b" => (
                    TaskStatus::Succeeded,
                    TaskMessage::Ok("rebased".into()),
                    outcomes.clone(),
                ),
                _ => (
                    TaskStatus::Succeeded,
                    TaskMessage::Ok("ok".into()),
                    outcomes.clone(),
                ),
            }
        });

        let _events: Vec<DagEvent> = rx.iter().collect();
        let recorded = received_outcomes.lock().unwrap();

        let push_a = recorded
            .iter()
            .find(|(id, _)| *id == TaskId::Push("feat/a".into()))
            .unwrap();
        assert!(push_a.1.contains(&TaskOutcome::Conflict));

        let push_b = recorded
            .iter()
            .find(|(id, _)| *id == TaskId::Push("feat/b".into()))
            .unwrap();
        assert!(
            !push_b.1.contains(&TaskOutcome::Conflict),
            "feat/b's push should not see feat/a's Conflict outcome"
        );
    }
}

/// Tracks which `PatchSource` last wrote each (branch, field) pair.
/// Used by `LiveTable` to suppress patches arriving from a lower-priority
/// source after a higher-priority source has already filled a field.
#[derive(Debug, Default)]
pub struct PatchSourceLog {
    last_writer: HashMap<String, Vec<(FieldSet, PatchSource)>>,
}

impl PatchSourceLog {
    /// Returns `true` if `source` is allowed to write `fields` for `branch`.
    /// Updates internal state to record the new write.
    pub fn try_admit(&mut self, branch: &str, fields: FieldSet, source: PatchSource) -> bool {
        let entries = self.last_writer.entry(branch.to_string()).or_default();
        // If any existing entry overlaps with `fields` and has a strictly
        // higher priority, reject.
        for (existing_fields, existing_source) in entries.iter() {
            if existing_fields.intersects(fields) && existing_source.priority() > source.priority()
            {
                return false;
            }
        }
        // Admit. Record (fields, source); we don't bother garbage-collecting
        // overlapping entries — `intersects` checks above are O(entries) and
        // the entry count per branch is bounded by the number of patch
        // clusters (~11).
        entries.push((fields, source));
        true
    }
}

#[cfg(test)]
mod patch_source_log_tests {
    use super::*;

    #[test]
    fn collector_then_post_fetch_admits_post_fetch() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::Collector));
        assert!(log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::PostFetch));
    }

    #[test]
    fn post_fetch_then_collector_rejects_collector() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::PostFetch));
        assert!(!log.try_admit("a", FieldSet::REMOTE_AHEAD_BEHIND, PatchSource::Collector));
    }

    #[test]
    fn disjoint_field_sets_do_not_block_each_other() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::SIZE, PatchSource::PostFetch));
        // Different field — Collector still allowed.
        assert!(log.try_admit("a", FieldSet::CHANGES, PatchSource::Collector));
    }

    #[test]
    fn different_branches_are_independent() {
        let mut log = PatchSourceLog::default();
        assert!(log.try_admit("a", FieldSet::SIZE, PatchSource::PostFetch));
        assert!(log.try_admit("b", FieldSet::SIZE, PatchSource::Collector));
    }
}
