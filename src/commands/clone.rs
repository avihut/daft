use crate::{
    check_dependencies, extract_repo_name,
    git::GitCommand,
    hints::maybe_show_shell_hint,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, TrustLevel},
    logging::init_logging,
    multi_remote::path::calculate_worktree_path,
    output::{CliOutput, Output, OutputConfig},
    remote::{get_default_branch_remote, get_remote_branches, is_remote_empty},
    resolve_initial_branch,
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
};
use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-clone")]
#[command(version = crate::VERSION)]
#[command(about = "Clone a repository into a worktree-based directory structure")]
#[command(long_about = r#"
Clones a repository into a directory structure optimized for worktree-based
development. The resulting layout is:

    <repository-name>/.git    (bare repository metadata)
    <repository-name>/<branch>  (worktree for the checked-out branch)

The command first queries the remote to determine the default branch (main,
master, or other configured default), then performs a bare clone and creates
the initial worktree. This structure allows multiple worktrees to be created
as siblings, each containing a different branch.

If the repository contains a .daft/hooks/ directory and the repository is
trusted, lifecycle hooks are executed. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(help = "The repository URL to clone (HTTPS or SSH)")]
    repository_url: String,

    #[arg(
        short = 'b',
        long = "branch",
        help = "Check out <branch> instead of the remote's default branch"
    )]
    branch: Option<String>,

    #[arg(
        short = 'n',
        long = "no-checkout",
        help = "Perform a bare clone only; do not create any worktree"
    )]
    no_checkout: bool,

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
        short = 'a',
        long = "all-branches",
        help = "Create a worktree for each remote branch, not just the default"
    )]
    all_branches: bool,

    #[arg(
        long = "trust-hooks",
        help = "Trust the repository and allow hooks to run without prompting"
    )]
    trust_hooks: bool,

    #[arg(long = "no-hooks", help = "Do not run any hooks from the repository")]
    no_hooks: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Organize worktree under this remote folder (enables multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-clone"));

    // Initialize logging based on verbose flag
    init_logging(args.verbose);

    if args.no_checkout && args.all_branches {
        anyhow::bail!("--no-checkout and --all-branches cannot be used together.\nUse --no-checkout to create only the bare repository, or --all-branches to create worktrees for all branches.");
    }

    if args.branch.is_some() && args.all_branches {
        anyhow::bail!("--branch and --all-branches cannot be used together.\nUse --branch to checkout a specific branch, or --all-branches to create worktrees for all branches.");
    }

    if args.branch.is_some() && args.no_checkout {
        anyhow::bail!("--branch and --no-checkout cannot be used together.\nUse --branch to checkout a specific branch, or --no-checkout to skip worktree creation.");
    }

    if args.trust_hooks && args.no_hooks {
        anyhow::bail!("--trust-hooks and --no-hooks cannot be used together.");
    }

    // Load settings from global config only (repo doesn't exist yet)
    let settings = DaftSettings::load_global()?;

    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_clone(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_clone(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    let repo_name = extract_repo_name(&args.repository_url)?;
    output.step(&format!("Repository name detected: '{repo_name}'"));

    // Try to get the default branch from remote first.
    // If this fails, check if the repository is empty (no commits).
    // This order ensures invalid URLs fail properly instead of being treated as empty repos.
    let (default_branch, target_branch, branch_exists, is_empty) =
        match get_default_branch_remote(&args.repository_url, settings.use_gitoxide) {
            Ok(default_branch) => {
                // Normal repo with commits - proceed with standard flow
                output.step(&format!("Default branch detected: '{default_branch}'"));

                // Determine the target branch and whether it exists on remote
                let (target_branch, branch_exists) = if let Some(ref specified_branch) = args.branch
                {
                    output.step(&format!(
                        "Checking if branch '{}' exists on remote...",
                        specified_branch
                    ));
                    let git =
                        GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
                    let exists = git
                        .ls_remote_branch_exists(&args.repository_url, specified_branch)
                        .unwrap_or(false);
                    if exists {
                        output.step(&format!("Branch '{specified_branch}' found on remote"));
                    } else {
                        output.warning(&format!(
                            "Branch '{specified_branch}' does not exist on remote"
                        ));
                    }
                    (specified_branch.clone(), exists)
                } else {
                    (default_branch.clone(), true)
                };
                (default_branch, target_branch, branch_exists, false)
            }
            Err(e) => {
                // Failed to get default branch - check if repo is empty
                if is_remote_empty(&args.repository_url, settings.use_gitoxide).unwrap_or(false) {
                    // Empty repo: use local default branch config
                    let local_default = resolve_initial_branch(&args.branch);
                    output.step(&format!(
                        "Empty repository detected, using branch: '{local_default}'"
                    ));
                    (local_default.clone(), local_default, false, true)
                } else {
                    // Not an empty repo - propagate the original error
                    return Err(e.context("Failed to determine default branch"));
                }
            }
        };

    let parent_dir = PathBuf::from(&repo_name);

    // Determine if we should use multi-remote mode
    // If --remote flag is provided, enable multi-remote layout for this clone
    let use_multi_remote = args.remote.is_some() || settings.multi_remote_enabled;
    let remote_for_path = args
        .remote
        .clone()
        .unwrap_or_else(|| settings.multi_remote_default.clone());

    let worktree_dir = calculate_worktree_path(
        &parent_dir,
        &target_branch,
        &remote_for_path,
        use_multi_remote,
    );

    output.step(&format!(
        "Target repository directory: './{}'",
        parent_dir.display()
    ));

    if !args.no_checkout {
        if args.all_branches {
            output.step("Worktrees will be created for all remote branches");
        } else if branch_exists {
            output.step(&format!(
                "Initial worktree will be in: './{}'",
                worktree_dir.display()
            ));
        } else {
            output.step("Worktree creation will be skipped (branch does not exist)");
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
    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);

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

    let config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };

    // Set up fetch refspec for bare repo (required for upstream tracking to work)
    output.step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&config.remote_name) {
        output.warning(&format!("Could not set fetch refspec: {e}"));
        // Continue execution - upstream tracking may not work but worktrees will
    }

    // Set multi-remote config if --remote was provided
    if args.remote.is_some() {
        output.step("Enabling multi-remote mode for this repository...");
        crate::multi_remote::config::set_multi_remote_enabled(&git, true)?;
        crate::multi_remote::config::set_multi_remote_default(&git, &remote_for_path)?;
    }

    // Create worktree if:
    // - Not in no-checkout mode, AND
    // - Either the branch exists on remote, OR the repo is empty (we'll create an orphan branch)
    let should_create_worktree = !args.no_checkout && (branch_exists || is_empty);

    if should_create_worktree {
        // Calculate the relative worktree path from the parent_dir (we're now in parent_dir)
        let relative_worktree_path = if use_multi_remote {
            PathBuf::from(&remote_for_path).join(&target_branch)
        } else {
            PathBuf::from(&target_branch)
        };

        if args.all_branches {
            if is_empty {
                // Empty repos have no branches to clone
                anyhow::bail!(
                    "Cannot use --all-branches with an empty repository (no branches exist)"
                );
            }
            create_all_worktrees(
                &git,
                &config,
                &default_branch,
                use_multi_remote,
                &remote_for_path,
                settings.use_gitoxide,
                output,
            )?;
        } else if is_empty {
            // Empty repo: create orphan worktree (no commits to checkout)
            create_orphan_worktree(&git, &target_branch, &relative_worktree_path, output)?;
        } else {
            create_single_worktree(&git, &target_branch, &relative_worktree_path, output)?;
        }

        output.step(&format!(
            "Changing directory to worktree: './{}'",
            relative_worktree_path.display()
        ));

        if let Err(e) = change_directory(&relative_worktree_path) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            return Err(e);
        }

        // Skip fetch and upstream setup for empty repos (no remote refs exist)
        if !is_empty {
            // Fetch to create remote tracking refs (bare clone only creates local refs)
            output.step(&format!(
                "Fetching from '{}' to set up remote tracking...",
                config.remote_name
            ));
            if let Err(e) = git.fetch(&config.remote_name, false) {
                output.warning(&format!("Could not fetch from remote: {e}"));
            }

            // Set up remote HEAD reference (must be after fetch so refs exist)
            if let Err(e) = git.remote_set_head_auto(&config.remote_name) {
                output.warning(&format!("Could not set remote HEAD: {e}"));
            }

            // Set up upstream tracking for the target branch (if enabled)
            if settings.checkout_upstream {
                output.step(&format!(
                    "Setting upstream to '{}/{}'...",
                    config.remote_name, target_branch
                ));
                if let Err(e) = git.set_upstream(&config.remote_name, &target_branch) {
                    output.warning(&format!(
                        "Could not set upstream tracking: {e}. You may need to set it manually."
                    ));
                }
            } else {
                output.step("Skipping upstream setup (disabled in config)");
            }
        }

        let current_dir = get_current_directory()?;

        // Git-like result message
        output.result(&format!("Cloned into '{repo_name}/{target_branch}'"));

        // Execute post-create hook (worktree was created)
        run_post_create_hook(
            args,
            &parent_dir,
            &git_dir,
            &config.remote_name,
            &current_dir,
            &target_branch,
            output,
        )?;

        // Execute post-clone hooks
        run_post_clone_hook(
            args,
            &parent_dir,
            &git_dir,
            &config.remote_name,
            &current_dir,
            &args.repository_url,
            &target_branch,
            output,
        )?;

        output.cd_path(&current_dir);
        maybe_show_shell_hint(output)?;
    } else if !args.no_checkout && !branch_exists {
        // Branch was specified but doesn't exist - stay in repo root
        let current_dir = get_current_directory()?;
        output.result(&format!(
            "Cloned '{repo_name}' (branch '{}' not found, no worktree created)",
            target_branch
        ));
        output.cd_path(&current_dir);
        maybe_show_shell_hint(output)?;
    } else {
        // Git-like result message for no-checkout mode
        output.result(&format!("Cloned '{repo_name}' (bare)"));
    }

    Ok(())
}

