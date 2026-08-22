//! Reasons a layout transform must not start — all of them, found up front.
//!
//! A transform carries a working tree's git state across a role change by
//! moving it. The only states it will not carry are the ones git itself is in
//! the middle of (a paused rebase, merge, cherry-pick, revert or bisect), the
//! ones where another git process holds the index, and the ones git's own
//! `worktree move` refuses (a locked worktree, populated submodules). Each is
//! a [`Blocker`]: structured, so the CLI can say where it is, why it blocks,
//! and exactly how to settle it — and collected rather than short-circuited,
//! so a user settles everything in one pass. `--dry-run` runs the same probe.
//!
//! Pure data and filesystem reads. Nothing here prints.

use std::path::{Path, PathBuf};

use crate::core::worktree::list::ChangedFiles;
use crate::git::op_state::{OpKind, probe_op_state_in_git_dir, resolve_worktree_git_dir};
use crate::git::worktree_state::find_registration;

/// Why a worktree was probed — the CLI phrases the two differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeReason {
    /// The pivot, whose role changes (main working tree ⇄ linked worktree).
    RoleChange,
    /// A linked worktree that `git worktree move` will relocate.
    Relocation,
}

/// `n of m` for an operation that reports progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpProgress {
    pub done: usize,
    pub total: usize,
}

/// A worktree that could take the repository root, offered to the user.
#[derive(Debug, Clone)]
pub struct PivotCandidate {
    pub branch: String,
    pub path: PathBuf,
    pub counts: ChangedFiles,
}

/// What blocks.
#[derive(Debug, Clone)]
pub enum BlockerKind {
    /// A paused rebase / am / merge / cherry-pick / revert / bisect.
    OperationInProgress {
        op: OpKind,
        progress: Option<OpProgress>,
        /// The state file or directory git keeps it in, relative to the
        /// private git dir (`rebase-merge`, `MERGE_HEAD`, …).
        marker: &'static str,
    },
    /// `index.lock` (or `HEAD.lock`) — another git process holds the worktree.
    IndexLocked { lock: PathBuf },
    /// Populated submodules: their `.git` pointers are relative to a git dir
    /// the move invalidates (and `git worktree move` refuses them outright).
    Submodules { paths: Vec<PathBuf> },
    /// `git worktree lock` — `git worktree move` refuses without `--force`.
    RegistrationLocked { reason: Option<String> },
    /// A bare repository's root role is a choice; nothing was supplied to make
    /// it with.
    MissingPivot { candidates: Vec<PivotCandidate> },
    /// A detached main working tree has no branch to name its directory.
    MissingAs {
        /// The sha-derived default name.
        derived: String,
        /// The commit HEAD is detached at (full oid).
        commit: String,
    },
    /// A move across volumes is a copy; it needs a yes that was not given.
    NeedsCopyConfirm {
        bytes: u64,
        moves: Vec<(PathBuf, PathBuf)>,
    },
}

/// One reason the transform cannot proceed, with everything needed to render
/// condition + where / why / settle commands.
#[derive(Debug, Clone)]
pub struct Blocker {
    pub kind: BlockerKind,
    /// The worktree the blocker is about, at its *current* path. `None` for
    /// repository-wide conditions.
    pub worktree_path: Option<PathBuf>,
    /// The private git dir the evidence was read from.
    pub git_dir: Option<PathBuf>,
    pub branch: Option<String>,
    pub reason: Option<ProbeReason>,
}

impl Blocker {
    /// A repository-wide blocker (no particular worktree).
    pub fn repo_wide(kind: BlockerKind) -> Self {
        Self {
            kind,
            worktree_path: None,
            git_dir: None,
            branch: None,
            reason: None,
        }
    }

    /// Whether this is a state the user has to settle in git before retrying
    /// (as opposed to a decision or confirmation daft needs from them).
    pub fn is_settle_first(&self) -> bool {
        matches!(
            self.kind,
            BlockerKind::OperationInProgress { .. }
                | BlockerKind::IndexLocked { .. }
                | BlockerKind::Submodules { .. }
                | BlockerKind::RegistrationLocked { .. }
        )
    }
}

