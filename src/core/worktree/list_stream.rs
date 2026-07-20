//! Streaming collector for `WorktreeInfo` cells.
//!
//! Spawns one worker thread per branch, each running cluster calls in a
//! fixed cheap-first order and emitting `DagEvent::WorktreeInfoUpdated`
//! patches into a shared channel. Cancellation is cooperative between
//! cluster calls. Re-runnable: callers invoke `spawn` again with a
//! narrower `FieldSet` and a different `PatchSource` to drive post-fetch
//! and post-task refreshes.

use crate::core::{
    ownership::OwnershipStrategy,
    worktree::{
        info_field::FieldSet,
        list::EntryKind,
        sync_dag::{DagEvent, PatchSource},
    },
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

use super::list::Stat;

#[derive(Debug, Clone)]
pub struct CollectorTarget {
    /// Branch name. `""` for detached (sandbox) entries.
    pub branch_name: String,
    pub path: Option<PathBuf>,
    pub kind: EntryKind,
    pub is_detached: bool,
}

pub struct CollectorContext {
    /// Whether worker threads should construct their `GitCommand` with
    /// gitoxide enabled. `GitCommand` itself is not `Sync` (it holds a
    /// `OnceLock<gix::ThreadSafeRepository>` whose internals contain
    /// non-thread-safe `Rc`s), so each worker constructs its own.
    pub use_gitoxide: bool,
    pub base_branch: String,
    pub remote_name: String,
    pub ownership_strategy: OwnershipStrategy,
    pub user_email: Option<String>,
    /// Resolved `git --git-common-dir`. Used as the root for the on-disk
    /// SHA-keyed cache that backs the slow `cached_*` cluster wrappers.
    pub git_common_dir: PathBuf,
}

pub struct CollectorRequest {
    pub targets: Vec<CollectorTarget>,
    pub fields: FieldSet,
    pub stat: Stat,
    pub source: PatchSource,
    pub ctx: Arc<CollectorContext>,
    /// Concurrency budget for the decoupled size coordinator, resolved by the
    /// caller from `DAFT_SIZE_WALK_JOBS` / `daft.list.sizeConcurrency` /
    /// `available_parallelism` via [`crate::core::size_walk::resolve_jobs`].
    pub size_jobs: usize,
}

pub struct CollectorHandle {
    cancel: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    /// Collector-only sentinel sender (kept alive by handle so the
    /// completion event fires only after all workers have observably
    /// joined or cancelled).
    sentinel: Option<(mpsc::Sender<DagEvent>, PatchSource)>,
}

impl CollectorHandle {
    /// Request cooperative cancellation. Workers exit between cluster calls.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Returns a clone of the cancel flag so external code (e.g. the
    /// renderer's Ctrl-C handler) can flip it. The collector workers
    /// observe the flag between cluster calls.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Wait for all workers to finish. Emits
    /// `DagEvent::WorktreeInfoCollectionDone` if and only if the spawning
    /// run was `source=Collector`.
    pub fn join(self) {
        self.join_after(|| {});
    }

    /// Like [`Self::join`], with a bounded barrier between the workers
    /// joining and the completion sentinel firing. `daft list` uses it to
    /// hold the live table's "collection in progress" state briefly while
    /// the detached forge refresh concludes, so fresh PR statuses land in
    /// the final frame instead of on the next run. The barrier must bound
    /// its own wait — the sentinel is what lets the renderer exit.
    pub fn join_after(mut self, barrier: impl FnOnce()) {
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        barrier();
        if let Some((tx, source)) = self.sentinel.take()
            && matches!(source, PatchSource::Collector)
        {
            let _ = tx.send(DagEvent::WorktreeInfoCollectionDone);
        }
    }
}

/// Spawn workers for the request. Workers stream patches into `tx`.
/// The caller MUST call `CollectorHandle::join` (or drop the handle, which
/// silently joins) for the completion sentinel to fire.
pub fn spawn(req: CollectorRequest, tx: mpsc::Sender<DagEvent>) -> CollectorHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let CollectorRequest {
        targets,
        fields,
        stat,
        source,
        ctx,
        size_jobs,
    } = req;

    // Size is decoupled from the per-worktree git-cluster workers into a single
    // coordinator (below): the walk runs through the shared bounded budget in
    // `core::size_walk` so one deep worktree parallelises internally while N
    // worktrees can't each spin up a full pool and oversubscribe the disk.
    // Capture (branch_name, path) now, before `targets` is consumed by the
    // git-cluster loop. Path-less targets (detached / branch-only) never got a
    // Size patch before and still don't.
    let size_targets: Vec<(String, PathBuf)> = if fields.contains(FieldSet::SIZE) {
        targets
            .iter()
            .filter_map(|t| t.path.clone().map(|p| (t.branch_name.clone(), p)))
            .collect()
    } else {
        Vec::new()
    };

    let mut handles = Vec::with_capacity(targets.len() + 1);
    for target in targets {
        let tx = tx.clone();
        let ctx = Arc::clone(&ctx);
        let cancel = Arc::clone(&cancel);
        let source = source.clone();
        handles.push(thread::spawn(move || {
            run_worker(target, fields, stat, source, ctx, cancel, tx);
        }));
    }

    // Size coordinator: one extra worker in `handles`, so `CollectorHandle::join`
    // waits for it and fires the completion sentinel only after sizes finish —
    // no change to join()/cancel accounting. Streams per-target `Size` patches
    // routed by `branch_name`, exactly like the old inline SIZE cluster did.
    if !size_targets.is_empty() {
        let tx = tx.clone();
        let cancel = Arc::clone(&cancel);
        let source = source.clone();
        handles.push(thread::spawn(move || {
            let (branch_names, paths): (Vec<String>, Vec<PathBuf>) =
                size_targets.into_iter().unzip();
            crate::core::size_walk::walk_streaming(
                &paths,
                Some(&cancel),
                size_jobs,
                |idx, size| {
                    let _ = tx.send(DagEvent::WorktreeInfoUpdated {
                        branch_name: branch_names[idx].clone(),
                        patch: crate::core::worktree::sync_dag::WorktreeInfoPatch::Size(size),
                        source: source.clone(),
                    });
                },
            );
        }));
    }

    CollectorHandle {
        cancel,
        handles,
        sentinel: Some((tx, source)),
    }
}

