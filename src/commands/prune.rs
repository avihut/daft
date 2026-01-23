use anyhow::{Context, Result};
use clap::Parser;
use daft::{
    config::counters::{INITIAL_BRANCHES_DELETED, INITIAL_WORKTREES_REMOVED, OPERATION_INCREMENT},
    get_git_common_dir, get_project_root,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, RemovalReason},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::remote_branch_exists,
    WorktreeConfig,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-prune")]
#[command(version = daft::VERSION)]
#[command(about = "Prunes local Git branches whose remote counterparts have been deleted")]
#[command(long_about = r#"
Prunes local Git branches whose remote counterparts have been deleted,
ensuring any associated worktrees are removed first.
"#)]
pub struct Args {
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

    let config = OutputConfig::new(false, args.verbose);
    let mut output = CliOutput::new(config);

    run_prune(&mut output)?;
    Ok(())
}

fn run_prune(output: &mut dyn Output) -> Result<()> {
    let config = WorktreeConfig::default();
    let git = GitCommand::new(output.is_quiet());
    let project_root = get_project_root()?;
    let git_dir = get_git_common_dir()?;
    let source_worktree = std::env::current_dir()?;

    output.step(&format!(
        "Fetching from remote {} and pruning stale remote-tracking branches...",
        config.remote_name
    ));
    git.fetch(&config.remote_name, true)
        .context("git fetch failed")?;

    output.step("Identifying local branches whose upstream branch is gone...");

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
    output.step("Checking for branches with worktrees that don't exist on remote...");
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
                output.debug(&format!(
                    "Found branch with worktree not on remote: {branch_name}"
                ));
            }
        }
    }

    if gone_branches.is_empty() {
        output.result("Nothing to prune");
        return Ok(());
    }

    output.step(&format!(
        "Found {} branches to potentially prune",
        gone_branches.len()
    ));
    for branch in &gone_branches {
        output.step(&format!(" - {branch}"));
    }

    let mut branches_deleted = INITIAL_BRANCHES_DELETED;
    let mut worktrees_removed = INITIAL_WORKTREES_REMOVED;

    for branch_name in &gone_branches {
        output.step(&format!("Processing branch: {branch_name}"));

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
            output.step(&format!(
                "Found associated worktree for {branch_name} at: {worktree_path}"
            ));

            let wt_path = PathBuf::from(&worktree_path);

            // Run pre-remove hook
            if let Err(e) = run_pre_remove_hook(
                &project_root,
                &git_dir,
                &config.remote_name,
                &source_worktree,
                &wt_path,
                branch_name,
                output,
            ) {
                output.warning(&format!("Pre-remove hook failed for {branch_name}: {e}"));
                // Continue with removal even if hook fails (warn mode)
            }

            if wt_path.exists() {
                output.step("Removing worktree...");
                if let Err(e) = git.worktree_remove(&wt_path, true) {
                    output.error(&format!(
                        "Failed to remove worktree {worktree_path}: {e}. Skipping deletion of branch {branch_name}."
                    ));
                    continue;
                }
                output.step(&format!("Worktree at {worktree_path} removed"));
                worktrees_removed += OPERATION_INCREMENT;
            } else {
                output.warning(&format!(
                    "Worktree directory {worktree_path} not found. Attempting to force remove record."
                ));
                if let Err(e) = git.worktree_remove(&wt_path, true) {
                    output.error(&format!(
                        "Failed to remove orphaned worktree record {worktree_path}: {e}. Skipping deletion of branch {branch_name}."
                    ));
                    continue;
                }
                output.step(&format!("Worktree record for {worktree_path} removed"));
                worktrees_removed += OPERATION_INCREMENT;
            }

            // Run post-remove hook
            if let Err(e) = run_post_remove_hook(
                &project_root,
                &git_dir,
                &config.remote_name,
                &source_worktree,
                &wt_path,
                branch_name,
                output,
            ) {
                output.warning(&format!("Post-remove hook failed for {branch_name}: {e}"));
            }
        } else {
            output.step(&format!("No associated worktree found for {branch_name}"));
        }

        // Now, attempt to delete the local branch
        output.step(&format!("Deleting local branch {branch_name}..."));
        if let Err(e) = git.branch_delete(branch_name, true) {
            output.error(&format!("Failed to delete branch {branch_name}: {e}"));
        } else {
            output.step(&format!("Branch {branch_name} deleted"));
            branches_deleted += OPERATION_INCREMENT;
        }
    }

    // Git-like result message
    if branches_deleted > 0 || worktrees_removed > 0 {
        output.result(&format!(
            "Pruned {} branches, removed {} worktrees",
            branches_deleted, worktrees_removed
        ));
    } else {
        output.result("Nothing pruned");
    }

    // Check if any worktrees might need manual pruning
    let worktree_list = git.worktree_list_porcelain()?;
    if worktree_list.contains("prunable") {
        output.warning(
            "Some prunable worktree data may exist. Run 'git worktree prune' to clean up.",
        );
    }

    Ok(())
}

fn run_pre_remove_hook(
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    source_worktree: &PathBuf,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let ctx = HookContext::new(
        HookType::PreRemove,
        "prune",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::RemoteDeleted);

    executor.execute(&ctx, output)?;

    Ok(())
}

fn run_post_remove_hook(
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    source_worktree: &PathBuf,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let ctx = HookContext::new(
        HookType::PostRemove,
        "prune",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::RemoteDeleted);

    executor.execute(&ctx, output)?;

    Ok(())
}
