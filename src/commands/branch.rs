//! Branch management command.
//!
//! Provides `git daft branch` subcommand for branch-related operations.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use daft::{
    get_project_root,
    git::GitCommand,
    is_git_repository,
    multi_remote::path::calculate_worktree_path,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
};
use std::io::{self, Write};

#[derive(Parser)]
#[command(name = "branch")]
#[command(about = "Branch and worktree management operations")]
#[command(long_about = r#"
Provides branch and worktree management operations, particularly useful
in multi-remote workflows.

The main subcommand is `move`, which moves a worktree to a different
remote folder when multi-remote mode is enabled.
"#)]
pub struct Args {
    #[command(subcommand)]
    command: BranchCommand,
}

#[derive(Subcommand)]
enum BranchCommand {
    /// Move a worktree to a different remote folder
    #[command(long_about = r#"
Moves a worktree from one remote folder to another. This is useful when:

- You forked a branch and want to organize it under a different remote
- You're transferring a feature branch from your fork to upstream
- You want to reorganize worktrees after adding a new remote

The worktree is physically moved on disk, and git's internal worktree
records are updated accordingly.

Options like --set-upstream can update the branch's tracking configuration
to match the new remote organization.
"#)]
    Move {
        #[arg(help = "Branch name or worktree path to move")]
        branch: String,

        #[arg(long, help = "Target remote folder")]
        to: String,

        #[arg(
            long,
            help = "Also update the branch's upstream tracking to the new remote"
        )]
        set_upstream: bool,

        #[arg(long, help = "Push the branch to the new remote")]
        push: bool,

        #[arg(long, help = "Delete the branch from the old remote after pushing")]
        delete_old: bool,

        #[arg(long, help = "Preview changes without executing")]
        dry_run: bool,

        #[arg(short = 'f', long, help = "Skip confirmation")]
        force: bool,
    },
}

pub fn run() -> Result<()> {
    // Skip "daft" and "branch" from args
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);

    match args.command {
        BranchCommand::Move {
            branch,
            to,
            set_upstream,
            push,
            delete_old,
            dry_run,
            force,
        } => cmd_move(&branch, &to, set_upstream, push, delete_old, dry_run, force),
    }
}

