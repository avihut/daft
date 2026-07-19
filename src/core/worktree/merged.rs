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

use crate::core::worktree::forge_ref::ForgeBranchRef;
use crate::core::worktree::ports::{ForgeMergedWitness, ForgeWitness};
use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::collections::BTreeSet;

/// Whether a branch's work reached the target — and, when the forge is what
/// proved it, which PR/MR did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergedVerdict {
    NotMerged,
    /// `via` names the PR/MR when the forge witnessed the merge; `None` when
    /// git itself found the work on the target.
    Merged {
        via: Option<ForgeBranchRef>,
    },
}

impl MergedVerdict {
    pub fn is_merged(self) -> bool {
        matches!(self, Self::Merged { .. })
    }

    /// The PR/MR that proved the merge, when one did.
    pub fn via(self) -> Option<ForgeBranchRef> {
        match self {
            Self::Merged { via } => via,
            Self::NotMerged => None,
        }
    }
}

/// Check whether a branch has been merged into the default branch.
///
/// Asks git first — locally, against both the default branch and its remote
/// tracking branch, which may be ahead. Only if every probe comes up empty
/// does it ask the forge, which is the authority on merges git cannot see
/// (a squash whose content was altered on the way in) but costs a network
/// round trip, so it goes last rather than first.
pub fn is_branch_merged(
    git: &GitCommand,
    branch: &str,
    default_branch: &str,
    remote_name: &str,
    witness: &dyn ForgeMergedWitness,
) -> Result<MergedVerdict> {
    // Check against local default branch first
    if is_branch_merged_into(git, branch, default_branch)? {
        return Ok(MergedVerdict::Merged { via: None });
    }

    // Also check against the remote tracking branch, which may be ahead of
    // local — but only when it exists and actually differs. Every probe above
    // re-runs against the new target, including step 4's history walk, so
    // repeating them for a ref that names the same commit (the usual state
    // right after a fetch) asks git the identical question twice.
    let remote_ref = format!("{remote_name}/{default_branch}");
    if remote_target_differs(git, default_branch, &remote_ref)
        && is_branch_merged_into(git, branch, &remote_ref)?
    {
        return Ok(MergedVerdict::Merged { via: None });
    }

    // Failing to resolve the tip is not an error here: it only means there is
    // nothing to pin a forge PR to, so the verdict stays unmerged.
    let Ok(tip) = git.rev_parse(&format!("refs/heads/{branch}")) else {
        return Ok(MergedVerdict::NotMerged);
    };

    Ok(match witness.witness(branch, &tip, default_branch) {
        ForgeWitness::MergedAtTip(pr) => MergedVerdict::Merged { via: Some(pr) },
        ForgeWitness::MergedAtOtherHead { pr, head_oid } => {
            // The PR's head moved past the tip we hold. That merge carried
            // this branch's work only if our tip is an ancestor of what
            // merged; if it is not, we hold commits the PR never had and
            // "unmerged" is the right answer.
            //
            // Needs `head_oid` as a local object, so a PR head that was never
            // fetched fails closed — the residual gap this cannot close.
            if git.merge_base_is_ancestor(&tip, &head_oid).unwrap_or(false) {
                MergedVerdict::Merged { via: Some(pr) }
            } else {
                MergedVerdict::NotMerged
            }
        }
        ForgeWitness::Unproven => MergedVerdict::NotMerged,
    })
}

/// Whether probing the remote tracking branch could tell us anything the
/// local target did not.
///
/// `false` when the ref does not resolve — a repo that never fetched the
/// remote's default simply has no such ref, which is not an error and must
/// not abort the caller before it reaches the forge witness — and `false`
/// when it names the same commit the local target does.
fn remote_target_differs(git: &GitCommand, local: &str, remote_ref: &str) -> bool {
    let Ok(remote_oid) = git.rev_parse(remote_ref) else {
        return false;
    };
    // An unresolvable local target means we cannot rule the remote out, so
    // probe it — skipping is only safe when we know the two agree.
    git.rev_parse(local)
        .map_or(true, |local_oid| local_oid != remote_oid)
}

