//! Writing shims into the hooks directory, and taking them back out.
//!
//! Everything here stays under `.git/`. Assessing daft as a hooks manager
//! must not touch the tracked tree: nothing to commit, nothing to revert,
//! and `uninstall` restores the repository byte for byte.
//!
//! The planning half is pure — it decides what *would* happen from directory
//! contents alone — so the interesting cases (a foreign hook, an unrecognised
//! script, a `core.hooksPath` pointing into the working tree) are testable
//! without a filesystem.

use super::gitdir::GitDirs;
use super::{GitStage, shim};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Suffix for a displaced hook. Chosen to sort next to the original and to
/// read as "what was here before daft".
pub const BACKUP_SUFFIX: &str = ".pre-daft";

/// Where the install record lives, relative to the shared git dir.
const SENTINEL_PATH: &str = ".daft/stage-shims.json";

/// What daft did to a repository's hooks directory.
///
/// Three jobs in one small file: it is the consent record (a `lefthook.yml`
/// alone must never make daft take over — someone has to ask), the bookkeeping
/// `uninstall` needs to put things back, and a stat-cheap probe for the push
/// path, which must decide whether daft manages a stage without reading
/// anything expensive.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sentinel {
    /// Format version of the shims installed, for future migrations.
    pub shim_version: u32,
    /// Stages daft installed a shim for, by git filename.
    pub stages: Vec<String>,
    /// Displaced hooks: stage filename → what it was.
    #[serde(default)]
    pub backups: BTreeMap<String, String>,
    /// A `core.hooksPath` that was unset to install, so uninstall can restore
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_hooks_path: Option<String>,
    /// When the install happened, for `hooks status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<String>,
}

/// The version [`shim::render`] currently produces.
pub const SHIM_VERSION: u32 = 1;

impl Sentinel {
    /// Read the record, or `None` when daft has not installed here.
    ///
    /// A malformed file reads as `None` rather than erroring: it is a cache
    /// of what daft did, and a corrupted one must not make every push fail.
    /// Reinstalling rewrites it.
    pub fn load(dirs: &GitDirs) -> Option<Self> {
        let raw = std::fs::read_to_string(dirs.common_dir.join(SENTINEL_PATH)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Whether a sentinel exists at all — the stat-only question the push
    /// path asks.
    pub fn exists(dirs: &GitDirs) -> bool {
        dirs.common_dir.join(SENTINEL_PATH).exists()
    }

    /// Write the record, creating `.git/.daft/` if needed.
    pub fn save(&self, dirs: &GitDirs) -> Result<()> {
        let path = dirs.common_dir.join(SENTINEL_PATH);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))
    }

    /// Remove the record.
    pub fn remove(dirs: &GitDirs) -> Result<()> {
        let path = dirs.common_dir.join(SENTINEL_PATH);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("failed to remove {}", path.display())),
        }
    }
}

/// What installing would do to one stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagePlan {
    /// Nothing there — write the shim.
    Install,
    /// A daft shim of an older rendering — rewrite it.
    Update,
    /// Byte-identical shim already present.
    Keep,
    /// A recognised manager's hook — move it aside, then write the shim.
    BackUp { manager: &'static str },
    /// Something daft does not recognise. Left alone unless forced, because
    /// it may be the only copy of somebody's work.
    Skip { reason: String },
    /// Forced past an unrecognised hook: back it up and install anyway.
    ForceBackUp,
}

impl StagePlan {
    /// Whether carrying this plan out writes a shim.
    pub fn writes_shim(&self) -> bool {
        !matches!(self, StagePlan::Keep | StagePlan::Skip { .. })
    }

    /// One-word status for the per-stage report.
    pub fn label(&self) -> &'static str {
        match self {
            StagePlan::Install => "installed",
            StagePlan::Update => "updated",
            StagePlan::Keep => "unchanged",
            StagePlan::BackUp { .. } | StagePlan::ForceBackUp => "backed up",
            StagePlan::Skip { .. } => "skipped",
        }
    }
}

