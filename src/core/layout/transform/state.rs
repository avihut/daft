//! Layout state representation for transform planning.
//!
//! `LayoutState` captures where everything is (source) or should be (target):
//! git_dir location, bare flag, and all worktree positions.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::core::layout::Layout;
use crate::core::multi_remote::path::build_template_context;

/// Snapshot of a repository's layout state.
#[derive(Debug, Clone)]
pub struct LayoutState {
    /// Absolute path to the `.git` directory.
    pub git_dir: PathBuf,
    /// Whether `core.bare` is true.
    pub is_bare: bool,
    /// The default branch name (the branch co-located with `.git` for non-bare,
    /// or the first/primary worktree for bare).
    pub default_branch: String,
    /// The project root / wrapper directory. For bare and wrapped non-bare
    /// layouts this is the parent of worktrees. For regular non-bare layouts
    /// this is the repo root itself.
    pub project_root: PathBuf,
    /// All worktree entries (including the default branch for bare layouts).
    pub worktrees: Vec<WorktreeEntry>,
}

/// A single worktree's position in a layout state.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    /// Branch name (e.g., "main", "feature/auth").
    pub branch: String,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Whether this is the default branch.
    pub is_default: bool,
}

/// Classification of a worktree during transform planning.
#[derive(Debug, Clone, PartialEq)]
pub enum WorktreeDisposition {
    /// Worktree conforms to the target template — will be relocated if needed.
    Conforming,
    /// Worktree does not match the target template — skipped by default.
    NonConforming,
    /// Worktree is the default branch and needs special handling (collapse/nest).
    DefaultBranch,
}

/// A classified worktree entry in the transform plan.
#[derive(Debug, Clone)]
pub struct ClassifiedWorktree {
    pub branch: String,
    pub current_path: PathBuf,
    pub target_path: PathBuf,
    pub disposition: WorktreeDisposition,
}

// ── State readers ──────────────────────────────────────────────────────────

/// Parse `git worktree list --porcelain` output into worktree entries.
///
/// Skips bare root entries and detached HEAD worktrees. Each block is
/// separated by a blank line. The `branch` line has a `refs/heads/` prefix
/// that is stripped before storing.
pub fn parse_porcelain_to_entries(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();

    for block in porcelain.split("\n\n") {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }

        let mut path: Option<PathBuf> = None;
        let mut branch: Option<String> = None;
        let mut is_bare = false;
        let mut is_detached = false;

        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(rest));
            } else if line == "bare" {
                is_bare = true;
            } else if line == "detached" {
                is_detached = true;
            } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
                branch = Some(rest.to_string());
            }
        }

        // Skip bare root entries and detached HEAD worktrees
        if is_bare || is_detached {
            continue;
        }

        if let (Some(p), Some(b)) = (path, branch) {
            entries.push(WorktreeEntry {
                branch: b,
                path: p,
                is_default: false,
            });
        }
    }

    entries
}

/// Read current layout state from the repo.
pub fn read_source_state(
    git: &crate::git::GitCommand,
    default_branch: &str,
) -> Result<LayoutState> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    let git_dir = git_dir.canonicalize().unwrap_or(git_dir);

    let is_bare = git
        .config_get("core.bare")?
        .map(|v| v.trim() == "true")
        .unwrap_or(false);

    let project_root = git_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not determine project root from git dir"))?
        .to_path_buf();

    let porcelain = git.worktree_list_porcelain()?;
    let mut worktrees = parse_porcelain_to_entries(&porcelain);

    // Mark the default branch
    for wt in &mut worktrees {
        if wt.branch == default_branch {
            wt.is_default = true;
        }
    }

    Ok(LayoutState {
        git_dir,
        is_bare,
        default_branch: default_branch.to_string(),
        project_root,
        worktrees,
    })
}

/// Evaluate the layout template for a branch to compute its target worktree path.
pub fn compute_target_worktree_path(
    layout: &Layout,
    project_root: &Path,
    branch: &str,
) -> Result<PathBuf> {
    let ctx = build_template_context(project_root, branch);
    layout.worktree_path(&ctx)
}

/// Derive where `.git` should live for the target layout.
///
/// - Bare layouts (`layout.needs_bare()`): `project_root/.git`
/// - Wrapped non-bare (`layout.needs_wrapper()`): evaluate template for
///   default_branch, append `/.git`
/// - Regular non-bare: `project_root/.git`
pub fn compute_target_git_dir(
    layout: &Layout,
    project_root: &Path,
    default_branch: &str,
) -> Result<PathBuf> {
    if layout.needs_bare() {
        return Ok(project_root.join(".git"));
    }

    if layout.needs_wrapper() {
        let worktree_path = compute_target_worktree_path(layout, project_root, default_branch)?;
        return Ok(worktree_path.join(".git"));
    }

    Ok(project_root.join(".git"))
}

