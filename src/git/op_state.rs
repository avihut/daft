//! Filesystem probe for in-progress git operations.
//!
//! When a merge / rebase / cherry-pick / revert / bisect pauses awaiting user
//! input, git records that fact as state files inside the worktree's private
//! git directory. This module reads them.
//!
//! Two properties make this the right shape for the perf-sensitive callers
//! (`daft list` runs it once per worktree, including on the live-render seed
//! path):
//!
//! - **No subprocesses.** A handful of `stat()`s plus at most one small file
//!   read per worktree. `git status` reads exactly these files to print
//!   "interactive rebase in progress"; shell prompts do the same.
//! - **Backend-agnostic.** These are file reads either way, so the gitoxide
//!   flip (#733) does not fork this code path. `gix::state::InProgress` models
//!   the same states, but adopting it would still leave `head-name` as a file
//!   read.
//!
//! The load-bearing recovery is [`OpState::branch`]: mid-rebase git detaches
//! HEAD to replay commits, so `git worktree list --porcelain` reports no
//! `branch` line — but `rebase-merge/head-name` still names the branch being
//! rebased, for the entire operation. Recovering it is what keeps a worktree's
//! identity from vanishing in `daft list` (#736).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// An in-progress git operation, identified by the state files git writes into
/// a worktree's private git directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    /// A rebase (either backend — `rebase-merge/` or `rebase-apply/`).
    Rebase,
    /// `git am` — a mailbox apply. Shares `rebase-apply/` with the rebase
    /// backend and is distinguished by the `applying` marker file, so it is
    /// never mislabeled as a rebase.
    Am,
    /// A merge (`MERGE_HEAD`). HEAD stays attached throughout.
    Merge,
    /// A cherry-pick (`CHERRY_PICK_HEAD`). HEAD stays attached.
    CherryPick,
    /// A revert (`REVERT_HEAD`). HEAD stays attached.
    Revert,
    /// A bisect (`BISECT_LOG`).
    Bisect,
}

impl OpKind {
    /// Present-continuous label, as rendered in the `status` column.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rebase => "rebasing",
            Self::Am => "applying",
            Self::Merge => "merging",
            Self::CherryPick => "cherry-picking",
            Self::Revert => "reverting",
            Self::Bisect => "bisecting",
        }
    }

    /// Stable machine-facing name, as emitted in structured output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Am => "am",
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Bisect => "bisect",
        }
    }
}

/// An in-progress operation, plus the branch git recorded it against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpState {
    pub kind: OpKind,
    /// The branch the operation is being applied to, when git records one:
    /// `head-name` for a rebase, `BISECT_START` for a bisect. `None` for
    /// operations that keep HEAD attached (the porcelain already names the
    /// branch), for `git am` (which writes no `head-name`), and whenever the
    /// recorded value is not a `refs/heads/` ref — a rebase started from a
    /// detached HEAD records the literal `detached HEAD`, which names nothing.
    pub branch: Option<String>,
}

/// Resolve the real `.git` directory for `worktree`.
///
/// In the main worktree `.git` is itself a directory. In a linked worktree,
/// `.git` is a file whose first line is `gitdir: <path>` pointing at the
/// per-worktree git dir (e.g. `.git/worktrees/<name>`). Returns an error if
/// the `.git` file is malformed or the resolved directory does not exist.
///
/// This is the canonical resolution for anything that reads per-worktree git
/// state: the operation probe below, the merge module's in-progress detection
/// and intent markers, and the hook conditions. Probing `worktree/.git` as a
/// directory instead silently reads nothing in a linked worktree — daft's
/// default layout — which is the bug this helper exists to prevent.
pub fn resolve_worktree_git_dir(worktree: &Path) -> Result<PathBuf> {
    let git_entry = worktree.join(".git");
    let git_dir = if git_entry.is_file() {
        let content = std::fs::read_to_string(&git_entry)
            .with_context(|| format!("failed to read .git at {}", git_entry.display()))?;
        let rel = content
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("gitdir: "))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "malformed .git file at {}: expected 'gitdir: <path>' on first line",
                    git_entry.display()
                )
            })?
            .trim();
        let p = PathBuf::from(rel);
        // Path::join replaces when its argument is absolute, so this is
        // correct whether the pointer is absolute or relative.
        if p.is_absolute() { p } else { worktree.join(p) }
    } else {
        git_entry
    };

    if !git_dir.is_dir() {
        anyhow::bail!(
            "target worktree at '{}' has no valid .git directory",
            worktree.display()
        );
    }
    Ok(git_dir)
}

