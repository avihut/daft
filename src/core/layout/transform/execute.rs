//! Plan executor with rollback support.
//!
//! Iterates through a `TransformPlan`'s operations, executing each one and
//! pushing a reverse operation onto a rollback stack. On failure the stack is
//! unwound in reverse order to restore the repository to its pre-transform
//! state (best-effort).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use super::plan::{TransformOp, TransformPlan};
use crate::core::ProgressSink;
use crate::git::GitCommand;

// ── Public result type ─────────────────────────────────────────────────────

/// Outcome of executing a transform plan.
#[derive(Debug)]
pub struct ExecuteResult {
    /// Number of operations that completed successfully.
    pub ops_completed: usize,
    /// Total number of operations in the plan.
    pub ops_total: usize,
}

// ── Top-level executor ─────────────────────────────────────────────────────

/// Execute every operation in the plan, maintaining a rollback stack.
///
/// On failure the executor attempts to undo completed operations in reverse
/// order, then propagates the original error. Progress messages are emitted
/// via `progress` for each step.
pub fn execute_plan(
    plan: &TransformPlan,
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<ExecuteResult> {
    let total = plan.ops.len();
    let mut rollback_stack: Vec<TransformOp> = Vec::new();

    for (i, op) in plan.ops.iter().enumerate() {
        progress.on_step(&format!("[{}/{}] {}", i + 1, total, describe_op(op)));

        if let Err(e) = execute_op(op, git, progress) {
            progress.on_warning(&format!("Operation failed: {e:#}"));
            progress.on_warning("Attempting rollback of completed operations...");

            if let Err(rb_err) = rollback(&rollback_stack, git, progress) {
                progress.on_warning(&format!("Rollback encountered errors: {rb_err:#}"));
            }

            return Err(e.context(format!(
                "Failed at step {}/{}: {}",
                i + 1,
                total,
                describe_op(op)
            )));
        }

        if let Some(rev) = reverse_op(op) {
            rollback_stack.push(rev);
        }
    }

    Ok(ExecuteResult {
        ops_completed: total,
        ops_total: total,
    })
}

// ── Per-op dispatch ────────────────────────────────────────────────────────

/// Execute a single transform operation.
fn execute_op(op: &TransformOp, git: &GitCommand, progress: &mut dyn ProgressSink) -> Result<()> {
    match op {
        TransformOp::StashChanges {
            branch,
            worktree_path,
        } => exec_stash_changes(branch, worktree_path, git),

        TransformOp::PopStash {
            branch,
            worktree_path,
        } => exec_pop_stash(branch, worktree_path, git, progress),

        TransformOp::MoveWorktree {
            branch: _,
            from,
            to,
        } => exec_move_worktree(from, to, git),

        TransformOp::MoveGitDir { from, to } => exec_move_git_dir(from, to),

        TransformOp::SetBare(bare) => exec_set_bare(*bare, git),

        TransformOp::RegisterWorktree { branch, path } => {
            exec_register_worktree(branch, path, progress)
        }

        TransformOp::UnregisterWorktree { branch } => exec_unregister_worktree(branch),

        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
        } => exec_collapse_into_root(worktree_path, root_path),

        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
        } => exec_nest_from_root(root_path, subdir_path),

        TransformOp::InitWorktreeIndex { path } => exec_init_worktree_index(path, progress),

        TransformOp::CreateDirectory { path } => {
            fs::create_dir_all(path)
                .with_context(|| format!("Failed to create directory: {}", path.display()))?;
            Ok(())
        }

        TransformOp::ValidateIntegrity => exec_validate_integrity(progress),
    }
}

// ── Individual op implementations ──────────────────────────────────────────

fn exec_stash_changes(_branch: &str, worktree_path: &Path, git: &GitCommand) -> Result<()> {
    let prev = crate::utils::get_current_directory()?;
    crate::utils::change_directory(worktree_path)?;

    let result = git.stash_push_with_untracked("daft-transform: temporary stash before move");

    crate::utils::change_directory(&prev)?;
    result
}

fn exec_pop_stash(
    _branch: &str,
    worktree_path: &Path,
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let prev = crate::utils::get_current_directory()?;
    crate::utils::change_directory(worktree_path)?;

    if let Err(e) = git.stash_pop() {
        progress.on_warning(&format!(
            "Could not restore stashed changes: {e}. Run 'git stash pop' manually."
        ));
    }

    crate::utils::change_directory(&prev)?;
    Ok(())
}

fn exec_move_worktree(from: &Path, to: &Path, git: &GitCommand) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;
    }

    git.worktree_move(from, to)?;

    // Clean up empty parent directories left behind
    cleanup_empty_parents(from);

    Ok(())
}

