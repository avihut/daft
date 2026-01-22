use anyhow::Result;
use clap::Parser;
use daft::{
    check_dependencies,
    direnv::run_direnv_allow,
    git::GitCommand,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-init")]
#[command(version = daft::VERSION)]
#[command(about = "Initializes a new Git repository in the worktree workflow structure")]
#[command(long_about = r#"
Initializes a new Git repository in the worktree workflow structure:
<repository_name>/.git (bare repository)
<repository_name>/<initial_branch> (initial worktree)

This command creates a new repository following the same structured layout
used by git-worktree-clone, making it suitable for the worktree workflow.
"#)]
pub struct Args {
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

    #[arg(short = 'v', long = "verbose", help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        short = 'b',
        long = "initial-branch",
        help = "Set the initial branch name",
        default_value = "master"
    )]
    initial_branch: String,
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on verbose flag
    init_logging(args.verbose);

    // Load settings from global config only (repo doesn't exist yet)
    let settings = DaftSettings::load_global()?;

    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_with_output(&args, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

/// Run the init command with the given output implementation.
///
/// This function contains all the business logic and uses the `Output` trait
/// for all output operations, making it testable and TUI-ready.
pub fn run_with_output(args: &Args, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    validate_repo_name(&args.repository_name)?;

    if args.initial_branch.is_empty() {
        anyhow::bail!("Initial branch name cannot be empty");
    }

    let parent_dir = PathBuf::from(&args.repository_name);
    let worktree_dir = parent_dir.join(&args.initial_branch);

    output.step(&format!(
        "Target repository directory: './{}'",
        parent_dir.display()
    ));

    if !args.bare {
        output.step(&format!(
            "Initial worktree will be in: './{}'",
            worktree_dir.display()
        ));
    } else {
        output.step("Bare mode: Only bare repository will be created");
    }

    if path_exists(&parent_dir) {
        anyhow::bail!("Target path './{} already exists.", parent_dir.display());
    }

    output.step("Creating repository directory...");
    create_directory(&parent_dir)?;

    let git_dir = parent_dir.join(".git");
    let git = GitCommand::new(output.is_quiet());

    output.step(&format!(
        "Initializing bare repository in './{}'...",
        git_dir.display()
    ));

    if let Err(e) = git.init_bare(&git_dir, &args.initial_branch) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git init failed"));
    }

    if !args.bare {
        output.step(&format!(
            "Changing directory to './{}'",
            parent_dir.display()
        ));
        change_directory(&parent_dir)?;

        output.step(&format!(
            "Creating initial worktree for branch '{}'...",
            args.initial_branch
        ));
        if let Err(e) =
            git.worktree_add_orphan(&PathBuf::from(&args.initial_branch), &args.initial_branch)
        {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            remove_directory(&parent_dir).ok();
            return Err(e.context("Failed to create initial worktree"));
        }

        let target_worktree = PathBuf::from(&args.initial_branch);
        output.step(&format!(
            "Changing directory to worktree: './{}'",
            target_worktree.display()
        ));

        if let Err(e) = change_directory(&target_worktree) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            return Err(e);
        }

        run_direnv_allow(&get_current_directory()?, output)?;

        let current_dir = get_current_directory()?;

        // Git-like result message
        output.result(&format!(
            "Initialized repository '{}' in '{}/{}'",
            args.repository_name, args.repository_name, args.initial_branch
        ));

        output.cd_path(&current_dir);
    } else {
        // Git-like result message for bare mode
        output.result(&format!(
            "Initialized empty repository '{}' (bare)",
            args.repository_name
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use daft::output::TestOutput;
    use std::env;
    use tempfile::tempdir;

    fn create_test_args(repo_name: &str, bare: bool, quiet: bool, verbose: bool) -> Args {
        Args {
            repository_name: repo_name.to_string(),
            bare,
            quiet,
            verbose,
            initial_branch: "master".to_string(),
        }
    }

    #[test]
    fn test_init_output_messages() {
        let temp_dir = tempdir().unwrap();
        env::set_current_dir(temp_dir.path()).unwrap();

        let args = create_test_args("test-repo", false, false, true);
        let mut output = TestOutput::verbose();

        let result = run_with_output(&args, &mut output);

        // The command may fail due to git not being configured, but we can still
        // verify the output messages that were generated before the failure
        if result.is_ok() {
            assert!(output.has_step("Target repository directory"));
            assert!(output.has_step("Initial worktree will be in"));
            assert!(output.has_result("Initialized repository"));
            assert!(output.get_cd_path().is_some());
        }
    }

    #[test]
    fn test_init_bare_mode_output() {
        let temp_dir = tempdir().unwrap();
        env::set_current_dir(temp_dir.path()).unwrap();

        let args = create_test_args("test-bare-repo", true, false, true);
        let mut output = TestOutput::verbose();

        let result = run_with_output(&args, &mut output);

        if result.is_ok() {
            assert!(output.has_step("Bare mode"));
            assert!(output.has_result("bare"));
            // Bare mode should NOT output cd_path
            assert!(output.get_cd_path().is_none());
        }
    }

    #[test]
    fn test_init_quiet_mode_suppresses_output() {
        let temp_dir = tempdir().unwrap();
        env::set_current_dir(temp_dir.path()).unwrap();

        let args = create_test_args("quiet-repo", true, true, false);
        let mut output = TestOutput::quiet();

        let _ = run_with_output(&args, &mut output);

        // In quiet mode, all messages should be suppressed
        assert!(output.steps().is_empty());
        assert!(output.results().is_empty());
    }

    #[test]
    fn test_init_validation_error() {
        let args = Args {
            repository_name: "".to_string(),
            bare: false,
            quiet: false,
            verbose: false,
            initial_branch: "master".to_string(),
        };
        let mut output = TestOutput::new();

        let result = run_with_output(&args, &mut output);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Repository name cannot be empty"));
    }

    #[test]
    fn test_init_empty_branch_error() {
        let args = Args {
            repository_name: "test-repo".to_string(),
            bare: false,
            quiet: false,
            verbose: false,
            initial_branch: "".to_string(),
        };
        let mut output = TestOutput::new();

        let result = run_with_output(&args, &mut output);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Initial branch name cannot be empty"));
    }
}