fn create_single_worktree(
    git: &GitCommand,
    branch: &str,
    worktree_path: &std::path::Path,
    output: &mut dyn Output,
) -> Result<()> {
    output.step(&format!(
        "Creating initial worktree for branch '{}' at '{}'...",
        branch,
        worktree_path.display()
    ));

    // Ensure parent directory exists (for multi-remote mode)
    if let Some(parent) = worktree_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
    }

    git.worktree_add(worktree_path, branch)
        .context("Failed to create initial worktree")?;
    Ok(())
}

/// Create an orphan worktree for an empty repository.
///
/// This is used when cloning an empty repository that has no commits.
/// The worktree is created with a new orphan branch that can receive
/// the first commit.
fn create_orphan_worktree(
    git: &GitCommand,
    branch: &str,
    worktree_path: &std::path::Path,
    output: &mut dyn Output,
) -> Result<()> {
    output.step(&format!(
        "Creating initial worktree for empty repository at '{}'...",
        worktree_path.display()
    ));

    // Ensure parent directory exists (for multi-remote mode)
    if let Some(parent) = worktree_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
    }

    git.worktree_add_orphan(worktree_path, branch)
        .context("Failed to create initial worktree for empty repository")?;
    Ok(())
}

fn create_all_worktrees(
    git: &GitCommand,
    config: &WorktreeConfig,
    _default_branch: &str,
    use_multi_remote: bool,
    remote_for_path: &str,
    use_gitoxide: bool,
    output: &mut dyn Output,
) -> Result<()> {
    output.step("Fetching all remote branches...");
    git.fetch(&config.remote_name, false)?;

    let remote_branches = get_remote_branches(&config.remote_name, use_gitoxide)
        .context("Failed to get remote branches")?;

    if remote_branches.is_empty() {
        anyhow::bail!("No remote branches found");
    }

    for branch in &remote_branches {
        let worktree_path = if use_multi_remote {
            PathBuf::from(remote_for_path).join(branch)
        } else {
            PathBuf::from(branch)
        };

        output.step(&format!(
            "Creating worktree for branch '{}' at '{}'...",
            branch,
            worktree_path.display()
        ));

        // Ensure parent directory exists (for multi-remote mode)
        if let Some(parent) = worktree_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    output.error(&format!("creating directory '{}': {e}", parent.display()));
                    continue;
                }
            }
        }

        if let Err(e) = git.worktree_add(&worktree_path, branch) {
            output.error(&format!("creating worktree for branch '{branch}': {e}"));
            continue;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_post_create_hook(
    args: &Args,
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    // Skip hooks if --no-hooks flag is set
    if args.no_hooks {
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    // If --trust-hooks flag is set, trust the repository first
    if args.trust_hooks {
        executor.trust_repository(git_dir, TrustLevel::Allow)?;
    }

    // Build the hook context
    let ctx = HookContext::new(
        HookType::PostCreate,
        "clone",
        project_root,
        git_dir,
        remote_name,
        worktree_path, // source and target are the same for clone
        worktree_path,
        branch_name,
    )
    .with_new_branch(false);

    // Execute the hook (ignore skipped result for post-create during clone)
    executor.execute(&ctx, output)?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_post_clone_hook(
    args: &Args,
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    worktree_path: &PathBuf,
    repository_url: &str,
    default_branch: &str,
    output: &mut dyn Output,
) -> Result<()> {
    // Skip hooks if --no-hooks flag is set
    if args.no_hooks {
        output.step("Skipping hooks (--no-hooks flag)");
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    // If --trust-hooks flag is set, trust the repository first
    if args.trust_hooks {
        output.step("Trusting repository for hooks (--trust-hooks flag)");
        executor.trust_repository(git_dir, TrustLevel::Allow)?;
    }

    // Build the hook context
    let ctx = HookContext::new(
        HookType::PostClone,
        "clone",
        project_root,
        git_dir,
        remote_name,
        worktree_path, // source and target are the same for clone
        worktree_path,
        default_branch,
    )
    .with_repository_url(repository_url)
    .with_default_branch(default_branch)
    .with_new_branch(false);

    // Execute the hook
    let result = executor.execute(&ctx, output)?;

    // If hooks were skipped due to trust, show notice
    if result.skipped {
        if let Some(reason) = &result.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(worktree_path, git_dir, output);
            }
        }
    }

    Ok(())
}