fn exec_move_git_dir(from: &Path, to: &Path) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create parent directory for .git: {}",
                parent.display()
            )
        })?;
    }

    // If the target path exists as a file (e.g., a worktree's .git pointer
    // file), remove it first — fs::rename can't overwrite a file with a dir.
    if to.is_file() {
        fs::remove_file(to)
            .with_context(|| format!("Failed to remove existing .git file at {}", to.display()))?;
    }

    fs::rename(from, to).with_context(|| {
        format!(
            "Failed to move .git directory from {} to {}",
            from.display(),
            to.display()
        )
    })?;

    fixup_gitdir_references(to)?;

    // CD to the new .git's parent so subsequent git commands can find the repo.
    // After NestFromRoot + MoveGitDir, the old CWD may no longer contain .git.
    if let Some(parent) = to.parent() {
        crate::utils::change_directory(parent)?;
    }

    Ok(())
}

fn exec_set_bare(bare: bool, git: &GitCommand) -> Result<()> {
    let value = if bare { "true" } else { "false" };
    git.config_set("core.bare", value)?;

    // When going bare, remove the index file so git doesn't think the bare repo
    // has a working tree with deletions.
    if bare {
        let git_dir = crate::core::repo::get_git_common_dir()?;
        let index_file = git_dir.join("index");
        if index_file.exists() {
            fs::remove_file(&index_file).with_context(|| {
                format!("Failed to remove index file: {}", index_file.display())
            })?;
        }
    }

    Ok(())
}

fn exec_register_worktree(
    branch: &str,
    path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    super::legacy::register_worktree(&git_dir, path, branch, progress)
}

fn exec_unregister_worktree(branch: &str) -> Result<()> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    let worktree_name = branch.replace('/', "-");
    let worktrees_dir = git_dir.join("worktrees").join(&worktree_name);

    if worktrees_dir.exists() {
        fs::remove_dir_all(&worktrees_dir).with_context(|| {
            format!(
                "Failed to remove worktree registration: {}",
                worktrees_dir.display()
            )
        })?;
    }

    Ok(())
}

