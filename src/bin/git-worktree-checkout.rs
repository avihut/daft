use anyhow::Result;
use clap::Parser;
use git_worktree_workflow::{
    direnv::run_direnv_allow, get_project_root, git::GitCommand, is_git_repository, utils::*,
    WorktreeConfig,
};

#[derive(Parser)]
#[command(name = "git-worktree-checkout")]
#[command(about = "Creates a git worktree checking out an existing branch")]
#[command(long_about = r#"
Creates a git worktree at the project root level, checking out an EXISTING branch,
set upstream tracking to the corresponding remote branch (if it exists),
run 'direnv allow' (if direnv exists), and finally cd into the new worktree.

Can be run from anywhere within the Git repository (including deep subdirectories).
The new worktree will be created at the project root level (alongside .git directory).
"#)]
struct Args {
    #[arg(help = "The name of the existing local or remote branch to check out")]
    branch_name: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

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

    if let Err(e) = git.worktree_add(&worktree_path, &args.branch_name) {
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

    Ok(())
}
