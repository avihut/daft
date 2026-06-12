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

// ── Classification ──────────────────────────────────────────────────────────

/// Provenance state of one in-scope file in a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedClass {
    /// Byte-identical to the recorded seed: daft put it there and nobody
    /// touched it. Deleting the worktree loses nothing.
    Pristine,
    /// Edited since it was seeded: genuine user data.
    Refined,
    /// No seed recorded (pre-provenance worktree, hand-authored file, store
    /// unavailable). Treated like Refined: protective.
    NoSeed,
}

/// Classification of one in-scope file, plus the two-way subsumption check
/// against the consolidation target.
#[derive(Debug, Clone)]
pub struct FileClass {
    /// `daft.yml` or `daft.local.yml`.
    pub filename: String,
    pub class: SeedClass,
    /// True when the target's copy already represents everything this file
    /// contains (`merge(target, source) == target`) — removal loses nothing
    /// even for a Refined/NoSeed copy (e.g. it was already consolidated).
    /// Always false when there is no target worktree to compare against.
    pub subsumed_by_target: bool,
}

impl FileClass {
    /// May this file's worktree copy be deleted without losing config?
    pub fn removable(&self) -> bool {
        matches!(self.class, SeedClass::Pristine) || self.subsumed_by_target
    }
}

/// Classify every in-scope untracked daft file in `source_wt`.
///
/// In scope: `daft.yml` when it exists and classifies as a visitor
/// (untracked) file; `daft.local.yml` whenever it exists. A file absent
/// from disk is simply not listed — nothing on disk means nothing to lose.
///
/// `seeds: None` (store unavailable) classifies everything as NoSeed.
/// `target_wt: None` (no worktree for the merge target) disables the
/// subsumption fallback.
pub fn classify_in_scope_files(
    seeds: Option<&SeedsContext>,
    branch_slug: &str,
    source_wt: &Path,
    target_wt: Option<&Path>,
) -> Vec<FileClass> {
    use crate::hooks::visitor_propagation::{
        VISITOR_DAFT_LOCAL_YML, VISITOR_DAFT_YML, file_has_divergence,
    };
    use crate::hooks::yaml_config_loader::{ConfigStatus, classify_main_config};

    let mut classes = Vec::new();

    for filename in [VISITOR_DAFT_YML, VISITOR_DAFT_LOCAL_YML] {
        let path = source_wt.join(filename);
        if !path.is_file() {
            continue;
        }
        if filename == VISITOR_DAFT_YML
            && !matches!(classify_main_config(source_wt), ConfigStatus::Visitor)
        {
            // Tracked daft.yml travels with git; not in scope.
            continue;
        }

        let class = match seeds {
            Some(ctx) => match (ctx.get_seed(branch_slug, filename), std::fs::read(&path)) {
                (Some(seed), Ok(bytes)) if seed.content.as_bytes() == bytes.as_slice() => {
                    SeedClass::Pristine
                }
                (Some(_), Ok(_)) => SeedClass::Refined,
                // Unreadable file or no seed row: protective.
                _ => SeedClass::NoSeed,
            },
            None => SeedClass::NoSeed,
        };

        // The subsumption check is the semantic fallback that lets refined
        // and NoSeed copies still be removable when the target already has
        // everything (e.g. they were consolidated earlier). An error in the
        // check counts as divergent — protective.
        let subsumed_by_target = match target_wt {
            Some(target) => !file_has_divergence(source_wt, target, filename).unwrap_or(true),
            None => false,
        };

        classes.push(FileClass {
            filename: filename.to_string(),
            class,
            subsumed_by_target,
        });
    }

    classes
}

/// True when every in-scope file may be deleted silently.
pub fn all_removable(classes: &[FileClass]) -> bool {
    classes.iter().all(FileClass::removable)
}

/// The files that block a silent removal (refined/no-seed and not subsumed).
pub fn blocking_files(classes: &[FileClass]) -> Vec<&FileClass> {
    classes.iter().filter(|c| !c.removable()).collect()
}

// ── Consolidation preview ───────────────────────────────────────────────────

/// Dry-run of consolidating one in-scope file into a target worktree: what
/// keys move, what conflicts, and the resolved content (or both
/// side-resolutions when a conflict needs an explicit choice). Shared by the
/// removal flow (`branch_delete`) and the merge flow.
pub struct ConsolidationPreview {
    pub filename: String,
    /// Key paths a three-way merge adopts from the source.
    pub adopt_keys: Vec<String>,
    /// Key paths both sides changed — a side must be chosen.
    pub conflict_keys: Vec<String>,
    /// True when there is no usable seed base: per-key reasoning is
    /// impossible and the choice is whole-file.
    pub whole_file: bool,
    pub resolution: PreviewResolution,
}