/// Compute the full target state by evaluating the template for each branch.
pub fn compute_target_state(
    layout: &Layout,
    project_root: &Path,
    default_branch: &str,
    source_worktrees: &[WorktreeEntry],
) -> Result<LayoutState> {
    let git_dir = compute_target_git_dir(layout, project_root, default_branch)?;
    let is_bare = layout.needs_bare();

    let mut worktrees = Vec::with_capacity(source_worktrees.len());
    for wt in source_worktrees {
        let target_path = compute_target_worktree_path(layout, project_root, &wt.branch)?;
        worktrees.push(WorktreeEntry {
            branch: wt.branch.clone(),
            path: target_path,
            is_default: wt.is_default,
        });
    }

    Ok(LayoutState {
        git_dir,
        is_bare,
        default_branch: default_branch.to_string(),
        project_root: project_root.to_path_buf(),
        worktrees,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout::BuiltinLayout;

    #[test]
    fn test_parse_porcelain_basic() {
        let porcelain = "worktree /home/user/myproject\nbare\n\nworktree /home/user/myproject/main\nbranch refs/heads/main\n\nworktree /home/user/myproject/develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[0].path, PathBuf::from("/home/user/myproject/main"));
        assert_eq!(entries[1].branch, "develop");
    }

    #[test]
    fn test_parse_porcelain_skips_detached() {
        let porcelain = "worktree /repo\nbare\n\nworktree /repo/main\nbranch refs/heads/main\n\nworktree /repo/sandbox\nHEAD abc123\ndetached\n\n";
        let entries = parse_porcelain_to_entries(porcelain);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch, "main");
    }

    #[test]
    fn test_parse_porcelain_nonbare() {
        // Non-bare repo: first entry has branch, no "bare" line
        let porcelain = "worktree /home/user/myproject\nbranch refs/heads/main\n\nworktree /home/user/myproject.develop\nbranch refs/heads/develop\n\n";
        let entries = parse_porcelain_to_entries(porcelain);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[1].branch, "develop");
    }

    #[test]
    fn test_target_worktree_path_contained() {
        let layout = BuiltinLayout::Contained.to_layout();
        let path = compute_target_worktree_path(
            &layout,
            Path::new("/home/user/myproject"),
            "feature/auth",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature/auth"));
    }

    #[test]
    fn test_target_worktree_path_sibling() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let path = compute_target_worktree_path(
            &layout,
            Path::new("/home/user/myproject"),
            "feature/auth",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject.feature-auth"));
    }

    #[test]
    fn test_target_worktree_path_contained_classic() {
        let layout = BuiltinLayout::ContainedClassic.to_layout();
        let path = compute_target_worktree_path(
            &layout,
            Path::new("/home/user/myproject"),
            "feature/auth",
        )
        .unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature/auth"));
    }

    #[test]
    fn test_target_git_dir_bare() {
        let layout = BuiltinLayout::Contained.to_layout();
        let git_dir =
            compute_target_git_dir(&layout, Path::new("/home/user/myproject"), "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/.git"));
    }

    #[test]
    fn test_target_git_dir_wrapped_nonbare() {
        let layout = BuiltinLayout::ContainedClassic.to_layout();
        let git_dir =
            compute_target_git_dir(&layout, Path::new("/home/user/myproject"), "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/main/.git"));
    }

    #[test]
    fn test_target_git_dir_regular_nonbare() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let git_dir =
            compute_target_git_dir(&layout, Path::new("/home/user/myproject"), "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/.git"));
    }

    #[test]
    fn test_compute_target_state() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let source_wts = vec![
            WorktreeEntry {
                branch: "main".into(),
                path: PathBuf::from("/repo/main"),
                is_default: true,
            },
            WorktreeEntry {
                branch: "develop".into(),
                path: PathBuf::from("/repo/develop"),
                is_default: false,
            },
        ];
        let target =
            compute_target_state(&layout, Path::new("/repo"), "main", &source_wts).unwrap();
        assert!(!target.is_bare);
        assert_eq!(target.git_dir, PathBuf::from("/repo/.git"));
        assert_eq!(target.worktrees.len(), 2);
        assert_eq!(target.worktrees[0].path, PathBuf::from("/repo.main"));
        assert_eq!(target.worktrees[1].path, PathBuf::from("/repo.develop"));
    }
}
