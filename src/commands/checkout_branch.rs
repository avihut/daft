use anyhow::Result;
use clap::Parser;
use daft::{
    config::git::{COMMITS_AHEAD_THRESHOLD, DEFAULT_COMMIT_COUNT},
    direnv::run_direnv_allow,
    get_current_branch, get_project_root,
    git::GitCommand,
    is_git_repository, log_error, log_info, log_warning, logging, output_cd_path,
    utils::*,
    WorktreeConfig,
};

#[derive(Parser)]
#[command(name = "git-worktree-checkout-branch")]
#[command(version = daft::VERSION)]
#[command(about = "Creates a git worktree with a new branch")]
#[command(long_about = r#"
Creates a git worktree at the project root level, create a new branch based on either
the CURRENT branch or a specified base branch, push the new branch to origin, set upstream tracking,
run 'direnv allow' (if direnv exists), and finally cd into the new worktree.

Can be run from anywhere within the Git repository (including deep subdirectories).
The new worktree will be created at the project root level (alongside .git directory).
"#)]
pub struct Args {
    #[arg(help = "The name for the new branch and the worktree directory")]
    new_branch_name: String,

    #[arg(help = "The branch to base the new branch on (defaults to current branch)")]
    base_branch_name: Option<String>,

    #[arg(short, long, help = "Suppress non-essential output")]
    quiet: bool,

