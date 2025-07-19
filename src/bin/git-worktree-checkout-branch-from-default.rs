use anyhow::{Context, Result};
use clap::Parser;
use git_worktree_workflow::{
    config::git::DEFAULT_EXIT_CODE, get_git_common_dir, is_git_repository,
    remote::get_default_branch_local, utils::*, WorktreeConfig,
};
use std::process::Command;

#[derive(Parser)]
#[command(name = "git-worktree-checkout-branch-from-default")]
#[command(about = "Creates a git worktree and branch based on the remote's default branch")]
#[command(long_about = r#"
Creates a git worktree and branch based on the REMOTE'S DEFAULT branch
(e.g., main, master). It determines the default branch and then calls 'git-worktree-checkout-branch'.
"#)]
struct Args {
    #[arg(help = "The name for the new branch and the worktree directory")]
    new_branch_name: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout_branch_from_default(&args) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout_branch_from_default(args: &Args) -> Result<()> {
    validate_branch_name(&args.new_branch_name)?;

    let config = WorktreeConfig::default();

    println!(
        "--> Determining default branch for remote '{}'...",
        config.remote_name
    );

    let git_common_dir = get_git_common_dir()?;
    let default_branch = get_default_branch_local(&git_common_dir, &config.remote_name)
        .context("Failed to determine default branch")?;

    println!(
        "--> [git-worktree-checkout-branch-from-default] Detected default origin branch: '{default_branch}'"
    );

    println!("--> [git-worktree-checkout-branch-from-default] Calling git-worktree-checkout-branch '{}' '{}'...", 
        args.new_branch_name, default_branch);

    let current_exe = std::env::current_exe()?;
    let exe_dir = current_exe
        .parent()
        .context("Failed to get executable directory")?;

    let checkout_branch_exe = exe_dir.join("git-worktree-checkout-branch");

    let status = Command::new(&checkout_branch_exe)
        .arg(&args.new_branch_name)
        .arg(&default_branch)
        .status()
        .context("Failed to execute git-worktree-checkout-branch")?;

    if !status.success() {
        anyhow::bail!(
            "git-worktree-checkout-branch failed with exit code: {}",
            status.code().unwrap_or(DEFAULT_EXIT_CODE)
        );
    }

    Ok(())
}
