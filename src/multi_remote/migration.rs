//! Migration logic for enabling and disabling multi-remote mode.
//!
//! Handles moving worktrees between flat and nested directory structures.

use crate::git::GitCommand;
use crate::output::Output;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Information about a worktree for migration purposes.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Full path to the worktree directory.
    pub path: PathBuf,
    /// Branch name checked out in this worktree.
    pub branch: Option<String>,
    /// Remote associated with this worktree (for multi-remote mode).
    pub remote: Option<String>,
}

/// A single migration operation.
#[derive(Debug, Clone)]
pub enum MigrationOp {
    /// Create a directory.
    CreateDir(PathBuf),
    /// Move a worktree from one location to another.
    MoveWorktree { from: PathBuf, to: PathBuf },
    /// Remove an empty directory.
    RemoveEmptyDir(PathBuf),
}

impl MigrationOp {
    /// Get a human-readable description of this operation.
    pub fn description(&self) -> String {
        match self {
            MigrationOp::CreateDir(path) => format!("mkdir -p {}", path.display()),
            MigrationOp::MoveWorktree { from, to } => {
                format!("mv {} {}", from.display(), to.display())
            }
            MigrationOp::RemoveEmptyDir(path) => format!("rmdir {}", path.display()),
        }
    }
}

/// A plan for migrating worktrees between single-remote and multi-remote layouts.
#[derive(Debug)]
pub struct MigrationPlan {
    /// Operations to execute in order.
    pub operations: Vec<MigrationOp>,
}

impl MigrationPlan {
    /// Create a migration plan to enable multi-remote mode.
    ///
    /// Moves worktrees from `project/branch` to `project/remote/branch`.
    pub fn for_enable(
        project_root: &Path,
        worktrees: &[WorktreeInfo],
        default_remote: &str,
    ) -> Self {
        let mut operations = Vec::new();
        let mut needed_dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for wt in worktrees {
            // Skip the bare repo's .git directory
            if wt.path.ends_with(".git") {
                continue;
            }

            let remote = wt.remote.as_deref().unwrap_or(default_remote).to_string();
            let remote_dir = project_root.join(&remote);

            // Add directory creation if needed
            if !needed_dirs.contains(&remote_dir) {
                if !remote_dir.exists() {
                    operations.push(MigrationOp::CreateDir(remote_dir.clone()));
                }
                needed_dirs.insert(remote_dir.clone());
            }

            // Calculate new path: move from project/branch to project/remote/branch
            if let Ok(relative) = wt.path.strip_prefix(project_root) {
                let new_path = remote_dir.join(relative);
                if new_path != wt.path {
                    operations.push(MigrationOp::MoveWorktree {
                        from: wt.path.clone(),
                        to: new_path,
                    });
                }
            }
        }

        Self { operations }
    }

    /// Create a migration plan to disable multi-remote mode.
    ///
    /// Moves worktrees from `project/remote/branch` to `project/branch`.
    pub fn for_disable(project_root: &Path, worktrees: &[WorktreeInfo]) -> Result<Self> {
        let mut operations = Vec::new();
        let mut remote_dirs_to_remove: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();

        // Group worktrees by their remote directory
        let mut worktrees_by_remote: HashMap<String, Vec<&WorktreeInfo>> = HashMap::new();
        for wt in worktrees {
            if wt.path.ends_with(".git") {
                continue;
            }

            if let Some(remote) = &wt.remote {
                worktrees_by_remote
                    .entry(remote.clone())
                    .or_default()
                    .push(wt);
                remote_dirs_to_remove.insert(project_root.join(remote));
            }
        }

        for wt in worktrees {
            if wt.path.ends_with(".git") {
                continue;
            }

            // Extract branch path from project/remote/branch
            if let Some(remote) = &wt.remote {
                let remote_prefix = project_root.join(remote);
                if let Ok(branch_path) = wt.path.strip_prefix(&remote_prefix) {
                    let new_path = project_root.join(branch_path);
                    if new_path != wt.path {
                        operations.push(MigrationOp::MoveWorktree {
                            from: wt.path.clone(),
                            to: new_path,
                        });
                    }
                }
            }
        }

        // Add cleanup operations for empty remote directories
        for dir in remote_dirs_to_remove {
            operations.push(MigrationOp::RemoveEmptyDir(dir));
        }

        Ok(Self { operations })
    }

    /// Preview the migration plan without executing.
    pub fn preview(&self, output: &mut dyn Output) {
        if self.operations.is_empty() {
            output.step("No migration needed");
            return;
        }

        output.step("Migration plan:");
        for op in &self.operations {
            output.step(&format!("  {}", op.description()));
        }
    }

