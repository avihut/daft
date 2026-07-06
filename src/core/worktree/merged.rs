//! Shared "is this branch merged?" detection.
//!
//! Used by branch-delete validation (Check 4) and by prune's
//! gone-but-unmerged guard. A remote branch disappearing does NOT imply the
//! work was merged — abandoned branches get their remotes deleted too — so
//! every removal path that infers "merged" from "gone" must verify with
//! these checks instead.
//!
//! Note: `core::worktree::merge` has its own ancestor-only
//! `is_branch_merged_into` for mid-merge bookkeeping; it intentionally does
//! NOT detect squash merges and must not be unified with this one.

use crate::git::GitCommand;
use anyhow::{Context, Result};

/// Check whether a branch has been merged into the default branch.
///
/// Checks against both the local default branch and its remote tracking
/// branch (which may be ahead of local).
pub fn is_branch_merged(
    git: &GitCommand,
    branch: &str,
    default_branch: &str,
    remote_name: &str,
) -> Result<bool> {
    // Check against local default branch first
    if is_branch_merged_into(git, branch, default_branch)? {
        return Ok(true);
    }

    // Also check against the remote tracking branch, which may be ahead of local
    let remote_ref = format!("{remote_name}/{default_branch}");
    if is_branch_merged_into(git, branch, &remote_ref)? {
        return Ok(true);
    }

    Ok(false)
}

/// Check whether `branch` has been merged into `target`.
///
/// Three checks, cheapest first:
///
/// 1. `merge-base --is-ancestor` — regular and fast-forward merges.
/// 2. `git cherry` — per-commit patch-id equivalence (all lines `-`).
///    Catches rebase merges and single-commit squashes, but NOT a
///    multi-commit branch squashed into one target commit: no individual
///    commit's patch-id matches the combined squash commit (#662).
/// 3. Cumulative-diff squash probe — a synthetic commit of the branch's
///    whole tree parented at the merge base represents the branch's total
///    diff as one commit; `git cherry` on that finds the squash commit.
///
/// Known remaining gap: a squash commit whose diff was altered during the
/// merge (conflict resolution, maintainer edits) matches no probe and is
/// still reported unmerged — deliberate, since every check here must be
/// conservative: reporting "merged" is what authorizes deleting the branch.
pub fn is_branch_merged_into(git: &GitCommand, branch: &str, target: &str) -> Result<bool> {
    // Step 1: Check if branch is an ancestor of the target (regular merge)
    let is_ancestor = git
        .merge_base_is_ancestor(branch, target)
        .context("merge-base check failed")?;

    if is_ancestor {
        return Ok(true);
    }

    // Step 2: Check for squash merge via git cherry.
    let cherry_output = git
        .cherry(target, branch)
        .context("git cherry check failed")?;

    let lines: Vec<&str> = cherry_output.lines().collect();

    // Empty output means no commits to compare
    if lines.is_empty() {
        return Ok(true);
    }

    // All lines must start with `-` for the branch to be considered squash-merged
    if lines.iter().all(|line| line.starts_with('-')) {
        return Ok(true);
    }

    // Step 3: Multi-commit squash probe.
    squash_probe(git, branch, target)
}

