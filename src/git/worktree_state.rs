//! Linked-worktree registration bookkeeping.
//!
//! A linked worktree is two files and a directory: `<worktree>/.git` (a
//! pointer file, `gitdir: <registration>`), and `<common>/worktrees/<name>/`
//! (the registration) holding `gitdir`, `commondir`, `HEAD` and every other
//! per-worktree file git keeps out of the common dir.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::core::ProgressSink;

/// Register the worktree with git's worktree tracking.
pub(crate) fn register_worktree(
    git_dir: &Path,
    worktree_path: &Path,
    current_branch: &str,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step(&format!(
        "Registering worktree for branch '{current_branch}'..."
    ));

    let worktree_git_file = worktree_path.join(".git");

    // Create .git file pointing to worktrees subdirectory
    let worktrees_root = git_dir.join("worktrees");
    let worktree_name = registration_name(&worktrees_root, worktree_path, current_branch)?;
    let worktrees_dir = worktrees_root.join(&worktree_name);
    fs::create_dir_all(&worktrees_dir).context("Failed to create worktrees directory")?;

    // Write gitdir file
    let gitdir_path = worktrees_dir.join("gitdir");
    fs::write(&gitdir_path, format!("{}\n", worktree_git_file.display()))
        .context("Failed to write gitdir file")?;

    // Write HEAD file
    let head_path = worktrees_dir.join("HEAD");
    fs::write(&head_path, format!("ref: refs/heads/{current_branch}\n"))
        .context("Failed to write HEAD file")?;

    // Write commondir file
    let commondir_path = worktrees_dir.join("commondir");
    fs::write(&commondir_path, "../..\n").context("Failed to write commondir file")?;

    // Update .git file in worktree
    let correct_gitdir = format!("gitdir: {}", worktrees_dir.display());
    fs::write(&worktree_git_file, correct_gitdir)
        .context("Failed to update .git file in worktree")?;

    Ok(())
}

/// Pick a free registration directory name for `worktree_path`.
///
/// Named the way `git worktree add` names them — after the path basename, with
/// a numeric suffix on collision. Deriving the name from the branch instead
/// (`task/x` -> `task-x`) can land on an unrelated worktree whose directory
/// happens to carry that name and silently overwrite its `gitdir`/`HEAD`.
fn registration_name(worktrees_root: &Path, worktree_path: &Path, branch: &str) -> Result<String> {
    let base = worktree_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| branch.replace('/', "-"));

    if !worktrees_root.join(&base).exists() {
        return Ok(base);
    }
    for n in 1..1000 {
        let candidate = format!("{base}{n}");
        if !worktrees_root.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    // Falling back to `base` here would hand back a name that is taken, and the
    // caller would overwrite that registration's `gitdir`/`HEAD` — the exact
    // silent clobber this function exists to prevent.
    anyhow::bail!(
        "Could not find a free worktree registration name for {} under {} \
         (tried '{base}' and '{base}1'..'{base}999').",
        worktree_path.display(),
        worktrees_root.display()
    )
}