/// Move a worktree to a different remote folder.
#[allow(clippy::fn_params_excessive_bools)]
fn cmd_move(
    branch: &str,
    to_remote: &str,
    set_upstream: bool,
    push: bool,
    delete_old: bool,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, true);
    let mut output = CliOutput::new(config);

    let project_root = get_project_root()?;
    let git = GitCommand::new(false);

    // Verify multi-remote mode is enabled
    if !settings.multi_remote_enabled {
        anyhow::bail!(
            "Multi-remote mode is not enabled.\n\
             Run 'git daft multi-remote enable' first, or use regular git commands."
        );
    }

    // Verify target remote exists
    let remotes = git.remote_list()?;
    if !remotes.contains(&to_remote.to_string()) {
        anyhow::bail!(
            "Remote '{}' does not exist. Available remotes: {}",
            to_remote,
            remotes.join(", ")
        );
    }

    // Find the worktree for this branch
    let worktree_path = git
        .resolve_worktree_path(branch, &project_root)
        .context(format!("Could not find worktree for branch '{}'", branch))?;

    // Get branch name from worktree
    let branch_name =
        get_branch_name_for_worktree(&git, &worktree_path)?.unwrap_or_else(|| branch.to_string());

    // Determine current remote (from path structure)
    let current_remote = worktree_path
        .strip_prefix(&project_root)
        .ok()
        .and_then(|p| p.components().next())
        .and_then(|c| c.as_os_str().to_str())
        .map(String::from);

    output.step(&format!("Branch: {}", branch_name));
    output.step(&format!("Current location: {}", worktree_path.display()));

    if let Some(ref remote) = current_remote {
        output.step(&format!("Current remote folder: {}", remote));
    }
    output.step(&format!("Target remote folder: {}", to_remote));

    // Check if already in target location
    if current_remote.as_deref() == Some(to_remote) {
        output.result("Worktree is already in the target remote folder");
        return Ok(());
    }

    // Calculate new path
    let new_path = calculate_worktree_path(&project_root, &branch_name, to_remote, true);
    output.step(&format!("New location: {}", new_path.display()));

    // Check if target path already exists
    if new_path.exists() {
        anyhow::bail!(
            "Target path already exists: {}\n\
             Remove the existing worktree first or choose a different remote.",
            new_path.display()
        );
    }

    // Summary of actions
    output.step("");
    output.step("Actions to perform:");
    output.step(&format!(
        "  1. Move worktree: {} -> {}",
        worktree_path.display(),
        new_path.display()
    ));

    if set_upstream {
        output.step(&format!("  2. Set upstream: {}/{}", to_remote, branch_name));
    }

    if push {
        output.step(&format!("  3. Push to remote: {}", to_remote));
    }

    if delete_old {
        if let Some(ref old_remote) = current_remote {
            output.step(&format!(
                "  4. Delete from old remote: {}/{}",
                old_remote, branch_name
            ));
        }
    }

    if dry_run {
        output.result("Dry run complete - no changes made");
        return Ok(());
    }

    if !skip_confirm {
        print!("\nProceed? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            output.result("Aborted");
            return Ok(());
        }
    }

    // Execute the move
    output.step("Moving worktree...");

    // Ensure parent directory exists
    if let Some(parent) = new_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
    }

    git.worktree_move(&worktree_path, &new_path)
        .context("Failed to move worktree")?;

    // Set upstream if requested
    if set_upstream {
        output.step(&format!(
            "Setting upstream to {}/{}...",
            to_remote, branch_name
        ));

        // Need to change to the worktree directory to set upstream
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&new_path)?;

        let result = git.set_upstream(to_remote, &branch_name);

        // Restore original directory
        std::env::set_current_dir(&original_dir)?;

        if let Err(e) = result {
            output.warning(&format!("Failed to set upstream: {}", e));
        }
    }

    // Push if requested
    if push {
        output.step(&format!("Pushing to {}...", to_remote));

        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&new_path)?;

        let result = git.push_set_upstream(to_remote, &branch_name);

        std::env::set_current_dir(&original_dir)?;

        if let Err(e) = result {
            output.warning(&format!("Failed to push: {}", e));
        }
    }

    // Delete from old remote if requested
    if delete_old {
        if let Some(old_remote) = current_remote {
            output.step(&format!(
                "Deleting from old remote {}/{}...",
                old_remote, branch_name
            ));

            let result = delete_remote_branch(&git, &old_remote, &branch_name);
            if let Err(e) = result {
                output.warning(&format!("Failed to delete from old remote: {}", e));
            }
        }
    }

    // Clean up empty remote directories
    if let Some(old_remote) = worktree_path
        .strip_prefix(&project_root)
        .ok()
        .and_then(|p| p.components().next())
        .and_then(|c| c.as_os_str().to_str())
    {
        let old_remote_dir = project_root.join(old_remote);
        if old_remote_dir.exists() {
            if let Ok(mut entries) = std::fs::read_dir(&old_remote_dir) {
                if entries.next().is_none() {
                    // Directory is empty, remove it
                    let _ = std::fs::remove_dir(&old_remote_dir);
                }
            }
        }
    }

    output.result(&format!("Worktree moved to {}/{}", to_remote, branch_name));

    Ok(())
}

/// Get the branch name for a worktree.
fn get_branch_name_for_worktree(
    git: &GitCommand,
    worktree_path: &std::path::Path,
) -> Result<Option<String>> {
    let porcelain_output = git.worktree_list_porcelain()?;

    let mut current_path: Option<std::path::PathBuf> = None;

    for line in porcelain_output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            if let Some(prev_path) = current_path.take() {
                if prev_path == worktree_path {
                    // We've moved past this worktree without finding a branch
                    return Ok(None);
                }
            }
            current_path = Some(std::path::PathBuf::from(path_str));
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if current_path.as_ref() == Some(&worktree_path.to_path_buf()) {
                return Ok(branch_ref.strip_prefix("refs/heads/").map(String::from));
            }
        }
    }

    Ok(None)
}

/// Delete a branch from a remote.
fn delete_remote_branch(_git: &GitCommand, remote: &str, branch: &str) -> Result<()> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["push", remote, "--delete", branch])
        .output()
        .context("Failed to execute git push --delete")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to delete remote branch: {}", stderr);
    }

    Ok(())
}
