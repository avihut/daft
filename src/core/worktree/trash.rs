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
//! # What keeps this from losing a worktree
//!
//! **Nothing is deleted until git has disowned it.** The rename happens before
//! the record drop, so there is a window in which the tree sits in the trash
//! while git still points at its old path — a `SIGINT` or a crash lands
//! squarely in it. A [sidecar file](origin_sidecar) written *before* the rename
//! marks the entry as not-yet-committed, and [`dispose`] deletes that sidecar
//! only once the record drop is verified. Reapers skip any entry that still has
//! one, so an interrupted removal costs disk space, never data. Resolving those
//! entries — restoring the worktree, or clearing it once git no longer wants it
//! — is [`reclaim`]'s job, driven by `daft doctor --fix`, which has a
//! `GitCommand` and a place to report.
//!
//! This protects against process death, not power loss: the sidecar is not
//! fsynced, because a barrier would cost more than the entire fast path.
//!
//! **Git's own refusals are preserved, not retired.** Without `force`, `git
//! worktree remove` runs two checks immediately before deleting, both much
//! later than daft's validation pass, and renaming first would silently retire
//! both. So [`dispose`] re-runs them and declines the fast path if either
//! fires, leaving the caller to invoke git and the user to see git's own
//! wording:
//!
//! - uncommitted changes ([`crate::git::GitCommand::has_uncommitted_changes_in`]);
//! - a populated submodule, which git refuses with "working trees containing
//!   submodules cannot be moved or removed" ([`has_populated_submodules`]).
//!
//! Both are gated on `force`, matching git: `git worktree remove --force`
//! removes a submodule-bearing worktree, so daft's forced path may too. A
//! *locked* worktree needs no check here — git wants `--force --force` for one
//! and [`drop_worktree_record`] only ever passes a single `--force`, so the
//! record survives, the restore fires, and git issues the refusal itself.
//!
//! **Reaping is convergent, not fire-and-forget.** A reaper that dies leaves a
//! directory in the trash, and the next removal in the repo sweeps it. A failed
//! reap is therefore delayed, never permanent, which is what keeps deferred
//! deletion from being a silent failure.

use crate::git::GitCommand;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Directory under `<git-common-dir>/.daft/` holding worktrees awaiting
/// deletion. Inside the git dir deliberately: several code paths classify
/// directories under the *project root* by shape (layout transform, the hook
/// config scanners, `daft repo remove`'s is-empty probe) and would read a
/// trash directory there as a worktree.
const TRASH_SUBDIR: &str = "trash";

/// Suffix marking the sidecar that protects a not-yet-committed trash entry.
/// Appended to the entry's full name rather than set with `Path::set_extension`
/// — a worktree called `feature.v2` trashes as `feature.v2-…`, and setting an
/// extension there would *replace* `v2` and produce a sidecar guarding nothing.
const ORIGIN_SUFFIX: &str = ".origin";

/// Shared prefix for every "the fast path did not apply" line, so that
/// `--verbose` output can be grepped for the reason with one pattern.
///
/// Worth the constant: a decline is otherwise indistinguishable from a fast
/// removal — same exit code, same rows, only a missing annotation — so a
/// removal that quietly walks the tree looks exactly like one that did not.
/// Diagnosing that from the outside is guesswork, and was.
const DECLINED: &str = "fast removal declined";

/// What [`dispose`] actually did. The distinction between the first two is
/// user-facing: only `Deferred` may claim the space is still coming back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Renamed aside and handed to a detached reaper. The path is free; the
    /// disk space is not yet back.
    Deferred,
    /// Renamed aside and reclaimed before returning, because background work
    /// was suppressed. Indistinguishable from `Deferred` to the caller's
    /// control flow, but *not* to the user: the space is already back, so
    /// saying "reclaiming in background" here would be a lie — and one that
    /// hides the cost, since the walk did happen on the critical path.
    Reclaimed,
    /// The fast path did not apply. The caller must remove the worktree the
    /// ordinary way — this is not an error, and carries no diagnosis with it.
    Declined,
}

