//! Core logic for `daft repo remove`.
//!
//! Resolves a repo target from a path or cwd, enumerates its worktrees,
//! and provides the per-task execution helpers used by the command.

use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};

pub use crate::core::worktree::prune::WorktreeEntry;

/// Identity of a repo to be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoTarget {
    pub bare_git_dir: PathBuf,
    pub project_root: PathBuf,
}

/// Resolve a repo target from an optional user-supplied path.
///
/// When `path` is `None`, the current working directory is used. The path must
/// be inside (or equal to) a Git repository. The bare git dir is resolved via
/// `git rev-parse --git-common-dir` (CLI mode) or `gix::discover` +
/// `repo.common_dir()` (gitoxide mode). The `project_root` is taken as the
/// parent of the bare git dir.
pub fn resolve_repo(path: Option<&Path>, use_gitoxide: bool) -> Result<RepoTarget> {
    let start = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("could not determine current directory")?,
    };
    if !start.exists() {
        bail!("{}: no such file or directory", start.display());
    }
    // Canonicalize `start` upfront so subsequent path joins are unambiguous.
    // Both backends can return `common_dir` as a relative path; on the CLI side
    // `git -C <start> rev-parse --git-common-dir` returns a path relative to
    // `start`, while gitoxide's `repo.common_dir()` returns a path relative to
    // the *process* cwd. With a relative `start` the gitoxide path then
    // double-prefixed (`<rel-start>/<rel-start>/.git`) when joined naively.
    // Anchoring `start` to an absolute path eliminates that ambiguity for both
    // backends. Regression: bash test `repo_remove_relative_path_from_parent`
    // under `DAFT_USE_GITOXIDE=1`.
    let start = std::fs::canonicalize(&start)
        .with_context(|| format!("could not canonicalize {}", start.display()))?;

    let common_dir = if use_gitoxide {
        resolve_common_dir_gix(&start)?
    } else {
        resolve_common_dir_cli(&start)?
    };
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        start.join(common_dir)
    };
    let bare_git_dir = std::fs::canonicalize(&common_dir)
        .with_context(|| format!("could not canonicalize {}", common_dir.display()))?;
    let project_root = bare_git_dir
        .parent()
        .ok_or_else(|| anyhow!("git dir {} has no parent", bare_git_dir.display()))?
        .to_path_buf();
    Ok(RepoTarget {
        bare_git_dir,
        project_root,
    })
}

fn resolve_common_dir_cli(start: &Path) -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(start)
        .arg("rev-parse")
        .arg("--git-common-dir")
        .output()
        .with_context(|| format!("git rev-parse failed in {}", start.display()))?;
    if !output.status.success() {
        bail!("{} is not inside a Git repository", start.display());
    }
    Ok(PathBuf::from(
        String::from_utf8(output.stdout)
            .context("git rev-parse output is not UTF-8")?
            .trim(),
    ))
}

fn resolve_common_dir_gix(start: &Path) -> Result<PathBuf> {
    let repo = gix::discover(start)
        .map_err(|_| anyhow!("{} is not inside a Git repository", start.display()))?;
    Ok(repo.common_dir().to_path_buf())
}

/// Enumerate the checked-out worktrees of `target`.
///
/// In CLI mode runs `git --git-dir <bare> worktree list --porcelain`. In
/// gitoxide mode opens the repo via `gix::open` and combines `repo.workdir()`
/// (main worktree) with `repo.worktrees()` (linked worktrees). The bare entry
/// is filtered out either way.
///
/// Operates against the bare git dir directly so the call still works when the
/// current working directory is the worktree being removed.
pub fn enumerate_worktrees(target: &RepoTarget, use_gitoxide: bool) -> Result<Vec<WorktreeEntry>> {
    if use_gitoxide {
        enumerate_worktrees_gix(target)
    } else {
        enumerate_worktrees_cli(target)
    }
}

fn enumerate_worktrees_cli(target: &RepoTarget) -> Result<Vec<WorktreeEntry>> {
    let output = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&target.bare_git_dir)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("git worktree list failed")?;
    if !output.status.success() {
        bail!(
            "git worktree list exited {}",
            output.status.code().unwrap_or(-1)
        );
    }
    let stdout = String::from_utf8(output.stdout).context("worktree-list output not UTF-8")?;
    // The shared parser RETAINS the bare entry; drop it here so a bare repo path
    // never reaches the removal loop (the gix backend likewise excludes bare).
    Ok(
        crate::core::worktree::porcelain::parse_worktree_list_porcelain(&stdout)
            .into_iter()
            .filter(|e| !e.is_bare)
            .collect(),
    )
}

