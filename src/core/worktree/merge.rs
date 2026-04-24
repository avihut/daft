//! Core logic for `daft merge`.
//!
//! This module owns the business logic for starting a merge in a target
//! worktree. In Slice 2 the scope is intentionally minimal: dispatch
//! `git merge <sources...>` in the target directory and report whether the
//! invocation conflicted. Richer outcome detection (already-up-to-date,
//! fast-forward vs. true merge, octopus announcements, target resolution)
//! lands in later slices.
//!
//! The direct use of `std::process::Command::new("git")` here is a Slice-2
//! shortcut. A later slice will replace it with a `GitCommand::merge_in`
//! helper (analogous to `GitCommand::rebase_in` in `src/git/remote.rs`) once
//! we need to capture stdout/stderr to detect signals like
//! "Already up to date." and distinguish true merge conflicts from other
//! failure modes.
//!
//! # Parameters
//!
//! [`StartParams`] captures the inputs to a merge start: the list of source
//! refs that will be merged into the target worktree's current branch.
//!
//! # Outcome
//!
//! [`StartOutcome`] reports the result: whether the merge was a no-op because
//! the target was already up to date, and whether the `git merge` invocation
//! exited non-zero. Later slices will expand this to distinguish fast-forward,
//! true merge, and octopus cases, and to separate real conflicts from other
//! failure modes.

use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Inputs to a merge-start operation.
#[derive(Debug, Clone)]
pub struct StartParams {
    /// One or more source refs to merge into the target worktree's branch.
    pub sources: Vec<String>,
    /// Optional target worktree/branch. `None` → current worktree's branch.
    pub target: Option<String>,
}

/// Target of a merge after resolution.
///
/// Slice 3 always produces a [`ResolvedTarget`] with a concrete worktree path.
/// A later slice (no-worktree target — ref-only merges) will likely change
/// `path` to `Option<PathBuf>` to represent a target branch that has no
/// checked-out worktree. Keeping the struct dedicated (rather than returning
/// a bare `(String, PathBuf)` tuple) makes that future change a local edit.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// Branch being merged into — used for display and comparisons.
    pub branch: String,
    /// Path to the target worktree on disk.
    pub path: PathBuf,
}

/// Resolve the merge target.
///
/// * `Some(t)` — delegate to [`GitCommand::resolve_worktree_path`] (which
///   matches `t` as a relative path, then a branch name, then a worktree
///   directory name), then read the target worktree's current branch via
///   [`branch_at_path`].
/// * `None` — the target is the current worktree: path from
///   [`GitCommand::get_current_worktree_path`], branch via the same
///   [`branch_at_path`] helper so both arms produce identical error
///   formatting for the same failure modes (detached HEAD, read failure).
///
/// Fails loudly on detached HEAD. Merging into a detached HEAD would fail
/// downstream anyway; the explicit error here surfaces the problem earlier.
pub fn resolve_target(
    target: Option<&str>,
    git: &GitCommand,
    project_root: &Path,
) -> Result<ResolvedTarget> {
    match target {
        Some(t) => {
            let path = git.resolve_worktree_path(t, project_root)?;
            let branch = branch_at_path(git, &path)?;
            Ok(ResolvedTarget { branch, path })
        }
        None => {
            let path = git.get_current_worktree_path()?;
            let branch = branch_at_path(git, &path)?;
            Ok(ResolvedTarget { branch, path })
        }
    }
}

/// Read the short branch name at `path`.
///
/// Respects [`GitCommand::use_gitoxide`]: when enabled, opens a
/// `gix::ThreadSafeRepository` at `path` and reads `HEAD` through gitoxide;
/// otherwise shells out `git -C <path> symbolic-ref --short HEAD`.
///
/// Both paths emit the same error message on detached HEAD ("detached HEAD")
/// so [`resolve_target`]'s two arms are indistinguishable from the user's
/// point of view for that failure mode.
fn branch_at_path(git: &GitCommand, path: &Path) -> Result<String> {
    if git.use_gitoxide {
        let ts = gix::ThreadSafeRepository::discover(path)
            .with_context(|| format!("failed to open git repo at '{}'", path.display()))?;
        let repo = ts.to_thread_local();
        let head = repo
            .head_ref()
            .with_context(|| format!("failed to read HEAD at '{}'", path.display()))?;
        return match head {
            Some(reference) => Ok(reference.name().shorten().to_string()),
            None => anyhow::bail!(
                "target worktree at '{}' has detached HEAD; checkout a branch first",
                path.display()
            ),
        };
    }

    let output = Command::new("git")
        .args([
            "-C",
            &path.display().to_string(),
            "symbolic-ref",
            "--short",
            "HEAD",
        ])
        .output()
        .with_context(|| format!("failed to read branch at '{}'", path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a symbolic ref") {
            anyhow::bail!(
                "target worktree at '{}' has detached HEAD; checkout a branch first",
                path.display()
            );
        }
        anyhow::bail!(
            "failed to read branch at '{}': {}",
            path.display(),
            stderr.trim()
        );
    }

    String::from_utf8(output.stdout)
        .context("invalid UTF-8 in branch name")
        .map(|s| s.trim().to_string())
}

