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
    /// A path the transform has to create is already taken. Merging into it
    /// would mix two trees, and the undo cannot tell them apart afterwards.
    DestinationOccupied { destination: PathBuf },
    /// A probe could not read what it needed. Unknown is not clear: the safe
    /// branch is to stop and say what could not be established.
    Unreadable { what: String, detail: String },
    /// A directory the transform renames would have to cross a filesystem
    /// boundary, where `rename(2)` cannot go.
    CrossesVolumes { from: PathBuf, to: PathBuf },
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
                | BlockerKind::DestinationOccupied { .. }
                | BlockerKind::Unreadable { .. }
                | BlockerKind::CrossesVolumes { .. }
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
        let bare = |kind: BlockerKind, git_dir: Option<PathBuf>| Blocker {
            kind,
            worktree_path: Some(path.clone()),
            git_dir,
            branch: branch.clone(),
            reason: Some(ProbeReason::RoleChange),
        };
        // A git dir that cannot be resolved is not an idle worktree: it is a
        // worktree whose state daft cannot see. Skipping it silently reported
        // "clear" for the case least likely to be clear.
        match resolve_worktree_git_dir(path) {
            Ok(git_dir) => {
                let mk = |kind: BlockerKind| bare(kind, Some(git_dir.clone()));
                if let Some(state) = probe_op_state_in_git_dir(&git_dir) {
                    out.push(mk(BlockerKind::OperationInProgress {
                        op: state.kind,
                        progress: op_progress(&git_dir, state.kind),
                        marker: op_marker(&git_dir, state.kind),
                    }));
                }
                // Every lock, not the first: the report's contract is one pass.
                for lock in held_locks(&git_dir) {
                    out.push(mk(BlockerKind::IndexLocked { lock }));
                }
            }
            Err(e) => out.push(bare(
                BlockerKind::Unreadable {
                    what: "this worktree's git directory".to_string(),
                    detail: format!("{e:#}"),
                },
                None,
            )),
        }
        // Independent of the git dir — and the probe most likely to fail for
        // the same reason a role change is unsafe, so its failure is reported
        // rather than read as "no submodules".
        match populated_submodules(path) {
            Ok(subs) if !subs.is_empty() => {
                out.push(bare(BlockerKind::Submodules { paths: subs }, None));
            }
            Ok(_) => {}
            Err(detail) => out.push(bare(
                BlockerKind::Unreadable {
                    what: "whether this worktree has populated submodules".to_string(),
                    detail,
                },
                None,
            )),
        }
    }
    out
}

/// The registration facts a *role change* dissolves without asking: a
/// `git worktree lock` marker, and a `modules/` directory left by a submodule.
///
/// `relocation_blockers` finds these for the linked worktrees that move; the
/// pivot is not one of those (it is the worktree whose role changes), so
/// without this it was the only worktree never checked — and its lock was
/// silently dissolved along with its registration.
pub fn pivot_registration_blockers(
    common_dir: &Path,
    path: &Path,
    branch: Option<&str>,
) -> Vec<Blocker> {
    registration_state_blockers(common_dir, path, branch, ProbeReason::RoleChange)
}

/// `locked` and `modules/` on the registration of `path`, if it has one.
fn registration_state_blockers(
    common_dir: &Path,
    path: &Path,
    branch: Option<&str>,
    reason: ProbeReason,
) -> Vec<Blocker> {
    let mut out = Vec::new();
    let Some(reg) = find_registration(common_dir, path) else {
        return out;
    };
    let mk = |kind: BlockerKind| Blocker {
        kind,
        worktree_path: Some(path.to_path_buf()),
        git_dir: Some(reg.clone()),
        branch: branch.map(str::to_string),
        reason: Some(reason),
    };
    let locked = reg.join("locked");
    if locked.exists() {
        let why = std::fs::read_to_string(&locked)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(mk(BlockerKind::RegistrationLocked { reason: why }));
    }
    if reg.join("modules").is_dir() {
        // git's cheap check: a `modules/` dir in the worktree's git dir, even
        // a stale one from a deinit'd submodule, makes `worktree move` refuse.
        out.push(mk(BlockerKind::Submodules {
            paths: vec![PathBuf::from(".git/modules")],
        }));
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
        let git_dir =
            find_registration(common_dir, path).or_else(|| resolve_worktree_git_dir(path).ok());
        let mk = |kind: BlockerKind| Blocker {
            kind,
            worktree_path: Some(path.clone()),
            git_dir: git_dir.clone(),
            branch: branch.clone(),
            reason: Some(ProbeReason::Relocation),
        };
        let from_registration = registration_state_blockers(
            common_dir,
            path,
            branch.as_deref(),
            ProbeReason::Relocation,
        );
        let already_named_modules = from_registration
            .iter()
            .any(|b| matches!(b.kind, BlockerKind::Submodules { .. }));
        out.extend(from_registration);
        match populated_submodules(path) {
            Ok(subs) if !subs.is_empty() => out.push(mk(BlockerKind::Submodules { paths: subs })),
            Ok(_) => {}
            Err(_) if already_named_modules => {}
            Err(detail) => out.push(mk(BlockerKind::Unreadable {
                what: "whether this worktree has populated submodules".to_string(),
                detail,
            })),
        }
    }
    out
}