/// A worktree still sitting in the trash, for `daft doctor` to report.
#[derive(Debug, Clone)]
pub struct PendingEntry {
    pub path: PathBuf,
    /// How long ago the entry was renamed in, read from the timestamp
    /// [`entry_name`] encodes. Deliberately *not* the directory's mtime, which
    /// `rename(2)` carries over from the worktree and so measures when the user
    /// last edited a file — a week-old checkout would look a week stranded the
    /// instant it was removed.
    pub age: Option<Duration>,
    /// True when the entry still has its [sidecar](origin_sidecar): the rename
    /// happened but the record drop never confirmed, so this is an interrupted
    /// removal rather than a reap in flight. No reaper will touch it; only
    /// [`reclaim`] resolves it.
    pub interrupted: bool,
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

/// The sidecar guarding `dest` until its removal is committed.
fn origin_sidecar(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(ORIGIN_SUFFIX);
    PathBuf::from(name)
}

/// Reproduce, from another module's tests, the state a removal leaves behind
/// when it dies between the rename and the record drop. Deliberately test-only:
/// in production the write happens inside [`dispose`], where its *ordering*
/// relative to the rename is the whole point.
#[cfg(test)]
pub fn write_origin_sidecar_for_test(dest: &Path, origin: &Path) {
    std::fs::write(origin_sidecar(dest), path_bytes(origin)).unwrap();
}

fn is_sidecar(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|n| n.ends_with(ORIGIN_SUFFIX))
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

// Off unix the fast path never runs (see `spawn_reaper`), so a lossy round-trip
// here is unreachable in practice; it exists to keep the module compiling.
#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

/// Move `worktree_path` out of the way and drop git's admin record for it,
/// leaving the actual deletion to a background reaper.
///
/// Returns [`Disposition::Declined`] — not an error — whenever the fast path
/// does not apply: a dirty tree or a populated submodule without `force`, a
/// main working tree, a rename that cannot happen (`EXDEV` for a worktree on
/// another filesystem, a permission problem), or a git record that will not
/// budge. The caller then performs the ordinary removal, so declining costs
/// correctness nothing and the user sees git's own diagnostics rather than ours.
pub fn dispose(
    git: &GitCommand,
    git_common_dir: &Path,
    worktree_path: &Path,
    force: bool,
) -> Disposition {
    // A main working tree has a `.git` *directory*; a linked worktree has a
    // `.git` file. Never rename the former — it holds the repo itself.
    if worktree_path.join(".git").is_dir() {
        crate::log_debug!("{DECLINED}: main working tree");
        return Disposition::Declined;
    }

    // Preserve the two guards git would have applied. Only `force` removals
    // skip them, exactly as `git worktree remove` does.
    if !force {
        match git.has_uncommitted_changes_in(worktree_path) {
            Ok(false) => {}
            // Dirty, or we could not tell: decline and let git refuse. Failing
            // closed matters here — a status probe that errors must not be
            // read as "clean".
            Ok(true) => {
                crate::log_debug!("{DECLINED}: worktree has uncommitted changes");
                return Disposition::Declined;
            }
            Err(e) => {
                crate::log_debug!("{DECLINED}: could not check for changes: {e}");
                return Disposition::Declined;
            }
        }
        if has_populated_submodules(worktree_path) {
            crate::log_debug!("{DECLINED}: worktree contains a populated submodule");
            return Disposition::Declined;
        }
    }

    // Capture identity before the rename: `canonicalize` on a path that no
    // longer exists falls back to the input, which would make the
    // still-registered check below compare the wrong thing.
    let canonical = std::fs::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.into());

    let trash = trash_dir(git_common_dir);
    if let Err(e) = std::fs::create_dir_all(&trash) {
        crate::log_debug!("{DECLINED}: cannot create {}: {e}", trash.display());
        return Disposition::Declined;
    }

    let dest = trash.join(entry_name(worktree_path));
    let sidecar = origin_sidecar(&dest);

    // Write the guard *before* the rename, so that every moment in which the
    // tree is in the trash without git having disowned it is a moment in which
    // reapers can see that it must not be touched.
    if let Err(e) = std::fs::write(&sidecar, path_bytes(&canonical)) {
        crate::log_debug!("{DECLINED}: cannot mark the trash entry: {e}");
        return Disposition::Declined;
    }