pub enum PreviewResolution {
    /// Unambiguous: write this content into the target.
    Resolved(String),
    /// Conflicted: the caller must pick a side (prompt or abort).
    NeedsSide {
        /// Conflicted keys keep the target's values.
        target_priority: String,
        /// Conflicted keys take the source's values.
        source_priority: String,
    },
}

/// Compute what consolidating `class`'s file from `source_wt` into
/// `target_wt` would write.
///
/// With a seed: a real three-way merge (`merge3`) — adopted keys and
/// conflicts reported per key path. Without a usable base (NoSeed,
/// unparseable YAML): whole-file mode — when the target has no such file the
/// source is copied verbatim (lossless); when it does, the situation is an
/// unresolvable whole-file conflict (`NeedsSide`) where the source-priority
/// resolution is the legacy two-way source-wins overlay. Nothing here ever
/// silently prefers a side.
pub fn prepare_consolidation(
    seeds: Option<&SeedsContext>,
    branch_slug: &str,
    source_wt: &Path,
    target_wt: &Path,
    class: &FileClass,
) -> ConsolidationPreview {
    use crate::hooks::config_merge::{merge_configs, merge3};
    use crate::hooks::yaml_config_loader::parse_yaml_config_str;

    let filename = &class.filename;
    let source_str = std::fs::read_to_string(source_wt.join(filename)).unwrap_or_default();
    let target_path = target_wt.join(filename);

    // Target has no such file: consolidation is a verbatim copy — comments
    // and formatting preserved, nothing to merge into, nothing to lose.
    if !target_path.is_file() {
        return ConsolidationPreview {
            filename: filename.clone(),
            adopt_keys: vec!["(entire file — target has none)".to_string()],
            conflict_keys: Vec::new(),
            whole_file: false,
            resolution: PreviewResolution::Resolved(source_str),
        };
    }

    let target_str = std::fs::read_to_string(&target_path).unwrap_or_default();
    let seed_content = (class.class == SeedClass::Refined)
        .then(|| {
            seeds
                .and_then(|ctx| ctx.get_seed(branch_slug, filename))
                .map(|row| row.content)
        })
        .flatten();

    let parsed = (
        seed_content.as_deref().map(parse_yaml_config_str),
        parse_yaml_config_str(&source_str),
        parse_yaml_config_str(&target_str),
    );

    if let (Some(Ok(base)), Ok(source), Ok(target)) = parsed {
        // Three-way: ours = target (it survives), theirs = source.
        let outcome = merge3(&base, &target, &source);
        if outcome.conflicts.is_empty() {
            let content =
                serde_yaml::to_string(&outcome.merged).unwrap_or_else(|_| source_str.clone());
            return ConsolidationPreview {
                filename: filename.clone(),
                adopt_keys: outcome.took_from_theirs,
                conflict_keys: Vec::new(),
                whole_file: false,
                resolution: PreviewResolution::Resolved(content),
            };
        }
        // Conflicted: pre-compute both side-resolutions. Swapping ours and
        // theirs flips which side wins the conflicted keys while one-sided
        // changes from both sides still flow through.
        let target_priority =
            serde_yaml::to_string(&outcome.merged).unwrap_or_else(|_| target_str.clone());
        let source_priority = serde_yaml::to_string(&merge3(&base, &source, &target).merged)
            .unwrap_or_else(|_| source_str.clone());
        return ConsolidationPreview {
            filename: filename.clone(),
            adopt_keys: outcome.took_from_theirs,
            conflict_keys: outcome.conflicts,
            whole_file: false,
            resolution: PreviewResolution::NeedsSide {
                target_priority,
                source_priority,
            },
        };
    }

    // No usable base and the target has its own copy: whole-file conflict.
    // The source-priority resolution is the legacy two-way overlay; the
    // target-priority resolution leaves the target untouched.
    let source_priority = match (
        parse_yaml_config_str(&target_str),
        parse_yaml_config_str(&source_str),
    ) {
        (Ok(target), Ok(source)) => serde_yaml::to_string(&merge_configs(target, source))
            .unwrap_or_else(|_| source_str.clone()),
        _ => source_str.clone(),
    };
    ConsolidationPreview {
        filename: filename.clone(),
        adopt_keys: Vec::new(),
        conflict_keys: vec!["(entire file — no seed provenance)".to_string()],
        whole_file: true,
        resolution: PreviewResolution::NeedsSide {
            target_priority: target_str,
            source_priority,
        },
    }
}

// ── Discard stash ───────────────────────────────────────────────────────────

/// Where a stashed copy goes under `<git-common-dir>/.daft/`.
#[derive(Debug, Clone, Copy)]
pub enum StashKind {
    /// Refinements the user chose to discard (forced removal, forced prune).
    Discarded,
    /// Pre-write backups (e.g. `daft file merge` target).
    Backup,
}