/// Check whether `branch` has been merged into `target`.
///
/// Four checks, cheapest first:
///
/// 1. `merge-base --is-ancestor` — regular and fast-forward merges.
/// 2. `git cherry` — per-commit patch-id equivalence (all lines `-`).
///    Catches rebase merges and single-commit squashes, but NOT a
///    multi-commit branch squashed into one target commit: no individual
///    commit's patch-id matches the combined squash commit (#662).
/// 3. Cumulative-diff squash probe — a synthetic commit of the branch's
///    whole tree parented at the merge base represents the branch's total
///    diff as one commit; `git cherry` on that finds the squash commit.
/// 4. Merge-tree squash probe — compares by *tree result* instead of
///    patch-id, which is what catches a squash whose diff drifted textually
///    from the branch's own (#737).
///
/// Checks 2 and 3 both reduce to patch-id equivalence, and patch-id hashes
/// context lines. That makes them fail on a perfectly ordinary squash: if
/// anything merged between the branch's fork point and its squash touched
/// within three lines of one of the branch's hunks, the context differs and
/// no patch-id matches. Check 4 exists because that is common rather than
/// exotic — in a repo where feature branches edit shared registration blocks,
/// two branches merging the same day near-guarantees it.
///
/// Known remaining gap: a squash commit whose *content* was altered during
/// the merge (conflict resolution, maintainer edits) matches no probe here
/// and is still reported unmerged. Deliberate — every check must be
/// conservative, since reporting "merged" is what authorizes deleting the
/// branch — and it is the forge witness in [`is_branch_merged`], not diff
/// archaeology, that settles those.
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

    // Both squash probes fork from the same point, so resolve it once.
    // Unrelated histories cannot have been squash-merged.
    let Some(base) = git
        .merge_base(target, branch)
        .context("squash probe merge-base failed")?
    else {
        return Ok(false);
    };

    // Step 3: Multi-commit squash probe.
    if squash_probe(git, branch, &base, target)? {
        return Ok(true);
    }

    // Step 4: Context-insensitive squash probe.
    merge_tree_probe(
        git,
        branch,
        &base,
        target,
        crate::git::supports_merge_tree(),
    )
}