/// Detect a squash merge of a multi-commit branch.
///
/// Synthesizes an unreferenced commit wrapping `branch`'s tree, parented at
/// the merge base with `target` — its diff is exactly the branch's
/// cumulative diff since forking. If `git cherry` finds a patch-equivalent
/// commit on `target`, the branch's combined work landed as one squash
/// commit. The probe object stays dangling and is swept by `git gc`; it is
/// only created after the cheaper ancestor and per-commit checks failed.
fn squash_probe(git: &GitCommand, branch: &str, target: &str) -> Result<bool> {
    let Some(base) = git
        .merge_base(target, branch)
        .context("squash probe merge-base failed")?
    else {
        // Unrelated histories cannot have been squash-merged.
        return Ok(false);
    };

    let tree = git
        .rev_parse(&format!("{branch}^{{tree}}"))
        .context("squash probe tree lookup failed")?;
    let synthetic = git
        .commit_tree(&tree, &base, "daft squash-merge probe")
        .context("squash probe commit failed")?;

    let cherry_output = git
        .cherry(target, &synthetic)
        .context("squash probe cherry failed")?;

    // Exactly one line is expected (the synthetic commit). `-` means a
    // patch-equivalent commit exists on the target. A missing or `+` line —
    // including the empty-diff case where the branch has no net change —
    // conservatively stays "not merged".
    Ok(cherry_output
        .lines()
        .next()
        .is_some_and(|line| line.starts_with('-')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    /// Test-only helper: run `git` quietly in `path` and panic if it fails.
    /// Routed through `git_command_at` so all inherited `GIT_*` vars are
    /// cleared (per the project's test-hygiene rule), not just GIT_DIR.
    fn git_ok(path: &Path, args: &[&str]) {
        let status = crate::utils::git_command_at(path)
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "git {args:?} failed in {}",
            path.display()
        );
    }

    /// RAII helper: saves the current working directory on construction and
    /// restores it on drop. Tests that call `std::env::set_current_dir` use
    /// this to avoid leaving cwd pointing at a deleted tempdir for the next
    /// test (which would panic in `std::env::current_dir`).
    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn new() -> Self {
            Self {
                original: std::env::current_dir().expect("cwd readable at test start"),
            }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            if std::env::set_current_dir(&self.original).is_err() {
                let _ = std::env::set_current_dir(std::env::temp_dir());
            }
        }
    }

    fn init_repo(path: &Path) {
        git_ok(path, &["init", "-q", "-b", "main"]);
        // Local config so git subprocesses have an identity without touching
        // global config.
        git_ok(path, &["config", "--local", "user.name", "Test"]);
        git_ok(path, &["config", "--local", "user.email", "test@test.com"]);
        git_ok(path, &["commit", "--allow-empty", "-q", "-m", "init"]);
    }

    /// Checkout `branch`, write `content` to `file`, stage and commit it.
    fn add_commit(path: &Path, branch: &str, file: &str, content: &str) {
        git_ok(path, &["checkout", "-q", branch]);
        std::fs::write(path.join(file), content).unwrap();
        git_ok(path, &["add", file]);
        git_ok(path, &["commit", "-q", "-m", &format!("add {file}")]);
    }

    /// Run `is_branch_merged_into` with the process cwd inside `path`
    /// (the GitCommand subprocess helpers resolve the repo from cwd).
    fn merged_into(path: &Path, branch: &str, target: &str) -> bool {
        let _guard = CwdGuard::new();
        std::env::set_current_dir(path).unwrap();
        let git = GitCommand::new(true);
        is_branch_merged_into(&git, branch, target).unwrap()
    }

    /// Regression test for #662: a branch of N>1 commits squash-merged into
    /// the target has no per-commit patch-id match (`git cherry` shows every
    /// commit as `+`), so the cherry check alone misclassifies it as
    /// unmerged. The cumulative-diff probe must detect it.
    #[test]
    #[serial]
    fn multi_commit_squash_detected_as_merged() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "a");
        add_commit(tmp.path(), "feat", "b.txt", "b");
        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        git_ok(tmp.path(), &["merge", "--squash", "feat"]);
        git_ok(tmp.path(), &["commit", "-q", "-m", "feat squashed (#1)"]);

        assert!(
            merged_into(tmp.path(), "feat", "main"),
            "multi-commit squash-merged branch must be detected as merged"
        );
    }

    #[test]
    #[serial]
    fn single_commit_squash_still_detected() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "a");
        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        git_ok(tmp.path(), &["merge", "--squash", "feat"]);
        git_ok(tmp.path(), &["commit", "-q", "-m", "feat squashed (#2)"]);

        assert!(merged_into(tmp.path(), "feat", "main"));
    }

    #[test]
    #[serial]
    fn regular_merge_still_detected() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "a");
        add_commit(tmp.path(), "feat", "b.txt", "b");
        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        git_ok(tmp.path(), &["merge", "-q", "--no-ff", "--no-edit", "feat"]);

        assert!(merged_into(tmp.path(), "feat", "main"));
    }

    #[test]
    #[serial]
    fn unmerged_branch_stays_unmerged() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "a");
        add_commit(tmp.path(), "feat", "b.txt", "b");
        // Advance main independently so the branches genuinely diverge.
        add_commit(tmp.path(), "main", "other.txt", "other");

        assert!(
            !merged_into(tmp.path(), "feat", "main"),
            "unmerged work must never be reported as merged"
        );
    }

    #[test]
    #[serial]
    fn unrelated_histories_not_merged() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "--orphan", "lonely"]);
        std::fs::write(tmp.path().join("l.txt"), "l").unwrap();
        git_ok(tmp.path(), &["add", "l.txt"]);
        git_ok(tmp.path(), &["commit", "-q", "-m", "orphan work"]);

        assert!(!merged_into(tmp.path(), "lonely", "main"));
    }
}