    if let Err(e) = std::fs::rename(worktree_path, &dest) {
        // EXDEV (worktree on another filesystem — the `centralized` layout, or
        // `--at` pointing off-volume) lands here, as does any permission
        // problem. Both are ordinary-path territory.
        crate::log_debug!("{DECLINED}: rename into the trash failed: {e}");
        let _ = std::fs::remove_file(&sidecar);
        return Disposition::Declined;
    }

    if !drop_worktree_record(git, worktree_path, &canonical) {
        crate::log_debug!("{DECLINED}: git would not drop the worktree record");
        // The directory is already gone from its original path, so the
        // ordinary path cannot run any more. Recover by putting it back.
        match std::fs::rename(&dest, worktree_path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&sidecar);
            }
            Err(e) => {
                // Restoring failed too. The tree stays in the trash *with* its
                // sidecar, which is exactly the state that keeps reapers off
                // it; `daft doctor` reports it and `reclaim` resolves it.
                crate::log_debug!("failed to restore worktree after record drop: {e}");
            }
        }
        return Disposition::Declined;
    }

    // Git has disowned the path. Dropping the sidecar is the commit point:
    // from here the entry is ordinary garbage and any reaper may take it.
    if let Err(e) = std::fs::remove_file(&sidecar) {
        // The tree is safe either way — it simply will not be reaped until
        // `daft doctor --fix` resolves it, so say so rather than claiming
        // background reclamation.
        crate::log_debug!("could not commit the trash entry, leaving it for doctor: {e}");
        return Disposition::Declined;
    }

    schedule_reap(&trash)
}

/// Does this worktree hold a submodule that is actually checked out?
///
/// Git's refusal is keyed on a gitlink in the index whose directory exists, so
/// a `.gitmodules` listing submodules nobody ran `submodule update` for does
/// *not* block removal — verified against git 2.55. Cheap in the common case:
/// one `stat` for the `.gitmodules` that almost never exists.
///
/// Fails closed. An unreadable or malformed `.gitmodules` reports `true`, which
/// costs a fast path and never a worktree.
fn has_populated_submodules(worktree_path: &Path) -> bool {
    if !worktree_path.join(".gitmodules").exists() {
        return false;
    }
    let output = crate::utils::git_command_at(worktree_path)
        .args([
            "config",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ])
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(output) = output else {
        return true;
    };
    // 0 = matches, 1 = no matches. Anything else (a malformed file, say) is a
    // question we could not answer.
    if !matches!(output.status.code(), Some(0 | 1)) {
        return true;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once(' '))
        .any(|(_, path)| worktree_path.join(path.trim()).join(".git").exists())
}

/// Reap anything left in this repo's trash: leftovers from a reaper that died,
/// or from a `dispose` whose spawn failed. Cheap when there is nothing to do,
/// which is the common case, so callers can run it unconditionally at command
/// start.
pub fn sweep(git_common_dir: &Path) {
    let trash = trash_dir(git_common_dir);
    if !has_reapable_entries(&trash) {
        return;
    }
    schedule_reap(&trash);
}