/// Probe worktrees whose **role changes** for the states a role change cannot
/// carry: an in-progress operation, a held index lock, populated submodules.
///
/// `worktrees` are `(path, branch)`; every blocker found is returned.
pub fn role_change_blockers(worktrees: &[(PathBuf, Option<String>)]) -> Vec<Blocker> {
    let mut out = Vec::new();
    for (path, branch) in worktrees {
        let Ok(git_dir) = resolve_worktree_git_dir(path) else {
            continue;
        };
        let mk = |kind: BlockerKind| Blocker {
            kind,
            worktree_path: Some(path.clone()),
            git_dir: Some(git_dir.clone()),
            branch: branch.clone(),
            reason: Some(ProbeReason::RoleChange),
        };
        if let Some(state) = probe_op_state_in_git_dir(&git_dir) {
            out.push(mk(BlockerKind::OperationInProgress {
                op: state.kind,
                progress: op_progress(&git_dir, state.kind),
                marker: op_marker(&git_dir, state.kind),
            }));
        }
        if let Some(lock) = held_lock(&git_dir) {
            out.push(mk(BlockerKind::IndexLocked { lock }));
        }
        let subs = populated_submodules(path);
        if !subs.is_empty() {
            out.push(mk(BlockerKind::Submodules { paths: subs }));
        }
    }
    out
}

/// Probe linked worktrees that **move** for what `git worktree move` refuses:
/// a locked registration, and submodules (git's own rule: a `modules/`
/// directory in the registration, or any populated gitlink).
pub fn relocation_blockers(
    common_dir: &Path,
    moving: &[(PathBuf, Option<String>)],
) -> Vec<Blocker> {
    let mut out = Vec::new();
    for (path, branch) in moving {
        let registration = find_registration(common_dir, path);
        let git_dir = registration
            .clone()
            .or_else(|| resolve_worktree_git_dir(path).ok());
        let mk = |kind: BlockerKind| Blocker {
            kind,
            worktree_path: Some(path.clone()),
            git_dir: git_dir.clone(),
            branch: branch.clone(),
            reason: Some(ProbeReason::Relocation),
        };
        if let Some(reg) = &registration {
            let locked = reg.join("locked");
            if locked.exists() {
                let reason = std::fs::read_to_string(&locked)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                out.push(mk(BlockerKind::RegistrationLocked { reason }));
            }
        }
        let mut subs = populated_submodules(path);
        if subs.is_empty()
            && let Some(reg) = &registration
            && reg.join("modules").is_dir()
        {
            // git's cheap check: a `modules/` dir in the worktree's git dir,
            // even a stale one from a deinit'd submodule, makes `worktree
            // move` refuse.
            subs.push(PathBuf::from(".git/modules"));
        }
        if !subs.is_empty() {
            out.push(mk(BlockerKind::Submodules { paths: subs }));
        }
    }
    out
}

/// The lock file another git process holds in `git_dir`, if any.
fn held_lock(git_dir: &Path) -> Option<PathBuf> {
    ["index.lock", "HEAD.lock"]
        .iter()
        .map(|l| git_dir.join(l))
        .find(|p| p.exists())
}

/// Git's own progress counters for the operations that keep them.
fn op_progress(git_dir: &Path, kind: OpKind) -> Option<OpProgress> {
    let read_num =
        |p: PathBuf| -> Option<usize> { std::fs::read_to_string(p).ok()?.trim().parse().ok() };
    match kind {
        OpKind::Rebase if git_dir.join("rebase-merge").is_dir() => Some(OpProgress {
            done: read_num(git_dir.join("rebase-merge/msgnum"))?,
            total: read_num(git_dir.join("rebase-merge/end"))?,
        }),
        OpKind::Rebase | OpKind::Am => Some(OpProgress {
            done: read_num(git_dir.join("rebase-apply/next"))?,
            total: read_num(git_dir.join("rebase-apply/last"))?,
        }),
        OpKind::CherryPick | OpKind::Revert => {
            // A multi-commit sequence keeps its remaining picks in
            // `sequencer/todo`; a single pick has no sequencer at all.
            let todo = std::fs::read_to_string(git_dir.join("sequencer/todo")).ok()?;
            let remaining = todo
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .count();
            Some(OpProgress {
                done: 0,
                total: remaining,
            })
        }
        OpKind::Merge | OpKind::Bisect => None,
    }
}