fn run_worker(
    target: CollectorTarget,
    fields: FieldSet,
    stat: Stat,
    source: PatchSource,
    ctx: Arc<CollectorContext>,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<DagEvent>,
) {
    use crate::core::ownership;
    use crate::core::worktree::list::{
        count_changed_files, count_changed_lines, get_ahead_behind, get_base_line_counts,
        get_branch_creation_timestamp, get_commit_metadata, get_forge_branch_ref,
        get_remote_line_counts, get_upstream_ahead_behind, max_mtime_of_files,
    };
    use crate::core::worktree::sync_dag::WorktreeInfoPatch as P;
    use crate::git::GitCommand;

    // Workers construct their own GitCommand: gix::ThreadSafeRepository is
    // !Sync, so wrapping GitCommand in Arc<CollectorContext> would block the
    // closure's Send bound. ctx.use_gitoxide carries the choice through.
    let git = GitCommand::new(true).with_gitoxide(ctx.use_gitoxide);

    macro_rules! emit {
        ($patch:expr) => {{
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = tx.send(DagEvent::WorktreeInfoUpdated {
                branch_name: target.branch_name.clone(),
                patch: $patch,
                source: source.clone(), // PatchSource is Clone, not Copy.
            });
        }};
    }

    let path = target.path.as_deref();

    // 1. BASE_AHEAD_BEHIND (skip detached) — content-addressed cache by
    //    (base_sha, head_sha). Falls through to compute on key-resolution
    //    failure; the wrapper itself skips writing on a None compute result.
    if fields.contains(FieldSet::BASE_AHEAD_BEHIND)
        && !target.is_detached
        && let Some(p) = path
    {
        let base_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, &ctx.base_branch);
        let head_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, "HEAD");
        let v = match (base_sha, head_sha) {
            (Some(b), Some(h)) => crate::core::worktree::cell_cache::cached_base_ahead_behind(
                &ctx.git_common_dir,
                &b,
                &h,
                || get_ahead_behind(&ctx.base_branch, &target.branch_name, p),
            ),
            _ => get_ahead_behind(&ctx.base_branch, &target.branch_name, p),
        };
        emit!(P::BaseAheadBehind(v));
    }

    // 2. CHANGES
    if fields.contains(FieldSet::CHANGES)
        && let Some(p) = path
    {
        let c = count_changed_files(p);
        emit!(P::Changes {
            staged: c.staged,
            unstaged: c.unstaged,
            untracked: c.untracked,
            conflicted: c.conflicted
        });
    }

    // 3. LAST_COMMIT — content-addressed cache by HEAD sha.
    if fields.contains(FieldSet::LAST_COMMIT)
        && let Some(p) = path
    {
        let head_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, "HEAD");
        let (timestamp, hash, subject) = match head_sha {
            Some(h) => crate::core::worktree::cell_cache::cached_last_commit(
                &ctx.git_common_dir,
                &h,
                || get_commit_metadata(p, &git),
            ),
            None => get_commit_metadata(p, &git),
        };
        emit!(P::LastCommit {
            timestamp,
            hash,
            subject
        });
    }

    // 4. BRANCH_AGE (skip detached)
    if fields.contains(FieldSet::BRANCH_AGE)
        && !target.is_detached
        && let Some(p) = path
    {
        let v = get_branch_creation_timestamp(&target.branch_name, p);
        emit!(P::BranchAge(v));
    }

    // 5. OWNER (skip detached)
    if fields.contains(FieldSet::OWNER)
        && !target.is_detached
        && let Some(p) = path
    {
        let owner = ownership::resolve_owner_with_fallbacks(
            &ctx.base_branch,
            &target.branch_name,
            p,
            ctx.ownership_strategy,
            ctx.user_email.as_deref(),
            Some(&ctx.remote_name),
        );
        emit!(P::Owner(owner));
    }

    // 5b. FORGE_REF (skip detached) — a cheap local `branch.<name>.merge` read.
    if fields.contains(FieldSet::FORGE_REF)
        && !target.is_detached
        && let Some(p) = path
    {
        emit!(P::ForgeRef(get_forge_branch_ref(&target.branch_name, p)));
    }

    // 6. REMOTE_AHEAD_BEHIND (skip detached) — content-addressed cache by
    //    (head_sha, upstream_sha). The upstream refspec uses the
    //    `<branch>@{upstream}` form so git resolves the configured upstream.
    if fields.contains(FieldSet::REMOTE_AHEAD_BEHIND)
        && !target.is_detached
        && let Some(p) = path
    {
        let head_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, "HEAD");
        let upstream_sha = crate::core::worktree::cell_cache::resolve_ref_sha(
            p,
            &format!("{}@{{upstream}}", target.branch_name),
        );
        let v = match (head_sha, upstream_sha) {
            (Some(h), Some(u)) => crate::core::worktree::cell_cache::cached_remote_ahead_behind(
                &ctx.git_common_dir,
                &h,
                &u,
                || get_upstream_ahead_behind(&target.branch_name, p),
            ),
            _ => get_upstream_ahead_behind(&target.branch_name, p),
        };
        emit!(P::RemoteAheadBehind(v));
    }

    // 7. Stat::Lines clusters
    if matches!(stat, Stat::Lines) {
        // BASE_LINES — content-addressed cache by (base_sha, head_sha).
        if fields.contains(FieldSet::BASE_LINES)
            && !target.is_detached
            && let Some(p) = path
        {
            let base_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, &ctx.base_branch);
            let head_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, "HEAD");
            let v = match (base_sha, head_sha) {
                (Some(b), Some(h)) => crate::core::worktree::cell_cache::cached_base_lines(
                    &ctx.git_common_dir,
                    &b,
                    &h,
                    || get_base_line_counts(&ctx.base_branch, &target.branch_name, p),
                ),
                _ => get_base_line_counts(&ctx.base_branch, &target.branch_name, p),
            };
            emit!(P::BaseLines(v));
        }
        if fields.contains(FieldSet::CHANGES_LINES)
            && let Some(p) = path
        {
            let (s, u) = count_changed_lines(p);
            emit!(P::ChangesLines {
                staged: s,
                unstaged: u
            });
        }
        // REMOTE_LINES — content-addressed cache by (head_sha, upstream_sha).
        if fields.contains(FieldSet::REMOTE_LINES)
            && !target.is_detached
            && let Some(p) = path
        {
            let head_sha = crate::core::worktree::cell_cache::resolve_ref_sha(p, "HEAD");
            let upstream_sha = crate::core::worktree::cell_cache::resolve_ref_sha(
                p,
                &format!("{}@{{upstream}}", target.branch_name),
            );
            let v = match (head_sha, upstream_sha) {
                (Some(h), Some(u)) => crate::core::worktree::cell_cache::cached_remote_lines(
                    &ctx.git_common_dir,
                    &h,
                    &u,
                    || get_remote_line_counts(&target.branch_name, p),
                ),
                _ => get_remote_line_counts(&target.branch_name, p),
            };
            emit!(P::RemoteLines(v));
        }
    }

    // MTIME (SIZE is decoupled into its own bounded coordinator — see `spawn`)
    if fields.contains(FieldSet::MTIME)
        && let Some(p) = path
    {
        // Re-count just to get the path list — cheap relative to mtime walk.
        let c = count_changed_files(p);
        if !c.paths.is_empty() {
            emit!(P::Mtime(max_mtime_of_files(p, &c.paths)));
        } else {
            emit!(P::Mtime(None));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_request_emits_only_completion_sentinel() {
        let (tx, rx) = mpsc::channel();
        let ctx = Arc::new(CollectorContext {
            use_gitoxide: false,
            base_branch: "master".into(),
            remote_name: "origin".into(),
            ownership_strategy: OwnershipStrategy::RecencyPlurality,
            user_email: None,
            git_common_dir: PathBuf::new(),
        });
        let handle = spawn(
            CollectorRequest {
                targets: vec![],
                fields: FieldSet::ALL,
                stat: Stat::Summary,
                source: PatchSource::Collector,
                ctx,
                size_jobs: 2,
            },
            tx,
        );
        handle.join();

        let events: Vec<DagEvent> = rx.iter().collect();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DagEvent::WorktreeInfoCollectionDone));
    }

    #[test]
    fn post_fetch_run_does_not_emit_completion_sentinel() {
        let (tx, rx) = mpsc::channel();
        let ctx = Arc::new(CollectorContext {
            use_gitoxide: false,
            base_branch: "master".into(),
            remote_name: "origin".into(),
            ownership_strategy: OwnershipStrategy::RecencyPlurality,
            user_email: None,
            git_common_dir: PathBuf::new(),
        });
        let handle = spawn(
            CollectorRequest {
                targets: vec![],
                fields: FieldSet::REMOTE_DERIVED,
                stat: Stat::Summary,
                source: PatchSource::PostFetch,
                ctx,
                size_jobs: 2,
            },
            tx,
        );
        handle.join();

        let events: Vec<DagEvent> = rx.iter().collect();
        assert_eq!(events.len(), 0);
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use std::process::{Command, Stdio};
    use tempfile::TempDir;

    fn init_temp_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        let p = dir.path();
        Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(p)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        // LOCAL config only — never use --global per CLAUDE.md.
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(p)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(p)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        std::fs::write(p.join("README"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(p)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(p)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .args(["branch", "-M", "master"])
            .current_dir(p)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        dir
    }

    #[test]
    fn collector_emits_changes_and_last_commit_for_a_real_repo() {
        let dir = init_temp_repo();
        let (tx, rx) = mpsc::channel();
        let ctx = Arc::new(CollectorContext {
            use_gitoxide: false,
            base_branch: "master".into(),
            remote_name: "origin".into(),
            ownership_strategy: OwnershipStrategy::RecencyPlurality,
            user_email: Some("test@test.com".into()),
            git_common_dir: dir.path().join(".git"),
        });
        let target = CollectorTarget {
            branch_name: "master".into(),
            path: Some(dir.path().to_path_buf()),
            kind: EntryKind::Worktree,
            is_detached: false,
        };
        let fields = FieldSet::CHANGES | FieldSet::LAST_COMMIT;
        let handle = spawn(
            CollectorRequest {
                targets: vec![target],
                fields,
                stat: Stat::Summary,
                source: PatchSource::Collector,
                ctx,
                size_jobs: 2,
            },
            tx,
        );
        handle.join();

        let events: Vec<DagEvent> = rx.iter().collect();
        let patches: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                DagEvent::WorktreeInfoUpdated { patch, .. } => Some(patch),
                _ => None,
            })
            .collect();

        assert!(patches.iter().any(|p| matches!(
            p,
            crate::core::worktree::sync_dag::WorktreeInfoPatch::Changes { .. }
        )));
        assert!(patches.iter().any(|p| matches!(
            p,
            crate::core::worktree::sync_dag::WorktreeInfoPatch::LastCommit { .. }
        )));
        // Did NOT request SIZE — must not appear.
        assert!(!patches.iter().any(|p| matches!(
            p,
            crate::core::worktree::sync_dag::WorktreeInfoPatch::Size(_)
        )));
        assert!(matches!(
            events.last(),
            Some(DagEvent::WorktreeInfoCollectionDone)
        ));

        // The LAST_COMMIT cluster runs through the cached_last_commit
        // wrapper, which should have written one entry under the cache
        // root we passed (`<git_common_dir>/.daft/cache/last-commit/`).
        let cache_dir = dir.path().join(".git").join(".daft").join("cache");
        let last_commit_dir = cache_dir.join("last-commit");
        let entries: Vec<_> = std::fs::read_dir(&last_commit_dir)
            .unwrap_or_else(|e| panic!("cache dir {} should exist: {e}", last_commit_dir.display()))
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            !entries.is_empty(),
            "expected cached last-commit entry under {}",
            last_commit_dir.display()
        );
    }
}