    /// Execute the migration plan.
    pub fn execute(&self, git: &GitCommand, output: &mut dyn Output) -> Result<()> {
        for op in &self.operations {
            output.step(&op.description());

            match op {
                MigrationOp::CreateDir(path) => {
                    fs::create_dir_all(path).with_context(|| {
                        format!("Failed to create directory: {}", path.display())
                    })?;
                }
                MigrationOp::MoveWorktree { from, to } => {
                    // Ensure parent directory exists
                    if let Some(parent) = to.parent() {
                        if !parent.exists() {
                            fs::create_dir_all(parent).with_context(|| {
                                format!("Failed to create parent directory: {}", parent.display())
                            })?;
                        }
                    }

                    // Use git worktree move for proper bookkeeping
                    git.worktree_move(from, to).with_context(|| {
                        format!(
                            "Failed to move worktree from {} to {}",
                            from.display(),
                            to.display()
                        )
                    })?;
                }
                MigrationOp::RemoveEmptyDir(path) => {
                    // Only remove if empty
                    if path.exists() {
                        let is_empty = fs::read_dir(path)
                            .map(|mut entries| entries.next().is_none())
                            .unwrap_or(false);

                        if is_empty {
                            fs::remove_dir(path).with_context(|| {
                                format!("Failed to remove empty directory: {}", path.display())
                            })?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if this plan has any operations.
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

/// List all worktrees in a repository.
pub fn list_worktrees(git: &GitCommand, project_root: &Path) -> Result<Vec<WorktreeInfo>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut worktrees = Vec::new();

    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            // Save previous worktree if any
            if let Some(path) = current_path.take() {
                let remote = infer_remote_from_path(project_root, &path);
                worktrees.push(WorktreeInfo {
                    path,
                    branch: current_branch.take(),
                    remote,
                });
            }
            current_path = Some(PathBuf::from(worktree_path));
            current_branch = None;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        }
    }

    // Don't forget the last worktree
    if let Some(path) = current_path.take() {
        let remote = infer_remote_from_path(project_root, &path);
        worktrees.push(WorktreeInfo {
            path,
            branch: current_branch.take(),
            remote,
        });
    }

    Ok(worktrees)
}

/// Infer the remote name from a worktree path in multi-remote layout.
fn infer_remote_from_path(project_root: &Path, worktree_path: &Path) -> Option<String> {
    if let Ok(relative) = worktree_path.strip_prefix(project_root) {
        let components: Vec<_> = relative.components().collect();
        // If path has structure like remote/branch, extract remote
        if components.len() >= 2 {
            if let Some(first) = components.first() {
                let name = first.as_os_str().to_str()?;
                // Check if this looks like a remote name (not .git or common branch patterns)
                if name != ".git" && !name.contains('/') {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_plan_for_enable_empty() {
        let project_root = Path::new("/home/user/project");
        let worktrees: Vec<WorktreeInfo> = vec![];

        let plan = MigrationPlan::for_enable(project_root, &worktrees, "origin");
        assert!(plan.is_empty());
    }

    #[test]
    fn test_migration_plan_for_enable_single_worktree() {
        let project_root = Path::new("/home/user/project");
        let worktrees = vec![WorktreeInfo {
            path: PathBuf::from("/home/user/project/main"),
            branch: Some("main".to_string()),
            remote: None,
        }];

        let plan = MigrationPlan::for_enable(project_root, &worktrees, "origin");
        assert!(!plan.is_empty());

        // Should create origin dir and move the worktree
        let mut has_create = false;
        let mut has_move = false;
        for op in &plan.operations {
            match op {
                MigrationOp::CreateDir(path) => {
                    has_create = true;
                    assert_eq!(path, &PathBuf::from("/home/user/project/origin"));
                }
                MigrationOp::MoveWorktree { from, to } => {
                    has_move = true;
                    assert_eq!(from, &PathBuf::from("/home/user/project/main"));
                    assert_eq!(to, &PathBuf::from("/home/user/project/origin/main"));
                }
                _ => {}
            }
        }
        assert!(has_create);
        assert!(has_move);
    }

    #[test]
    fn test_migration_plan_for_disable() {
        let project_root = Path::new("/home/user/project");
        let worktrees = vec![WorktreeInfo {
            path: PathBuf::from("/home/user/project/origin/main"),
            branch: Some("main".to_string()),
            remote: Some("origin".to_string()),
        }];

        let plan = MigrationPlan::for_disable(project_root, &worktrees).unwrap();
        assert!(!plan.is_empty());

        // Should move worktree and remove empty dir
        let mut has_move = false;
        for op in &plan.operations {
            if let MigrationOp::MoveWorktree { from, to } = op {
                has_move = true;
                assert_eq!(from, &PathBuf::from("/home/user/project/origin/main"));
                assert_eq!(to, &PathBuf::from("/home/user/project/main"));
            }
        }
        assert!(has_move);
    }
}
