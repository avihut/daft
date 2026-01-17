use anyhow::Result;
use clap::Parser;
use daft::{
    direnv::run_direnv_allow, get_project_root, git::GitCommand, is_git_repository, log_error,
    log_info, log_warning, logging::init_logging, output_cd_path, utils::*, WorktreeConfig,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-checkout")]
#[command(version = daft::VERSION)]
#[command(about = "Creates a git worktree checking out an existing branch")]
#[command(long_about = r#"
Creates a git worktree at the project root level, checking out an EXISTING branch,
set upstream tracking to the corresponding remote branch (if it exists),
run 'direnv allow' (if direnv exists), and finally cd into the new worktree.

Can be run from anywhere within the Git repository (including deep subdirectories).
The new worktree will be created at the project root level (alongside .git directory).
"#)]
pub struct Args {
    #[arg(help = "The name of the existing local or remote branch to check out")]
    branch_name: String,

    #[arg(short, long, help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Carry uncommitted changes to the worktree"
    )]
    carry: bool,

    #[arg(long, help = "Don't carry uncommitted changes (default)")]
    no_carry: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on verbosity flag
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout(&args) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout(args: &Args) -> Result<()> {
    validate_branch_name(&args.branch_name)?;

    let project_root = get_project_root()?;
    let worktree_path = project_root.join(&args.branch_name);

    let config = WorktreeConfig::default();
    let git = GitCommand::new(config.quiet);

    println!("Attempting to create Git worktree and checkout branch:");
    println!("  Path:         {}", worktree_path.display());
    println!("  Branch:       {}", args.branch_name);
    println!("  Project Root: {}", project_root.display());
    println!("---");

    // Check if worktree already exists for this branch
    if let Some(existing_path) = find_existing_worktree_for_branch(&git, &args.branch_name)? {
        println!(
            "--> Branch '{}' already has a worktree at '{}'",
            args.branch_name,
            existing_path.display()
        );
        println!("--> Changing to existing worktree...");
        change_directory(&existing_path)?;
        run_direnv_allow(&get_current_directory()?, config.quiet)?;
        println!("---");
        println!(
            "Switched to existing worktree for branch '{}'.",
            args.branch_name
        );
        output_cd_path(&get_current_directory()?);
        return Ok(());
    }

    // Fetch latest changes from remote to ensure we have the latest version of the branch
    println!(
        "--> Fetching latest changes from remote '{}'...",
        config.remote_name
    );
    if let Err(e) = git.fetch(&config.remote_name, false) {
        println!(
            "Warning: Failed to fetch from remote '{}': {}",
            config.remote_name, e
        );
    }

    // Also fetch the specific branch to ensure remote tracking branch is updated
    println!(
        "--> Fetching specific branch '{}' from remote '{}'...",
        args.branch_name, config.remote_name
    );
    if let Err(e) = git.fetch_refspec(
        &config.remote_name,
        &format!("{}:{}", args.branch_name, args.branch_name),
    ) {
        println!("Warning: Failed to fetch specific branch: {e}");
    }

    // Check if remote branch exists and use it if local branch is behind
    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, args.branch_name);
    let checkout_ref = if git.show_ref_exists(&remote_branch_ref)? {
        println!(
            "--> Remote branch '{}/{}' found, using it for worktree creation",
            config.remote_name, args.branch_name
        );
        format!("{}/{}", config.remote_name, args.branch_name)
    } else {
        println!(
            "--> Remote branch '{}/{}' not found, using local branch",
            config.remote_name, args.branch_name
        );
        args.branch_name.clone()
    };

    // Check for uncommitted changes and stash them if --carry is set
    let stash_created = if args.carry {
        match git.has_uncommitted_changes() {
            Ok(true) => {
                println!("--> Stashing uncommitted changes...");
                if let Err(e) = git.stash_push_with_untracked("daft: carry changes to worktree") {
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
        false
    };

    if let Err(e) = git.worktree_add(&worktree_path, &checkout_ref) {
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
        "Git worktree created successfully at '{}' checking out branch '{}'.",
        worktree_path.display(),
        args.branch_name
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
        println!("--> Applying stashed changes to worktree...");
        if let Err(e) = git.stash_pop() {
            log_error!("Failed to apply stashed changes: {e}");
            eprintln!(
                "Stash could not be applied cleanly. Resolve conflicts and run: git stash pop"
            );
        } else {
            println!("--> Changes successfully applied to worktree");
        }
    }

    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, args.branch_name);
    println!(
        "--> Checking for remote branch '{}/{}'...",
        config.remote_name, args.branch_name
    );

    if git.show_ref_exists(&remote_branch_ref)? {
        println!(
            "--> Remote branch found. Attempting: git branch --set-upstream-to={}/{}",
            config.remote_name, args.branch_name
        );

        if let Err(e) = git.set_upstream(&config.remote_name, &args.branch_name) {
            println!("---");
            eprintln!(
                "Warning: Failed to set upstream tracking for branch '{}' to '{}/{}': {}",
                args.branch_name, config.remote_name, args.branch_name, e
            );
            eprintln!("         Worktree created and CD'd into, but upstream may need manual configuration.");
        } else {
            println!(
                "--> Upstream tracking set successfully to '{}/{}'.",
                config.remote_name, args.branch_name
            );
        }
    } else {
        println!(
            "--> Remote branch '{}/{}' not found. Skipping upstream setup.",
            config.remote_name, args.branch_name
        );
        println!("    You might need to push the branch or check the remote name/branch name.");
    }

    run_direnv_allow(&get_current_directory()?, config.quiet)?;

    println!("---");
    println!("Overall Success: Worktree created, branch checked out, upstream handled, direnv handled (if present), and CD'd into worktree.");

    output_cd_path(&get_current_directory()?);

    Ok(())
}

/// Check if a worktree already exists for the given branch name.
/// Returns the path to the existing worktree if found.
fn find_existing_worktree_for_branch(
    git: &GitCommand,
    branch_name: &str,
) -> Result<Option<PathBuf>> {
    let porcelain_output = git.worktree_list_porcelain()?;

    let mut current_path: Option<PathBuf> = None;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(worktree_path));
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if let Some(branch) = branch_ref.strip_prefix("refs/heads/") {
                if branch == branch_name {
                    return Ok(current_path.take());
                }
            }
            current_path = None;
        } else if line.is_empty() {
            current_path = None;
        }
    }

    Ok(None)
}