/// An in-progress git operation detected on a worktree.
///
/// These correspond to the well-known state files git writes into the
/// worktree's `.git` directory when a merge/rebase/cherry-pick/bisect is
/// paused awaiting user input. We refuse to start a new merge against a
/// target in one of these states; stacking operations would bury the
/// user under two layers of conflicts.
#[derive(Debug, PartialEq, Eq)]
pub enum InProgressOp {
    Merge,
    Rebase,
    CherryPick,
    Bisect,
}

impl InProgressOp {
    /// Human-readable name, used in refusal messages (e.g. "mid-rebase").
    pub fn description(&self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Bisect => "bisect",
        }
    }
}

/// Detect whether a worktree has an in-progress merge/rebase/cherry-pick/bisect.
///
/// Inspects the worktree's real git directory for the marker files git
/// writes when an operation is paused. In a linked worktree, `.git` is a
/// file with `gitdir: <path>` pointing at the actual per-worktree git dir
/// (e.g. `.git/worktrees/<name>`); in the main worktree, `.git` is itself
/// a directory. Both shapes are handled.
///
/// We intentionally check for directory/file *existence* rather than
/// parsing contents — git populates these atomically and their presence
/// alone is the signal git itself uses (see `git status` output).
pub fn detect_in_progress(worktree: &Path) -> Result<Option<InProgressOp>> {
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
        if p.is_absolute() {
            p
        } else {
            worktree.join(p)
        }
    } else {
        git_entry
    };

    if !git_dir.is_dir() {
        anyhow::bail!(
            "target worktree at '{}' has no valid .git directory",
            worktree.display()
        );
    }

    if git_dir.join("MERGE_HEAD").exists() {
        return Ok(Some(InProgressOp::Merge));
    }
    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        return Ok(Some(InProgressOp::Rebase));
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Ok(Some(InProgressOp::CherryPick));
    }
    if git_dir.join("BISECT_LOG").exists() {
        return Ok(Some(InProgressOp::Bisect));
    }
    Ok(None)
}

/// Refuse a merge when the target worktree has uncommitted/untracked changes.
///
/// Delegates to [`GitCommand::has_uncommitted_changes_in`] (`src/git/stash.rs`)
/// so dirtiness is decided exactly the same way the rest of daft decides it
/// (via `git status --porcelain`, which treats untracked files as dirty).
///
/// The refusal message names the canonical remediation options.
///
/// TODO(slice-13): Make the refusal configurable via the
/// `daft.merge.requireCleanTarget` setting, and restore the full hint
/// pointing users at that toggle. For Slice 4 we hard-code
/// `require_clean = true` — always refuse dirty — because the settings
/// plumbing lands later in the plan, and advertising a key users can't yet
/// set would be misleading.
pub fn validate_clean_target(git: &GitCommand, target: &ResolvedTarget) -> Result<()> {
    if git.has_uncommitted_changes_in(&target.path)? {
        anyhow::bail!(
            "target worktree '{}' has uncommitted changes; commit or stash them before merging",
            target.path.display()
        );
    }
    Ok(())
}

/// Refuse merging a branch into itself.
///
/// A `git merge X` run from a worktree whose branch is `X` is always a
/// semantic no-op at best and an ambiguous user mistake at worst. Failing
/// here — before we touch git — gives a clear, actionable error instead of
/// a cryptic "Already up to date." when the user likely meant to target a
/// different branch via `--into`.
///
/// Note: comparison is nominal, not OID-based. `origin/main` and commit SHAs
/// are not normalized. Stronger resolution may land in a later slice.
pub fn validate_distinct(sources: &[String], target: &ResolvedTarget) -> Result<()> {
    let target_normalized = strip_refs_heads(&target.branch);
    for src in sources {
        if strip_refs_heads(src) == target_normalized {
            anyhow::bail!("cannot merge branch '{}' into the same branch", src);
        }
    }
    Ok(())
}

fn strip_refs_heads(s: &str) -> &str {
    s.strip_prefix("refs/heads/").unwrap_or(s)
}

/// Returns the octopus announcement for `sources` merging into `target_branch`,
/// or `None` for a single source.
///
/// Format: `"Merging N sources into <target> via octopus strategy"`.
///
/// Pure function — no I/O. The caller decides how to surface the message
/// (stderr, logger, TUI). [`execute_start`] prints it to stderr before invoking
/// `git merge` so the announcement is visible even if git's octopus strategy
/// refuses with a conflict.
pub fn announcement(sources: &[String], target_branch: &str) -> Option<String> {
    if sources.len() >= 2 {
        Some(format!(
            "Merging {} sources into {} via octopus strategy",
            sources.len(),
            target_branch
        ))
    } else {
        None
    }
}

