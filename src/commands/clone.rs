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
<repository_name>/<default_branch_name>

It determines the repository name from the URL and queries the remote
to find the default branch (e.g., main, master, develop) *before* cloning.
After cloning, it runs 'direnv allow' in the new directory and cds into it.
"#)]
pub struct Args {
    #[arg(help = "Repository URL to clone")]
    repository_url: String,

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

    let config = OutputConfig::new(args.quiet, args.verbose);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_clone(&args, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_clone(args: &Args, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    let repo_name = extract_repo_name(&args.repository_url)?;
    output.step(&format!("Repository name detected: '{repo_name}'"));

    let default_branch = get_default_branch_remote(&args.repository_url)
        .context("Failed to determine default branch")?;
    output.step(&format!("Default branch detected: '{default_branch}'"));

    let parent_dir = PathBuf::from(&repo_name);
    let worktree_dir = parent_dir.join(&default_branch);

    output.step(&format!(
        "Target repository directory: './{}'",
        parent_dir.display()
    ));

    if !args.no_checkout {
        if args.all_branches {
            output.step("Worktrees will be created for all remote branches");
        } else {
            output.step(&format!(
                "Initial worktree will be in: './{}'",
                worktree_dir.display()
            ));
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

    let config = WorktreeConfig::default();

    // Set up remote HEAD reference for better default branch detection
    if let Err(e) = git.remote_set_head_auto(&config.remote_name) {
        output.warning(&format!("Could not set remote HEAD: {e}"));
        // Continue execution - this is not critical
    }

    if !args.no_checkout {
        if args.all_branches {
            create_all_worktrees(&git, &config, &default_branch, output)?;
        } else {
            create_single_worktree(&git, &default_branch, output)?;
        }

        let target_worktree = PathBuf::from(&default_branch);
        output.step(&format!(
            "Changing directory to worktree: './{}'",
            target_worktree.display()
        ));

        if let Err(e) = change_directory(&target_worktree) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            return Err(e);
        }

        run_direnv_allow(&get_current_directory()?, output.is_quiet())?;

        let current_dir = get_current_directory()?;

        // Git-like result message
        output.result(&format!("Cloned into '{repo_name}/{default_branch}'"));

        output.cd_path(&current_dir);
    } else {
        // Git-like result message for no-checkout mode
        output.result(&format!("Cloned '{repo_name}' (bare)"));
    }

    Ok(())
}

fn create_single_worktree(
    git: &GitCommand,
    default_branch: &str,
    output: &mut dyn Output,
) -> Result<()> {
    output.step(&format!(
        "Creating initial worktree for branch '{default_branch}'..."
    ));
    git.worktree_add(&PathBuf::from(default_branch), default_branch)
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
