//! Fast worktree disposal: rename the directory aside, drop git's admin
//! record, and leave the unlink walk to a detached reaper.
//!
//! `git worktree remove` deletes the directory itself, with a serial unlink
//! walk that is O(files) on the critical path — seconds to tens of seconds for
//! a worktree carrying `node_modules/` or `target/`. That walk is the whole of
//! the command's perceived cost, and none of it is work the user is waiting on
//! for a *correct* result: once the directory is out of the way and git's admin
//! entry is gone, the user-visible contract is discharged (branch gone, git
//! consistent, path free for a fresh `daft start`). Only disk reclamation is
//! left, and that can happen after the process exits.
//!
//! So: [`dispose`] renames the worktree into `<git-common-dir>/.daft/trash/`
//! (one syscall, O(1)), drops the record, and hands the tree to
//! `daft __reap-trash`.
//!
//! This is the same shape [`crate::coordinator::log_store`] already uses to
//! evict job logs (rename to `.deleting-…`, then `remove_dir_all`), and the
//! same owned-scratch-dir-plus-stale-sweep shape as [`super::temp_worktree`].
//!
//! Two properties worth stating because they are easy to lose in a refactor:
//!
//! - **A dirty worktree is never renamed.** Without `force`, `git worktree
//!   remove` performs its own uncommitted-changes check immediately before
//!   deleting — a guard distinct from, and much later than, daft's validation
//!   pass. Renaming first would silently retire it. [`dispose`] therefore
//!   re-checks and declines the fast path when the tree is dirty, so the caller
//!   falls through to git and the user sees git's own refusal, unchanged.
//! - **Reaping is convergent, not fire-and-forget.** A reaper that dies leaves
//!   a directory in the trash, and the next removal in the repo sweeps it. A
//!   failed reap is therefore delayed, never permanent, which is what keeps
//!   deferred deletion from being a silent failure.

use crate::git::GitCommand;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directory under `<git-common-dir>/.daft/` holding worktrees awaiting
/// deletion. Inside the git dir deliberately: several code paths classify
/// directories under the *project root* by shape (layout transform, the hook
/// config scanners, `daft repo remove`'s is-empty probe) and would read a
/// trash directory there as a worktree.
const TRASH_SUBDIR: &str = "trash";

/// Whether [`dispose`] deferred the delete or the caller must do it itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Renamed aside and handed to a reaper. The path is free; the disk space
    /// is not yet back.
    Deferred,
    /// The fast path did not apply. The caller must remove the worktree the
    /// ordinary way — this is not an error, and carries no diagnosis with it.
    Declined,
}

/// A worktree still sitting in the trash, for `daft doctor` to report.
#[derive(Debug, Clone)]
pub struct PendingEntry {
    pub path: PathBuf,
    /// How long ago the entry was renamed in, when that can be determined.
    pub age: Option<std::time::Duration>,
}

/// Where this repo's pending deletions live.
pub fn trash_dir(git_common_dir: &Path) -> PathBuf {
    git_common_dir.join(".daft").join(TRASH_SUBDIR)
}

/// True when `path` is inside some repo's `.daft/trash/`. The reaper deletes
/// recursively from a path it was handed, so it verifies this before touching
/// anything.
pub fn is_trash_path(path: &Path) -> bool {
    let mut parts = path.components().rev();
    matches!(parts.next(), Some(c) if c.as_os_str() == TRASH_SUBDIR)
        && matches!(parts.next(), Some(c) if c.as_os_str() == ".daft")
}

