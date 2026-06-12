//! Seed provenance for untracked visitor daft files.
//!
//! A *seed* is the exact content daft last wrote INTO a worktree's
//! untracked `daft.yml` / `daft.local.yml` — at worktree creation
//! (propagation), starter installation, or after a consolidation refreshed
//! the copy. Seeds let lifecycle commands answer the question the old
//! two-way divergence check could not: *did this worktree's copy change
//! since daft put it there?* A byte-identical copy is **pristine** and can
//! be deleted with the worktree; an edited copy is **refined** user data.
//!
//! Invariant: a seed records content that flowed INTO a worktree from
//! elsewhere — never content authored in it. Consolidations therefore
//! refresh the SOURCE worktree's seed (its refinements now live in the
//! target too) and never the target's: the target's merged content exists
//! nowhere else, and marking it pristine would make the only copy silently
//! removable.
//!
//! Every operation here is best-effort: the store lives under the daft
//! state dir, and a missing/locked/newer-schema store must never block or
//! fail a worktree operation. Failures degrade to "no seed recorded",
//! which downstream classification treats as refined (protective).

use std::path::Path;

use crate::coordinator::adapters::SqliteJobsStore;
use crate::coordinator::ports::SeedsStorePort;
use crate::store::models::VisitorSeedRow;

/// Handle to the per-repo seed store. Construction is fallible-by-design:
/// `None` means "operate without provenance" (NoSeed semantics), never an
/// error the caller has to handle.
pub struct SeedsContext {
    repo_hash: String,
    store: Box<dyn SeedsStorePort>,
}

impl SeedsContext {
    /// Open the seed store for the repo whose git common dir is
    /// `git_common_dir`. Returns `None` (with a debug log) on any failure:
    /// unreadable/uncreatable `daft-id`, state dir problems, schema newer
    /// than this binary, permissions.
    pub fn open(git_common_dir: &Path) -> Option<Self> {
        let repo_hash =
            match crate::core::repo_identity::compute_repo_id_from_common_dir(git_common_dir) {
                Ok(id) => id,
                Err(e) => {
                    crate::log_debug!("visitor seeds unavailable (repo identity): {e:#}");
                    return None;
                }
            };
        let db_path = match crate::store::paths::for_repo(&repo_hash) {
            Ok(p) => p,
            Err(e) => {
                crate::log_debug!("visitor seeds unavailable (store path): {e}");
                return None;
            }
        };
        Self::open_with_db_path(repo_hash, &db_path)
    }

    /// Test variant: resolve the store under an explicit state base instead
    /// of `daft_state_dir()`. Mirrors [`Self::open`] otherwise.
    pub fn open_in(git_common_dir: &Path, state_base: &Path) -> Option<Self> {
        let repo_hash =
            crate::core::repo_identity::compute_repo_id_from_common_dir(git_common_dir).ok()?;
        let db_path = crate::store::paths::for_repo_under(state_base, &repo_hash).ok()?;
        Self::open_with_db_path(repo_hash, &db_path)
    }

    fn open_with_db_path(repo_hash: String, db_path: &Path) -> Option<Self> {
        let base = db_path.parent()?;
        match SqliteJobsStore::for_repo_base(base) {
            Ok(store) => Some(Self {
                repo_hash,
                store: Box::new(store),
            }),
            Err(e) => {
                crate::log_debug!("visitor seeds unavailable (store open): {e:#}");
                None
            }
        }
    }

    /// Test-only: inject a mock port.
    #[cfg(test)]
    pub fn for_test(repo_hash: String, store: Box<dyn SeedsStorePort>) -> Self {
        Self { repo_hash, store }
    }