/// The state file or directory that marks the operation, for the report.
fn op_marker(git_dir: &Path, kind: OpKind) -> &'static str {
    match kind {
        OpKind::Rebase if git_dir.join("rebase-merge").is_dir() => "rebase-merge",
        OpKind::Rebase | OpKind::Am => "rebase-apply",
        OpKind::Merge => "MERGE_HEAD",
        OpKind::CherryPick => "CHERRY_PICK_HEAD",
        OpKind::Revert => "REVERT_HEAD",
        OpKind::Bisect => "BISECT_LOG",
    }
}

/// Gitlinks in `worktree`'s index whose submodule is checked out (has a
/// `.git` inside) — git's `validate_no_submodules` notion of "populated".
pub(crate) fn populated_submodules(worktree: &Path) -> Vec<PathBuf> {
    let Ok(out) = crate::utils::git_command_at(worktree)
        .args(["ls-files", "--stage", "-z"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout
        .split('\0')
        .filter(|entry| entry.starts_with("160000 "))
        .filter_map(|entry| entry.split_once('\t').map(|(_, path)| PathBuf::from(path)))
        .filter(|rel| worktree.join(rel).join(".git").symlink_metadata().is_ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    /// A main-worktree shape: `.git` is a real directory.
    fn main_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        let git_dir = wt.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        (tmp, wt, git_dir)
    }

    fn probe(wt: &Path) -> Vec<Blocker> {
        role_change_blockers(&[(wt.to_path_buf(), Some("feature/x".into()))])
    }

    #[test]
    fn rebase_merge_blocks_with_msgnum_progress() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(
            &git_dir.join("rebase-merge/head-name"),
            "refs/heads/feature/x\n",
        );
        write(&git_dir.join("rebase-merge/msgnum"), "3\n");
        write(&git_dir.join("rebase-merge/end"), "7\n");
        let blockers = probe(&wt);
        assert_eq!(blockers.len(), 1);
        match &blockers[0].kind {
            BlockerKind::OperationInProgress {
                op,
                progress,
                marker,
            } => {
                assert_eq!(*op, OpKind::Rebase);
                assert_eq!(*progress, Some(OpProgress { done: 3, total: 7 }));
                assert_eq!(*marker, "rebase-merge");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(blockers[0].reason, Some(ProbeReason::RoleChange));
        assert_eq!(blockers[0].worktree_path.as_deref(), Some(wt.as_path()));
    }

    #[test]
    fn rebase_apply_progress_uses_next_and_last() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(
            &git_dir.join("rebase-apply/head-name"),
            "refs/heads/feature/x\n",
        );
        write(&git_dir.join("rebase-apply/next"), "2\n");
        write(&git_dir.join("rebase-apply/last"), "5\n");
        let blockers = probe(&wt);
        match &blockers[0].kind {
            BlockerKind::OperationInProgress {
                op,
                progress,
                marker,
            } => {
                assert_eq!(*op, OpKind::Rebase);
                assert_eq!(*progress, Some(OpProgress { done: 2, total: 5 }));
                assert_eq!(*marker, "rebase-apply");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn am_session_is_applying_not_rebasing() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(&git_dir.join("rebase-apply/applying"), "");
        let blockers = probe(&wt);
        assert!(matches!(
            blockers[0].kind,
            BlockerKind::OperationInProgress { op: OpKind::Am, .. }
        ));
    }

    #[test]
    fn sequencer_todo_counts_only_real_picks() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(&git_dir.join("CHERRY_PICK_HEAD"), "abc\n");
        write(
            &git_dir.join("sequencer/todo"),
            "pick 1111 one\n# a comment\n\npick 2222 two\n",
        );
        let blockers = probe(&wt);
        match &blockers[0].kind {
            BlockerKind::OperationInProgress {
                op,
                progress,
                marker,
            } => {
                assert_eq!(*op, OpKind::CherryPick);
                assert_eq!(*progress, Some(OpProgress { done: 0, total: 2 }));
                assert_eq!(*marker, "CHERRY_PICK_HEAD");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn merge_revert_and_bisect_each_block() {
        for (file, op, marker) in [
            ("MERGE_HEAD", OpKind::Merge, "MERGE_HEAD"),
            ("REVERT_HEAD", OpKind::Revert, "REVERT_HEAD"),
            ("BISECT_LOG", OpKind::Bisect, "BISECT_LOG"),
        ] {
            let (_tmp, wt, git_dir) = main_worktree();
            write(&git_dir.join(file), "x\n");
            let blockers = probe(&wt);
            assert_eq!(blockers.len(), 1, "{file}");
            match &blockers[0].kind {
                BlockerKind::OperationInProgress {
                    op: got, marker: m, ..
                } => {
                    assert_eq!(*got, op);
                    assert_eq!(*m, marker);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn index_lock_blocks_and_names_the_lock_path() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(&git_dir.join("index.lock"), "");
        let blockers = probe(&wt);
        assert_eq!(blockers.len(), 1);
        match &blockers[0].kind {
            BlockerKind::IndexLocked { lock } => assert_eq!(lock, &git_dir.join("index.lock")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn an_idle_worktree_has_no_blockers() {
        let (_tmp, wt, _git_dir) = main_worktree();
        assert!(probe(&wt).is_empty());
    }

    #[test]
    fn every_blocker_is_reported_not_just_the_first() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(&git_dir.join("MERGE_HEAD"), "x\n");
        write(&git_dir.join("index.lock"), "");
        let tmp2 = tempfile::tempdir().unwrap();
        let wt2 = tmp2.path().join("other");
        std::fs::create_dir_all(wt2.join(".git/rebase-merge")).unwrap();
        let blockers = role_change_blockers(&[
            (wt.clone(), Some("a".into())),
            (wt2.clone(), Some("b".into())),
        ]);
        assert_eq!(blockers.len(), 3);
        assert!(blockers.iter().all(|b| b.is_settle_first()));
    }

    #[test]
    fn a_linked_worktree_is_probed_through_its_pointer_file() {
        // `.git` is a *file* naming the private dir — the shape a naive
        // `wt/.git/rebase-merge` probe silently misses.
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("repo/.git");
        let reg = common.join("worktrees/x");
        std::fs::create_dir_all(reg.join("rebase-merge")).unwrap();
        let wt = tmp.path().join("repo/x");
        std::fs::create_dir_all(&wt).unwrap();
        write(&wt.join(".git"), &format!("gitdir: {}\n", reg.display()));
        let blockers = probe(&wt);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].git_dir.as_deref(), Some(reg.as_path()));
    }

    #[test]
    fn locked_registration_blocks_a_moving_worktree_and_carries_the_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("repo/.git");
        let reg = common.join("worktrees/x");
        std::fs::create_dir_all(&reg).unwrap();
        let wt = tmp.path().join("repo/x");
        std::fs::create_dir_all(&wt).unwrap();
        write(
            &reg.join("gitdir"),
            &format!("{}\n", wt.join(".git").display()),
        );
        write(&wt.join(".git"), &format!("gitdir: {}\n", reg.display()));
        write(&reg.join("locked"), "running a long build\n");
        let blockers = relocation_blockers(&common, &[(wt.clone(), Some("x".into()))]);
        assert_eq!(blockers.len(), 1);
        match &blockers[0].kind {
            BlockerKind::RegistrationLocked { reason } => {
                assert_eq!(reason.as_deref(), Some("running a long build"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(blockers[0].reason, Some(ProbeReason::Relocation));
    }

    #[test]
    fn a_modules_dir_in_the_registration_blocks_a_move_like_git_does() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("repo/.git");
        let reg = common.join("worktrees/x");
        std::fs::create_dir_all(reg.join("modules/sub")).unwrap();
        let wt = tmp.path().join("repo/x");
        std::fs::create_dir_all(&wt).unwrap();
        write(
            &reg.join("gitdir"),
            &format!("{}\n", wt.join(".git").display()),
        );
        write(&wt.join(".git"), &format!("gitdir: {}\n", reg.display()));
        let blockers = relocation_blockers(&common, &[(wt.clone(), Some("x".into()))]);
        assert!(matches!(blockers[0].kind, BlockerKind::Submodules { .. }));
    }

    #[test]
    fn an_unlocked_moving_worktree_has_no_blockers() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("repo/.git");
        let reg = common.join("worktrees/x");
        std::fs::create_dir_all(&reg).unwrap();
        let wt = tmp.path().join("repo/x");
        std::fs::create_dir_all(&wt).unwrap();
        write(
            &reg.join("gitdir"),
            &format!("{}\n", wt.join(".git").display()),
        );
        write(&wt.join(".git"), &format!("gitdir: {}\n", reg.display()));
        assert!(relocation_blockers(&common, &[(wt.clone(), Some("x".into()))]).is_empty());
    }
}