/// Result of a merge-start operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartOutcome {
    /// The target branch was already up to date with the sources; nothing to do.
    pub already_up_to_date: bool,
    /// True if `git merge` exited non-zero for any reason (conflict, unknown ref, bad state).
    /// Slice 5+ will refine this into `conflicted` vs other failure modes via stderr parsing
    /// or `.git/MERGE_HEAD` inspection.
    pub failed: bool,
}

/// Execute a merge against the resolved target.
///
/// Resolves the target (explicit `--into` value or current worktree) through
/// [`resolve_target`], then dispatches `git merge <sources...>` with the
/// target worktree as CWD so git updates the correct worktree's index and
/// working tree.
///
/// Returns [`StartOutcome`] describing the result. In this Slice-3 form we
/// still detect failure solely via git's exit status; `already_up_to_date` is
/// always reported as `false` here and will be upgraded in later slices.
///
/// # Signature stability
///
/// Taking `git: &GitCommand` and `project_root: &Path` lets later slices add
/// flag passthrough (Slice 6, via `StartParams`) and ref-only targets
/// (Slice 9, via changes to [`ResolvedTarget`]) without another signature
/// churn.
pub fn execute_start(
    params: &StartParams,
    git: &GitCommand,
    project_root: &Path,
) -> Result<StartOutcome> {
    let resolved = resolve_target(params.target.as_deref(), git, project_root)?;

    // Pre-flight safety rails. Order matters: cheapest/purely-syntactic check
    // first (source vs. target branch name), then state checks on the target
    // worktree (in-progress op, dirty tree) which touch the filesystem.
    validate_distinct(&params.sources, &resolved)?;
    if let Some(op) = detect_in_progress(&resolved.path)? {
        anyhow::bail!(
            "target worktree '{}' is mid-{}; finish or abort it first",
            resolved.branch,
            op.description()
        );
    }
    validate_clean_target(git, &resolved)?;

    // Announce octopus before invoking git so users see the strategy name even
    // if git's octopus refuses with a conflict. Single-source merges emit
    // nothing — `git merge <source>` is the plain case and needs no herald.
    // Stderr keeps progress output out of stdout (reserved for the final
    // "Merge complete." / "Already up to date." result line).
    if let Some(msg) = announcement(&params.sources, &resolved.branch) {
        eprintln!("{msg}");
    }

    let mut argv: Vec<String> = vec!["merge".to_string()];
    argv.extend(params.sources.iter().cloned());

    let status = Command::new("git")
        .args(&argv)
        .current_dir(&resolved.path)
        .status()
        .with_context(|| {
            format!(
                "failed to invoke `git merge` in '{}'",
                resolved.path.display()
            )
        })?;

    Ok(StartOutcome {
        already_up_to_date: false,
        failed: !status.success(),
    })
}

#[cfg(test)]
mod tests {
    //! Test coverage notes for [`resolve_target`]:
    //!
    //! * `branch_at_path` is covered directly against a real `git init`ed
    //!   temp directory.
    //! * The happy-path for `resolve_target(Some(...), ...)` and
    //!   `resolve_target(None, ...)` requires a multi-worktree fixture
    //!   (either setting CWD to exercise the `None` branch, or using
    //!   `git worktree add` to exercise `Some(...)`). Both are expensive
    //!   to stand up here and fragile in parallel tests because
    //!   `get_current_worktree_path` and `symbolic_ref_short_head` read
    //!   the process CWD. End-to-end coverage lives in the YAML scenario
    //!   `tests/manual/scenarios/merge/cross-worktree.yml`.
    //! * The error-path `resolve_target(Some("bogus"), ...)` bubbles up
    //!   from `GitCommand::resolve_worktree_path`, which is exercised by
    //!   its own tests and by the `carry` scenarios.
    use super::*;
    use std::process::Command as ShellCommand;

    fn init_repo(path: &Path) {
        ShellCommand::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        // Identity via env avoids any global config dependency.
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "init"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();
    }

