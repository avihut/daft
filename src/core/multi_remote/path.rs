//! Path calculation for multi-remote worktree organization.

use crate::core::layout::template::TemplateContext;
use crate::git::GitCommand;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Calculate the worktree path based on multi-remote mode.
///
/// When multi-remote mode is disabled:
///   `project_root/branch_name`
///
/// When multi-remote mode is enabled:
///   `project_root/remote_name/branch_name`
pub fn calculate_worktree_path(
    project_root: &Path,
    branch_name: &str,
    remote_name: &str,
    multi_remote_enabled: bool,
) -> PathBuf {
    if multi_remote_enabled {
        project_root.join(remote_name).join(branch_name)
    } else {
        project_root.join(branch_name)
    }
}

/// Resolve the remote to use for a branch.
///
/// Priority:
/// 1. Explicit remote flag (if provided)
/// 2. Branch's tracking remote (git config branch.<name>.remote)
/// 3. Default remote from settings
pub fn resolve_remote_for_branch(
    git: &GitCommand,
    branch_name: &str,
    explicit_remote: Option<&str>,
    default_remote: &str,
) -> Result<String> {
    // 1. Explicit --remote flag takes precedence
    if let Some(remote) = explicit_remote {
        return Ok(remote.to_string());
    }

    // 2. Try to get the branch's tracking remote
    if let Ok(Some(tracking_remote)) = git.get_branch_tracking_remote(branch_name) {
        if !tracking_remote.is_empty() {
            return Ok(tracking_remote);
        }
    }

    // 3. Fall back to default remote
    Ok(default_remote.to_string())
}

/// Extract the remote name from an existing worktree path (when multi-remote is enabled).
///
/// For a path like `project/origin/feature/foo`, extracts `origin`.
/// Returns None if the path doesn't have the expected structure.
pub fn extract_remote_from_path(project_root: &Path, worktree_path: &Path) -> Option<String> {
    let relative = worktree_path.strip_prefix(project_root).ok()?;
    let components: Vec<_> = relative.components().collect();

    // In multi-remote mode, first component should be the remote name
    if components.len() >= 2 {
        components
            .first()
            .and_then(|c| c.as_os_str().to_str())
            .map(String::from)
    } else {
        None
    }
}

/// Extract the branch name from a worktree path.
///
/// For single-remote mode: `project/feature/foo` -> `feature/foo`
/// For multi-remote mode: `project/origin/feature/foo` -> `feature/foo`
pub fn extract_branch_from_path(
    project_root: &Path,
    worktree_path: &Path,
    multi_remote_enabled: bool,
) -> Option<String> {
    let relative = worktree_path.strip_prefix(project_root).ok()?;
    let components: Vec<_> = relative.components().collect();

    if multi_remote_enabled {
        // Skip the first component (remote name)
        if components.len() >= 2 {
            let branch_components: Vec<_> = components.iter().skip(1).collect();
            let branch_path: PathBuf = branch_components.iter().collect();
            branch_path.to_str().map(String::from)
        } else {
            None
        }
    } else {
        relative.to_str().map(String::from)
    }
}

/// Build a TemplateContext from repository information.
///
/// Used by layout-aware commands to compute worktree paths from templates.
pub fn build_template_context(repo_path: &Path, branch_name: &str) -> TemplateContext {
    let repo = repo_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    TemplateContext {
        repo_path: repo_path.to_path_buf(),
        repo,
        branch: branch_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_worktree_path_single_remote() {
        let project_root = Path::new("/home/user/project");
        let result = calculate_worktree_path(project_root, "feature/foo", "origin", false);
        assert_eq!(result, PathBuf::from("/home/user/project/feature/foo"));
    }

    #[test]
    fn test_calculate_worktree_path_multi_remote() {
        let project_root = Path::new("/home/user/project");
        let result = calculate_worktree_path(project_root, "feature/foo", "origin", true);
        assert_eq!(
            result,
            PathBuf::from("/home/user/project/origin/feature/foo")
        );

        let result = calculate_worktree_path(project_root, "main", "upstream", true);
        assert_eq!(result, PathBuf::from("/home/user/project/upstream/main"));
    }

    #[test]
    fn test_extract_remote_from_path() {
        let project_root = Path::new("/home/user/project");

        let worktree_path = Path::new("/home/user/project/origin/feature/foo");
        assert_eq!(
            extract_remote_from_path(project_root, worktree_path),
            Some("origin".to_string())
        );

        let worktree_path = Path::new("/home/user/project/upstream/main");
        assert_eq!(
            extract_remote_from_path(project_root, worktree_path),
            Some("upstream".to_string())
        );

        // Single component (not multi-remote structure)
        let worktree_path = Path::new("/home/user/project/main");
        assert_eq!(extract_remote_from_path(project_root, worktree_path), None);
    }

    #[test]
    fn test_extract_branch_from_path_single_remote() {
        let project_root = Path::new("/home/user/project");
        let worktree_path = Path::new("/home/user/project/feature/foo");

        assert_eq!(
            extract_branch_from_path(project_root, worktree_path, false),
            Some("feature/foo".to_string())
        );
    }

    #[test]
    fn test_extract_branch_from_path_multi_remote() {
        let project_root = Path::new("/home/user/project");
        let worktree_path = Path::new("/home/user/project/origin/feature/foo");

        assert_eq!(
            extract_branch_from_path(project_root, worktree_path, true),
            Some("feature/foo".to_string())
        );
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use crate::core::layout::BuiltinLayout;
    use std::path::PathBuf;

    #[test]
    fn test_build_template_context() {
        let ctx = build_template_context(Path::new("/home/user/myproject"), "feature/auth");
        assert_eq!(ctx.repo_path, PathBuf::from("/home/user/myproject"));
        assert_eq!(ctx.repo, "myproject");
        assert_eq!(ctx.branch, "feature/auth");
    }

    #[test]
    fn test_build_template_context_root_path() {
        let ctx = build_template_context(Path::new("/"), "main");
        assert_eq!(ctx.repo, "unknown");
    }

    #[test]
    fn test_contained_layout_via_helper() {
        let layout = BuiltinLayout::Contained.to_layout();
        let ctx = build_template_context(Path::new("/home/user/myproject"), "feature/auth");
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature/auth"));
    }

    #[test]
    fn test_sibling_layout_via_helper() {
        let layout = BuiltinLayout::Sibling.to_layout();
        let ctx = build_template_context(Path::new("/home/user/myproject"), "feature/auth");
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject.feature-auth"));
    }

    #[test]
    fn test_nested_layout_via_helper() {
        let layout = BuiltinLayout::Nested.to_layout();
        let ctx = build_template_context(Path::new("/home/user/myproject"), "feature/auth");
        let path = layout.worktree_path(&ctx).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/user/myproject/.worktrees/feature-auth")
        );
    }
}