fn enumerate_worktrees_gix(target: &RepoTarget) -> Result<Vec<WorktreeEntry>> {
    let repo = gix::open(&target.bare_git_dir)
        .with_context(|| format!("gix::open({}) failed", target.bare_git_dir.display()))?;

    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();

    if let Some(workdir) = repo.workdir() {
        let path = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
        let (branch, is_detached) = head_branch_for_repo(&repo);
        seen.push(path.clone());
        out.push(WorktreeEntry {
            path,
            branch,
            is_bare: false,
            is_detached,
        });
    }

    for proxy in repo.worktrees().context("gix repo.worktrees() failed")? {
        // Skip linked worktrees whose base path can't be resolved (broken
        // symlinks, manually `rm -rf`'d worktrees that weren't pruned).
        // The CLI backend (`git worktree list --porcelain`) would still
        // emit the stale entry, but for removal that's moot: the bare
        // dir's `worktrees/<name>/` admin entry is destroyed when we
        // `fs::remove_dir_all` the bare's git_common_dir afterwards, so
        // skipping the entry here just means we don't attempt a redundant
        // (and impossible) filesystem removal of a path that's already gone.
        let path = match proxy.base() {
            Ok(p) => std::fs::canonicalize(&p).unwrap_or(p),
            Err(_) => continue,
        };
        if seen.iter().any(|p| p == &path) {
            continue;
        }
        let branch = read_worktree_head_branch(proxy.git_dir());
        let is_detached = branch.is_none();
        seen.push(path.clone());
        out.push(WorktreeEntry {
            path,
            branch,
            is_bare: false,
            is_detached,
        });
    }
    Ok(out)
}

fn head_branch_for_repo(repo: &gix::Repository) -> (Option<String>, bool) {
    match repo.head_ref() {
        Ok(Some(r)) => (Some(r.name().shorten().to_string()), false),
        Ok(None) => (None, true),
        Err(_) => (None, true),
    }
}

fn read_worktree_head_branch(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

/// Outcome of removing a single worktree from the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveWorktreeOutcome {
    /// `git worktree remove --force` succeeded cleanly.
    Removed,
    /// `git worktree remove --force` failed; we removed the directory directly
    /// and ran `git worktree prune`.
    RemovedViaFallback,
}