/// Reap everything in `trash` that git has already disowned. This is the
/// reaper's body; it also runs inline when background work is suppressed.
///
/// Entries still carrying a [sidecar](origin_sidecar) are left strictly alone:
/// their removal never committed, so the worktree may still be the only copy of
/// the user's ignored files and git may still point at its old path.
pub fn reap_now(trash: &Path) {
    if !is_trash_path(trash) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(trash) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_sidecar(&path) {
            // A sidecar whose entry is gone guards nothing; anything else is
            // still doing its job.
            if !guarded_entry(&path).exists() {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        if origin_sidecar(&path).exists() {
            crate::log_debug!(
                "leaving {}: its removal never completed",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(&path) {
            // Leave it for the next sweep rather than retrying here.
            crate::log_debug!("reap of {} failed: {e}", path.display());
        }
    }
    // The trash directory itself stays. Removing it would let a reaper pull the
    // floor out from under a concurrent `dispose` between its `create_dir_all`
    // and its `rename`, which costs that removal the fast path for no reason.
}

/// The entry a sidecar guards: its own path minus the suffix.
fn guarded_entry(sidecar: &Path) -> PathBuf {
    let name = sidecar.as_os_str().to_string_lossy();
    PathBuf::from(name.trim_end_matches(ORIGIN_SUFFIX).to_string())
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

    // Wait for the lock rather than giving up on it. Nobody is blocked on this
    // process, and a reaper that exits on contention would strand the very
    // entry it was spawned for: the reaper already running enumerated the trash
    // before that entry was renamed in, so it will never see it.
    reap_locked(trash, Wait::Block);
    Ok(())
}

/// Whether a reap may wait for the single-flight lock.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// Detached reapers wait: nothing is blocked on them, and giving up would
    /// strand the entry they were spawned for.
    Block,
    /// Foreground reaps do not — the user is waiting, and another process
    /// already draining the trash is a perfectly good outcome.
    Give,
}

/// Drain `trash` while holding the single-flight lock, mirroring
/// `log_clean::run_clean_logs`. Returns `false` when another reaper holds it
/// and we chose not to wait, meaning the trash is being drained but not by us.
///
/// Every reap goes through here, including the inline one and `daft doctor
/// --fix`: two reapers racing over the same directory would each see the
/// other's half-deleted trees and log spurious failures, and back-to-back
/// removals are exactly when a foreground sweep meets a still-running detached
/// reaper.
///
/// The lock sits beside the trash dir rather than inside it, so draining the
/// trash does not delete the lock.
fn reap_locked(trash: &Path, wait: Wait) -> bool {
    let Some(lock_file) = open_lock(trash) else {
        // No lock file to be had. Reaping unlocked risks a noisy race; not
        // reaping at all risks trash that never drains. Prefer the noise.
        reap_now(trash);
        return true;
    };
    use fs2::FileExt;
    let acquired = match wait {
        Wait::Block => lock_file.lock_exclusive().is_ok(),
        Wait::Give => lock_file.try_lock_exclusive().is_ok(),
    };
    if !acquired {
        return false;
    }
    reap_now(trash);
    true
}

fn open_lock(trash: &Path) -> Option<std::fs::File> {
    let lock_path = trash.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .ok()
}

/// Worktrees still awaiting deletion, for `daft doctor`.
pub fn pending(git_common_dir: &Path) -> Vec<PendingEntry> {
    let trash = trash_dir(git_common_dir);
    let Ok(entries) = std::fs::read_dir(&trash) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        // Sidecars describe an entry rather than being one; listing them would
        // report every interrupted removal twice.
        .filter(|path| !is_sidecar(path))
        .map(|path| PendingEntry {
            age: entry_age(&path),
            interrupted: origin_sidecar(&path).exists(),
            path,
        })
        .collect()
}

/// Resolve everything waiting in this repo's trash, on the caller's thread.
///
/// This is `daft doctor --fix`. Unlike a reaper it can act on interrupted
/// removals, because it has a `GitCommand` and somewhere to report: an entry
/// git still points at is *restored* to where git expects it, so a removal cut
/// short becomes a no-op rather than a wedge; one git no longer wants is
/// deleted like any other garbage.
///
/// Returns an error naming what could not be reclaimed, so a fix that did not
/// fix anything cannot report success.
pub fn reclaim(git: &GitCommand, git_common_dir: &Path) -> anyhow::Result<()> {
    let trash = trash_dir(git_common_dir);
    if !trash.exists() {
        return Ok(());
    }
    let lock = open_lock(&trash);
    if let Some(file) = &lock {
        use fs2::FileExt;
        let _ = file.lock_exclusive();
    }

    let mut failures: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(&trash)
        .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default();

    for path in entries {
        if is_sidecar(&path) {
            if !guarded_entry(&path).exists() {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let sidecar = origin_sidecar(&path);
        let origin = std::fs::read(&sidecar).ok().map(|b| path_from_bytes(&b));
        // An interrupted removal that git still points at belongs back where
        // git expects it — deleting it would destroy ignored files the
        // cancelled removal was supposed to leave alone.
        if let Some(origin) = origin
            && !origin.as_os_str().is_empty()
            && !origin.exists()
            && still_registered(git, &origin)
        {
            match std::fs::rename(&path, &origin) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&sidecar);
                }
                Err(e) => failures.push(format!("{}: {e}", origin.display())),
            }
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&sidecar);
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }

    if let Some(file) = &lock {
        let _ = fs2::FileExt::unlock(file);
    }

    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("could not reclaim {}", failures.join("; "))
    }
}

