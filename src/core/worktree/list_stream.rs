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
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
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
}

pub struct CollectorRequest {
    pub targets: Vec<CollectorTarget>,
    pub fields: FieldSet,
    pub stat: Stat,
    pub source: PatchSource,
    pub ctx: Arc<CollectorContext>,
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

    /// Wait for all workers to finish. Emits
    /// `DagEvent::WorktreeInfoCollectionDone` if and only if the spawning
    /// run was `source=Collector`.
    pub fn join(mut self) {
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        if let Some((tx, source)) = self.sentinel.take() {
            if matches!(source, PatchSource::Collector) {
                let _ = tx.send(DagEvent::WorktreeInfoCollectionDone);
            }
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
    } = req;

    let mut handles = Vec::with_capacity(targets.len());
    for target in targets {
        let tx = tx.clone();
        let ctx = Arc::clone(&ctx);
        let cancel = Arc::clone(&cancel);
        let source = source.clone();
        handles.push(thread::spawn(move || {
            run_worker(target, fields, stat, source, ctx, cancel, tx);
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
    _fields: FieldSet,
    _stat: Stat,
    _source: PatchSource,
    _ctx: Arc<CollectorContext>,
    _cancel: Arc<AtomicBool>,
    _tx: mpsc::Sender<DagEvent>,
) {
    // Cluster calls land in Task 7. For now: no-op.
    let _ = target;
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
        });
        let handle = spawn(
            CollectorRequest {
                targets: vec![],
                fields: FieldSet::ALL,
                stat: Stat::Summary,
                source: PatchSource::Collector,
                ctx,
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
        });
        let handle = spawn(
            CollectorRequest {
                targets: vec![],
                fields: FieldSet::REMOTE_DERIVED,
                stat: Stat::Summary,
                source: PatchSource::PostFetch,
                ctx,
            },
            tx,
        );
        handle.join();

        let events: Vec<DagEvent> = rx.iter().collect();
        assert_eq!(events.len(), 0);
    }
}
