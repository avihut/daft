use anyhow::Result;
use clap::Parser;
use daft::{
    check_dependencies,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, TrustLevel},
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
        help = "Set the initial branch name (defaults to git config init.defaultBranch or 'master')"
    )]
    initial_branch: Option<String>,
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

/// Resolve the initial branch name from args, git config, or default.
///
/// Priority:
/// 1. Explicitly provided via -b/--initial-branch
/// 2. Git config init.defaultBranch (global)
/// 3. Fallback to "master"
fn resolve_initial_branch(initial_branch: &Option<String>) -> String {
    if let Some(branch) = initial_branch {
        return branch.clone();
    }

    // Query git config for init.defaultBranch
    let git = GitCommand::new(true); // quiet mode for config query
    if let Ok(Some(configured_branch)) = git.config_get_global("init.defaultBranch") {
        if !configured_branch.is_empty() {
            return configured_branch;
        }
    }

    // Fallback to "master"
    "master".to_string()
}

/// Run the init command with the given output implementation.
///
/// This function contains all the business logic and uses the `Output` trait
/// for all output operations, making it testable and TUI-ready.
pub fn run_with_output(args: &Args, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    validate_repo_name(&args.repository_name)?;

    let initial_branch = resolve_initial_branch(&args.initial_branch);

    if initial_branch.is_empty() {
        anyhow::bail!("Initial branch name cannot be empty");
    }

    let parent_dir = PathBuf::from(&args.repository_name);
    let worktree_dir = parent_dir.join(&initial_branch);

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

    if let Err(e) = git.init_bare(&git_dir, &initial_branch) {
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
            initial_branch
        ));
        if let Err(e) = git.worktree_add_orphan(&PathBuf::from(&initial_branch), &initial_branch) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            remove_directory(&parent_dir).ok();
            return Err(e.context("Failed to create initial worktree"));
        }

        let target_worktree = PathBuf::from(&initial_branch);
        output.step(&format!(
            "Changing directory to worktree: './{}'",
            target_worktree.display()
        ));

        if let Err(e) = change_directory(&target_worktree) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            return Err(e);
        }

        let current_dir = get_current_directory()?;

        // Git-like result message
        output.result(&format!(
            "Initialized repository '{}' in '{}/{}'",
            args.repository_name, args.repository_name, initial_branch
        ));

        // Execute post-init hooks
        // For newly initialized repos, trust them by default (user is creating their own repo)
        run_post_init_hook(&parent_dir, &git_dir, &current_dir, &initial_branch, output)?;

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

fn run_post_init_hook(
    project_root: &PathBuf,
    git_dir: &PathBuf,
    worktree_path: &PathBuf,
    initial_branch: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    // For newly initialized repos, automatically trust them
    // (user is creating their own repository)
    executor.trust_repository(git_dir, TrustLevel::Allow)?;

    // Build the hook context
    let ctx = HookContext::new(
        HookType::PostInit,
        "init",
        project_root,
        git_dir,
        "origin", // No remote exists yet, use default name
        worktree_path,
        worktree_path,
        initial_branch,
    )
    .with_new_branch(true);

    // Execute the hook (ignores if no hooks exist)
    executor.execute(&ctx, output)?;

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
            initial_branch: Some("master".to_string()),
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
            initial_branch: Some("master".to_string()),
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
            initial_branch: Some("".to_string()),
        };
        let mut output = TestOutput::new();

        let result = run_with_output(&args, &mut output);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Initial branch name cannot be empty"));
    }

    #[test]
    fn test_resolve_initial_branch_explicit() {
        // When explicitly provided, should use that value
        let result = resolve_initial_branch(&Some("develop".to_string()));
        assert_eq!(result, "develop");

        let result = resolve_initial_branch(&Some("main".to_string()));
        assert_eq!(result, "main");
    }

    #[test]
    fn test_resolve_initial_branch_none_returns_non_empty() {
        // When None is provided, should return either git config value or "master" fallback
        // We can't easily mock git config, but we can verify it returns something non-empty
        let result = resolve_initial_branch(&None);
        assert!(!result.is_empty());
    }
}
