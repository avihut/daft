use anyhow::{Context, Result};
use clap::Parser;
use daft::{
    check_dependencies,
    direnv::run_direnv_allow,
    extract_repo_name,
    git::GitCommand,
    logging::init_logging,
    output_cd_path, quiet_echo,
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

    #[arg(
        short = 'a',
        long = "all-branches",
        help = "Create worktrees for all remote branches, not just the default"
    )]
    all_branches: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    // Initialize logging - quiet mode disables verbose output
    init_logging(!args.quiet);

    if args.no_checkout && args.all_branches {
        anyhow::bail!("--no-checkout and --all-branches cannot be used together.\nUse --no-checkout to create only the bare repository, or --all-branches to create worktrees for all branches.");
    }

    let original_dir = get_current_directory()?;

    if let Err(e) = run_clone(&args) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_clone(args: &Args) -> Result<()> {
    check_dependencies()?;

    let repo_name = extract_repo_name(&args.repository_url)?;
    quiet_echo(
        &format!("Repository name detected: '{repo_name}'"),
        args.quiet,
    );

    let default_branch = get_default_branch_remote(&args.repository_url)
        .context("Failed to determine default branch")?;
    quiet_echo(
        &format!("Default branch detected: '{default_branch}'"),
        args.quiet,
    );

    let parent_dir = PathBuf::from(&repo_name);
    let worktree_dir = parent_dir.join(&default_branch);

    quiet_echo(
        &format!("Target repository directory: './{}'", parent_dir.display()),
        args.quiet,
    );

    if !args.no_checkout {
        if args.all_branches {
            quiet_echo(
                "Worktrees will be created for all remote branches",
                args.quiet,
            );
        } else {
            quiet_echo(
                &format!(
                    "Initial worktree will be in: './{}'",
                    worktree_dir.display()
                ),
                args.quiet,
            );
        }
    } else {
        quiet_echo(
            "No-checkout mode: Only bare repository will be created",
            args.quiet,
        );
    }

    if path_exists(&parent_dir) {
        anyhow::bail!("Target path './{} already exists.", parent_dir.display());
    }

    quiet_echo("Creating repository directory...", args.quiet);
    create_directory(&parent_dir)?;

    let git_dir = parent_dir.join(".git");
    let git = GitCommand::new(args.quiet);

    quiet_echo(
        &format!("Cloning bare repository into './{}'...", git_dir.display()),
        args.quiet,
    );

    if let Err(e) = git.clone_bare(&args.repository_url, &git_dir) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git clone failed"));
    }

    // Change to the repository directory to set up remote HEAD
    quiet_echo(
        &format!("--> Changing directory to './{}'", parent_dir.display()),
        args.quiet,
    );
    change_directory(&parent_dir)?;

    let config = WorktreeConfig::default();

    // Set up remote HEAD reference for better default branch detection
    if let Err(e) = git.remote_set_head_auto(&config.remote_name) {
        quiet_echo(
            &format!("--> Warning: Could not set remote HEAD: {e}"),
            args.quiet,
        );
        // Continue execution - this is not critical
    }

    if !args.no_checkout {
        if args.all_branches {
            create_all_worktrees(&git, &config, &default_branch, args.quiet)?;
        } else {
            create_single_worktree(&git, &default_branch, args.quiet)?;
        }

        let target_worktree = PathBuf::from(&default_branch);
        quiet_echo(
            &format!(
                "--> Changing directory to worktree: './{}'",
                target_worktree.display()
            ),
            args.quiet,
        );

        if let Err(e) = change_directory(&target_worktree) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            return Err(e);
        }

        run_direnv_allow(&get_current_directory()?, args.quiet)?;

        let git_dir_result = git.get_git_dir().unwrap_or_else(|_| "unknown".to_string());
        print_success_message(
            &repo_name,
            &get_current_directory()?,
            &git_dir_result,
            args.quiet,
        );

        output_cd_path(&get_current_directory()?);
    } else {
        quiet_echo("---", args.quiet);
        quiet_echo("Success!", args.quiet);
        quiet_echo(
            &format!("Repository '{repo_name}' cloned successfully (no-checkout mode)."),
            args.quiet,
        );
        quiet_echo(
            &format!("The bare Git repository is at: '{}'", git_dir.display()),
            args.quiet,
        );
        quiet_echo("No worktree was created. You can create worktrees using 'git worktree add' from within the repository directory.", args.quiet);
        quiet_echo(
            &format!(
                "You are still in the original directory: {}",
                get_current_directory()?.display()
            ),
            args.quiet,
        );
    }

    Ok(())
}

fn create_single_worktree(git: &GitCommand, default_branch: &str, quiet: bool) -> Result<()> {
    quiet_echo(
        &format!("Creating initial worktree for branch '{default_branch}'..."),
        quiet,
    );
    git.worktree_add(&PathBuf::from(default_branch), default_branch)
        .context("Failed to create initial worktree")?;
    Ok(())
}

fn create_all_worktrees(
    git: &GitCommand,
    config: &WorktreeConfig,
    _default_branch: &str,
    quiet: bool,
) -> Result<()> {
    quiet_echo("Fetching all remote branches...", quiet);
    git.fetch(&config.remote_name, false)?;

    let remote_branches =
        get_remote_branches(&config.remote_name).context("Failed to get remote branches")?;

    if remote_branches.is_empty() {
        anyhow::bail!("No remote branches found");
    }

    for branch in &remote_branches {
        quiet_echo(
            &format!("Creating worktree for branch '{branch}'..."),
            quiet,
        );
        if let Err(e) = git.worktree_add(&PathBuf::from(branch), branch) {
            eprintln!("Error creating worktree for branch '{branch}': {e}");
            continue;
        }
    }

    Ok(())
}
