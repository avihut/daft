use anyhow::Result;
use clap::Parser;
use git_worktree_workflow::{
    check_dependencies, direnv::run_direnv_allow, git::GitCommand, quiet_echo, utils::*,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-init")]
#[command(about = "Initializes a new Git repository in the worktree workflow structure")]
#[command(long_about = r#"
Initializes a new Git repository in the worktree workflow structure:
<repository_name>/.git (bare repository)
<repository_name>/<initial_branch> (initial worktree)

This command creates a new repository following the same structured layout
used by git-worktree-clone, making it suitable for the worktree workflow.
"#)]
struct Args {
    #[arg(help = "Repository name to initialize")]
    repository_name: String,

    #[arg(
        long = "bare",
        help = "Only create the bare repository structure but do not create the initial worktree"
    )]
    bare: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Suppress all output and run silently"
    )]
    quiet: bool,

    #[arg(
        short = 'b',
        long = "initial-branch",
        help = "Set the initial branch name",
        default_value = "master"
    )]
    initial_branch: String,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let original_dir = get_current_directory()?;

    if let Err(e) = run_init(&args) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_init(args: &Args) -> Result<()> {
    check_dependencies()?;

    validate_repo_name(&args.repository_name)?;

    if args.initial_branch.is_empty() {
        anyhow::bail!("Initial branch name cannot be empty");
    }

    let parent_dir = PathBuf::from(&args.repository_name);
    let worktree_dir = parent_dir.join(&args.initial_branch);

    quiet_echo(
        &format!("Target repository directory: './{}'", parent_dir.display()),
        args.quiet,
    );

    if !args.bare {
        quiet_echo(
            &format!(
                "Initial worktree will be in: './{}'",
                worktree_dir.display()
            ),
            args.quiet,
        );
    } else {
        quiet_echo(
            "Bare mode: Only bare repository will be created",
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
        &format!(
            "Initializing bare repository in './{}'...",
            git_dir.display()
        ),
        args.quiet,
    );

    if let Err(e) = git.init_bare(&git_dir, &args.initial_branch) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git init failed"));
    }

    if !args.bare {
        quiet_echo(
            &format!("--> Changing directory to './{}'", parent_dir.display()),
            args.quiet,
        );
        change_directory(&parent_dir)?;

        quiet_echo(
            &format!(
                "Creating initial worktree for branch '{}'...",
                args.initial_branch
            ),
            args.quiet,
        );
        if let Err(e) = git.worktree_add(&PathBuf::from(&args.initial_branch), &args.initial_branch)
        {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            remove_directory(&parent_dir).ok();
            return Err(e.context("Failed to create initial worktree"));
        }

        let target_worktree = PathBuf::from(&args.initial_branch);
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
            &args.repository_name,
            &get_current_directory()?,
            &git_dir_result,
            args.quiet,
        );
    } else {
        quiet_echo("---", args.quiet);
        quiet_echo("Success!", args.quiet);
        quiet_echo(
            &format!(
                "Repository '{}' initialized successfully (bare mode).",
                args.repository_name
            ),
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