/// Move `worktree_path` out of the way and drop git's admin record for it,
/// leaving the actual deletion to a background reaper.
///
/// Returns [`Disposition::Declined`] — not an error — whenever the fast path
/// does not apply: a dirty tree without `force`, a main working tree, a rename
/// that cannot happen (`EXDEV` for a worktree on another filesystem, a
/// permission problem), or a git record that will not budge. The caller then
/// performs the ordinary removal, so declining costs correctness nothing and
/// the user sees git's own diagnostics rather than ours.
pub fn dispose(
    git: &GitCommand,
    git_common_dir: &Path,
    worktree_path: &Path,
    force: bool,
) -> Disposition {
    // A main working tree has a `.git` *directory*; a linked worktree has a
    // `.git` file. Never rename the former — it holds the repo itself.
    if worktree_path.join(".git").is_dir() {
        return Disposition::Declined;
    }

    // Preserve the guard git would have applied. Only `force` removals skip
    // it, exactly as `git worktree remove` does.
    if !force {
        match git.has_uncommitted_changes_in(worktree_path) {
            Ok(false) => {}
            // Dirty, or we could not tell: decline and let git refuse. Failing
            // closed matters here — a status probe that errors must not be
            // read as "clean".
            _ => return Disposition::Declined,
        }
    }

    // Capture identity before the rename: `canonicalize` on a path that no
    // longer exists falls back to the input, which would make the
    // still-registered check below compare the wrong thing.
    let canonical = std::fs::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.into());

    let trash = trash_dir(git_common_dir);
    if let Err(e) = std::fs::create_dir_all(&trash) {
        crate::log_debug!("trash dir creation failed at {}: {e}", trash.display());
        return Disposition::Declined;
    }

    let dest = trash.join(entry_name(worktree_path));
    if let Err(e) = std::fs::rename(worktree_path, &dest) {
        // EXDEV (worktree on another filesystem — the `centralized` layout, or
        // `--at` pointing off-volume) lands here, as does any permission
        // problem. Both are ordinary-path territory.
        crate::log_debug!("worktree rename to trash failed: {e}");
        return Disposition::Declined;
    }

    if !drop_worktree_record(git, worktree_path, &canonical) {
        // The directory is already gone from its original path, so the
        // ordinary path cannot run any more. Recover by putting it back.
        if let Err(e) = std::fs::rename(&dest, worktree_path) {
            crate::log_debug!("failed to restore worktree after record drop: {e}");
            // Restoring failed too. The directory is in the trash and git
            // still has a record; the reaper would delete the user's tree
            // while git believes it exists. Leave it alone — the sweep will
            // not touch it because the record still names it, and `daft
            // doctor` will report it.
            return Disposition::Declined;
        }
        return Disposition::Declined;
    }

    schedule_reap(&trash);
    Disposition::Deferred
}

/// Reap anything left in this repo's trash: leftovers from a reaper that died,
/// or from a `dispose` whose spawn failed. Cheap when the trash is empty, which
/// is the common case, so callers can run it unconditionally at command start.
pub fn sweep(git_common_dir: &Path) {
    let trash = trash_dir(git_common_dir);
    if !has_entries(&trash) {
        return;
    }
    schedule_reap(&trash);
}

/// Reap everything in `trash` right now. This is the reaper's body; it also
/// runs inline when background work is suppressed.
pub fn reap_now(trash: &Path) {
    if !is_trash_path(trash) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(trash) else {
        return;
    };
    for entry in entries.flatten() {
        if let Err(e) = std::fs::remove_dir_all(entry.path()) {
            // Leave it for the next sweep rather than retrying here.
            crate::log_debug!("reap of {} failed: {e}", entry.path().display());
        }
    }
    // Best-effort; fails harmlessly while another reaper still holds entries.
    let _ = std::fs::remove_dir(trash);
}