/// Drop git's admin entry for a worktree whose directory is already gone.
///
/// `git worktree remove --force` handles this directly, and the record is
/// verified gone rather than trusted from an exit code. There is deliberately
/// no `git worktree prune` fallback: prune is repo-wide and immediately drops
/// the record of *every* worktree whose directory is currently missing, so a
/// removal here could deregister a colleague's worktree on an unmounted volume.
/// Failing instead costs one slow removal, which the caller handles by
/// restoring the tree and letting git remove it the ordinary way.
fn drop_worktree_record(git: &GitCommand, worktree_path: &Path, canonical: &Path) -> bool {
    git.worktree_remove(worktree_path, true).is_ok() && !still_registered(git, canonical)
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
fn schedule_reap(trash: &Path) -> Disposition {
    // `cfg!(test)` is not redundant with the env gates: under `cargo test`
    // `current_exe()` is the *test* binary, so a spawn would fork the test
    // harness with `__reap-trash` as a filter argument rather than starting a
    // reaper — a stray process per removal, and trash that never drains.
    if cfg!(test)
        || crate::should_skip_background_tasks(crate::cli::argv())
        || std::env::var_os("DAFT_NO_TRASH_REAP").is_some()
    {
        crate::log_debug!("reclaiming inline: background reaping is suppressed");
        return reap_inline(trash);
    }
    if let Err(e) = spawn_reaper(trash) {
        crate::log_debug!("reclaiming inline: could not spawn a reaper: {e}");
        return reap_inline(trash);
    }
    crate::log_debug!("deferred: a detached reaper is reclaiming the space");
    Disposition::Deferred
}

/// Drain the trash on the critical path, reporting what actually happened.
///
/// Losing the lock race is not `Reclaimed`: another reaper holds the trash and
/// may not be finished, so claiming the space is already back would overstate
/// it in exactly the direction that hides a slow removal.
fn reap_inline(trash: &Path) -> Disposition {
    if reap_locked(trash, Wait::Give) {
        Disposition::Reclaimed
    } else {
        Disposition::Deferred
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

/// Is there anything a reaper could actually take? Entries still guarded by a
/// sidecar do not count: spawning a process to walk past them every command
/// would be pure noise.
fn has_reapable_entries(trash: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(trash) else {
        return false;
    };
    entries
        .flatten()
        .map(|e| e.path())
        .any(|p| !is_sidecar(&p) && !origin_sidecar(&p).exists())
}

/// A collision-free name for a trashed worktree. The original name leads so a
/// human reading `daft doctor` output can tell what is waiting to be deleted,
/// and the trailing nanosecond stamp is what [`entry_age`] reads back.
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

/// How long ago an entry was trashed, from the stamp [`entry_name`] wrote.
///
/// The stamp is read from the right, so a worktree whose own name contains
/// hyphens (most of them) parses correctly.
fn entry_age(path: &Path) -> Option<Duration> {
    let name = path.file_name()?.to_str()?;
    let (_, nanos) = name.rsplit_once('-')?;
    let nanos: u64 = nanos.parse().ok()?;
    let trashed = UNIX_EPOCH.checked_add(Duration::from_nanos(nanos))?;
    SystemTime::now().duration_since(trashed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand in for `dispose`'s output without needing a real repo: a trashed
    /// directory whose removal has been committed (no sidecar).
    fn committed_entry(trash: &Path, name: &str) -> PathBuf {
        let entry = trash.join(name);
        std::fs::create_dir_all(entry.join("payload")).unwrap();
        entry
    }

    /// …and one whose removal was interrupted after the rename.
    fn interrupted_entry(trash: &Path, name: &str, origin: &Path) -> PathBuf {
        let entry = committed_entry(trash, name);
        std::fs::write(origin_sidecar(&entry), path_bytes(origin)).unwrap();
        entry
    }

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

    /// A dotted worktree name is why the sidecar is a suffix and not an
    /// extension: `set_extension` would eat the `v2` and guard nothing.
    #[test]
    fn a_sidecar_guards_a_dotted_entry_name() {
        let entry = Path::new("/t/.daft/trash/feature.v2-991-17");
        let sidecar = origin_sidecar(entry);
        assert_eq!(
            sidecar,
            PathBuf::from("/t/.daft/trash/feature.v2-991-17.origin")
        );
        assert!(is_sidecar(&sidecar));
        assert!(!is_sidecar(entry));
        assert_eq!(guarded_entry(&sidecar), entry);
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

    /// The invariant the whole design rests on: an entry whose removal never
    /// committed is not garbage. Git may still point at its old path, and it
    /// may hold ignored files — `.env`, `node_modules/` — that exist nowhere
    /// else, which the dirty probe deliberately does not see.
    #[test]
    fn a_reaper_never_takes_an_uncommitted_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = trash_dir(&tmp.path().join(".git"));
        std::fs::create_dir_all(&trash).unwrap();
        let doomed = committed_entry(&trash, "committed-1-2");
        let spared = interrupted_entry(&trash, "interrupted-1-3", &tmp.path().join("feature"));

        reap_now(&trash);

        assert!(!doomed.exists(), "a committed entry must be reaped");
        assert!(
            spared.join("payload").exists(),
            "an interrupted removal must survive every sweep"
        );
        assert!(
            origin_sidecar(&spared).exists(),
            "its guard must survive too"
        );
    }

    /// A sidecar outliving its entry protects nothing and would otherwise
    /// accumulate forever.
    #[test]
    fn an_orphan_sidecar_is_cleaned_up() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = trash_dir(&tmp.path().join(".git"));
        std::fs::create_dir_all(&trash).unwrap();
        let orphan = origin_sidecar(&trash.join("gone-1-2"));
        std::fs::write(&orphan, path_bytes(Path::new("/nowhere"))).unwrap();

        reap_now(&trash);

        assert!(!orphan.exists());
    }

    /// Removing the trash dir would let a reaper pull the floor out from under
    /// a concurrent `dispose` between its `create_dir_all` and its `rename`.
    #[test]
    fn reaping_leaves_the_trash_dir_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = trash_dir(&tmp.path().join(".git"));
        committed_entry(&trash, "wt-1-2");

        reap_now(&trash);

        assert!(trash.exists(), "the trash dir must outlive its contents");
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

    /// A trash holding only uncommitted entries has nothing a reaper may take,
    /// so a sweep must not spawn one on every command for the rest of time.
    #[test]
    fn sweep_ignores_a_trash_of_only_uncommitted_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        let trash = trash_dir(&git_dir);
        std::fs::create_dir_all(&trash).unwrap();
        interrupted_entry(&trash, "interrupted-1-2", &tmp.path().join("feature"));

        assert!(!has_reapable_entries(&trash));
    }

    #[test]
    fn sweep_without_a_trash_dir_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        sweep(&tmp.path().join(".git"));
    }

    /// Every reap takes the single-flight lock, the inline one included — a
    /// foreground sweep must not race a detached reaper that is still walking
    /// the same tree. Back-to-back removals are when that actually happens.
    #[test]
    fn an_inline_reap_yields_to_a_reaper_already_holding_the_lock() {
        use fs2::FileExt;

        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        let trash = trash_dir(&git_dir);
        std::fs::create_dir_all(trash.join("busy")).unwrap();

        // Stand in for a live reaper holding the lock.
        let held = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(trash.with_extension("lock"))
            .unwrap();
        held.try_lock_exclusive().unwrap();

        // Deferred, not Reclaimed: someone else owns the drain, and it may not
        // be finished. Claiming the space is back would be the overstatement
        // that hides a still-running walk.
        assert_eq!(reap_inline(&trash), Disposition::Deferred);
        assert!(trash.join("busy").exists(), "must not touch a locked trash");

        FileExt::unlock(&held).unwrap();
        assert_eq!(reap_inline(&trash), Disposition::Reclaimed);
        assert!(!trash.join("busy").exists());
    }

    #[test]
    fn entry_names_do_not_collide() {
        let a = entry_name(Path::new("/repo/feature"));
        let b = entry_name(Path::new("/repo/feature"));
        assert_ne!(a, b);
        assert!(a.starts_with("feature-"), "{a}");
    }

    /// Age comes from the stamp in the name, not the directory's mtime —
    /// `rename(2)` carries mtime over from the worktree, so a checkout last
    /// edited a week ago would read as a week stranded the moment it was
    /// removed.
    #[test]
    fn age_is_measured_from_when_the_entry_was_trashed() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = trash_dir(&tmp.path().join(".git"));
        let entry = trash.join(entry_name(Path::new("/repo/my-feature-branch")));
        std::fs::create_dir_all(&entry).unwrap();

        let age = entry_age(&entry).expect("a freshly stamped entry has a readable age");
        assert!(age < Duration::from_secs(5), "{age:?}");

        // A week-old stamp reads as a week, whatever the directory's mtime is.
        let old = trash.join(format!(
            "old-1-{}",
            (SystemTime::now() - Duration::from_secs(7 * 86_400))
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&old).unwrap();
        assert!(entry_age(&old).unwrap() > Duration::from_secs(6 * 86_400));
    }

    #[test]
    fn an_unstamped_entry_has_no_age() {
        assert!(entry_age(Path::new("/t/.daft/trash/handmade")).is_none());
    }

    /// Write a `.gitmodules` declaring `path`, optionally checking it out.
    fn declare_submodule(worktree: &Path, path: &str, populated: bool) {
        std::fs::create_dir_all(worktree).unwrap();
        std::fs::write(
            worktree.join(".gitmodules"),
            format!("[submodule \"s\"]\n\tpath = {path}\n\turl = ./s\n"),
        )
        .unwrap();
        let checkout = worktree.join(path);
        std::fs::create_dir_all(&checkout).unwrap();
        if populated {
            std::fs::write(checkout.join(".git"), "gitdir: ../../.git/modules/s").unwrap();
        }
    }

    /// The overwhelmingly common case, and the one that must stay cheap: no
    /// `.gitmodules`, so the probe is a single `stat` and no subprocess.
    #[test]
    fn a_worktree_without_gitmodules_has_no_submodules() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!has_populated_submodules(tmp.path()));
    }

    /// Git's refusal is keyed on a gitlink whose directory is checked out, so
    /// a `.gitmodules` nobody ran `submodule update` for must not cost the
    /// fast path.
    #[test]
    fn a_declared_but_uninitialized_submodule_does_not_block_the_fast_path() {
        let tmp = tempfile::tempdir().unwrap();
        declare_submodule(tmp.path(), "vendor/lib", false);
        assert!(!has_populated_submodules(tmp.path()));
    }

    /// This is the case git dies on with "working trees containing submodules
    /// cannot be moved or removed". Renaming aside would defeat that check —
    /// git looks for paths that exist, and after the rename none do.
    #[test]
    fn a_populated_submodule_blocks_the_fast_path() {
        let tmp = tempfile::tempdir().unwrap();
        declare_submodule(tmp.path(), "vendor/lib", true);
        assert!(has_populated_submodules(tmp.path()));
    }

    /// A `.gitmodules` git cannot parse is a question we could not answer, and
    /// the safe answer costs a fast path rather than a worktree.
    #[test]
    fn an_unparseable_gitmodules_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitmodules"), "[submodule \"s\"\n\tpath\n").unwrap();
        assert!(has_populated_submodules(tmp.path()));
    }

    #[test]
    fn pending_reports_nothing_when_the_trash_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(pending(&tmp.path().join(".git")).is_empty());
    }

    #[test]
    fn pending_reports_each_waiting_worktree_once() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        let trash = trash_dir(&git_dir);
        std::fs::create_dir_all(&trash).unwrap();
        committed_entry(&trash, "wt-1-2");
        interrupted_entry(&trash, "wt-1-3", &tmp.path().join("feature"));

        let entries = pending(&git_dir);

        // Two entries, not three: the sidecar describes one of them rather
        // than being one itself.
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.age.is_some()));
        assert_eq!(entries.iter().filter(|e| e.interrupted).count(), 1);
    }
}
