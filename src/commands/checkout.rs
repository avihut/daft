use anyhow::Result;
use clap::Parser;
use daft::{
    direnv::run_direnv_allow,
    get_project_root,
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    utils::*,
    WorktreeConfig,
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

    let config = OutputConfig::new(false, args.verbose);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout(&args, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout(args: &Args, output: &mut dyn Output) -> Result<()> {
    validate_branch_name(&args.branch_name)?;

    let project_root = get_project_root()?;
    let worktree_path = project_root.join(&args.branch_name);

    let config = WorktreeConfig::default();
    let git = GitCommand::new(output.is_quiet());

    output.step(&format!(
        "Path: {}, Branch: {}, Project Root: {}",
        worktree_path.display(),
        args.branch_name,
        project_root.display()
    ));

    // Check if worktree already exists for this branch
    if let Some(existing_path) = find_existing_worktree_for_branch(&git, &args.branch_name)? {
        output.step(&format!(
            "Branch '{}' already has a worktree at '{}'",
            args.branch_name,
            existing_path.display()
        ));
        output.step("Changing to existing worktree...");
        change_directory(&existing_path)?;
        run_direnv_allow(&get_current_directory()?, output)?;
        output.result(&format!(
            "Switched to existing worktree '{}'",
            args.branch_name
        ));
        output.cd_path(&get_current_directory()?);
        return Ok(());
    }

    // Fetch latest changes from remote to ensure we have the latest version of the branch
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

    // Also fetch the specific branch to ensure remote tracking branch is updated
    output.step(&format!(
        "Fetching specific branch '{}' from remote '{}'...",
        args.branch_name, config.remote_name
    ));
    if let Err(e) = git.fetch_refspec(
        &config.remote_name,
        &format!("{}:{}", args.branch_name, args.branch_name),
    ) {
        output.warning(&format!("Failed to fetch specific branch: {e}"));
    }

    // Check if local and/or remote branch exists
    let local_branch_ref = format!("refs/heads/{}", args.branch_name);
    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, args.branch_name);
    let local_exists = git.show_ref_exists(&local_branch_ref)?;
    let remote_exists = git.show_ref_exists(&remote_branch_ref)?;

    if !local_exists && !remote_exists {
        anyhow::bail!(
            "Branch '{}' does not exist locally or on remote '{}'",
            args.branch_name,
            config.remote_name
        );
    }

    // Determine whether to use local branch or create from remote
    let use_local_branch = if local_exists {
        output.step(&format!(
            "Local branch '{}' found, using it for worktree creation",
            args.branch_name
        ));
        true
    } else {
        output.step(&format!(
            "Local branch '{}' not found, will create from remote '{}/{}'",
            args.branch_name, config.remote_name, args.branch_name
        ));
        false
    };

    // Check for uncommitted changes and stash them if --carry is set
    let stash_created = if args.carry {
        match git.has_uncommitted_changes() {
            Ok(true) => {
                output.step("Stashing uncommitted changes...");
                if let Err(e) = git.stash_push_with_untracked("daft: carry changes to worktree") {
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
        false
    };

    // Create worktree: use local branch directly, or create local branch from remote
    let worktree_result = if use_local_branch {
        git.worktree_add(&worktree_path, &args.branch_name)
    } else {
        // Create a new local branch tracking the remote branch
        let remote_ref = format!("{}/{}", config.remote_name, args.branch_name);
        git.worktree_add_new_branch(&worktree_path, &args.branch_name, &remote_ref)
    };

    if let Err(e) = worktree_result {
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
        "Worktree created at '{}' checking out branch '{}'",
        worktree_path.display(),
        args.branch_name
    ));

    output.step(&format!(
        "Changing directory to worktree: {}",
        worktree_path.display()
    ));
    change_directory(&worktree_path)?;

    // Apply stashed changes to the new worktree
    if stash_created {
        output.step("Applying stashed changes to worktree...");
        if let Err(e) = git.stash_pop() {
            output.warning(&format!(
                "Stash could not be applied cleanly. Resolve conflicts and run 'git stash pop'. Error: {e}"
            ));
        } else {
            output.step("Changes successfully applied to worktree");
        }
    }

    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, args.branch_name);
    output.step(&format!(
        "Checking for remote branch '{}/{}'...",
        config.remote_name, args.branch_name
    ));

    if git.show_ref_exists(&remote_branch_ref)? {
        output.step(&format!(
            "Setting upstream to '{}/{}'...",
            config.remote_name, args.branch_name
        ));

        if let Err(e) = git.set_upstream(&config.remote_name, &args.branch_name) {
            output.warning(&format!(
                "Failed to set upstream tracking: {}. Worktree created, but upstream may need manual configuration.",
                e
            ));
        } else {
            output.step(&format!(
                "Upstream tracking set to '{}/{}'",
                config.remote_name, args.branch_name
            ));
        }
    } else {
        output.step(&format!(
            "Remote branch '{}/{}' not found, skipping upstream setup",
            config.remote_name, args.branch_name
        ));
    }

    run_direnv_allow(&get_current_directory()?, output)?;

    // Git-like result message
    output.result(&format!("Prepared worktree '{}'", args.branch_name));

    output.cd_path(&get_current_directory()?);

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