/// Entry point for the `daft __reap-trash <dir>` background process: drain one
/// repo's trash and exit.
///
/// Errors go nowhere — the process is detached with null stdio, so there is no
/// channel to report on, exactly as [`crate::commands::forge_cache::run_refresh_forge`]
/// describes. That is tolerable here because the failure mode is benign and
/// self-correcting: whatever this process fails to delete stays in the trash,
/// the next removal in the repo sweeps it again, and `daft doctor` reports
/// anything that outlives several attempts. A directory that persists *is* the
/// error report.
pub fn run_reap_trash(trash: &Path) -> anyhow::Result<()> {
    // Detach from the parent's session/TTY per the spawn-self contract.
    #[cfg(unix)]
    nix::unistd::setsid().ok();

    if !is_trash_path(trash) {
        anyhow::bail!("refusing to reap {}: not a daft trash dir", trash.display());
    }

    // Single-flight, mirroring `log_clean::run_clean_logs`. Two reapers racing
    // over the same directory would each see the other's half-deleted trees and
    // log spurious failures. The lock sits beside the trash dir rather than
    // inside it, so draining the trash does not delete the lock.
    let lock_path = trash.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    use fs2::FileExt;
    if lock_file.try_lock_exclusive().is_err() {
        return Ok(()); // another reaper is already draining this trash
    }

    reap_now(trash);
    Ok(())
}

/// Worktrees still awaiting deletion, for `daft doctor`.
pub fn pending(git_common_dir: &Path) -> Vec<PendingEntry> {
    let trash = trash_dir(git_common_dir);
    let Ok(entries) = std::fs::read_dir(&trash) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| {
            let age = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok());
            PendingEntry {
                path: entry.path(),
                age,
            }
        })
        .collect()
}

