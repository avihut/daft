//! Core logic for `daft repo remove`.
//!
//! Resolves a repo target from a path or cwd, enumerates its worktrees,
//! and provides the per-task execution helpers used by the command.

use anyhow::{anyhow, bail, Context, Result};
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
    Ok(parse_worktree_list_porcelain(&stdout))
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
    let _ = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&target.bare_git_dir)
        .args(["worktree", "prune"])
        .status();
    Ok(RemoveWorktreeOutcome::RemovedViaFallback)
}

/// Remove the bare git directory for `target`, walk up the parent chain
/// removing empty parent directories, and clean up the trust DB entry for
/// this bare path.
///
/// The empty-parent walk is best-effort: it stops at the first non-empty
/// directory so user data outside the repo is never touched. Trust DB
/// cleanup is also best-effort — failures to load or save the database are
/// swallowed because the repo is already gone at that point.
pub fn remove_bare_directory(target: &RepoTarget) -> Result<()> {
    if target.bare_git_dir.exists() {
        std::fs::remove_dir_all(&target.bare_git_dir)
            .with_context(|| format!("rm -rf {} failed", target.bare_git_dir.display()))?;
    }
    let mut cursor = target.project_root.clone();
    while cursor.exists() {
        let mut iter = match std::fs::read_dir(&cursor) {
            Ok(it) => it,
            Err(_) => break,
        };
        if iter.next().is_some() {
            break;
        }
        if std::fs::remove_dir(&cursor).is_err() {
            break;
        }
        match cursor.parent() {
            Some(p) => cursor = p.to_path_buf(),
            None => break,
        }
    }
    // Drop trust DB entry. Best-effort. Only re-write the file when something
    // actually changed; otherwise loading + saving on every remove pollutes
    // the user's real `repos.json` (and tests that don't sandbox it).
    if let Ok(mut db) = crate::hooks::TrustDatabase::load() {
        if db.reset_repo(&target.bare_git_dir) {
            let _ = db.save();
        }
    }
    Ok(())
}

/// Parse the porcelain output of `git worktree list --porcelain`, dropping the
/// bare entry. Pure-string helper so it can be unit-tested without spawning
/// git.
fn parse_worktree_list_porcelain(stdout: &str) -> Vec<WorktreeEntry> {
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let mut is_bare = false;
    let mut is_detached = false;
    for line in stdout.lines() {
        if line.is_empty() {
            if let Some(p) = path.take() {
                if !is_bare {
                    out.push(WorktreeEntry {
                        path: p,
                        branch: branch.take(),
                        is_bare,
                        is_detached,
                    });
                }
                branch = None;
                is_bare = false;
                is_detached = false;
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(p) = path.take() {
                if !is_bare {
                    out.push(WorktreeEntry {
                        path: p,
                        branch: branch.take(),
                        is_bare,
                        is_detached,
                    });
                }
                branch = None;
                is_bare = false;
                is_detached = false;
            }
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            branch = Some(rest.to_string());
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            is_detached = true;
        }
    }
    if let Some(p) = path {
        if !is_bare {
            out.push(WorktreeEntry {
                path: p,
                branch,
                is_bare,
                is_detached,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo(dir: &Path) {
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(dir)
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
            .status()
            .unwrap();
        Command::new("git")
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .args(["commit", "-q", "-m", "init"])
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
    fn remove_bare_directory_deletes_bare_and_empty_parents() {
        let tmp = tempfile::tempdir().unwrap();
        // Sentinel so the walk-up has a non-empty floor at tmp.path() — the
        // spec walks past empty parents (e.g. nested `.worktrees` layouts), so
        // we have to give it something to stop on or it would consume tmp too.
        std::fs::write(tmp.path().join("keep-me"), b"sibling").unwrap();
        let project = tmp.path().join("myrepo");
        std::fs::create_dir_all(&project).unwrap();
        Command::new("git")
            .arg("init")
            .arg("--bare")
            .arg("-q")
            .arg(project.join(".git"))
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
        assert!(tmp.path().exists());
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