/// Decide what to do with one stage, from the file that is there now.
///
/// Pure: `existing` is the current file's contents (`None` when absent) and
/// `desired` is what the shim would be.
pub fn plan_stage(existing: Option<&str>, desired: &str, force: bool) -> StagePlan {
    let Some(existing) = existing else {
        return StagePlan::Install;
    };
    if existing == desired {
        return StagePlan::Keep;
    }
    if shim::is_daft_shim(existing) {
        return StagePlan::Update;
    }
    if let Some(manager) = shim::recognized_foreign_manager(existing) {
        return StagePlan::BackUp { manager };
    }
    if force {
        return StagePlan::ForceBackUp;
    }
    StagePlan::Skip {
        reason: "an unrecognised hook is already installed here".to_string(),
    }
}

/// Why installing cannot proceed at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallBlocker {
    /// `core.hooksPath` points somewhere daft will not write.
    HooksPath {
        path: String,
        /// Whether it points inside the working tree (a tracked hooks
        /// directory, husky-style) rather than elsewhere under `.git`.
        in_worktree: bool,
    },
}

/// Whether a configured `core.hooksPath` blocks installing.
///
/// Daft installs into git's default hooks directory rather than pointing
/// `core.hooksPath` at its own: that setting is winner-take-all, so claiming
/// it would silently shadow every hook daft does *not* manage. But somebody
/// else may already have claimed it, and then daft's shims would be the ones
/// never called — so installing has to stop and say so rather than write
/// files that do nothing.
///
/// A path inside the working tree is called out separately because it is
/// almost always husky, and unsetting it means the repository's *tracked*
/// hooks stop running for everyone who pulls — which is a decision for a
/// person, not a flag's side effect.
pub fn hooks_path_blocker(hooks_path: Option<&str>, dirs: &GitDirs) -> Option<InstallBlocker> {
    let raw = hooks_path?.trim();
    if raw.is_empty() {
        return None;
    }
    let resolved = {
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            p
        } else {
            dirs.worktree_root.join(p)
        }
    };
    // Pointing at the directory daft installs into is not a blocker — it is
    // the default said out loud.
    if resolved == dirs.default_hooks_dir() {
        return None;
    }
    let in_worktree =
        resolved.starts_with(&dirs.worktree_root) && !resolved.starts_with(&dirs.common_dir);
    Some(InstallBlocker::HooksPath {
        path: raw.to_string(),
        in_worktree,
    })
}

/// One stage's outcome, for the report install prints.
#[derive(Debug, Clone)]
pub struct StageOutcome {
    pub stage: GitStage,
    pub plan: StagePlan,
}

/// Install shims for `stages` into the repository's hooks directory.
///
/// Every supported stage is installed, not only the configured ones: the
/// no-op dispatch is cheap, and installing per-config would mean editing
/// `daft.yml` silently fails to take effect until someone remembers to
/// reinstall — the kind of staleness that is only ever discovered by a gate
/// not firing.
pub fn install(
    dirs: &GitDirs,
    binary: &Path,
    force: bool,
    saved_hooks_path: Option<String>,
) -> Result<Vec<StageOutcome>> {
    let hooks_dir = dirs.default_hooks_dir();
    std::fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("failed to create {}", hooks_dir.display()))?;

    let mut outcomes = Vec::with_capacity(GitStage::all().len());
    let mut backups = Sentinel::load(dirs).map(|s| s.backups).unwrap_or_default();
    let mut installed: Vec<String> = Vec::new();

    for &stage in GitStage::all() {
        let name = stage.git_hook_filename();
        let path = hooks_dir.join(name);
        let existing = std::fs::read_to_string(&path).ok();
        let desired = shim::render(stage, binary);
        let plan = plan_stage(existing.as_deref(), &desired, force);

        if matches!(plan, StagePlan::BackUp { .. } | StagePlan::ForceBackUp) {
            let backup = hooks_dir.join(format!("{name}{BACKUP_SUFFIX}"));
            std::fs::rename(&path, &backup)
                .with_context(|| format!("failed to back up {}", path.display()))?;
            backups.insert(name.to_string(), backup.display().to_string());
        }

        if plan.writes_shim() {
            write_executable(&path, &desired)?;
        }
        if !matches!(plan, StagePlan::Skip { .. }) {
            installed.push(name.to_string());
        }
        outcomes.push(StageOutcome { stage, plan });
    }

    Sentinel {
        shim_version: SHIM_VERSION,
        stages: installed,
        backups,
        saved_hooks_path,
        installed_at: Some(chrono::Utc::now().to_rfc3339()),
    }
    .save(dirs)?;

    Ok(outcomes)
}