/// Detect a squash merge of a multi-commit branch.
///
/// Synthesizes an unreferenced commit wrapping `branch`'s tree, parented at
/// `base` (the merge base with `target`) — its diff is exactly the branch's
/// cumulative diff since forking. If `git cherry` finds a patch-equivalent
/// commit on `target`, the branch's combined work landed as one squash
/// commit. The probe object stays dangling and is swept by `git gc`; it is
/// only created after the cheaper ancestor and per-commit checks failed.
fn squash_probe(git: &GitCommand, branch: &str, base: &str, target: &str) -> Result<bool> {
    let tree = git
        .rev_parse(&format!("{branch}^{{tree}}"))
        .context("squash probe tree lookup failed")?;
    let synthetic = git
        .commit_tree(&tree, base, "daft squash-merge probe")
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

/// Detect a squash merge whose diff no longer matches the branch's own.
///
/// Where [`squash_probe`] asks "does some commit carry the same patch?", this
/// asks "does some commit carry the same *result*?" — immune to context drift,
/// because a tree hash has no notion of surrounding lines.
///
/// For each first-parent commit `C` on `target` since the merge base:
///
/// 1. Cheap filter — `C`'s changed-file set must equal the branch's
///    cumulative changed-file set. Usually this alone leaves one candidate.
/// 2. Proof — three-way merge the branch into `C^` in memory. If the
///    resulting tree equals `C`'s tree, then `C` *is* the branch's work
///    applied at that point, i.e. the squash commit.
///
/// Only reached after the cheaper checks failed, and only when git can run
/// `merge-tree --write-tree` (2.38+); `capable` is a parameter rather than a
/// direct probe call so both arms stay unit-testable on any dev machine.
///
/// Conservative in the same way as its siblings. A branch with no net change
/// abstains — otherwise any empty commit on the target would match it — and a
/// squash whose content was edited during the merge matches no tree and stays
/// unmerged.
fn merge_tree_probe(
    git: &GitCommand,
    branch: &str,
    base: &str,
    target: &str,
    capable: bool,
) -> Result<bool> {
    if !capable {
        return Ok(false);
    }

    let branch_files: BTreeSet<String> = git
        .diff_name_only(base, branch)
        .context("merge-tree probe branch diff failed")?
        .into_iter()
        .collect();
    if branch_files.is_empty() {
        return Ok(false);
    }

    let candidates = git
        .first_parent_commits(base, target)
        .context("merge-tree probe candidate walk failed")?;

    for candidate in candidates {
        // A root commit has no `C^` to merge onto.
        let Some(first_parent) = candidate.parents.first() else {
            continue;
        };
        if candidate.files.into_iter().collect::<BTreeSet<_>>() != branch_files {
            continue;
        }

        let merged_tree = git
            .merge_tree_write_tree(first_parent, branch)
            .context("merge-tree probe merge failed")?;
        if merged_tree.is_some_and(|tree| tree == candidate.tree) {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::forge_ref::ForgeRefKind;
    use crate::core::worktree::ports::NoopForgeWitness;
    use crate::test_support::CwdGuard;
    use serial_test::serial;
    use std::path::Path;
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

    fn init_repo(path: &Path) {
        git_ok(path, &["init", "-q", "-b", "main"]);
        // Local config so git subprocesses have an identity without touching
        // global config.
        git_ok(path, &["config", "--local", "user.name", "Test"]);
        git_ok(path, &["config", "--local", "user.email", "test@test.com"]);
        git_ok(path, &["commit", "--allow-empty", "-q", "-m", "init"]);
    }

    /// Resolve a revision to its full hash.
    fn rev(path: &Path, revision: &str) -> String {
        let out = crate::utils::git_command_at(path)
            .args(["rev-parse", revision])
            .output()
            .expect("git rev-parse runs");
        assert!(out.status.success(), "rev-parse {revision} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Checkout `branch`, write `content` to `file`, stage and commit it.
    fn add_commit(path: &Path, branch: &str, file: &str, content: &str) {
        git_ok(path, &["checkout", "-q", branch]);
        std::fs::write(path.join(file), content).unwrap();
        git_ok(path, &["add", file]);
        git_ok(path, &["commit", "-q", "-m", &format!("add {file}")]);
    }

    /// Ten numbered lines — room for two edits far enough apart to be separate
    /// hunks, but close enough to share a ±3-line context window.
    fn numbered_lines() -> String {
        (1..=10).map(|n| format!("line {n}\n")).collect::<String>()
    }

    /// Rewrite one line of `file` on the currently checked-out branch.
    fn edit_line(path: &Path, file: &str, line: &str, replacement: &str) {
        let contents = std::fs::read_to_string(path.join(file)).unwrap();
        let updated = contents.replace(&format!("{line}\n"), &format!("{replacement}\n"));
        assert_ne!(contents, updated, "expected to rewrite {line}");
        std::fs::write(path.join(file), updated).unwrap();
        git_ok(path, &["commit", "-q", "-a", "-m", &format!("edit {line}")]);
    }

    /// The #737 shape: a branch forks, an unrelated change lands on the target
    /// *inside the context window* of one of the branch's hunks, then the
    /// branch is squash-merged cleanly.
    fn repo_with_context_drifted_squash() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        std::fs::write(tmp.path().join("f.txt"), numbered_lines()).unwrap();
        git_ok(tmp.path(), &["add", "f.txt"]);
        git_ok(tmp.path(), &["commit", "-q", "-m", "seed"]);

        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        edit_line(tmp.path(), "f.txt", "line 5", "line 5 CHANGED");
        add_commit(tmp.path(), "feat", "g.txt", "g");

        // Three lines away: a different hunk, but the same context window.
        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        edit_line(tmp.path(), "f.txt", "line 8", "line 8 DRIFT");

        git_ok(tmp.path(), &["merge", "--squash", "feat"]);
        git_ok(tmp.path(), &["commit", "-q", "-m", "feat squashed (#737)"]);
        tmp
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

    /// Regression test for #737. The branch's work landed as an ordinary
    /// squash commit, but an intermediate merge shifted the context lines
    /// around one of its hunks, so every patch-id check (`git cherry` and the
    /// #662 cumulative-diff probe) reports it unmerged. Only the merge-tree
    /// probe, which compares trees rather than patches, sees through it.
    ///
    /// This is the shape that stranded `daft-725` after PR #731 was merged.
    #[test]
    #[serial]
    fn context_drifted_squash_detected_as_merged() {
        if merge_tree_unsupported() {
            return;
        }
        let tmp = repo_with_context_drifted_squash();

        assert!(
            merged_into(tmp.path(), "feat", "main"),
            "a squash whose context drifted must still be detected as merged"
        );
    }

    /// The drift really does defeat the patch-id checks — otherwise the test
    /// above would pass without the new probe and prove nothing.
    #[test]
    #[serial]
    fn context_drift_defeats_the_patch_id_probes() {
        let tmp = repo_with_context_drifted_squash();
        let _guard = CwdGuard::enter(tmp.path());
        let git = GitCommand::new(true);
        let base = fork_point(&git, "feat", "main");

        assert!(
            !squash_probe(&git, "feat", &base, "main").unwrap(),
            "the cumulative-diff probe is expected to miss a drifted squash"
        );
        assert!(
            !merge_tree_probe(&git, "feat", &base, "main", /* capable */ false).unwrap(),
            "without merge-tree support the branch stays unmerged"
        );
        assert!(
            merge_tree_probe(&git, "feat", &base, "main", /* capable */ true).unwrap(),
            "with merge-tree support the squash is found"
        );
    }

    /// The merge base the probes fork from, resolved once by the caller in
    /// production and here by hand.
    fn fork_point(git: &GitCommand, branch: &str, target: &str) -> String {
        git.merge_base(target, branch)
            .expect("merge-base runs")
            .expect("the fixtures share history")
    }

    /// Whether the running git predates `merge-tree --write-tree` (2.38).
    ///
    /// Tests that assert a *drifted* squash is found are asserting the step-4
    /// probe, which the capability gate turns off below that version — where
    /// falling back to the pre-#737 answer is correct behaviour, not a
    /// regression. Without this they fail red on older distro and
    /// corporate-pinned gits for an environment reason.
    fn merge_tree_unsupported() -> bool {
        let unsupported = !crate::git::supports_merge_tree();
        if unsupported {
            eprintln!("skipped: git is older than 2.38, so the merge-tree probe is off");
        }
        unsupported
    }

    /// A squash that was *edited* after the fact (the maintainer amended the
    /// content, or resolved a conflict differently) matches no tree. Staying
    /// unmerged here is the deliberate conservative floor — proving those
    /// merges is the forge witness's job, not the probe's.
    #[test]
    #[serial]
    fn altered_squash_content_stays_unmerged() {
        let tmp = repo_with_context_drifted_squash();
        // Amend the squash so its tree no longer matches the branch's work.
        std::fs::write(tmp.path().join("g.txt"), "g, but rewritten on main").unwrap();
        git_ok(tmp.path(), &["commit", "-q", "-a", "--amend", "--no-edit"]);

        assert!(
            !merged_into(tmp.path(), "feat", "main"),
            "an altered-content squash must not be reported merged"
        );
    }

    /// The cheap filter is a filter, not a verdict: a commit touching exactly
    /// the branch's files with different content must not match.
    #[test]
    #[serial]
    fn same_file_set_with_different_content_stays_unmerged() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "branch content");
        add_commit(tmp.path(), "feat", "b.txt", "branch content");

        // Same two files, unrelated content, committed straight to main.
        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        std::fs::write(tmp.path().join("a.txt"), "main content").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "main content").unwrap();
        git_ok(tmp.path(), &["add", "a.txt", "b.txt"]);
        git_ok(
            tmp.path(),
            &["commit", "-q", "-m", "same files, other work"],
        );

        assert!(!merged_into(tmp.path(), "feat", "main"));
    }

    /// A branch with no net change has an empty cumulative file set, which
    /// would otherwise equal the file set of *any* empty commit on the target
    /// — and merging a net-zero branch onto that commit's parent reproduces
    /// its tree exactly, so the probe would match on nothing at all. The
    /// abstain guard is what stops it.
    ///
    /// Asserted against the probe directly: the end-to-end verdict for this
    /// shape is already decided by the older cumulative-diff check, whose
    /// synthetic empty patch collides with the empty commit's patch-id.
    #[test]
    #[serial]
    fn probe_abstains_when_the_branch_has_no_net_change() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "tmp.txt", "scratch");
        std::fs::remove_file(tmp.path().join("tmp.txt")).unwrap();
        git_ok(tmp.path(), &["commit", "-q", "-a", "-m", "revert scratch"]);

        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        git_ok(
            tmp.path(),
            &["commit", "-q", "--allow-empty", "-m", "empty"],
        );

        let _guard = CwdGuard::enter(tmp.path());
        let git = GitCommand::new(true);
        let base = fork_point(&git, "feat", "main");
        assert!(
            !merge_tree_probe(&git, "feat", &base, "main", /* capable */ true).unwrap(),
            "a net-zero branch must not match an empty commit's tree"
        );
    }

    /// A witness that vouches for one branch, and refuses everything else —
    /// the shape a real forge answer takes. `head_oid` chooses which outcome:
    /// `None` means the PR head is whatever tip it is asked about, `Some`
    /// pins a different head so the caller has to reason about containment.
    struct FakeWitness {
        branch: &'static str,
        forge_ref: ForgeBranchRef,
        head_oid: Option<String>,
    }

    impl FakeWitness {
        fn at_tip(branch: &'static str, number: u32) -> Self {
            Self {
                branch,
                forge_ref: ForgeBranchRef::new(ForgeRefKind::GithubPr, number),
                head_oid: None,
            }
        }

        fn at_head(branch: &'static str, number: u32, head_oid: &str) -> Self {
            Self {
                head_oid: Some(head_oid.to_string()),
                ..Self::at_tip(branch, number)
            }
        }
    }

    impl crate::core::worktree::ports::ForgeMergedWitness for FakeWitness {
        fn witness(&self, branch: &str, _tip_oid: &str, _target_branch: &str) -> ForgeWitness {
            if branch != self.branch {
                return ForgeWitness::Unproven;
            }
            match &self.head_oid {
                None => ForgeWitness::MergedAtTip(self.forge_ref),
                Some(head_oid) => ForgeWitness::MergedAtOtherHead {
                    pr: self.forge_ref,
                    head_oid: head_oid.clone(),
                },
            }
        }
    }

    /// Run the full check, which probes the remote tracking branch as well as
    /// the local one. Stamps `origin/main` at `main` first so these
    /// remote-less fixtures exercise the same shape a real repo has — and,
    /// since the two then name one commit, the skip that keeps the second
    /// pass from repeating the first's history walk.
    fn witnessed(path: &Path, branch: &str, witness: &dyn ForgeMergedWitness) -> MergedVerdict {
        git_ok(path, &["update-ref", "refs/remotes/origin/main", "main"]);
        let _guard = CwdGuard::new();
        std::env::set_current_dir(path).unwrap();
        let git = GitCommand::new(true);
        is_branch_merged(&git, branch, "main", "origin", witness).unwrap()
    }

    /// The case no git probe can reach: the squash was edited on the way in,
    /// so no patch and no tree match — but the forge watched it merge.
    #[test]
    #[serial]
    fn forge_witness_settles_a_branch_git_cannot_place() {
        let tmp = repo_with_context_drifted_squash();
        std::fs::write(tmp.path().join("g.txt"), "g, but rewritten on main").unwrap();
        git_ok(tmp.path(), &["commit", "-q", "-a", "--amend", "--no-edit"]);
        let witness = FakeWitness::at_tip("feat", 731);

        let verdict = witnessed(tmp.path(), "feat", &witness);

        assert!(verdict.is_merged());
        assert_eq!(
            verdict.via().map(|r| r.number),
            Some(731),
            "the verdict must name the PR that proved it, so the deletion can be explained"
        );
    }

    /// The PR advanced past the tip we hold — a suggestion accepted in the
    /// web UI, a push from another machine — and then merged. Our tip is
    /// contained in what merged, so the work did land.
    ///
    /// This is the population the witness exists for: a squash altered on the
    /// way in. Pinning on equality alone would strand it forever, since the
    /// remote branch is gone and the local tip can never catch up.
    #[test]
    #[serial]
    fn a_pr_head_that_advanced_past_our_tip_still_witnesses() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "a");
        let tip = rev(tmp.path(), "feat");
        // The reviewer's suggestion, committed on the PR branch by the forge.
        add_commit(tmp.path(), "feat", "b.txt", "suggested");
        let pr_head = rev(tmp.path(), "feat");
        // Our worktree never saw it: rewind the local branch to where we were.
        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        git_ok(tmp.path(), &["branch", "-q", "-f", "feat", &tip]);
        add_commit(tmp.path(), "main", "other.txt", "other");

        let witness = FakeWitness::at_head("feat", 731, &pr_head);
        let verdict = witnessed(tmp.path(), "feat", &witness);

        assert_eq!(
            verdict.via().map(|r| r.number),
            Some(731),
            "our tip is an ancestor of the merged head, so the work landed"
        );
    }

    /// The same mismatch, but our tip is *not* contained in what merged: we
    /// hold commits the PR never had. Branch-name reuse looks exactly like
    /// this, and the answer must stay unmerged — this is the direction that
    /// authorizes deleting work.
    #[test]
    #[serial]
    fn a_pr_head_that_does_not_contain_our_tip_stays_unmerged() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "old-feat"]);
        add_commit(tmp.path(), "old-feat", "old.txt", "old work");
        let unrelated_head = rev(tmp.path(), "old-feat");
        // A different branch that happens to carry the same name upstream.
        git_ok(tmp.path(), &["checkout", "-q", "main"]);
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "new.txt", "unrelated new work");
        add_commit(tmp.path(), "main", "other.txt", "other");

        let witness = FakeWitness::at_head("feat", 700, &unrelated_head);
        let verdict = witnessed(tmp.path(), "feat", &witness);

        assert_eq!(verdict, MergedVerdict::NotMerged);
    }

    /// A head the local repo has never fetched cannot be reasoned about, so
    /// the check fails closed rather than trusting the forge's word alone.
    #[test]
    #[serial]
    fn an_unfetched_pr_head_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "a");
        add_commit(tmp.path(), "main", "other.txt", "other");

        // A well-formed OID that names no object here.
        let witness = FakeWitness::at_head("feat", 731, "0123456789abcdef0123456789abcdef01234567");
        let verdict = witnessed(tmp.path(), "feat", &witness);

        assert_eq!(verdict, MergedVerdict::NotMerged);
    }

    /// A witness that declines leaves the branch exactly where the git probes
    /// left it: unmerged.
    #[test]
    #[serial]
    fn a_declining_witness_leaves_the_branch_unmerged() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        git_ok(tmp.path(), &["checkout", "-q", "-b", "feat"]);
        add_commit(tmp.path(), "feat", "a.txt", "a");
        add_commit(tmp.path(), "main", "other.txt", "other");

        let verdict = witnessed(tmp.path(), "feat", &NoopForgeWitness);

        assert_eq!(verdict, MergedVerdict::NotMerged);
    }

    /// When git itself finds the work, the forge is never consulted and no PR
    /// is named — naming one would imply the merge needed proving.
    #[test]
    #[serial]
    fn git_proof_names_no_pr_and_skips_the_witness() {
        if merge_tree_unsupported() {
            return;
        }
        let tmp = repo_with_context_drifted_squash();
        // A witness that would vouch for this branch, if it were asked.
        let witness = FakeWitness::at_tip("feat", 731);

        let verdict = witnessed(tmp.path(), "feat", &witness);

        assert_eq!(
            verdict,
            MergedVerdict::Merged { via: None },
            "the merge-tree probe proves this one; the forge is not consulted"
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
