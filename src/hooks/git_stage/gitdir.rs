//! Git directory resolution without spawning git.
//!
//! Two hot paths need to know where a repo's shared git directory is before
//! they know whether they have any work to do at all: the shim dispatch that
//! fires on every commit, and [`StageRunner::manages_stage`], whose port
//! contract caps it at stat-level probes. Both would be dominated by a
//! `git rev-parse` subprocess, so everything here reads plain files instead —
//! `.git` (a directory in a main worktree, a `gitdir:` pointer file in a
//! linked one) and the linked worktree's `commondir`.
//!
//! [`StageRunner::manages_stage`]: crate::core::worktree::ports::StageRunner::manages_stage

use crate::core::layout::template::normalize_path;
use std::path::{Path, PathBuf};

/// The directories a stage lookup needs, resolved from a starting path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// Root of the worktree the starting path lives in — the directory that
    /// holds the `.git` entry.
    pub worktree_root: PathBuf,
    /// Per-worktree git directory: `.git` itself in a main worktree,
    /// `<common>/worktrees/<name>` in a linked one.
    pub git_dir: PathBuf,
    /// The shared git directory — where `hooks/` and `config` live. Equal to
    /// [`Self::git_dir`] in a main worktree.
    pub common_dir: PathBuf,
}

impl GitDirs {
    /// Where git looks for hook scripts absent a `core.hooksPath` override.
    ///
    /// Deliberately the shared dir: hooks are a property of the repository,
    /// not of one worktree, so a shim installed from any worktree fires from
    /// all of them.
    pub fn default_hooks_dir(&self) -> PathBuf {
        self.common_dir.join("hooks")
    }

    /// daft's private per-repo directory inside the shared git dir. Holds the
    /// stage-shim sentinel — state that must survive a worktree being removed
    /// and must never appear in the tracked tree.
    pub fn daft_dir(&self) -> PathBuf {
        self.common_dir.join(".daft")
    }
}

/// Resolve the git directories for the repo containing `start`, walking up
/// until a `.git` entry is found.
///
/// Returns `None` when `start` is outside any repository, when the `.git`
/// pointer file is malformed, or when it points somewhere that is not a
/// directory. Callers on the dispatch path treat every `None` the same way —
/// no repo, no stage, nothing to do — so the distinction is not worth an
/// error type here.
pub fn discover(start: &Path) -> Option<GitDirs> {
    let start = std::path::absolute(start).ok()?;
    let mut cur: &Path = &start;
    loop {
        if let Some(git_dir) = resolve_git_entry(cur) {
            let common_dir = resolve_common_dir(&git_dir);
            return Some(GitDirs {
                worktree_root: cur.to_path_buf(),
                git_dir,
                common_dir,
            });
        }
        cur = cur.parent()?;
    }
}

/// The git dir named by `dir/.git`, or `None` when there is no usable entry.
fn resolve_git_entry(dir: &Path) -> Option<PathBuf> {
    let entry = dir.join(".git");
    let meta = std::fs::symlink_metadata(&entry).ok()?;
    let git_dir = if meta.is_file() {
        // A linked worktree: first line is `gitdir: <path>`, absolute or
        // relative to the worktree root.
        let content = std::fs::read_to_string(&entry).ok()?;
        let pointer = content.lines().next()?.strip_prefix("gitdir:")?.trim();
        if pointer.is_empty() {
            return None;
        }
        let p = PathBuf::from(pointer);
        // `join` replaces when its argument is absolute, so this handles both
        // spellings without branching on `is_absolute`.
        normalize_path(&dir.join(p))
    } else {
        // Either a real directory or a symlink to one; `is_dir` below settles
        // it either way.
        entry
    };
    git_dir.is_dir().then_some(git_dir)
}

/// The shared git dir for `git_dir`, read from its `commondir` file.
///
/// A main worktree has no `commondir`, so it is its own common dir.
fn resolve_common_dir(git_dir: &Path) -> PathBuf {
    let Ok(content) = std::fs::read_to_string(git_dir.join("commondir")) else {
        return git_dir.to_path_buf();
    };
    let Some(pointer) = content
        .lines()
        .next()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    else {
        return git_dir.to_path_buf();
    };
    normalize_path(&git_dir.join(pointer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn main_worktree_is_its_own_common_dir() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();

        let dirs = discover(&root).unwrap();
        assert_eq!(dirs.worktree_root, root);
        assert_eq!(dirs.git_dir, root.join(".git"));
        assert_eq!(dirs.common_dir, root.join(".git"));
        assert_eq!(dirs.default_hooks_dir(), root.join(".git/hooks"));
    }

    #[test]
    fn discovery_walks_up_from_a_nested_directory() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        let nested = root.join("src/deep/deeper");
        fs::create_dir_all(&nested).unwrap();

        let dirs = discover(&nested).unwrap();
        assert_eq!(dirs.worktree_root, root);
    }

    #[test]
    fn linked_worktree_resolves_pointer_and_commondir() {
        // daft's default layout: a bare-ish common dir with sibling worktrees
        // whose `.git` is a pointer file.
        let tmp = tempdir().unwrap();
        let common = tmp.path().join("project/.git");
        let wt_git = common.join("worktrees/feature");
        fs::create_dir_all(&wt_git).unwrap();
        fs::write(wt_git.join("commondir"), "../..\n").unwrap();

        let worktree = tmp.path().join("project/feature");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", wt_git.display()),
        )
        .unwrap();

        let dirs = discover(&worktree).unwrap();
        assert_eq!(dirs.worktree_root, worktree);
        assert_eq!(dirs.git_dir, wt_git);
        assert_eq!(dirs.common_dir, common);
        // The hooks dir is the SHARED one — a shim installed from any worktree
        // must fire from all of them.
        assert_eq!(dirs.default_hooks_dir(), common.join("hooks"));
        assert_eq!(dirs.daft_dir(), common.join(".daft"));
    }

    #[test]
    fn relative_gitdir_pointer_resolves_against_the_worktree() {
        let tmp = tempdir().unwrap();
        let common = tmp.path().join(".git");
        let wt_git = common.join("worktrees/feature");
        fs::create_dir_all(&wt_git).unwrap();
        fs::write(wt_git.join("commondir"), "../..").unwrap();

        let worktree = tmp.path().join("feature");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join(".git"), "gitdir: ../.git/worktrees/feature\n").unwrap();

        let dirs = discover(&worktree).unwrap();
        assert_eq!(dirs.git_dir, wt_git);
        assert_eq!(dirs.common_dir, common);
    }

    #[test]
    fn outside_a_repository_resolves_to_nothing() {
        let tmp = tempdir().unwrap();
        assert!(discover(tmp.path()).is_none());
    }

    #[test]
    fn malformed_pointer_file_resolves_to_nothing() {
        let tmp = tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join(".git"), "this is not a gitdir pointer\n").unwrap();

        assert!(discover(&worktree).is_none());
    }

    #[test]
    fn pointer_to_a_missing_directory_resolves_to_nothing() {
        let tmp = tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            "gitdir: /nonexistent/worktrees/gone\n",
        )
        .unwrap();

        assert!(discover(&worktree).is_none());
    }
}
