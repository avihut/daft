use anyhow::{Context, Result};
use clap::Parser;
use daft::{
    config::counters::{INITIAL_BRANCHES_DELETED, INITIAL_WORKTREES_REMOVED, OPERATION_INCREMENT},
    git::GitCommand,
    is_git_repository, log_debug, log_error, log_info,
    logging::init_logging,
    remote::remote_branch_exists,
    WorktreeConfig,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-prune")]
#[command(version)]
#[command(about = "Prunes local Git branches whose remote counterparts have been deleted")]
#[command(long_about = r#"
Prunes local Git branches whose remote counterparts have been deleted,
ensuring any associated worktrees are removed first.
"#)]
struct Args {
    #[arg(short, long, help = "Enable verbose output")]
    verbose: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on verbosity flag
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    run_prune()?;
    Ok(())
}

fn run_prune() -> Result<()> {
    let config = WorktreeConfig::default();
    let git = GitCommand::new(config.quiet);

    log_info!(
        "Fetching from remote {} and pruning stale remote-tracking branches...",
        config.remote_name
    );
    git.fetch(&config.remote_name, true)
        .context("git fetch failed")?;

    log_info!("Identifying local branches whose upstream branch is gone...");

    let mut gone_branches = Vec::new();

    // Method 1: Use git branch -vv and grep to find branches with gone upstream
    let branch_output = git.branch_list_verbose()?;
    for line in branch_output.lines() {
        if line.contains(": gone]") {
            // Extract branch name - it's the first word, removing the * if current branch
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(branch_name) = parts.first() {
                let branch_name = branch_name.trim_start_matches('*').trim();
                if !branch_name.is_empty() {
                    gone_branches.push(branch_name.to_string());
                }
            }
        }
    }

    // Method 2: Also check for branches that don't exist on remote but have worktrees
    println!("Checking for branches with worktrees that don't exist on remote...");
    let ref_output = git.for_each_ref("%(refname:short)", "refs/heads")?;

    for line in ref_output.lines() {
        let branch_name = line.trim();
        if branch_name.is_empty() || branch_name == "master" || branch_name == "main" {
            continue;
        }

        // Check if this branch has a worktree
        let worktree_output = git.worktree_list_porcelain()?;
        let target_branch_ref = format!("refs/heads/{branch_name}");
        let mut has_worktree = false;

        let mut current_path = String::new();
        for wt_line in worktree_output.lines() {
            if wt_line.starts_with("worktree ") {
                current_path = wt_line.strip_prefix("worktree ").unwrap_or("").to_string();
            } else if !current_path.is_empty() && wt_line == format!("branch {target_branch_ref}") {
                has_worktree = true;
                break;
            } else if wt_line.is_empty() {
                current_path.clear();
            }
        }

        if has_worktree {
            // Check if this branch exists on remote
            if !remote_branch_exists(&config.remote_name, branch_name)?
                && !gone_branches.contains(&branch_name.to_string())
            {
                gone_branches.push(branch_name.to_string());
                log_debug!("Found branch with worktree not on remote: {branch_name}");
            }
        }
    }

    if gone_branches.is_empty() {
        log_info!("No local branches found that need to be pruned. Nothing to do.");
        return Ok(());
    }

    println!(
        "Found {} branches to potentially prune:",
        gone_branches.len()
    );
    for branch in &gone_branches {
        println!(" - {branch}");
    }
    println!();

    let mut branches_deleted = INITIAL_BRANCHES_DELETED;
    let mut worktrees_removed = INITIAL_WORKTREES_REMOVED;

    for branch_name in &gone_branches {
        println!("--- Processing branch: {branch_name} ---");

        // Check for worktree using --porcelain
        let worktree_output = git.worktree_list_porcelain()?;
        let target_branch_ref = format!("refs/heads/{branch_name}");
        let mut worktree_path = String::new();
        let mut current_path = String::new();

        for line in worktree_output.lines() {
            if line.starts_with("worktree ") {
                current_path = line.strip_prefix("worktree ").unwrap_or("").to_string();
            } else if !current_path.is_empty() && line == format!("branch {target_branch_ref}") {
                worktree_path = current_path.clone();
                break;
            } else if line.is_empty() {
                current_path.clear();
            }
        }

        if !worktree_path.is_empty() {
            println!("Found associated worktree for {branch_name} at: {worktree_path}");

            let wt_path = PathBuf::from(&worktree_path);
            if wt_path.exists() {
                println!("Attempting to remove worktree...");
                if let Err(e) = git.worktree_remove(&wt_path, true) {
                    log_error!(
                        "Error: Failed to remove worktree {worktree_path}: {e}. Skipping deletion of branch {branch_name}."
                    );
                    continue;
                }
                println!("Worktree at {worktree_path} removed successfully.");
                worktrees_removed += OPERATION_INCREMENT;
            } else {
                println!("Warning: Worktree directory {worktree_path} not found. Attempting git worktree prune might be needed separately.");
                println!("Attempting to force remove the worktree record anyway...");
                if let Err(e) = git.worktree_remove(&wt_path, true) {
                    eprintln!("Error: Failed to remove potentially orphaned worktree record {worktree_path}: {e}. Skipping deletion of branch {branch_name}.");
                    continue;
                }
                println!("Worktree record for {worktree_path} removed successfully.");
                worktrees_removed += OPERATION_INCREMENT;
            }
        } else {
            println!("No associated worktree found for {branch_name}.");
        }

        // Now, attempt to delete the local branch
        println!("Attempting to delete local branch {branch_name}...");
        if let Err(e) = git.branch_delete(branch_name, true) {
            eprintln!("Error: Failed to delete branch {branch_name}: {e}");
        } else {
            println!("Local branch {branch_name} deleted successfully.");
            branches_deleted += OPERATION_INCREMENT;
        }

        println!("----------------------------------------");
    }

    println!();
    println!("--- Summary ---");
    println!("Branches deleted: {branches_deleted}");
    println!("Worktrees removed: {worktrees_removed}");
    println!("Pruning process complete.");

    // Check if any worktrees might need manual pruning
    let worktree_list = git.worktree_list_porcelain()?;
    if worktree_list.contains("prunable") {
        println!();
        println!(
            "Note: Some prunable worktree data may exist. Run git worktree prune to clean up."
        );
    }

    Ok(())
}