    #[arg(short, long, help = "Enable verbose debug output")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Carry uncommitted changes to the new worktree (default)"
    )]
    carry: bool,

    #[arg(long, help = "Don't carry uncommitted changes to the new worktree")]
    no_carry: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on verbosity flags
    logging::init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout_branch(&args) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout_branch(args: &Args) -> Result<()> {
    validate_branch_name(&args.new_branch_name)?;

    let base_branch = match &args.base_branch_name {
        Some(branch) => {
            log_info!("--> Using explicitly provided base branch: '{branch}'");
            branch.clone()
        }
        None => {
            log_info!("--> Base branch not specified, using current branch...");
            let current = get_current_branch()?;
            log_info!("--> Using current branch as base: '{current}'");
            current
        }
    };

    let project_root = get_project_root()?;
    let worktree_path = project_root.join(&args.new_branch_name);

    let config = WorktreeConfig::default();
    let git = GitCommand::new(args.quiet);

    // Fetch latest changes from remote to ensure we have the latest version of the base branch
    log_info!(
        "Fetching latest changes from remote '{}'...",
        config.remote_name
    );
    if let Err(e) = git.fetch(&config.remote_name, false) {
        log_warning!(
            "Failed to fetch from remote '{}': {}",
            config.remote_name,
            e
        );
    }

    // Ensure remote tracking branches are created (needed for bare repositories)
    log_info!("--> Setting up remote tracking branches...");
    if let Err(e) = git.fetch_refspec(
        &config.remote_name,
        &format!("+refs/heads/*:refs/remotes/{}/*", config.remote_name),
    ) {
        log_warning!("Failed to set up remote tracking branches: {e}");
    }

    // Three-way branch selection algorithm for optimal worktree base branch
    // This sophisticated algorithm ensures we always use the most appropriate branch
    // as the base for creating new worktrees, considering both local and remote states.
    let local_branch_ref = format!("refs/heads/{base_branch}");
    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, base_branch);

    let checkout_base =
        if git.show_ref_exists(&remote_branch_ref)? && git.show_ref_exists(&local_branch_ref)? {
            // Case 1: Both local and remote branches exist - Advanced conflict resolution
            //
            // This is the most complex scenario where we need to determine which branch
            // represents the "better" starting point for new development. We use commit
            // comparison to intelligently choose between local and remote versions.
            //
            // Strategy: Check if local branch has commits that are ahead of remote.
            // - If local is ahead: Prefer local (indicates active local development)
            // - If local is equal/behind: Prefer remote (ensures latest upstream changes)
            let local_ahead = git
                .rev_list_count(&format!(
                    "{}..{}",
                    &format!("{}/{}", config.remote_name, base_branch),
                    &base_branch
                ))
                .unwrap_or(DEFAULT_COMMIT_COUNT)
                > COMMITS_AHEAD_THRESHOLD;

            if local_ahead {
                // Local branch has unpushed commits - prioritize preserving local work
                log_info!("Using local branch '{base_branch}' as base (has local commits)");
                base_branch.clone()
            } else {
                // Remote branch is equal or ahead - use remote for latest changes
                log_info!(
                    "Using remote branch '{}/{}' as base (has latest changes)",
                    config.remote_name,
                    base_branch
                );
                format!("{}/{}", config.remote_name, base_branch)
            }
        } else if git.show_ref_exists(&local_branch_ref)? {
            // Case 2: Only local branch exists - Use local branch
            // This handles cases where the branch exists locally but hasn't been pushed
            // to remote yet, or where remote tracking has been lost.
            log_info!("Using local branch '{base_branch}' as base");
            base_branch.clone()
        } else if git.show_ref_exists(&remote_branch_ref)? {
            // Case 3: Only remote branch exists - Use remote branch
            // This handles cases where the branch exists on remote but hasn't been
            // checked out locally yet, common in team development scenarios.
            log_info!(
                "Local branch '{}' not found, using remote branch '{}/{}'",
                base_branch,
                config.remote_name,
                base_branch
            );
            format!("{}/{}", config.remote_name, base_branch)
        } else {
            // Case 4: Neither local nor remote branch exists - Use branch name as-is
            // This is a fallback case where the specified branch doesn't exist anywhere.
            // Git will handle the error appropriately during worktree creation if the
            // branch truly doesn't exist, or create it if it's meant to be a new branch.
            log_info!("Neither local nor remote branch found for '{base_branch}', using as-is");
            base_branch.clone()
        };

    // At this point, checkout_base contains the optimal branch reference determined
    // by our three-way selection algorithm, ready for worktree creation

    // Check for uncommitted changes and stash them if --no-carry is not set
    let stash_created = if !args.no_carry {
        match git.has_uncommitted_changes() {
            Ok(true) => {
                println!("--> Stashing uncommitted changes...");
                if let Err(e) = git.stash_push_with_untracked("daft: carry changes to new worktree")
                {
                    log_error!("Failed to stash changes: {e}");
                    anyhow::bail!("Failed to stash uncommitted changes: {e}");
                }
                true
            }
            Ok(false) => {
                log_info!("--> No uncommitted changes to carry");
                false
            }
            Err(e) => {
                log_warning!("Could not check for uncommitted changes: {e}");
                false
            }
        }
    } else {
        log_info!("--> Skipping carry (--no-carry flag set)");
        false
    };

    log_info!("Attempting to create Git worktree:");
    log_info!("  Path:         {}", worktree_path.display());
    log_info!("  New Branch:   {}", args.new_branch_name);
    println!("  From Branch:  {checkout_base}");
    println!("  Project Root: {}", project_root.display());
    println!("---");

    if let Err(e) =
        git.worktree_add_new_branch(&worktree_path, &args.new_branch_name, &checkout_base)
    {
        // If worktree creation fails and we stashed changes, restore them
        if stash_created {
            log_info!("--> Restoring stashed changes due to worktree creation failure...");
            if let Err(pop_err) = git.stash_pop() {
                log_error!("Failed to restore stashed changes: {pop_err}");
                eprintln!("Warning: Your changes are still in the stash. Run 'git stash pop' to restore them.");
            }
        }
        anyhow::bail!("Failed to create git worktree: {}", e);
    }

    if !worktree_path.exists() {
        anyhow::bail!(
            "Worktree directory was not created at '{}'",
            worktree_path.display()
        );
    }

    println!(
        "Git worktree created successfully at '{}'.",
        worktree_path.display()
    );
    println!("---");

    println!(
        "--> Changing directory to worktree: {}",
        worktree_path.display()
    );
    change_directory(&worktree_path)?;
    println!(
        "--> Successfully changed directory to {}",
        get_current_directory()?.display()
    );

    // Apply stashed changes to the new worktree
    if stash_created {
        println!("--> Applying stashed changes to new worktree...");
        if let Err(e) = git.stash_pop() {
            log_error!("Failed to apply stashed changes: {e}");
            eprintln!(
                "Stash could not be applied cleanly. Resolve conflicts and run: git stash pop"
            );
        } else {
            println!("--> Changes successfully applied to new worktree");
        }
    }

    println!(
        "--> Attempting: git push --set-upstream {} \"{}\"",
        config.remote_name, args.new_branch_name
    );

    if let Err(e) = git.push_set_upstream(&config.remote_name, &args.new_branch_name) {
        log_error!("---");
        log_error!(
            "Error: Failed to push branch '{}' to '{}' or set upstream: {}",
            args.new_branch_name,
            config.remote_name,
            e
        );
        eprintln!(
            "Worktree was created at '{}', but push/tracking failed.",
            get_current_directory()?.display()
        );
        eprintln!(
            "You ARE currently in the new worktree directory: {}",
            get_current_directory()?.display()
        );
        eprintln!("You may need to resolve the push issue manually.");
        return Err(e);
    }

    println!(
        "--> Push to '{}' and upstream tracking set successfully.",
        config.remote_name
    );

    run_direnv_allow(&get_current_directory()?, config.quiet)?;

    println!("---");
    println!("Overall Success: Worktree created, branch pushed/tracked, direnv handled (if present), and CD'd into worktree.");

    output_cd_path(&get_current_directory()?);

    Ok(())
}