/// Drop git's admin entry for a worktree whose directory is already gone.
///
/// `git worktree remove --force` handles this directly; `git worktree prune` is
/// the documented mechanism and stands behind it. Either way the record is
/// verified gone rather than trusted from an exit code, because `prune` is
/// repo-wide and will also clear records for *other* worktrees that happen to
/// be missing — a green exit says nothing about ours specifically.
fn drop_worktree_record(git: &GitCommand, worktree_path: &Path, canonical: &Path) -> bool {
    if git.worktree_remove(worktree_path, true).is_ok() && !still_registered(git, canonical) {
        return true;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = crate::utils::git_command_at(&cwd)
        .args(["worktree", "prune"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    !still_registered(git, canonical)
}

fn still_registered(git: &GitCommand, canonical: &Path) -> bool {
    let Ok(porcelain) = git.worktree_list_porcelain() else {
        // Cannot tell — assume it is, so the caller restores and falls back.
        return true;
    };
    super::porcelain::parse_worktree_list_porcelain(&porcelain)
        .iter()
        .any(|e| std::fs::canonicalize(&e.path).unwrap_or_else(|_| e.path.clone()) == canonical)
}

/// Hand `trash` to a detached reaper, or drain it inline when background work
/// is suppressed.
///
/// Inline is not merely a test convenience: the manual suite runs ~2000 daft
/// invocations, and a spawn per removal would either pile up orphaned processes
/// or leave unreaped directories that change `read_dir`-based assertions.
/// [`crate::should_skip_background_tasks`] is the same gate the other
/// background tasks use.
fn schedule_reap(trash: &Path) {
    // `cfg!(test)` is not redundant with the env gates: under `cargo test`
    // `current_exe()` is the *test* binary, so a spawn would fork the test
    // harness with `__reap-trash` as a filter argument rather than starting a
    // reaper — a stray process per removal, and trash that never drains.
    if cfg!(test)
        || crate::should_skip_background_tasks(crate::cli::argv())
        || std::env::var_os("DAFT_NO_TRASH_REAP").is_some()
    {
        reap_now(trash);
        return;
    }
    if spawn_reaper(trash).is_err() {
        reap_now(trash);
    }
}

#[cfg(unix)]
fn spawn_reaper(trash: &Path) -> anyhow::Result<()> {
    // canonicalize() is load-bearing: removal is reached through symlinks
    // (`git-worktree-branch-delete`, `git-worktree-prune`, …) and dispatch is
    // by argv[0], so spawning the symlink would route the child into that
    // command's arm, where clap rejects `__reap-trash` — silently, since the
    // child's stderr is /dev/null.
    let exe = std::env::current_exe()?.canonicalize()?;
    std::process::Command::new(exe)
        .arg("__reap-trash")
        .arg(trash)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(not(unix))]
fn spawn_reaper(_trash: &Path) -> anyhow::Result<()> {
    // No detached reaper off unix: there is no session to leave, and Windows
    // refuses to rename a directory any process holds open, so the fast path
    // is unreliable there anyway. Callers drain inline.
    anyhow::bail!("background reaping is unix-only")
}

fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

/// A collision-free name for a trashed worktree. The original name leads so a
/// human reading `daft doctor` output can tell what is waiting to be deleted.
fn entry_name(worktree_path: &Path) -> String {
    let base = worktree_path
        .file_name()
        .map(|n| n.to_string_lossy().replace(['/', '\\'], "-"))
        .unwrap_or_else(|| "worktree".to_string());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{base}-{}-{nanos}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_lives_under_the_git_dir() {
        let dir = trash_dir(Path::new("/repo/.git"));
        assert_eq!(dir, PathBuf::from("/repo/.git/.daft/trash"));
    }

    #[test]
    fn only_a_daft_trash_path_is_reapable() {
        assert!(is_trash_path(Path::new("/repo/.git/.daft/trash")));
        assert!(!is_trash_path(Path::new("/repo/.git/.daft")));
        assert!(!is_trash_path(Path::new("/repo/.git")));
        assert!(!is_trash_path(Path::new("/")));
        assert!(!is_trash_path(Path::new("/Users/someone")));
        // A directory merely *named* trash is not ours.
        assert!(!is_trash_path(Path::new("/tmp/trash")));
    }

    #[test]
    fn reap_refuses_a_path_outside_the_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let victim = tmp.path().join("not-trash");
        std::fs::create_dir_all(victim.join("payload")).unwrap();

        reap_now(&victim);

        assert!(
            victim.join("payload").exists(),
            "reap_now must refuse a path that is not a .daft/trash dir"
        );
    }

    #[test]
    fn reap_drains_the_trash_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = trash_dir(&tmp.path().join(".git"));
        std::fs::create_dir_all(trash.join("wt-1/nested")).unwrap();
        std::fs::create_dir_all(trash.join("wt-2")).unwrap();
        std::fs::write(trash.join("wt-1/nested/f.txt"), "x").unwrap();

        reap_now(&trash);

        assert!(!trash.join("wt-1").exists());
        assert!(!trash.join("wt-2").exists());
    }

    #[test]
    fn reap_on_an_absent_trash_dir_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        reap_now(&trash_dir(&tmp.path().join(".git")));
    }

    /// The convergence guarantee: whatever a dead reaper left behind is
    /// reclaimed by the next removal in the repo. This is what makes a failed
    /// background delete delayed rather than permanent.
    #[test]
    fn sweep_reclaims_what_a_dead_reaper_left() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        let trash = trash_dir(&git_dir);
        std::fs::create_dir_all(trash.join("stranded/deep")).unwrap();
        std::fs::write(trash.join("stranded/deep/f.txt"), "x").unwrap();

        sweep(&git_dir);

        assert!(!trash.join("stranded").exists());
    }

    #[test]
    fn sweep_without_a_trash_dir_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        sweep(&tmp.path().join(".git"));
    }

    #[test]
    fn entry_names_do_not_collide() {
        let a = entry_name(Path::new("/repo/feature"));
        let b = entry_name(Path::new("/repo/feature"));
        assert_ne!(a, b);
        assert!(a.starts_with("feature-"), "{a}");
    }

    #[test]
    fn pending_reports_nothing_when_the_trash_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(pending(&tmp.path().join(".git")).is_empty());
    }

    #[test]
    fn pending_reports_each_waiting_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        let trash = trash_dir(&git_dir);
        std::fs::create_dir_all(trash.join("wt-1")).unwrap();
        std::fs::create_dir_all(trash.join("wt-2")).unwrap();

        let entries = pending(&git_dir);

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.age.is_some()));
    }
}