    /// Record the current on-disk bytes of `worktree/<filename>` as the
    /// branch's seed for that file. Reads the file post-write so the seed
    /// is exactly what daft left on disk. Best-effort.
    pub fn record_seed_file(&self, branch_slug: &str, worktree: &Path, filename: &str) {
        let path = worktree.join(filename);
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                crate::log_debug!("seed not recorded for {}: {e}", path.display());
                return;
            }
        };
        self.record_seed_content(branch_slug, filename, &content);
    }

    /// Record several files in one call (the propagation result shape).
    pub fn record_seeds(&self, branch_slug: &str, worktree: &Path, filenames: &[String]) {
        for filename in filenames {
            self.record_seed_file(branch_slug, worktree, filename);
        }
    }

    /// Record explicit content as the seed (consolidation paths that already
    /// hold the bytes). Best-effort.
    pub fn record_seed_content(&self, branch_slug: &str, filename: &str, content: &str) {
        if let Err(e) = self
            .store
            .record_seed(&self.repo_hash, branch_slug, filename, content)
        {
            crate::log_debug!("seed not recorded for {branch_slug}/{filename}: {e:#}");
        }
    }

    /// Fetch the seed row for one file. Returns `None` both for "never
    /// seeded" and for store read errors (logged at debug level) — callers
    /// cannot and should not distinguish the two.
    pub fn get_seed(&self, branch_slug: &str, filename: &str) -> Option<VisitorSeedRow> {
        match self.store.get_seed(&self.repo_hash, branch_slug, filename) {
            Ok(row) => row,
            Err(e) => {
                crate::log_debug!("seed read failed for {branch_slug}/{filename}: {e:#}");
                None
            }
        }
    }

    /// Drop one file's seed (e.g. the consolidation deleted the source
    /// file). Best-effort.
    pub fn delete_seed(&self, branch_slug: &str, filename: &str) {
        if let Err(e) = self
            .store
            .delete_seed(&self.repo_hash, branch_slug, filename)
        {
            crate::log_debug!("seed delete failed for {branch_slug}/{filename}: {e:#}");
        }
    }

    /// Drop every seed for a branch — call after its worktree/branch is
    /// removed. Best-effort.
    pub fn delete_seeds_for_branch(&self, branch_slug: &str) {
        if let Err(e) = self
            .store
            .delete_seeds_for_branch(&self.repo_hash, branch_slug)
        {
            crate::log_debug!("seed cleanup failed for {branch_slug}: {e:#}");
        }
    }

    /// Every seed recorded for this repo (debug/audit surface).
    pub fn list_seeds(&self) -> Vec<VisitorSeedRow> {
        match self.store.list_seeds_for_repo(&self.repo_hash) {
            Ok(rows) => rows,
            Err(e) => {
                crate::log_debug!("seed list failed: {e:#}");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn open_in_round_trips_through_a_real_store() {
        let common = tempdir().unwrap();
        let state = tempdir().unwrap();
        let wt = tempdir().unwrap();
        fs::write(wt.path().join("daft.yml"), "hooks: {}\n").unwrap();

        let ctx = SeedsContext::open_in(common.path(), state.path())
            .expect("store opens under injected state base");
        ctx.record_seed_file("feat/x", wt.path(), "daft.yml");

        let seed = ctx.get_seed("feat/x", "daft.yml").expect("seed recorded");
        assert_eq!(seed.content, "hooks: {}\n");

        // Same identity on reopen: daft-id is stable.
        let ctx2 = SeedsContext::open_in(common.path(), state.path()).unwrap();
        assert!(ctx2.get_seed("feat/x", "daft.yml").is_some());

        ctx2.delete_seeds_for_branch("feat/x");
        assert!(ctx2.get_seed("feat/x", "daft.yml").is_none());
    }

    #[test]
    fn record_seed_file_skips_missing_file() {
        let common = tempdir().unwrap();
        let state = tempdir().unwrap();
        let wt = tempdir().unwrap();

        let ctx = SeedsContext::open_in(common.path(), state.path()).unwrap();
        ctx.record_seed_file("feat/x", wt.path(), "daft.yml");
        assert!(ctx.get_seed("feat/x", "daft.yml").is_none());
    }

    struct FailingPort;
    impl SeedsStorePort for FailingPort {
        fn record_seed(&self, _: &str, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
            anyhow::bail!("disk on fire")
        }
        fn get_seed(&self, _: &str, _: &str, _: &str) -> anyhow::Result<Option<VisitorSeedRow>> {
            anyhow::bail!("disk on fire")
        }
        fn delete_seed(&self, _: &str, _: &str, _: &str) -> anyhow::Result<()> {
            anyhow::bail!("disk on fire")
        }
        fn delete_seeds_for_branch(&self, _: &str, _: &str) -> anyhow::Result<usize> {
            anyhow::bail!("disk on fire")
        }
        fn list_seeds_for_repo(&self, _: &str) -> anyhow::Result<Vec<VisitorSeedRow>> {
            anyhow::bail!("disk on fire")
        }
    }

    #[test]
    fn store_errors_degrade_to_none_and_never_panic() {
        let wt = tempdir().unwrap();
        fs::write(wt.path().join("daft.yml"), "hooks: {}\n").unwrap();
        let ctx = SeedsContext::for_test("repo".into(), Box::new(FailingPort));

        ctx.record_seed_file("feat/x", wt.path(), "daft.yml");
        assert!(ctx.get_seed("feat/x", "daft.yml").is_none());
        ctx.delete_seed("feat/x", "daft.yml");
        ctx.delete_seeds_for_branch("feat/x");
        assert!(ctx.list_seeds().is_empty());
    }
}