/// Destinations the plan has to create that something already occupies.
///
/// The execution-time guards refuse these too, but only after earlier ops have
/// run — and a `NestFromRoot` that merges into an occupied directory cannot be
/// undone, because its recorded inverse moves *everything* back out, including
/// what was already there. Reporting it as a blocker keeps the refusal on the
/// side of the plan where nothing has moved yet.
pub fn destination_blockers(destinations: &[(PathBuf, Option<String>)]) -> Vec<Blocker> {
    destinations
        .iter()
        .filter(|(dest, _)| occupied(dest))
        .map(|(dest, branch)| Blocker {
            kind: BlockerKind::DestinationOccupied {
                destination: dest.clone(),
            },
            worktree_path: None,
            git_dir: None,
            branch: branch.clone(),
            reason: None,
        })
        .collect()
}

/// Whether something is in the way at `path`.
///
/// An *empty* directory is not in the way: `rename(2)` replaces one, and
/// `exec_nest_from_root` accepts one (a previous operation in the same plan may
/// have created the parent chain). A blocker stricter than the executor it
/// protects would refuse transforms that work.
fn occupied(path: &Path) -> bool {
    match std::fs::read_dir(path) {
        // A directory: in the way only if it holds something.
        Ok(mut entries) => entries.next().is_some(),
        // Not a directory, or unreadable — `symlink_metadata` separates "a file
        // or symlink is here" from "nothing is here".
        Err(_) => path.symlink_metadata().is_ok(),
    }
}

/// Renames the plan performs that would have to cross a filesystem boundary.
///
/// Only linked-worktree moves have a copy path; the root-role operations
/// (nesting, collapsing, renaming a main working tree, moving `.git`) are
/// `rename(2)` and nothing else, so a boundary between source and destination
/// has to be refused before they start rather than discovered half-way.
pub fn volume_blockers(renames: &[(PathBuf, PathBuf)]) -> Vec<Blocker> {
    renames
        .iter()
        .filter(|(from, to)| {
            crate::core::fs_volume::strategy_for(from, to)
                != crate::core::fs_volume::MoveStrategy::Rename
        })
        .map(|(from, to)| Blocker {
            kind: BlockerKind::CrossesVolumes {
                from: from.clone(),
                to: to.clone(),
            },
            worktree_path: Some(from.clone()),
            git_dir: None,
            branch: None,
            reason: None,
        })
        .collect()
}