/// Remove a single worktree from disk.
///
/// First tries `git --git-dir <bare> worktree remove --force <path>`. If that
/// fails or leaves the directory in place, falls back to `rm -rf` followed by
/// `git worktree prune` so the bare repo's administrative state is consistent.
pub fn remove_worktree_filesystem(
    target: &RepoTarget,
    worktree_path: &Path,
) -> Result<RemoveWorktreeOutcome> {
    let try_git_remove = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&target.bare_git_dir)
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .output()
        .context("git worktree remove failed to launch")?;

    if try_git_remove.status.success() && !worktree_path.exists() {
        return Ok(RemoveWorktreeOutcome::Removed);
    }

    if worktree_path.exists() {
        std::fs::remove_dir_all(worktree_path)
            .with_context(|| format!("rm -rf {} failed", worktree_path.display()))?;
    }
    // `worktree prune` runs after we've already removed the bare git dir in
    // some flows (e.g. `daft repo remove --force` removes worktrees then the
    // bare dir). The binary then errors with "fatal: not a git repository"
    // on stderr — harmless, since this is a best-effort `_ = …` call, but
    // noisy in test logs. Redirect both streams so the cleanup is silent.
    let _ = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&target.bare_git_dir)
        .args(["worktree", "prune"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(RemoveWorktreeOutcome::RemovedViaFallback)
}

/// Remove the bare git directory for `target`, remove the project root
/// directory itself if empty, and clean up the trust DB entry.
///
/// **Never walks above `target.project_root`.** A previous version walked up
/// the parent chain removing empty directories, which could consume the
/// user's containing directory (e.g. `/tmp/sandbox/test/` after removing
/// `/tmp/sandbox/test/myrepo`). Anything above the project root is user
/// territory and must not be touched.
///
/// Trust DB cleanup is best-effort — failures to load or save the database
/// are swallowed because the repo is already gone at that point.
pub fn remove_bare_directory(target: &RepoTarget) -> Result<()> {
    // Tombstone the catalog entry while daft-id and the paths still exist —
    // removed repos stay addressable (`daft hooks jobs --repo`, re-clone by
    // name). Best-effort; if deletion fails below, the next in-repo command
    // resurrects the entry via lazy registration.
    crate::catalog::note_repo_removed(&target.bare_git_dir, &target.project_root);
    if target.bare_git_dir.exists() {
        std::fs::remove_dir_all(&target.bare_git_dir)
            .with_context(|| format!("rm -rf {} failed", target.bare_git_dir.display()))?;
    }
    // Remove project_root itself when empty (it is the layout boundary the
    // user identified by passing the path or running from inside it). Do NOT
    // walk to its parent — that's user-owned space.
    if target.project_root.exists() {
        let is_empty = std::fs::read_dir(&target.project_root)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if is_empty {
            let _ = std::fs::remove_dir(&target.project_root);
        }
    }
    // Drop trust DB entry. Best-effort. Only re-write the file when something
    // actually changed; otherwise loading + saving on every remove pollutes
    // the user's real `repos.json` (and tests that don't sandbox it).
    if let Ok(mut db) = crate::hooks::TrustDatabase::load()
        && db.reset_repo(&target.bare_git_dir)
    {
        let _ = db.save();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn init_repo(dir: &Path) {
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    /// Run the same body twice — once in CLI mode, once in gitoxide mode — so
    /// every assertion holds for both backends.
    fn for_each_backend<F: FnMut(bool)>(mut body: F) {
        for use_gitoxide in [false, true] {
            body(use_gitoxide);
        }
    }

    #[test]
    fn resolve_repo_from_repo_root() {
        for_each_backend(|use_gitoxide| {
            let tmp = tempfile::tempdir().unwrap();
            init_repo(tmp.path());
            let target = resolve_repo(Some(tmp.path()), use_gitoxide).unwrap();
            assert_eq!(target.project_root, tmp.path().canonicalize().unwrap());
            assert!(target.bare_git_dir.ends_with(".git"));
        });
    }

    #[test]
    fn resolve_repo_from_subdirectory() {
        for_each_backend(|use_gitoxide| {
            let tmp = tempfile::tempdir().unwrap();
            init_repo(tmp.path());
            let sub = tmp.path().join("nested/dir");
            std::fs::create_dir_all(&sub).unwrap();
            let target = resolve_repo(Some(&sub), use_gitoxide).unwrap();
            assert_eq!(target.project_root, tmp.path().canonicalize().unwrap());
        });
    }

    #[test]
    fn resolve_repo_errors_for_non_git_dir() {
        for_each_backend(|use_gitoxide| {
            let tmp = tempfile::tempdir().unwrap();
            let err = resolve_repo(Some(tmp.path()), use_gitoxide)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("not inside a Git repository"),
                "unexpected error: {err}"
            );
        });
    }

    #[test]
    fn resolve_repo_handles_relative_path_from_parent() {
        // CI regression: under DAFT_USE_GITOXIDE=1, `daft repo remove
        // <relative-path>` from the parent dir failed with
        // "could not canonicalize <rel>/<rel>/.git" — gitoxide's
        // `repo.common_dir()` returned a path relative to the *process* cwd,
        // not relative to `start`, so the naive `start.join(common_dir)`
        // double-prefixed the directory. Guard both backends.
        for_each_backend(|use_gitoxide| {
            let tmp = tempfile::tempdir().unwrap();
            let canonical_tmp = tmp.path().canonicalize().unwrap();
            let repo_dir = canonical_tmp.join("myrepo");
            std::fs::create_dir(&repo_dir).unwrap();
            init_repo(&repo_dir);

            // Switch into the parent and pass a relative path to the repo
            // — the same shape a user types: `daft repo remove myrepo`.
            let prev_cwd = std::env::current_dir().unwrap();
            std::env::set_current_dir(&canonical_tmp).unwrap();
            let result = resolve_repo(Some(Path::new("myrepo")), use_gitoxide);
            // Restore before asserting so a panic doesn't leak cwd into other
            // tests in the same process.
            std::env::set_current_dir(prev_cwd).unwrap();

            let target = result.unwrap_or_else(|e| {
                panic!("relative path failed (use_gitoxide={use_gitoxide}): {e:#}")
            });
            assert_eq!(target.project_root, repo_dir);
            assert!(target.bare_git_dir.ends_with(".git"));
        });
    }

    #[test]
    fn resolve_repo_errors_for_missing_path() {
        for_each_backend(|use_gitoxide| {
            let err = resolve_repo(
                Some(Path::new("/definitely/does/not/exist/xyz123")),
                use_gitoxide,
            )
            .unwrap_err()
            .to_string();
            assert!(err.contains("no such file or directory"), "{err}");
        });
    }

    /// Initialize a repo at `dir` and create one initial commit. Returns when
    /// the initial commit is in place so worktrees can be checked out off it.
    fn init_repo_with_commit(dir: &Path) {
        init_repo(dir);
        std::fs::write(dir.join("README"), b"hi").unwrap();
        Command::new("git")
            .current_dir(dir)
            .args(["add", "."])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .args(["commit", "-q", "-m", "init"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    #[test]
    fn enumerate_worktrees_returns_only_main_for_fresh_repo() {
        for_each_backend(|use_gitoxide| {
            let tmp = tempfile::tempdir().unwrap();
            init_repo_with_commit(tmp.path());

            let target = resolve_repo(Some(tmp.path()), use_gitoxide).unwrap();
            let worktrees = enumerate_worktrees(&target, use_gitoxide).unwrap();
            assert_eq!(worktrees.len(), 1);
            assert_eq!(worktrees[0].path, tmp.path().canonicalize().unwrap());
        });
    }

    #[test]
    fn enumerate_worktrees_lists_main_and_linked() {
        for_each_backend(|use_gitoxide| {
            let tmp = tempfile::tempdir().unwrap();
            init_repo_with_commit(tmp.path());
            let wt = tmp.path().join("wt-feat");
            Command::new("git")
                .current_dir(tmp.path())
                .args(["worktree", "add", wt.to_str().unwrap(), "-b", "feat"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();

            let target = resolve_repo(Some(tmp.path()), use_gitoxide).unwrap();
            let mut worktrees = enumerate_worktrees(&target, use_gitoxide).unwrap();
            worktrees.sort_by(|a, b| a.path.cmp(&b.path));
            assert_eq!(
                worktrees.len(),
                2,
                "expected 2 worktrees, got {worktrees:?}"
            );
            let branches: Vec<&str> = worktrees
                .iter()
                .map(|e| e.branch.as_deref().unwrap_or("<detached>"))
                .collect();
            assert!(
                branches.contains(&"feat"),
                "missing feat branch: {branches:?}"
            );
        });
    }

    #[test]
    fn remove_bare_directory_removes_bare_and_empty_project_root_only() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("myrepo");
        std::fs::create_dir_all(&project).unwrap();
        Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg("-q")
            .arg(project.join(".git"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        let bare = project.join(".git");
        assert!(bare.exists());

        let target = RepoTarget {
            bare_git_dir: bare.clone(),
            project_root: project.clone(),
        };
        remove_bare_directory(&target).unwrap();
        assert!(!bare.exists(), "bare dir should be gone");
        assert!(!project.exists(), "empty project root should be cleaned up");
        assert!(
            tmp.path().exists(),
            "parent of project_root must NEVER be removed, even when empty"
        );
    }

    #[test]
    fn remove_bare_directory_does_not_delete_empty_parent_of_project_root() {
        // Critical regression test: an earlier implementation walked up the
        // parent chain removing empty directories, which could consume the
        // user's containing directory (e.g. /tmp/sandbox/test/ after removing
        // /tmp/sandbox/test/myrepo). Anything above project_root is user
        // territory.
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("test");
        let project = parent.join("myrepo");
        std::fs::create_dir_all(&project).unwrap();
        Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg("-q")
            .arg(project.join(".git"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        let bare = project.join(".git");

        // `parent` contains only `myrepo/`, so it would become empty after
        // project_root removal — the bug under test.
        let target = RepoTarget {
            bare_git_dir: bare.clone(),
            project_root: project.clone(),
        };
        remove_bare_directory(&target).unwrap();

        assert!(!bare.exists(), "bare dir should be gone");
        assert!(!project.exists(), "empty project root should be removed");
        assert!(
            parent.exists(),
            "parent of project_root must NOT be removed even when empty after the operation \
             (would consume the user's containing directory)"
        );
    }

    #[test]
    fn remove_bare_directory_leaves_non_empty_parent_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("myrepo");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("README"), b"hi").unwrap();
        Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg("-q")
            .arg(project.join(".git"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        let bare = project.join(".git");

        let target = RepoTarget {
            bare_git_dir: bare.clone(),
            project_root: project.clone(),
        };
        remove_bare_directory(&target).unwrap();
        assert!(!bare.exists());
        assert!(
            project.exists(),
            "non-empty project root must not be removed"
        );
    }

    #[test]
    fn remove_worktree_filesystem_deletes_dir_and_runs_git_remove() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo_with_commit(tmp.path());

        let wt = tmp.path().join("wt-feat");
        Command::new("git")
            .current_dir(tmp.path())
            .args(["worktree", "add", wt.to_str().unwrap(), "-b", "feat"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(wt.exists(), "worktree was not created");

        let target = resolve_repo(Some(tmp.path()), false).unwrap();
        let outcome = remove_worktree_filesystem(&target, &wt).unwrap();
        assert!(matches!(
            outcome,
            RemoveWorktreeOutcome::Removed | RemoveWorktreeOutcome::RemovedViaFallback
        ));
        assert!(!wt.exists(), "worktree should have been removed");
    }
}
