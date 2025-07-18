use anyhow::Result;
use clap::Parser;
use git_worktree_workflow::{
    direnv::run_direnv_allow, get_current_branch, get_project_root, git::GitCommand,
    is_git_repository, utils::*, WorktreeConfig,
};

#[derive(Parser)]
#[command(name = "git-worktree-checkout-branch")]
#[command(about = "Creates a git worktree with a new branch")]
#[command(long_about = r#"
Creates a git worktree at the project root level, create a new branch based on either
the CURRENT branch or a specified base branch, push the new branch to origin, set upstream tracking,
run 'direnv allow' (if direnv exists), and finally cd into the new worktree.

Can be run from anywhere within the Git repository (including deep subdirectories).
The new worktree will be created at the project root level (alongside .git directory).
"#)]
struct Args {
    #[arg(help = "The name for the new branch and the worktree directory")]
    new_branch_name: String,

    #[arg(help = "The branch to base the new branch on (defaults to current branch)")]
    base_branch_name: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

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
            println!("--> Using explicitly provided base branch: '{branch}'");
            branch.clone()
        }
        None => {
            println!("--> Base branch not specified, using current branch...");
            let current = get_current_branch()?;
            println!("--> Using current branch as base: '{current}'");
            current
        }
    };

    let project_root = get_project_root()?;
    let worktree_path = project_root.join(&args.new_branch_name);

    let config = WorktreeConfig::default();
    let git = GitCommand::new(config.quiet);

    // Fetch latest changes from remote to ensure we have the latest version of the base branch
    println!("--> Fetching latest changes from remote '{}'...", config.remote_name);
    if let Err(e) = git.fetch(&config.remote_name, false) {
        println!("Warning: Failed to fetch from remote '{}': {}", config.remote_name, e);
    }
    
    // Fetch the specific remote branch (but don't try to update local branch if it's checked out)
    println!("--> Fetching remote tracking branch for '{}'...", base_branch);
    if let Ok(output) = std::process::Command::new("git")
        .args(["fetch", &config.remote_name, &format!("{}:refs/remotes/{}/{}", base_branch, config.remote_name, base_branch)])
        .output() {
        if !output.status.success() {
            println!("Warning: Failed to fetch remote tracking branch: {}", String::from_utf8_lossy(&output.stderr));
        }
    }

    // Check if remote branch exists and is ahead of local branch
    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, base_branch);
    let checkout_base = if git.show_ref_exists(&remote_branch_ref)? {
        println!("--> Remote branch '{}/{}' found, using it as base", config.remote_name, base_branch);
        format!("{}/{}", config.remote_name, base_branch)
    } else {
        println!("--> Remote branch '{}/{}' not found, using local branch", config.remote_name, base_branch);
        base_branch.clone()
    };

    println!("Attempting to create Git worktree:");
    println!("  Path:         {}", worktree_path.display());
    println!("  New Branch:   {}", args.new_branch_name);
    println!("  From Branch:  {checkout_base}");
    println!("  Project Root: {}", project_root.display());
    println!("---");

    if let Err(e) = git.worktree_add_new_branch(&worktree_path, &args.new_branch_name, &checkout_base)
    {
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

    println!(
        "--> Attempting: git push --set-upstream {} \"{}\"",
        config.remote_name, args.new_branch_name
    );

    if let Err(e) = git.push_set_upstream(&config.remote_name, &args.new_branch_name) {
        eprintln!("---");
        eprintln!(
            "Error: Failed to push branch '{}' to '{}' or set upstream: {}",
            args.new_branch_name, config.remote_name, e
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

    Ok(())
}