/// Every lock file another git process holds in `git_dir`.
///
/// All of them, not the first: a report that promises every condition in one
/// pass cannot send the user back for a second lock it already saw.
fn held_locks(git_dir: &Path) -> Vec<PathBuf> {
    ["index.lock", "HEAD.lock"]
        .iter()
        .map(|l| git_dir.join(l))
        .filter(|p| p.exists())
        .collect()
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
pub(crate) fn populated_submodules(worktree: &Path) -> Result<Vec<PathBuf>, String> {
    let out = crate::utils::git_command_at(worktree)
        .args(["ls-files", "--stage", "-z"])
        .output()
        .map_err(|e| format!("running git ls-files in {}: {e}", worktree.display()))?;
    if !out.status.success() {
        // The most likely reason `ls-files` fails is an `index.lock` held by
        // another git — exactly the state that makes a role change unsafe. An
        // empty list here would be a false "no submodules".
        return Err(format!(
            "git ls-files failed in {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout
        .split('\0')
        .filter(|entry| entry.starts_with("160000 "))
        .filter_map(|entry| entry.split_once('\t').map(|(_, path)| PathBuf::from(path)))
        .filter(|rel| worktree.join(rel).join(".git").symlink_metadata().is_ok())
        .collect())
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

    /// Run git in `dir` with a fixed test identity — never global config.
    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = crate::utils::git_command_at(dir)
            // A developer's global commit.gpgsign=true would route every
            // fixture commit through gpg and fail the suite for reasons that
            // have nothing to do with the probe under test.
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A **real** repository whose main working tree is the shape the probes
    /// read. Real, not a hand-built `.git` directory: the submodule probe runs
    /// `git ls-files` and fails closed, so a fixture git cannot open would test
    /// the failure path for every probe rather than the probe under test.
    fn main_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        git_ok(&wt, &["init", "-q", "."]);
        git_ok(&wt, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        let git_dir = wt.join(".git");
        (tmp, wt, git_dir)
    }

    /// A real repository with one real linked worktree: `(tmp, common, wt,
    /// registration)`. `wt/.git` is a *file* naming the registration — the
    /// shape a naive `wt/.git/<marker>` probe silently misses.
    fn linked_worktree() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        // Canonical: git records the resolved path in the pointer file, and
        // macOS puts tempdirs behind the /var -> /private/var symlink.
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        git_ok(&root, &["init", "-q", "."]);
        git_ok(&root, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        let wt = base.join("x");
        git_ok(
            &root,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "x"],
        );
        let common = root.join(".git");
        let registration = common.join("worktrees/x");
        assert!(
            registration.is_dir(),
            "git names the registration after the path basename"
        );
        (tmp, common, wt, registration)
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
    fn a_sequencer_todo_counts_the_remaining_picks() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(&git_dir.join("CHERRY_PICK_HEAD"), "abc\n");
        write(
            &git_dir.join("sequencer/todo"),
            "# comment\n\npick aaa one\npick bbb two\n",
        );
        let blockers = probe(&wt);
        match &blockers[0].kind {
            BlockerKind::OperationInProgress { op, progress, .. } => {
                assert_eq!(*op, OpKind::CherryPick);
                assert_eq!(*progress, Some(OpProgress { done: 0, total: 2 }));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn merge_revert_and_bisect_each_block() {
        for (marker, expect) in [
            ("MERGE_HEAD", OpKind::Merge),
            ("REVERT_HEAD", OpKind::Revert),
            ("BISECT_LOG", OpKind::Bisect),
        ] {
            let (_tmp, wt, git_dir) = main_worktree();
            write(&git_dir.join(marker), "x\n");
            let blockers = probe(&wt);
            assert_eq!(blockers.len(), 1, "{marker}");
            match &blockers[0].kind {
                BlockerKind::OperationInProgress { op, .. } => assert_eq!(*op, expect),
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

    /// #875 review: the lock probe used `.find()`, so a worktree holding both
    /// locks reported one and sent the user back for the other — in a report
    /// whose contract is "every condition, in one pass".
    #[test]
    fn both_locks_are_reported_not_just_the_first() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(&git_dir.join("index.lock"), "");
        write(&git_dir.join("HEAD.lock"), "");
        let locks: Vec<PathBuf> = probe(&wt)
            .into_iter()
            .filter_map(|b| match b.kind {
                BlockerKind::IndexLocked { lock } => Some(lock),
                _ => None,
            })
            .collect();
        assert_eq!(
            locks,
            vec![git_dir.join("index.lock"), git_dir.join("HEAD.lock")]
        );
    }

    #[test]
    fn an_idle_worktree_has_no_blockers() {
        let (_tmp, wt, _git_dir) = main_worktree();
        assert!(probe(&wt).is_empty());
    }

    /// #875 review: an unresolvable git dir used to `continue`, skipping every
    /// probe for that worktree — reporting "clear" for the case least likely
    /// to be clear. Unknown takes the safe branch.
    #[test]
    fn an_unresolvable_git_dir_is_a_blocker_not_a_pass() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("not-a-worktree");
        std::fs::create_dir_all(&wt).unwrap();
        let blockers = probe(&wt);
        assert!(
            !blockers.is_empty(),
            "a worktree whose state daft cannot read must not read as idle"
        );
        assert!(
            blockers
                .iter()
                .any(|b| matches!(b.kind, BlockerKind::Unreadable { .. })),
            "{blockers:?}"
        );
        assert!(blockers.iter().all(|b| b.is_settle_first()));
    }

    /// A gitlink whose submodule is checked out — git's own
    /// `validate_no_submodules` notion of "populated".
    #[test]
    fn a_populated_submodule_blocks_a_role_change() {
        let (_tmp, wt, _git_dir) = main_worktree();
        let sha = git_ok(&wt, &["rev-parse", "HEAD"]).trim().to_string();
        git_ok(
            &wt,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{sha},sub"),
            ],
        );
        // Unpopulated: a gitlink with nothing checked out does not block.
        assert!(probe(&wt).is_empty());

        write(&wt.join("sub/.git"), "gitdir: ../.git/modules/sub\n");
        let blockers = probe(&wt);
        assert_eq!(blockers.len(), 1);
        match &blockers[0].kind {
            BlockerKind::Submodules { paths } => assert_eq!(paths, &[PathBuf::from("sub")]),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn every_blocker_is_reported_not_just_the_first() {
        let (_tmp, wt, git_dir) = main_worktree();
        write(&git_dir.join("MERGE_HEAD"), "x\n");
        write(&git_dir.join("index.lock"), "");
        let (_tmp2, wt2, git_dir2) = main_worktree();
        write(&git_dir2.join("rebase-merge/head-name"), "refs/heads/b\n");
        let blockers = role_change_blockers(&[
            (wt.clone(), Some("a".into())),
            (wt2.clone(), Some("b".into())),
        ]);
        assert_eq!(blockers.len(), 3);
        assert!(blockers.iter().all(|b| b.is_settle_first()));
    }

    #[test]
    fn a_linked_worktree_is_probed_through_its_pointer_file() {
        let (_tmp, _common, wt, reg) = linked_worktree();
        write(&reg.join("rebase-merge/head-name"), "refs/heads/x\n");
        let blockers = probe(&wt);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].git_dir.as_deref(), Some(reg.as_path()));
    }

    #[test]
    fn locked_registration_blocks_a_moving_worktree_and_carries_the_reason() {
        let (_tmp, common, wt, reg) = linked_worktree();
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

    /// #875 review: the pivot is the one worktree whose registration is about
    /// to be dissolved, and it is not a "moving" worktree — so it was the only
    /// one never checked for a lock, and `git worktree lock` on it was
    /// silently undone by the transform.
    #[test]
    fn the_pivots_own_lock_is_found_too() {
        let (_tmp, common, wt, reg) = linked_worktree();
        write(&reg.join("locked"), "CI runs against this\n");
        let blockers = pivot_registration_blockers(&common, &wt, Some("x"));
        assert_eq!(blockers.len(), 1);
        assert!(matches!(
            blockers[0].kind,
            BlockerKind::RegistrationLocked { .. }
        ));
        assert_eq!(blockers[0].reason, Some(ProbeReason::RoleChange));
    }

    #[test]
    fn a_modules_dir_in_the_registration_blocks_a_move_like_git_does() {
        let (_tmp, common, wt, reg) = linked_worktree();
        std::fs::create_dir_all(reg.join("modules/sub")).unwrap();
        let blockers = relocation_blockers(&common, &[(wt.clone(), Some("x".into()))]);
        assert!(matches!(blockers[0].kind, BlockerKind::Submodules { .. }));
    }

    #[test]
    fn an_unlocked_moving_worktree_has_no_blockers() {
        let (_tmp, common, wt, _reg) = linked_worktree();
        assert!(relocation_blockers(&common, &[(wt.clone(), Some("x".into()))]).is_empty());
    }

    /// #875 review: a `NestFromRoot` merging into an occupied directory cannot
    /// be undone — its recorded inverse moves everything back out, including
    /// what was there first. The plan refuses before anything moves.
    #[test]
    fn an_occupied_destination_is_a_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let taken = tmp.path().join("taken");
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join("someone-elses.txt"), "x").unwrap();
        let free = tmp.path().join("free");
        // An *empty* directory is not in the way: rename(2) replaces one, and
        // a previous op in the same plan may have created the parent chain.
        // A blocker stricter than the executor refuses transforms that work.
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();

        let blockers = destination_blockers(&[
            (taken.clone(), Some("x".into())),
            (free.clone(), Some("y".into())),
            (empty.clone(), Some("z".into())),
        ]);
        assert_eq!(blockers.len(), 1);
        match &blockers[0].kind {
            BlockerKind::DestinationOccupied { destination } => assert_eq!(destination, &taken),
            other => panic!("unexpected {other:?}"),
        }
        assert!(blockers[0].is_settle_first());
    }

    #[test]
    fn same_volume_renames_raise_no_volume_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a");
        std::fs::create_dir_all(&from).unwrap();
        let to = tmp.path().join("b");
        assert!(volume_blockers(&[(from, to)]).is_empty());
    }
}
