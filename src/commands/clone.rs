use anyhow::{Context, Result};
use clap::Parser;
use daft::{
    check_dependencies,
    direnv::run_direnv_allow,
    extract_repo_name,
    git::GitCommand,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::{get_default_branch_remote, get_remote_branches},
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-clone")]
#[command(version = daft::VERSION)]
#[command(about = "Clones a Git repository into a worktree-based directory structure")]
#[command(long_about = r#"
Clones a Git repository into a specific directory structure:
<repository_name>/<branch_name>

It determines the repository name from the URL and queries the remote
to find the default branch (e.g., main, master, develop) *before* cloning,
unless a specific branch is specified with -b.
After cloning, it runs 'direnv allow' in the new directory and cds into it.
"#)]
pub struct Args {
    #[arg(help = "Repository URL to clone")]
    repository_url: String,

    #[arg(
        short = 'b',
        long = "branch",
        help = "Checkout this branch instead of the remote's default branch"
    )]
    branch: Option<String>,

    #[arg(
        short = 'n',
        long = "no-checkout",
        help = "Only clone the repository and create the .git folder but do not checkout the default branch worktree"
    )]
    no_checkout: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Suppress all output and run silently"
    )]
    quiet: bool,

    #[arg(short = 'v', long = "verbose", help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        short = 'a',
        long = "all-branches",
        help = "Create worktrees for all remote branches, not just the default"
    )]
    all_branches: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on verbose flag
    init_logging(args.verbose);

    if args.no_checkout && args.all_branches {
        anyhow::bail!("--no-checkout and --all-branches cannot be used together.\nUse --no-checkout to create only the bare repository, or --all-branches to create worktrees for all branches.");
    }

    if args.branch.is_some() && args.all_branches {
        anyhow::bail!("--branch and --all-branches cannot be used together.\nUse --branch to checkout a specific branch, or --all-branches to create worktrees for all branches.");
    }

    if args.branch.is_some() && args.no_checkout {
        anyhow::bail!("--branch and --no-checkout cannot be used together.\nUse --branch to checkout a specific branch, or --no-checkout to skip worktree creation.");
    }

    // Load settings from global config only (repo doesn't exist yet)
    let settings = DaftSettings::load_global()?;

    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_clone(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_clone(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    let repo_name = extract_repo_name(&args.repository_url)?;
    output.step(&format!("Repository name detected: '{repo_name}'"));

    let default_branch = get_default_branch_remote(&args.repository_url)
        .context("Failed to determine default branch")?;
    output.step(&format!("Default branch detected: '{default_branch}'"));

    // Determine the target branch and whether it exists on remote
    let (target_branch, branch_exists) = if let Some(ref specified_branch) = args.branch {
        output.step(&format!(
            "Checking if branch '{}' exists on remote...",
            specified_branch
        ));
        let git = GitCommand::new(output.is_quiet());
        let exists = git
            .ls_remote_branch_exists(&args.repository_url, specified_branch)
            .unwrap_or(false);
        if exists {
            output.step(&format!("Branch '{specified_branch}' found on remote"));
        } else {
            output.warning(&format!(
                "Branch '{specified_branch}' does not exist on remote"
            ));
        }
        (specified_branch.clone(), exists)
    } else {
        (default_branch.clone(), true)
    };

    let parent_dir = PathBuf::from(&repo_name);
    let worktree_dir = parent_dir.join(&target_branch);

    output.step(&format!(
        "Target repository directory: './{}'",
        parent_dir.display()
    ));

    if !args.no_checkout {
        if args.all_branches {
            output.step("Worktrees will be created for all remote branches");
        } else if branch_exists {
            output.step(&format!(
                "Initial worktree will be in: './{}'",
                worktree_dir.display()
            ));
        } else {
            output.step("Worktree creation will be skipped (branch does not exist)");
        }
    } else {
        output.step("No-checkout mode: Only bare repository will be created");
    }

    if path_exists(&parent_dir) {
        anyhow::bail!("Target path './{} already exists.", parent_dir.display());
    }

    output.step("Creating repository directory...");
    create_directory(&parent_dir)?;

    let git_dir = parent_dir.join(".git");
    let git = GitCommand::new(output.is_quiet());

    output.step(&format!(
        "Cloning bare repository into './{}'...",
        git_dir.display()
    ));

    if let Err(e) = git.clone_bare(&args.repository_url, &git_dir) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git clone failed"));
    }

    // Change to the repository directory to set up remote HEAD
    output.step(&format!(
        "Changing directory to './{}'",
        parent_dir.display()
    ));
    change_directory(&parent_dir)?;

    let config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };

    // Set up fetch refspec for bare repo (required for upstream tracking to work)
    output.step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&config.remote_name) {
        output.warning(&format!("Could not set fetch refspec: {e}"));
        // Continue execution - upstream tracking may not work but worktrees will
    }

    if !args.no_checkout && branch_exists {
        if args.all_branches {
            create_all_worktrees(&git, &config, &default_branch, output)?;
        } else {
            create_single_worktree(&git, &target_branch, output)?;
        }

        let target_worktree = PathBuf::from(&target_branch);
        output.step(&format!(
            "Changing directory to worktree: './{}'",
            target_worktree.display()
        ));

        if let Err(e) = change_directory(&target_worktree) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            return Err(e);
        }

        // Fetch to create remote tracking refs (bare clone only creates local refs)
        output.step(&format!(
            "Fetching from '{}' to set up remote tracking...",
            config.remote_name
        ));
        if let Err(e) = git.fetch(&config.remote_name, false) {
            output.warning(&format!("Could not fetch from remote: {e}"));
        }

        // Set up remote HEAD reference (must be after fetch so refs exist)
        if let Err(e) = git.remote_set_head_auto(&config.remote_name) {
            output.warning(&format!("Could not set remote HEAD: {e}"));
        }

        // Set up upstream tracking for the target branch (if enabled)
        if settings.checkout_upstream {
            output.step(&format!(
                "Setting upstream to '{}/{}'...",
                config.remote_name, target_branch
            ));
            if let Err(e) = git.set_upstream(&config.remote_name, &target_branch) {
                output.warning(&format!(
                    "Could not set upstream tracking: {e}. You may need to set it manually."
                ));
            }
        } else {
            output.step("Skipping upstream setup (disabled in config)");
        }

        run_direnv_allow(&get_current_directory()?, output)?;

        let current_dir = get_current_directory()?;

        // Git-like result message
        output.result(&format!("Cloned into '{repo_name}/{target_branch}'"));

        output.cd_path(&current_dir);
    } else if !args.no_checkout && !branch_exists {
        // Branch was specified but doesn't exist - stay in repo root
        let current_dir = get_current_directory()?;
        output.result(&format!(
            "Cloned '{repo_name}' (branch '{}' not found, no worktree created)",
            target_branch
        ));
        output.cd_path(&current_dir);
    } else {
        // Git-like result message for no-checkout mode
        output.result(&format!("Cloned '{repo_name}' (bare)"));
    }

    Ok(())
}

fn create_single_worktree(git: &GitCommand, branch: &str, output: &mut dyn Output) -> Result<()> {
    output.step(&format!(
        "Creating initial worktree for branch '{branch}'..."
    ));
    git.worktree_add(&PathBuf::from(branch), branch)
        .context("Failed to create initial worktree")?;
    Ok(())
}

fn create_all_worktrees(
    git: &GitCommand,
    config: &WorktreeConfig,
    _default_branch: &str,
    output: &mut dyn Output,
) -> Result<()> {
    output.step("Fetching all remote branches...");
    git.fetch(&config.remote_name, false)?;

    let remote_branches =
        get_remote_branches(&config.remote_name).context("Failed to get remote branches")?;

    if remote_branches.is_empty() {
        anyhow::bail!("No remote branches found");
    }

    for branch in &remote_branches {
        output.step(&format!("Creating worktree for branch '{branch}'..."));
        if let Err(e) = git.worktree_add(&PathBuf::from(branch), branch) {
            output.error(&format!("creating worktree for branch '{branch}': {e}"));
            continue;
        }
    }

    Ok(())
}