    #[test]
    fn branch_at_path_reads_current_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let git = GitCommand::new(true);
        let branch = branch_at_path(&git, tmp.path()).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn branch_at_path_reads_via_gitoxide() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let git = GitCommand::new(true).with_gitoxide(true);
        let branch = branch_at_path(&git, tmp.path()).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn branch_at_path_fails_on_detached_head() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Detach HEAD at the current commit.
        ShellCommand::new("git")
            .args(["checkout", "--detach", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();

        let git = GitCommand::new(true);
        let err = branch_at_path(&git, tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("detached HEAD"), "unexpected error: {msg}");
    }

    #[test]
    fn start_params_holds_sources() {
        let params = StartParams {
            sources: vec!["feature/x".to_string(), "feature/y".to_string()],
            target: None,
        };
        assert_eq!(params.sources.len(), 2);
        assert_eq!(params.sources[0], "feature/x");
        assert!(params.target.is_none());
    }

    #[test]
    fn start_params_holds_target() {
        let params = StartParams {
            sources: vec!["feature/x".to_string()],
            target: Some("main".to_string()),
        };
        assert_eq!(params.target.as_deref(), Some("main"));
    }

    #[test]
    fn start_outcome_default_is_clean() {
        let outcome = StartOutcome::default();
        assert!(!outcome.already_up_to_date);
        assert!(!outcome.failed);
    }

    #[test]
    fn refuses_when_source_equals_target() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: PathBuf::from("/repo/main"),
        };
        let err = validate_distinct(&["main".to_string()], &target).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("same branch"), "unexpected error: {msg}");
    }

    #[test]
    fn allows_distinct_source_and_target() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: PathBuf::from("/repo/main"),
        };
        assert!(validate_distinct(&["feat".to_string()], &target).is_ok());
    }

    #[test]
    fn refuses_when_target_matches_later_source() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: PathBuf::from("/tmp"),
        };
        let result = validate_distinct(&["feat".into(), "main".into()], &target);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("same branch"));
    }

    #[test]
    fn refuses_when_source_uses_refs_heads_prefix() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: PathBuf::from("/tmp"),
        };
        let result = validate_distinct(&["refs/heads/main".into()], &target);
        assert!(result.is_err());
    }

    #[test]
    fn detects_in_progress_merge() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/MERGE_HEAD"), "deadbeef").unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Merge)
        );
    }

    #[test]
    fn detects_in_progress_rebase() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/rebase-merge")).unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Rebase)
        );
    }

    #[test]
    fn detects_in_progress_rebase_apply_variant() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git/rebase-apply")).unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Rebase)
        );
    }

    #[test]
    fn detects_in_progress_cherry_pick() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/CHERRY_PICK_HEAD"), "c0ffee").unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::CherryPick)
        );
    }

    #[test]
    fn detects_in_progress_bisect() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/BISECT_LOG"), "").unwrap();
        assert_eq!(
            detect_in_progress(tmp.path()).unwrap(),
            Some(InProgressOp::Bisect)
        );
    }

    #[test]
    fn clean_worktree_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert_eq!(detect_in_progress(tmp.path()).unwrap(), None);
    }

    #[test]
    fn validate_clean_target_ok_on_clean_repo() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let git = GitCommand::new(true);
        let target = ResolvedTarget {
            branch: "main".into(),
            path: tmp.path().to_path_buf(),
        };
        assert!(validate_clean_target(&git, &target).is_ok());
    }

    #[test]
    fn validate_clean_target_refuses_untracked_file() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Untracked file — `git status --porcelain` reports it as `?? path`.
        std::fs::write(tmp.path().join("dirty.txt"), "hello\n").unwrap();
        let git = GitCommand::new(true);
        let target = ResolvedTarget {
            branch: "main".into(),
            path: tmp.path().to_path_buf(),
        };
        let err = validate_clean_target(&git, &target).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("uncommitted changes"),
            "unexpected error: {msg}"
        );
        assert!(
            msg.contains("commit or stash"),
            "expected remediation hint in error: {msg}"
        );
    }

    #[test]
    fn announces_octopus_for_multi_source() {
        let sources = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let msg = announcement(&sources, "main").expect("multi-source should announce");
        assert!(msg.contains("3 sources"), "unexpected message: {msg}");
        assert!(msg.contains("octopus"), "unexpected message: {msg}");
        assert!(msg.contains("main"), "unexpected message: {msg}");
    }

    #[test]
    fn no_announcement_for_single_source() {
        let sources = vec!["feat".to_string()];
        assert!(announcement(&sources, "main").is_none());
    }

    #[test]
    fn follows_linked_worktree_gitdir_pointer() {
        // Simulate a linked worktree layout: .git is a file whose first line
        // reads `gitdir: <relative-path>` pointing at the per-worktree dir.
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        let real_gitdir = tmp.path().join("real-gitdir");
        std::fs::create_dir_all(real_gitdir.join("rebase-merge")).unwrap();
        // .git is a file pointing at the real gitdir (using an absolute path
        // here exercises the is_absolute() branch in detect_in_progress).
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", real_gitdir.display()),
        )
        .unwrap();

        assert_eq!(
            detect_in_progress(&worktree).unwrap(),
            Some(InProgressOp::Rebase)
        );
    }
}
