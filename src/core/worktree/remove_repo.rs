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
/// be inside (or equal to) a Git repository; the bare git dir is then resolved
/// via `git rev-parse --git-common-dir`, and the `project_root` is taken as the
/// parent of the bare git dir.
pub fn resolve_repo(path: Option<&Path>) -> Result<RepoTarget> {
    let start = match path {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().context("could not determine current directory")?,
    };
    if !start.exists() {
        bail!("{}: no such file or directory", start.display());
    }

    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(&start)
        .arg("rev-parse")
        .arg("--git-common-dir")
        .output()
        .with_context(|| format!("git rev-parse failed in {}", start.display()))?;
    if !output.status.success() {
        bail!("{} is not inside a Git repository", start.display());
    }
    let common_dir = PathBuf::from(
        String::from_utf8(output.stdout)
            .context("git rev-parse output is not UTF-8")?
            .trim(),
    );
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

/// Enumerate the checked-out worktrees of `target`.
///
/// Runs `git --git-dir <bare> worktree list --porcelain` and filters out the
/// bare entry. Operates by `--git-dir` rather than cwd so the call still works
/// when the current working directory is being removed.
pub fn enumerate_worktrees(target: &RepoTarget) -> Result<Vec<WorktreeEntry>> {
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

    #[test]
    fn resolve_repo_from_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let target = resolve_repo(Some(tmp.path())).unwrap();
        assert_eq!(target.project_root, tmp.path().canonicalize().unwrap());
        assert!(target.bare_git_dir.ends_with(".git"));
    }

    #[test]
    fn resolve_repo_from_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let sub = tmp.path().join("nested/dir");
        std::fs::create_dir_all(&sub).unwrap();
        let target = resolve_repo(Some(&sub)).unwrap();
        assert_eq!(target.project_root, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn resolve_repo_errors_for_non_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let err = resolve_repo(Some(tmp.path())).unwrap_err().to_string();
        assert!(
            err.contains("not inside a Git repository"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_repo_errors_for_missing_path() {
        let err = resolve_repo(Some(Path::new("/definitely/does/not/exist/xyz123")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such file or directory"), "{err}");
    }

    #[test]
    fn enumerate_worktrees_returns_only_main_for_fresh_repo() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        std::fs::write(tmp.path().join("README"), b"hi").unwrap();
        Command::new("git")
            .current_dir(tmp.path())
            .args(["add", "."])
            .status()
            .unwrap();
        Command::new("git")
            .current_dir(tmp.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .args(["commit", "-q", "-m", "init"])
            .status()
            .unwrap();

        let target = resolve_repo(Some(tmp.path())).unwrap();
        let worktrees = enumerate_worktrees(&target).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path, tmp.path().canonicalize().unwrap());
    }
}