impl StashKind {
    fn dir(self) -> &'static str {
        match self {
            StashKind::Discarded => "discarded",
            StashKind::Backup => "backups",
        }
    }
}

/// Copy `file` to `<git-common-dir>/.daft/<kind>/<label>/<file-name>`,
/// suffixing `-1`, `-2`, … on collision so nothing is overwritten. `label`
/// is typically the branch slug (slashes nest directories, mirroring how
/// worktree paths nest). Returns the destination for the user-facing
/// message; `None` (with a debug log) on any failure — a failed stash must
/// never fail the operation that requested it.
pub fn stash_file(
    git_common_dir: &Path,
    kind: StashKind,
    label: &str,
    file: &Path,
) -> Option<std::path::PathBuf> {
    let file_name = file.file_name()?;
    let dir = git_common_dir.join(".daft").join(kind.dir()).join(label);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::log_debug!("stash dir creation failed at {}: {e}", dir.display());
        return None;
    }

    let mut dest = dir.join(file_name);
    let mut suffix = 0u32;
    while dest.exists() {
        suffix += 1;
        if suffix > 999 {
            crate::log_debug!("stash collision overflow for {}", dest.display());
            return None;
        }
        dest = dir.join(format!("{}-{suffix}", file_name.to_string_lossy()));
    }

    match std::fs::copy(file, &dest) {
        Ok(_) => Some(dest),
        Err(e) => {
            crate::log_debug!("stash copy failed to {}: {e}", dest.display());
            None
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

    // ── Classification ──────────────────────────────────────────────

    /// Init a temp git repo so `classify_main_config` sees untracked files
    /// as Visitor. Local config only (never global).
    fn init_git(dir: &Path) {
        use std::process::Command;
        Command::new("git")
            .args(["init"])
            .arg(dir)
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.email", "t@t.com"])
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.name", "T"])
            .output()
            .unwrap();
    }

    /// Test fixture holding the seeds context plus every TempDir guard it
    /// depends on (dropping the state dir under an open pool would break
    /// later reads).
    struct ClassifyFixture {
        ctx: SeedsContext,
        src: tempfile::TempDir,
        tgt: tempfile::TempDir,
        _common: tempfile::TempDir,
        _state: tempfile::TempDir,
    }

    /// Fixture: seeds ctx + source/target worktrees (both git repos), with
    /// `content` seeded for feat/x's daft.yml.
    fn classify_fixture(content: &str) -> ClassifyFixture {
        let common = tempdir().unwrap();
        let state = tempdir().unwrap();
        let src = tempdir().unwrap();
        let tgt = tempdir().unwrap();
        init_git(src.path());
        init_git(tgt.path());
        fs::write(src.path().join("daft.yml"), content).unwrap();

        let ctx = SeedsContext::open_in(common.path(), state.path()).unwrap();
        ctx.record_seed_file("feat/x", src.path(), "daft.yml");
        ClassifyFixture {
            ctx,
            src,
            tgt,
            _common: common,
            _state: state,
        }
    }

    const SEED_A: &str = "hooks:\n  worktree-post-create:\n    jobs:\n      - name: setup\n        run: echo setup-v1\n";
    const TARGET_B: &str = "hooks:\n  worktree-post-create:\n    jobs:\n      - name: setup\n        run: echo setup-v2\n";
    const REFINED: &str = "hooks:\n  worktree-post-create:\n    jobs:\n      - name: setup\n        run: echo setup-v1\n      - name: extra\n        run: echo extra-job\n";

    #[test]
    fn pristine_stale_copy_is_removable_against_evolved_target() {
        // THE headline case from issue #628: the worktree still holds the
        // seeded A, the target moved on to B. Two-way divergence says
        // "divergent"; provenance says "pristine" — removable.
        let f = classify_fixture(SEED_A);
        fs::write(f.tgt.path().join("daft.yml"), TARGET_B).unwrap();

        let classes =
            classify_in_scope_files(Some(&f.ctx), "feat/x", f.src.path(), Some(f.tgt.path()));
        assert_eq!(classes.len(), 1);
        assert_eq!(classes[0].class, SeedClass::Pristine);
        assert!(
            !classes[0].subsumed_by_target,
            "two-way check alone would have blocked this"
        );
        assert!(all_removable(&classes));
    }

    #[test]
    fn refined_copy_blocks_unless_subsumed() {
        let f = classify_fixture(SEED_A);
        fs::write(f.src.path().join("daft.yml"), REFINED).unwrap();
        fs::write(f.tgt.path().join("daft.yml"), SEED_A).unwrap();

        let classes =
            classify_in_scope_files(Some(&f.ctx), "feat/x", f.src.path(), Some(f.tgt.path()));
        assert_eq!(classes[0].class, SeedClass::Refined);
        assert!(!all_removable(&classes));
        assert_eq!(blocking_files(&classes).len(), 1);

        // Once the target holds the refinement too, the copy is subsumed.
        fs::write(f.tgt.path().join("daft.yml"), REFINED).unwrap();
        let classes =
            classify_in_scope_files(Some(&f.ctx), "feat/x", f.src.path(), Some(f.tgt.path()));
        assert_eq!(classes[0].class, SeedClass::Refined);
        assert!(classes[0].subsumed_by_target);
        assert!(all_removable(&classes));
    }

    #[test]
    fn no_store_classifies_noseed_and_falls_back_to_subsumption() {
        let src = tempdir().unwrap();
        let tgt = tempdir().unwrap();
        init_git(src.path());
        init_git(tgt.path());
        fs::write(src.path().join("daft.local.yml"), "hooks: {}\n").unwrap();

        // Divergent (target lacks the file): blocked.
        let classes = classify_in_scope_files(None, "feat/x", src.path(), Some(tgt.path()));
        assert_eq!(classes[0].class, SeedClass::NoSeed);
        assert!(!all_removable(&classes));

        // Identical content in target: subsumed, removable.
        fs::write(tgt.path().join("daft.local.yml"), "hooks: {}\n").unwrap();
        let classes = classify_in_scope_files(None, "feat/x", src.path(), Some(tgt.path()));
        assert!(all_removable(&classes));
    }

    #[test]
    fn absent_files_and_tracked_daft_yml_are_out_of_scope() {
        let f = classify_fixture(SEED_A);
        // Delete the file: nothing in scope at all.
        fs::remove_file(f.src.path().join("daft.yml")).unwrap();
        let classes =
            classify_in_scope_files(Some(&f.ctx), "feat/x", f.src.path(), Some(f.tgt.path()));
        assert!(classes.is_empty());

        // A tracked daft.yml is git's business, not propagation's.
        fs::write(f.src.path().join("daft.yml"), SEED_A).unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(f.src.path())
            .args(["add", "daft.yml"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(f.src.path())
            .args(["commit", "-m", "track"])
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .output()
            .unwrap();
        let classes =
            classify_in_scope_files(Some(&f.ctx), "feat/x", f.src.path(), Some(f.tgt.path()));
        assert!(classes.is_empty());
    }

    #[test]
    fn missing_target_disables_subsumption_only() {
        let f = classify_fixture(SEED_A);
        fs::write(f.src.path().join("daft.yml"), REFINED).unwrap();
        let classes = classify_in_scope_files(Some(&f.ctx), "feat/x", f.src.path(), None);
        assert_eq!(classes[0].class, SeedClass::Refined);
        assert!(!classes[0].subsumed_by_target);
        assert!(!all_removable(&classes));

        // Pristine stays removable even without a target.
        fs::write(f.src.path().join("daft.yml"), SEED_A).unwrap();
        let classes = classify_in_scope_files(Some(&f.ctx), "feat/x", f.src.path(), None);
        assert!(all_removable(&classes));
    }

    // ── Stash ────────────────────────────────────────────────────────

    #[test]
    fn stash_copies_with_nested_label_and_collision_suffix() {
        let common = tempdir().unwrap();
        let wt = tempdir().unwrap();
        let file = wt.path().join("daft.yml");
        fs::write(&file, "v1").unwrap();

        let dest = stash_file(common.path(), StashKind::Discarded, "feat/x", &file).unwrap();
        assert!(
            dest.ends_with(".daft/discarded/feat/x/daft.yml"),
            "{dest:?}"
        );
        assert_eq!(fs::read_to_string(&dest).unwrap(), "v1");

        // Second stash of the same name lands beside it, never overwrites.
        fs::write(&file, "v2").unwrap();
        let dest2 = stash_file(common.path(), StashKind::Discarded, "feat/x", &file).unwrap();
        assert!(dest2.ends_with("daft.yml-1"), "{dest2:?}");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "v1", "original intact");
        assert_eq!(fs::read_to_string(&dest2).unwrap(), "v2");

        // Backup kind gets its own tree.
        let b = stash_file(common.path(), StashKind::Backup, "file-merge", &file).unwrap();
        assert!(b.ends_with(".daft/backups/file-merge/daft.yml"), "{b:?}");
    }

    #[test]
    fn stash_missing_source_returns_none() {
        let common = tempdir().unwrap();
        let missing = common.path().join("nope/daft.yml");
        assert!(stash_file(common.path(), StashKind::Discarded, "x", &missing).is_none());
    }
}
