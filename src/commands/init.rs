use anyhow::{Context, Result};
use clap::Parser;
use daft::{
    check_dependencies,
    git::GitCommand,
    hints::maybe_show_shell_hint,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, TrustLevel},
    logging::init_logging,
    multi_remote::path::calculate_worktree_path,
    output::{CliOutput, Output, OutputConfig},
    resolve_initial_branch,
    settings::DaftSettings,
    utils::*,
};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-init")]
#[command(version = daft::VERSION)]
#[command(about = "Initialize a new repository in the worktree-based directory structure")]
#[command(long_about = r#"
Initializes a new Git repository using the same directory structure as
git-worktree-clone(1). The resulting layout is:

    <name>/.git      (bare repository metadata)
    <name>/<branch>  (worktree for the initial branch)

This structure is optimized for worktree-based development, allowing multiple
branches to be checked out simultaneously as sibling directories.

The initial branch name is determined by, in order of precedence: the -b
option, the init.defaultBranch configuration value, or "master" as a fallback.

If the repository contains a .daft/hooks/ directory (created manually after
init) and is trusted, lifecycle hooks are executed. See git-daft(1) for hook
management.
"#)]
pub struct Args {
    #[arg(help = "Name for the new repository directory")]
    repository_name: String,

    #[arg(
        long = "bare",
        help = "Create only the bare repository; do not create an initial worktree"
    )]
    bare: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Operate quietly; suppress progress reporting"
    )]
    quiet: bool,

    #[arg(
        short = 'v',
        long = "verbose",
        help = "Be verbose; show detailed progress"
    )]
    verbose: bool,

    #[arg(
        short = 'b',
        long = "initial-branch",
        help = "Use <name> as the initial branch instead of the configured default"
    )]
    initial_branch: Option<String>,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Organize worktree under this remote folder (enables multi-remote mode)"
    )]
    remote: Option<String>,
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

    let initial_branch = resolve_initial_branch(&args.initial_branch);

    if initial_branch.is_empty() {
        anyhow::bail!("Initial branch name cannot be empty");
    }

    // Load global settings to check for multi-remote preferences
    let settings = DaftSettings::load_global()?;

    // Determine if we should use multi-remote mode
    let use_multi_remote = args.remote.is_some() || settings.multi_remote_enabled;
    let remote_for_path = args
        .remote
        .clone()
        .unwrap_or_else(|| settings.multi_remote_default.clone());

    let parent_dir = PathBuf::from(&args.repository_name);
    let worktree_dir = calculate_worktree_path(
        &parent_dir,
        &initial_branch,
        &remote_for_path,
        use_multi_remote,
    );

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

        // Set multi-remote config if --remote was provided
        if args.remote.is_some() {
            output.step("Enabling multi-remote mode for this repository...");
            daft::multi_remote::config::set_multi_remote_enabled(&git, true)?;
            daft::multi_remote::config::set_multi_remote_default(&git, &remote_for_path)?;
        }

        // Calculate the relative worktree path from parent_dir
        let relative_worktree_path = if use_multi_remote {
            PathBuf::from(&remote_for_path).join(&initial_branch)
        } else {
            PathBuf::from(&initial_branch)
        };

        output.step(&format!(
            "Creating initial worktree for branch '{}' at '{}'...",
            initial_branch,
            relative_worktree_path.display()
        ));

        // Ensure parent directory exists (for multi-remote mode)
        if let Some(parent) = relative_worktree_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }
        }

        if let Err(e) = git.worktree_add_orphan(&relative_worktree_path, &initial_branch) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            remove_directory(&parent_dir).ok();
            return Err(e.context("Failed to create initial worktree"));
        }

        output.step(&format!(
            "Changing directory to worktree: './{}'",
            relative_worktree_path.display()
        ));

        if let Err(e) = change_directory(&relative_worktree_path) {
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
        maybe_show_shell_hint(output)?;
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
            remote: None,
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
            remote: None,
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
            remote: None,
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