/// The in-progress operation in `worktree`, or `None` when it is idle.
///
/// Best-effort by design: a worktree whose `.git` pointer is missing or
/// malformed reports `None` rather than erroring, so one broken entry cannot
/// fail a whole `daft list`. Callers that need the distinction (the merge
/// preflight) resolve the git dir themselves and use
/// [`probe_op_state_in_git_dir`].
pub fn probe_op_state(worktree: &Path) -> Option<OpState> {
    let git_dir = resolve_worktree_git_dir(worktree).ok()?;
    probe_op_state_in_git_dir(&git_dir)
}

/// [`probe_op_state`] against an already-resolved private git directory.
///
/// Probe order follows git's own (`wt-status.c`): `MERGE_HEAD` first, then
/// rebase, then the sequencer states. The orders cannot actually collide — a
/// conflicted rebase writes `REBASE_HEAD` and `rebase-merge/`, never
/// `MERGE_HEAD` — but matching git keeps the classification honest if that
/// ever changes.
pub fn probe_op_state_in_git_dir(git_dir: &Path) -> Option<OpState> {
    if git_dir.join("MERGE_HEAD").exists() {
        return Some(OpState {
            kind: OpKind::Merge,
            branch: None,
        });
    }

    // The merge backend (git's default since 2.26) — the branch being rebased
    // is in `head-name` for the whole operation.
    if git_dir.join("rebase-merge").is_dir() {
        return Some(OpState {
            kind: OpKind::Rebase,
            branch: head_name_branch(&git_dir.join("rebase-merge")),
        });
    }

    // The apply backend shares its directory with `git am`. `applying` is
    // git's own marker for the latter (it is what makes `git status` print
    // "You are in the middle of an am session"), and an am writes no
    // `head-name` — so it recovers no branch.
    if git_dir.join("rebase-apply").is_dir() {
        let dir = git_dir.join("rebase-apply");
        let kind = if dir.join("applying").exists() {
            OpKind::Am
        } else {
            OpKind::Rebase
        };
        return Some(OpState {
            kind,
            branch: head_name_branch(&dir),
        });
    }

    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Some(OpState {
            kind: OpKind::CherryPick,
            branch: None,
        });
    }
    if git_dir.join("REVERT_HEAD").exists() {
        return Some(OpState {
            kind: OpKind::Revert,
            branch: None,
        });
    }
    if git_dir.join("BISECT_LOG").exists() {
        return Some(OpState {
            kind: OpKind::Bisect,
            branch: bisect_start_branch(git_dir),
        });
    }

    None
}

/// The short branch name from `<dir>/head-name`, which git writes as a full
/// `refs/heads/<branch>` ref with a trailing newline. Anything else — the
/// literal `detached HEAD` a detached-start rebase records, a truncated write
/// — names no branch.
fn head_name_branch(dir: &Path) -> Option<String> {
    let raw = read_trimmed(&dir.join("head-name"))?;
    raw.strip_prefix("refs/heads/")
        .filter(|b| !b.is_empty())
        .map(str::to_string)
}

/// The branch a bisect started from, per `BISECT_START`. Git writes either the
/// short branch name or, when the bisect began on a detached HEAD, a commit
/// SHA — which names no branch.
fn bisect_start_branch(git_dir: &Path) -> Option<String> {
    let raw = read_trimmed(&git_dir.join("BISECT_START"))?;
    // Defensive: accept a fully-qualified ref too, though git writes the short
    // form here (unlike `head-name`).
    let name = raw.strip_prefix("refs/heads/").unwrap_or(&raw);
    if name.is_empty() || is_hex_sha(name) {
        return None;
    }
    Some(name.to_string())
}