fn exec_collapse_into_root(worktree_path: &Path, root_path: &Path) -> Result<()> {
    let staging = root_path.join(".daft-transform-staging");
    fs::create_dir_all(&staging)
        .with_context(|| format!("Failed to create staging dir: {}", staging.display()))?;

    // Move each file/dir from worktree_path to staging (skip .git)
    for entry in fs::read_dir(worktree_path)
        .with_context(|| format!("Failed to read worktree dir: {}", worktree_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        fs::rename(entry.path(), staging.join(&name))
            .with_context(|| format!("Failed to move {} to staging", entry.path().display()))?;
    }

    // Remove the worktree's .git pointer file (it's a text file, not a
    // directory) that linked back to the bare repo's worktree registration.
    // This file is orphaned after collapse — the UnregisterWorktree op will
    // clean up the registration side.
    let wt_git_file = worktree_path.join(".git");
    if wt_git_file.is_file() {
        fs::remove_file(&wt_git_file).ok();
    }

    // Remove the now-empty worktree dir
    fs::remove_dir(worktree_path).ok();

    // CD to root_path — the old worktree dir may have been the CWD, and it's
    // now deleted. Subsequent ops (SetBare, etc.) need a valid CWD.
    crate::utils::change_directory(root_path)?;

    // Move from staging to root
    for entry in fs::read_dir(&staging)
        .with_context(|| format!("Failed to read staging dir: {}", staging.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        fs::rename(entry.path(), root_path.join(&name)).with_context(|| {
            format!(
                "Failed to move {} from staging to root",
                entry.path().display()
            )
        })?;
    }

    fs::remove_dir(&staging).ok();

    Ok(())
}

fn exec_nest_from_root(root_path: &Path, subdir_path: &Path) -> Result<()> {
    let staging = root_path.join(".daft-transform-staging");
    fs::create_dir_all(&staging)
        .with_context(|| format!("Failed to create staging dir: {}", staging.display()))?;

    let staging_name = staging
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();

    // Move each file/dir from root into staging, skipping .git, the staging
    // dir itself, and linked worktree directories (directories containing a
    // .git file, which indicates they are linked worktrees).
    for entry in fs::read_dir(root_path)
        .with_context(|| format!("Failed to read root dir: {}", root_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();

        if name == ".git" || name == staging_name {
            continue;
        }

        // Skip linked worktree directories (they have a .git *file* inside)
        if entry.file_type()?.is_dir() {
            let dotgit = entry.path().join(".git");
            if dotgit.exists() && dotgit.is_file() {
                continue;
            }
        }

        fs::rename(entry.path(), staging.join(&name))
            .with_context(|| format!("Failed to move {} to staging", entry.path().display()))?;
    }

    // Create target subdir and move files there
    fs::create_dir_all(subdir_path)
        .with_context(|| format!("Failed to create subdir: {}", subdir_path.display()))?;

    for entry in fs::read_dir(&staging)
        .with_context(|| format!("Failed to read staging dir: {}", staging.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        fs::rename(entry.path(), subdir_path.join(&name)).with_context(|| {
            format!(
                "Failed to move {} from staging to subdir",
                entry.path().display()
            )
        })?;
    }

    fs::remove_dir(&staging).ok();

    Ok(())
}

fn exec_init_worktree_index(path: &Path, progress: &mut dyn ProgressSink) -> Result<()> {
    let reset_result = Command::new("git")
        .args(["reset", "--mixed", "HEAD"])
        .current_dir(path)
        .output()
        .context("Failed to initialize worktree index")?;

    if !reset_result.status.success() {
        let stderr = String::from_utf8_lossy(&reset_result.stderr);
        progress.on_warning(&format!("git reset warning: {}", stderr.trim()));
    }

    Ok(())
}

fn exec_validate_integrity(progress: &mut dyn ProgressSink) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    // 1. Run git fsck to check repository integrity
    progress.on_step("Running git fsck...");
    match Command::new("git").args(["fsck", "--no-dangling"]).output() {
        Ok(result) if result.status.success() => {}
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            let msg = stderr.trim();
            if !msg.is_empty() {
                errors.push(format!("git fsck: {msg}"));
            }
        }
        Err(e) => {
            progress.on_warning(&format!("Could not run git fsck: {e}"));
        }
    }

    // 2. Check each worktree for unexpected dirty state
    progress.on_step("Verifying worktree state...");
    match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .output()
    {
        Ok(result) if result.status.success() => {
            let porcelain = String::from_utf8_lossy(&result.stdout);
            let mut current_path: Option<String> = None;
            let mut is_bare = false;

            for line in porcelain.lines() {
                if let Some(path) = line.strip_prefix("worktree ") {
                    current_path = Some(path.to_string());
                    is_bare = false;
                } else if line == "bare" {
                    is_bare = true;
                } else if line.is_empty() {
                    // Check non-bare worktrees for dirty state
                    if let Some(ref path) = current_path {
                        if !is_bare {
                            if let Ok(status) = Command::new("git")
                                .args(["status", "--porcelain"])
                                .current_dir(path)
                                .output()
                            {
                                if status.status.success() {
                                    let out = String::from_utf8_lossy(&status.stdout);
                                    // Filter out layout artifacts (.gitignore,
                                    // .worktrees/) that are cleaned up after the
                                    // transform completes.
                                    let real_changes = out
                                        .lines()
                                        .filter(|l| !l.is_empty())
                                        .filter(|l| {
                                            let path_part = if l.len() > 3 { &l[3..] } else { l };
                                            !path_part.starts_with(".gitignore")
                                                && !path_part.starts_with(".worktrees/")
                                                && !path_part.starts_with(".worktrees")
                                        })
                                        .count();
                                    if real_changes > 0 {
                                        errors.push(format!(
                                            "Worktree at {} has unexpected dirty state",
                                            path
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    current_path = None;
                    is_bare = false;
                }
            }
        }
        _ => {
            progress.on_warning("Could not verify worktree states");
        }
    }

    if errors.is_empty() {
        progress.on_step("Integrity check passed");
        Ok(())
    } else {
        for err in &errors {
            progress.on_warning(&format!("Integrity issue: {err}"));
        }
        anyhow::bail!(
            "Transform completed but integrity check found {} issue{}. \
             The repository may need manual inspection.",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
        )
    }
}

// ── Gitdir fixup ───────────────────────────────────────────────────────────

/// After moving `.git`, update worktree `.git` files to point to the new
/// worktrees registration paths.
///
/// Each file at `<new_git_dir>/worktrees/<name>/gitdir` contains the absolute
/// path to a worktree's `.git` file. We read that path and then overwrite the
/// worktree's `.git` file so it points back to `<new_git_dir>/worktrees/<name>`.
fn fixup_gitdir_references(new_git_dir: &Path) -> Result<()> {
    let worktrees_dir = new_git_dir.join("worktrees");
    if !worktrees_dir.exists() {
        return Ok(());
    }

    let entries = fs::read_dir(&worktrees_dir).with_context(|| {
        format!(
            "Failed to read worktrees directory: {}",
            worktrees_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let gitdir_file = entry.path().join("gitdir");
        if !gitdir_file.exists() {
            continue;
        }

        // The gitdir file contains the absolute path to the worktree's .git file
        let worktree_git_path = fs::read_to_string(&gitdir_file)
            .with_context(|| format!("Failed to read gitdir file: {}", gitdir_file.display()))?;
        let worktree_git_path = PathBuf::from(worktree_git_path.trim());

        if !worktree_git_path.exists() {
            continue;
        }

        // Skip if the path is a directory — this means .git moved INTO this
        // worktree's location (e.g., contained-classic where the default
        // branch directory IS where .git lives). No pointer file to update.
        if worktree_git_path.is_dir() {
            continue;
        }

        // Update the worktree's .git file to point to the new registration path
        let new_registration_path = entry.path();
        fs::write(
            &worktree_git_path,
            format!("gitdir: {}", new_registration_path.display()),
        )
        .with_context(|| {
            format!(
                "Failed to update .git file at {}",
                worktree_git_path.display()
            )
        })?;
    }

    Ok(())
}

// ── Rollback ───────────────────────────────────────────────────────────────

/// Compute the reverse operation for rollback purposes.
///
/// Returns `None` for operations that cannot be meaningfully reversed
/// (stash, register/unregister, index init, validation, directory creation).
fn reverse_op(op: &TransformOp) -> Option<TransformOp> {
    match op {
        TransformOp::MoveWorktree { branch, from, to } => Some(TransformOp::MoveWorktree {
            branch: branch.clone(),
            from: to.clone(),
            to: from.clone(),
        }),

        TransformOp::MoveGitDir { from, to } => Some(TransformOp::MoveGitDir {
            from: to.clone(),
            to: from.clone(),
        }),

        TransformOp::SetBare(bare) => Some(TransformOp::SetBare(!bare)),

        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
        } => Some(TransformOp::NestFromRoot {
            root_path: root_path.clone(),
            subdir_path: worktree_path.clone(),
        }),

        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
        } => Some(TransformOp::CollapseIntoRoot {
            worktree_path: subdir_path.clone(),
            root_path: root_path.clone(),
        }),

        // These operations are not easily reversible
        TransformOp::StashChanges { .. }
        | TransformOp::PopStash { .. }
        | TransformOp::RegisterWorktree { .. }
        | TransformOp::UnregisterWorktree { .. }
        | TransformOp::InitWorktreeIndex { .. }
        | TransformOp::CreateDirectory { .. }
        | TransformOp::ValidateIntegrity => None,
    }
}

/// Execute reverse operations in reverse order to undo a partial transform.
fn rollback(
    stack: &[TransformOp],
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let mut first_error: Option<anyhow::Error> = None;

    for op in stack.iter().rev() {
        progress.on_step(&format!("Rollback: {}", describe_op(op)));
        if let Err(e) = execute_op(op, git, progress) {
            progress.on_warning(&format!("Rollback step failed: {e:#}"));
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    match first_error {
        Some(e) => Err(e.context("Rollback completed with errors")),
        None => {
            progress.on_step("Rollback completed successfully");
            Ok(())
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Walk up from a path and remove empty directories until we hit a non-empty
/// one or reach a filesystem root.
fn cleanup_empty_parents(path: &Path) {
    let mut current = path.to_path_buf();
    while let Some(parent) = current.parent() {
        // Stop at filesystem root
        if parent == current {
            break;
        }
        // Try to remove — will fail (harmlessly) if non-empty
        if fs::remove_dir(&current).is_err() {
            break;
        }
        current = parent.to_path_buf();
    }
}

/// Human-readable description of a transform operation, suitable for progress
/// output.
pub fn describe_op(op: &TransformOp) -> String {
    match op {
        TransformOp::StashChanges { branch, .. } => {
            format!("Stash changes in '{branch}'")
        }
        TransformOp::PopStash { branch, .. } => {
            format!("Restore stashed changes in '{branch}'")
        }
        TransformOp::MoveWorktree { branch, from, to } => {
            format!(
                "Move worktree '{branch}': {} -> {}",
                from.display(),
                to.display()
            )
        }
        TransformOp::MoveGitDir { from, to } => {
            format!("Move .git: {} -> {}", from.display(), to.display())
        }
        TransformOp::SetBare(bare) => {
            format!("Set core.bare = {bare}")
        }
        TransformOp::RegisterWorktree { branch, path } => {
            format!("Register worktree '{branch}' at {}", path.display())
        }
        TransformOp::UnregisterWorktree { branch } => {
            format!("Unregister worktree '{branch}'")
        }
        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
        } => {
            format!(
                "Collapse {} into {}",
                worktree_path.display(),
                root_path.display()
            )
        }
        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
        } => {
            format!(
                "Nest {} into {}",
                root_path.display(),
                subdir_path.display()
            )
        }
        TransformOp::InitWorktreeIndex { path } => {
            format!("Initialize worktree index at {}", path.display())
        }
        TransformOp::CreateDirectory { path } => {
            format!("Create directory {}", path.display())
        }
        TransformOp::ValidateIntegrity => "Validate repository integrity".to_string(),
    }
}