/// What uninstalling did.
#[derive(Debug, Clone, Default)]
pub struct UninstallReport {
    /// Stages whose shim was removed.
    pub removed: Vec<String>,
    /// Stages whose backed-up hook was put back.
    pub restored: Vec<String>,
    /// Stages holding a hook daft did not write, left alone.
    pub left_alone: Vec<String>,
    /// A `core.hooksPath` value the caller should restore.
    pub restore_hooks_path: Option<String>,
}

/// Remove daft's shims and put back whatever they displaced.
///
/// Only marker-proven files are removed. A stage whose hook someone replaced
/// by hand since install is reported and left, because the alternative is
/// deleting work on the strength of a filename.
pub fn uninstall(dirs: &GitDirs) -> Result<UninstallReport> {
    let hooks_dir = dirs.default_hooks_dir();
    let sentinel = Sentinel::load(dirs).unwrap_or_default();
    let mut report = UninstallReport {
        restore_hooks_path: sentinel.saved_hooks_path.clone(),
        ..Default::default()
    };

    for &stage in GitStage::all() {
        let name = stage.git_hook_filename();
        let path = hooks_dir.join(name);
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !shim::is_daft_shim(&contents) {
            report.left_alone.push(name.to_string());
            continue;
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        report.removed.push(name.to_string());

        let backup = hooks_dir.join(format!("{name}{BACKUP_SUFFIX}"));
        if backup.exists() {
            std::fs::rename(&backup, &path)
                .with_context(|| format!("failed to restore {}", backup.display()))?;
            report.restored.push(name.to_string());
        }
    }

    Sentinel::remove(dirs)?;
    Ok(report)
}

/// Write a hook file with the executable bit set.
fn write_executable(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // git ignores a hook without the bit, which would make install
        // silently do nothing.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to make {} executable", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn dirs_at(root: &Path) -> GitDirs {
        GitDirs {
            worktree_root: root.to_path_buf(),
            git_dir: root.join(".git"),
            common_dir: root.join(".git"),
        }
    }

    fn scratch_repo() -> (tempfile::TempDir, GitDirs) {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        let dirs = dirs_at(&root);
        (tmp, dirs)
    }

    fn daft_bin() -> PathBuf {
        PathBuf::from("/usr/local/bin/daft")
    }

    // ── planning (pure) ────────────────────────────────────────────────

    #[test]
    fn an_empty_slot_installs() {
        assert_eq!(plan_stage(None, "shim", false), StagePlan::Install);
    }

    #[test]
    fn an_identical_shim_is_left_alone() {
        assert_eq!(plan_stage(Some("shim"), "shim", false), StagePlan::Keep);
    }

    #[test]
    fn an_older_daft_shim_is_rewritten() {
        let old = shim::render(GitStage::PreCommit, Path::new("/old/daft"));
        let new = shim::render(GitStage::PreCommit, Path::new("/new/daft"));
        assert_eq!(plan_stage(Some(&old), &new, false), StagePlan::Update);
    }

    #[test]
    fn a_recognised_manager_is_backed_up_without_asking() {
        // Its config regenerates it, so moving it aside loses nothing.
        let plan = plan_stage(
            Some("#!/bin/sh\nexec lefthook run pre-commit"),
            "shim",
            false,
        );
        assert_eq!(
            plan,
            StagePlan::BackUp {
                manager: "lefthook"
            }
        );
    }

    #[test]
    fn an_unrecognised_hook_stops_the_stage_rather_than_being_moved() {
        // It may be the only copy of somebody's work.
        let plan = plan_stage(Some("#!/bin/bash\nmake check"), "shim", false);
        assert!(matches!(plan, StagePlan::Skip { .. }));
        assert!(!plan.writes_shim());
    }

    #[test]
    fn force_backs_up_an_unrecognised_hook_instead_of_skipping() {
        let plan = plan_stage(Some("#!/bin/bash\nmake check"), "shim", true);
        assert_eq!(plan, StagePlan::ForceBackUp);
        assert!(plan.writes_shim());
    }

    // ── core.hooksPath ─────────────────────────────────────────────────

    #[test]
    fn no_hooks_path_does_not_block() {
        let (_tmp, dirs) = scratch_repo();
        assert_eq!(hooks_path_blocker(None, &dirs), None);
        assert_eq!(hooks_path_blocker(Some("  "), &dirs), None);
    }

    #[test]
    fn a_hooks_path_naming_the_default_does_not_block() {
        let (_tmp, dirs) = scratch_repo();
        let default = dirs.default_hooks_dir().display().to_string();
        assert_eq!(hooks_path_blocker(Some(&default), &dirs), None);
    }

    #[test]
    fn a_hooks_path_into_the_working_tree_is_flagged_as_such() {
        // Almost always husky. Unsetting it stops the repository's tracked
        // hooks for everyone who pulls, which is a person's decision.
        let (_tmp, dirs) = scratch_repo();
        let blocker = hooks_path_blocker(Some(".husky"), &dirs).unwrap();
        assert_eq!(
            blocker,
            InstallBlocker::HooksPath {
                path: ".husky".to_string(),
                in_worktree: true,
            }
        );
    }

    #[test]
    fn a_hooks_path_elsewhere_blocks_without_the_worktree_warning() {
        let (_tmp, dirs) = scratch_repo();
        let blocker = hooks_path_blocker(Some("/opt/shared-hooks"), &dirs).unwrap();
        assert!(matches!(
            blocker,
            InstallBlocker::HooksPath {
                in_worktree: false,
                ..
            }
        ));
    }

    // ── install / uninstall round trip ─────────────────────────────────

    #[test]
    fn install_writes_every_stage_and_records_a_sentinel() {
        let (_tmp, dirs) = scratch_repo();
        let outcomes = install(&dirs, &daft_bin(), false, None).unwrap();

        assert_eq!(outcomes.len(), GitStage::all().len());
        for &stage in GitStage::all() {
            let path = dirs.default_hooks_dir().join(stage.git_hook_filename());
            let contents = fs::read_to_string(&path).unwrap();
            assert_eq!(shim::declared_stage(&contents), Some(stage), "{stage}");
        }
        let sentinel = Sentinel::load(&dirs).expect("sentinel written");
        assert_eq!(sentinel.shim_version, SHIM_VERSION);
        assert_eq!(sentinel.stages.len(), GitStage::all().len());
    }

    #[test]
    #[cfg(unix)]
    fn installed_shims_are_executable() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, dirs) = scratch_repo();
        install(&dirs, &daft_bin(), false, None).unwrap();
        let path = dirs.default_hooks_dir().join("pre-commit");
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        // git ignores a hook without the bit, which would make install
        // silently do nothing at all.
        assert_ne!(mode & 0o111, 0, "mode {mode:o}");
    }

    #[test]
    fn reinstalling_is_idempotent() {
        let (_tmp, dirs) = scratch_repo();
        install(&dirs, &daft_bin(), false, None).unwrap();
        let second = install(&dirs, &daft_bin(), false, None).unwrap();
        assert!(
            second.iter().all(|o| o.plan == StagePlan::Keep),
            "{second:?}"
        );
    }

    #[test]
    fn a_foreign_hook_is_backed_up_and_restored_byte_for_byte() {
        // The assessment promise: uninstall leaves the repository as it was.
        let (_tmp, dirs) = scratch_repo();
        let original = "#!/bin/sh\nexec lefthook run pre-commit \"$@\"\n";
        let path = dirs.default_hooks_dir().join("pre-commit");
        fs::write(&path, original).unwrap();

        install(&dirs, &daft_bin(), false, None).unwrap();
        assert!(shim::is_daft_shim(&fs::read_to_string(&path).unwrap()));
        assert!(
            dirs.default_hooks_dir()
                .join(format!("pre-commit{BACKUP_SUFFIX}"))
                .exists()
        );

        let report = uninstall(&dirs).unwrap();
        assert!(report.restored.contains(&"pre-commit".to_string()));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert!(!Sentinel::exists(&dirs));
    }

    #[test]
    fn an_unrecognised_hook_survives_install_untouched() {
        let (_tmp, dirs) = scratch_repo();
        let mine = "#!/bin/bash\nmake check || exit 1\n";
        let path = dirs.default_hooks_dir().join("pre-push");
        fs::write(&path, mine).unwrap();

        let outcomes = install(&dirs, &daft_bin(), false, None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), mine);
        let plan = &outcomes
            .iter()
            .find(|o| o.stage == GitStage::PrePush)
            .unwrap()
            .plan;
        assert!(matches!(plan, StagePlan::Skip { .. }));
        // …and the sentinel does not claim a stage daft did not take.
        let sentinel = Sentinel::load(&dirs).unwrap();
        assert!(!sentinel.stages.contains(&"pre-push".to_string()));
    }

    #[test]
    fn uninstall_leaves_hooks_daft_did_not_write() {
        let (_tmp, dirs) = scratch_repo();
        install(&dirs, &daft_bin(), false, None).unwrap();

        // Someone replaced one shim by hand after installing.
        let path = dirs.default_hooks_dir().join("commit-msg");
        fs::write(&path, "#!/bin/sh\nmy own thing\n").unwrap();

        let report = uninstall(&dirs).unwrap();
        assert!(report.left_alone.contains(&"commit-msg".to_string()));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "#!/bin/sh\nmy own thing\n"
        );
    }

    #[test]
    fn uninstall_reports_a_hooks_path_to_restore() {
        let (_tmp, dirs) = scratch_repo();
        install(&dirs, &daft_bin(), true, Some(".husky".to_string())).unwrap();
        let report = uninstall(&dirs).unwrap();
        assert_eq!(report.restore_hooks_path.as_deref(), Some(".husky"));
    }

    #[test]
    fn a_corrupt_sentinel_reads_as_absent_rather_than_erroring() {
        // It is a cache of what daft did; a broken one must not make every
        // push fail. Reinstalling rewrites it.
        let (_tmp, dirs) = scratch_repo();
        fs::create_dir_all(dirs.common_dir.join(".daft")).unwrap();
        fs::write(dirs.common_dir.join(SENTINEL_PATH), "{ not json").unwrap();
        assert!(Sentinel::load(&dirs).is_none());
        // …but `exists` still answers the stat-level question truthfully.
        assert!(Sentinel::exists(&dirs));
    }

    #[test]
    fn uninstalling_what_was_never_installed_is_a_no_op() {
        let (_tmp, dirs) = scratch_repo();
        let report = uninstall(&dirs).unwrap();
        assert!(report.removed.is_empty());
        assert!(report.restored.is_empty());
    }
}