fn is_hex_sha(s: &str) -> bool {
    s.len() >= 40 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Read a small state file, trimming the trailing newline git writes. `None`
/// when the file is absent or unreadable — every caller treats that as "git
/// recorded nothing", never as an error.
fn read_trimmed(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A main-worktree shape: `.git` is a real directory.
    fn main_worktree() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        (tmp, git_dir)
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn rebase_recovers_branch_from_head_name() {
        let (tmp, git_dir) = main_worktree();
        // Exactly what git writes: a full ref with a trailing newline.
        write(
            &git_dir.join("rebase-merge/head-name"),
            "refs/heads/feat/x\n",
        );
        assert_eq!(
            probe_op_state(tmp.path()),
            Some(OpState {
                kind: OpKind::Rebase,
                branch: Some("feat/x".to_string()),
            })
        );
    }

    #[test]
    fn rebase_without_head_name_still_reports_the_operation() {
        let (tmp, git_dir) = main_worktree();
        std::fs::create_dir_all(git_dir.join("rebase-merge")).unwrap();
        assert_eq!(
            probe_op_state(tmp.path()),
            Some(OpState {
                kind: OpKind::Rebase,
                branch: None,
            })
        );
    }

    #[test]
    fn detached_start_rebase_recovers_no_branch() {
        // `git rebase` from a detached HEAD records the literal string rather
        // than a ref — it names nothing, but the operation is still real.
        let (tmp, git_dir) = main_worktree();
        write(&git_dir.join("rebase-merge/head-name"), "detached HEAD\n");
        let state = probe_op_state(tmp.path()).unwrap();
        assert_eq!(state.kind, OpKind::Rebase);
        assert_eq!(state.branch, None);
    }

    #[test]
    fn empty_head_name_recovers_no_branch() {
        let (tmp, git_dir) = main_worktree();
        write(&git_dir.join("rebase-merge/head-name"), "refs/heads/\n");
        assert_eq!(probe_op_state(tmp.path()).unwrap().branch, None);
    }

    #[test]
    fn rebase_apply_backend_is_a_rebase() {
        let (tmp, git_dir) = main_worktree();
        write(
            &git_dir.join("rebase-apply/head-name"),
            "refs/heads/legacy\n",
        );
        assert_eq!(
            probe_op_state(tmp.path()),
            Some(OpState {
                kind: OpKind::Rebase,
                branch: Some("legacy".to_string()),
            })
        );
    }

    #[test]
    fn am_session_is_not_a_rebase() {
        // `git am` shares rebase-apply/ but writes `applying` and no head-name.
        let (tmp, git_dir) = main_worktree();
        write(&git_dir.join("rebase-apply/applying"), "");
        assert_eq!(
            probe_op_state(tmp.path()),
            Some(OpState {
                kind: OpKind::Am,
                branch: None,
            })
        );
    }

    #[test]
    fn merge_is_detected_and_names_no_branch() {
        // A merge keeps HEAD attached, so the porcelain already has the name.
        let (tmp, git_dir) = main_worktree();
        write(&git_dir.join("MERGE_HEAD"), "deadbeef\n");
        assert_eq!(
            probe_op_state(tmp.path()),
            Some(OpState {
                kind: OpKind::Merge,
                branch: None,
            })
        );
    }

    #[test]
    fn cherry_pick_and_revert_are_distinguished() {
        let (tmp, git_dir) = main_worktree();
        write(&git_dir.join("CHERRY_PICK_HEAD"), "c0ffee\n");
        assert_eq!(probe_op_state(tmp.path()).unwrap().kind, OpKind::CherryPick);

        let (tmp2, git_dir2) = main_worktree();
        write(&git_dir2.join("REVERT_HEAD"), "c0ffee\n");
        assert_eq!(probe_op_state(tmp2.path()).unwrap().kind, OpKind::Revert);
    }

    #[test]
    fn bisect_recovers_the_original_branch() {
        let (tmp, git_dir) = main_worktree();
        write(&git_dir.join("BISECT_LOG"), "git bisect start\n");
        write(&git_dir.join("BISECT_START"), "main\n");
        assert_eq!(
            probe_op_state(tmp.path()),
            Some(OpState {
                kind: OpKind::Bisect,
                branch: Some("main".to_string()),
            })
        );
    }

    #[test]
    fn bisect_from_detached_head_recovers_no_branch() {
        let (tmp, git_dir) = main_worktree();
        write(&git_dir.join("BISECT_LOG"), "git bisect start\n");
        write(
            &git_dir.join("BISECT_START"),
            "1234567890abcdef1234567890abcdef12345678\n",
        );
        assert_eq!(probe_op_state(tmp.path()).unwrap().branch, None);
    }

    #[test]
    fn clean_worktree_reports_no_operation() {
        let (tmp, _git_dir) = main_worktree();
        assert_eq!(probe_op_state(tmp.path()), None);
    }

    // ── linked-worktree shapes (`.git` is a FILE) ────────────────────────────
    // daft's default layout. Probing `worktree/.git` as a directory silently
    // finds nothing here, which is exactly the class of bug this covers.

    /// A linked worktree: `.git` is a file pointing at `<common>/worktrees/<id>`.
    /// Returns (tempdir, worktree path, private git dir).
    fn linked_worktree(absolute_pointer: bool) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("repo/.git");
        let private = common.join("worktrees/wt-a");
        std::fs::create_dir_all(&private).unwrap();
        let worktree = tmp.path().join("wt-a");
        std::fs::create_dir_all(&worktree).unwrap();
        let pointer = if absolute_pointer {
            private.display().to_string()
        } else {
            // Git writes an absolute path, but the resolver accepts a relative
            // one too — pin that so the fallback can't rot.
            "../repo/.git/worktrees/wt-a".to_string()
        };
        std::fs::write(worktree.join(".git"), format!("gitdir: {pointer}\n")).unwrap();
        (tmp, worktree, private)
    }

    #[test]
    fn linked_worktree_with_absolute_pointer_is_probed() {
        let (_tmp, worktree, private) = linked_worktree(true);
        write(
            &private.join("rebase-merge/head-name"),
            "refs/heads/feat/y\n",
        );
        assert_eq!(
            probe_op_state(&worktree),
            Some(OpState {
                kind: OpKind::Rebase,
                branch: Some("feat/y".to_string()),
            })
        );
    }

    #[test]
    fn linked_worktree_with_relative_pointer_is_probed() {
        let (_tmp, worktree, private) = linked_worktree(false);
        write(&private.join("MERGE_HEAD"), "deadbeef\n");
        assert_eq!(probe_op_state(&worktree).unwrap().kind, OpKind::Merge);
    }

    #[test]
    fn linked_worktree_resolves_to_its_private_git_dir() {
        let (_tmp, worktree, private) = linked_worktree(true);
        assert_eq!(
            resolve_worktree_git_dir(&worktree).unwrap(),
            private,
            "the resolved dir is the private gitdir, not the common dir"
        );
    }

    #[test]
    fn malformed_git_file_errors_but_probe_degrades_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "not a gitdir pointer\n").unwrap();

        assert!(resolve_worktree_git_dir(&worktree).is_err());
        // Best-effort: one broken worktree must not fail a whole `daft list`.
        assert_eq!(probe_op_state(&worktree), None);
    }

    #[test]
    fn dangling_gitdir_pointer_degrades_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            "gitdir: /nonexistent/worktrees/gone\n",
        )
        .unwrap();

        assert!(resolve_worktree_git_dir(&worktree).is_err());
        assert_eq!(probe_op_state(&worktree), None);
    }
}
