use anyhow::Result;
use clap::Parser;
use daft::{
    config::git::{COMMITS_AHEAD_THRESHOLD, DEFAULT_COMMIT_COUNT},
    direnv::run_direnv_allow,
    get_current_branch, get_project_root,
    git::GitCommand,
    is_git_repository, logging,
    output::{CliOutput, Output, OutputConfig},
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

    let config = OutputConfig::new(args.quiet, args.verbose);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout_branch(&args, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout_branch(args: &Args, output: &mut dyn Output) -> Result<()> {
    validate_branch_name(&args.new_branch_name)?;

    let base_branch = match &args.base_branch_name {
        Some(branch) => {
            output.step(&format!(
                "Using explicitly provided base branch: '{branch}'"
            ));
            branch.clone()
        }
        None => {
            output.step("Base branch not specified, using current branch...");
            let current = get_current_branch()?;
            output.step(&format!("Using current branch as base: '{current}'"));
            current
        }
    };

    let project_root = get_project_root()?;
    let worktree_path = project_root.join(&args.new_branch_name);

    let config = WorktreeConfig::default();
    let git = GitCommand::new(output.is_quiet());

    // Fetch latest changes from remote to ensure we have the latest version of the base branch
    output.step(&format!(
        "Fetching latest changes from remote '{}'...",
        config.remote_name
    ));
    if let Err(e) = git.fetch(&config.remote_name, false) {
        output.warning(&format!(
            "Failed to fetch from remote '{}': {}",
            config.remote_name, e
        ));
    }

    // Ensure remote tracking branches are created (needed for bare repositories)
    output.step("Setting up remote tracking branches...");
    if let Err(e) = git.fetch_refspec(
        &config.remote_name,
        &format!("+refs/heads/*:refs/remotes/{}/*", config.remote_name),
    ) {
        output.warning(&format!("Failed to set up remote tracking branches: {e}"));
    }

    // Three-way branch selection algorithm for optimal worktree base branch
    let local_branch_ref = format!("refs/heads/{base_branch}");
    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, base_branch);

    let checkout_base =
        if git.show_ref_exists(&remote_branch_ref)? && git.show_ref_exists(&local_branch_ref)? {
            // Both local and remote exist - use commit comparison
            let local_ahead = git
                .rev_list_count(&format!(
                    "{}..{}",
                    &format!("{}/{}", config.remote_name, base_branch),
                    &base_branch
                ))
                .unwrap_or(DEFAULT_COMMIT_COUNT)
                > COMMITS_AHEAD_THRESHOLD;

            if local_ahead {
                output.step(&format!(
                    "Using local branch '{base_branch}' as base (has local commits)"
                ));
                base_branch.clone()
            } else {
                output.step(&format!(
                    "Using remote branch '{}/{}' as base (has latest changes)",
                    config.remote_name, base_branch
                ));
                format!("{}/{}", config.remote_name, base_branch)
            }
        } else if git.show_ref_exists(&local_branch_ref)? {
            output.step(&format!("Using local branch '{base_branch}' as base"));
            base_branch.clone()
        } else if git.show_ref_exists(&remote_branch_ref)? {
            output.step(&format!(
                "Local branch '{}' not found, using remote branch '{}/{}'",
                base_branch, config.remote_name, base_branch
            ));
            format!("{}/{}", config.remote_name, base_branch)
        } else {
            output.step(&format!(
                "Neither local nor remote branch found for '{base_branch}', using as-is"
            ));
            base_branch.clone()
        };

    // Check for uncommitted changes and stash them if --no-carry is not set
    let stash_created = if !args.no_carry {
        match git.has_uncommitted_changes() {
            Ok(true) => {
                output.step("Stashing uncommitted changes...");
                if let Err(e) = git.stash_push_with_untracked("daft: carry changes to new worktree")
                {
                    anyhow::bail!("Failed to stash uncommitted changes: {e}");
                }
                true
            }
            Ok(false) => {
                output.step("No uncommitted changes to carry");
                false
            }
            Err(e) => {
                output.warning(&format!("Could not check for uncommitted changes: {e}"));
                false
            }
        }
    } else {
        output.step("Skipping carry (--no-carry flag set)");
        false
    };

    output.step(&format!(
        "Creating worktree at '{}' with new branch '{}' from '{}'",
        worktree_path.display(),
        args.new_branch_name,
        checkout_base
    ));

    if let Err(e) =
        git.worktree_add_new_branch(&worktree_path, &args.new_branch_name, &checkout_base)
    {
        // If worktree creation fails and we stashed changes, restore them
        if stash_created {
            output.step("Restoring stashed changes due to worktree creation failure...");
            if let Err(pop_err) = git.stash_pop() {
                output.warning(&format!(
                    "Your changes are still in the stash. Run 'git stash pop' to restore them. Error: {pop_err}"
                ));
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

    output.step(&format!(
        "Changing directory to worktree: {}",
        worktree_path.display()
    ));
    change_directory(&worktree_path)?;

    // Apply stashed changes to the new worktree
    if stash_created {
        output.step("Applying stashed changes to new worktree...");
        if let Err(e) = git.stash_pop() {
            output.warning(&format!(
                "Stash could not be applied cleanly. Resolve conflicts and run 'git stash pop'. Error: {e}"
            ));
        } else {
            output.step("Changes successfully applied to new worktree");
        }
    }

    output.step(&format!(
        "Pushing and setting upstream to '{}/{}'...",
        config.remote_name, args.new_branch_name
    ));

    if let Err(e) = git.push_set_upstream(&config.remote_name, &args.new_branch_name) {
        output.warning(&format!(
            "Failed to push branch '{}' to '{}' or set upstream: {}. You may need to resolve the push issue manually.",
            args.new_branch_name, config.remote_name, e
        ));
        return Err(e);
    }

    output.step(&format!(
        "Push to '{}' and upstream tracking set successfully",
        config.remote_name
    ));

    run_direnv_allow(&get_current_directory()?, output.is_quiet())?;

    // Git-like result message
    output.result(&format!(
        "Created worktree '{}' from '{}'",
        args.new_branch_name, checkout_base
    ));

    output.cd_path(&get_current_directory()?);

    Ok(())
}
