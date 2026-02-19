use crate::{
    get_git_common_dir, get_project_root,
    git::GitCommand,
    hints::maybe_show_shell_hint,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig},
    is_git_repository,
    logging::init_logging,
    multi_remote::path::{calculate_worktree_path, resolve_remote_for_branch},
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-checkout")]
#[command(version = crate::VERSION)]
#[command(about = "Create a worktree for an existing branch")]
#[command(long_about = r#"
Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

This command can be run from anywhere within the repository. If a worktree
for the specified branch already exists, no new worktree is created; the
working directory is changed to the existing worktree instead.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(help = "Name of the branch to check out")]
    branch_name: String,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes (this is the default)")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-checkout"));

    // Initialize logging based on verbosity flag
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    // Load settings from git config
    let settings = DaftSettings::load()?;

    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(false, args.verbose, autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let branch_name = &args.branch_name;

    validate_branch_name(branch_name)?;

    let project_root = get_project_root()?;
    let git_dir = get_git_common_dir()?;
    let source_worktree = get_current_directory()?;

    let config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);

    // Resolve remote for multi-remote mode
    let remote_for_path = resolve_remote_for_branch(
        &git,
        branch_name,
        args.remote.as_deref(),
        &settings.multi_remote_default,
    )?;

    // Calculate worktree path based on multi-remote mode
    let worktree_path = calculate_worktree_path(
        &project_root,
        branch_name,
        &remote_for_path,
        settings.multi_remote_enabled,
    );

    output.step(&format!(
        "Path: {}, Branch: {}, Project Root: {}",
        worktree_path.display(),
        branch_name,
        project_root.display()
    ));

    // Check if worktree already exists for this branch
    if let Some(existing_path) = find_existing_worktree_for_branch(&git, branch_name)? {
        output.step(&format!(
            "Branch '{}' already has a worktree at '{}'",
            branch_name,
            existing_path.display()
        ));
        output.step("Changing to existing worktree...");
        change_directory(&existing_path)?;
        output.result(&format!("Switched to existing worktree '{}'", branch_name));

        // Run exec commands (after cd, before cd_path)
        let exec_result = crate::exec::run_exec_commands(&args.exec, output);

        output.cd_path(&get_current_directory()?);
        maybe_show_shell_hint(output)?;

        // Propagate exec error after cd_path is written
        exec_result?;

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
        branch_name, config.remote_name
    ));
    if let Err(e) = git.fetch_refspec(
        &config.remote_name,
        &format!("{}:{}", branch_name, branch_name),
    ) {
        output.warning(&format!("Failed to fetch specific branch: {e}"));
    }

    // Check if local and/or remote branch exists
    let local_branch_ref = format!("refs/heads/{}", branch_name);
    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, branch_name);
    let local_exists = git.show_ref_exists(&local_branch_ref)?;
    let remote_exists = git.show_ref_exists(&remote_branch_ref)?;

    if !local_exists && !remote_exists {
        anyhow::bail!(
            "Branch '{}' does not exist locally or on remote '{}'",
            branch_name,
            config.remote_name
        );
    }

    // Determine whether to use local branch or create from remote
    let use_local_branch = if local_exists {
        output.step(&format!(
            "Local branch '{}' found, using it for worktree creation",
            branch_name
        ));
        true
    } else {
        output.step(&format!(
            "Local branch '{}' not found, will create from remote '{}/{}'",
            branch_name, config.remote_name, branch_name
        ));
        false
    };

    // Determine carry behavior:
    // 1. --carry flag explicitly set -> carry
    // 2. --no-carry flag explicitly set -> don't carry
    // 3. Neither flag set -> use settings.checkout_carry
    let should_carry = if args.carry {
        true
    } else if args.no_carry {
        false
    } else {
        settings.checkout_carry
    };

    // Check for uncommitted changes and stash them if should_carry is true
    // Skip the check if we're not inside a work tree (e.g., running from the bare repo root)
    let in_worktree = git.rev_parse_is_inside_work_tree().unwrap_or(false);
    let stash_created = if should_carry && in_worktree {
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

    // Run pre-create hook
    run_pre_create_hook(
        &project_root,
        &git_dir,
        &config.remote_name,
        &source_worktree,
        &worktree_path,
        branch_name,
        false, // not a new branch (existing branch checkout)
        output,
    )?;

    // Create worktree: use local branch directly, or create local branch from remote
    let worktree_result = if use_local_branch {
        git.worktree_add(&worktree_path, branch_name)
    } else {
        // Create a new local branch tracking the remote branch
        let remote_ref = format!("{}/{}", config.remote_name, branch_name);
        git.worktree_add_new_branch(&worktree_path, branch_name, &remote_ref)
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
        branch_name
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

    // Set upstream only if checkout_upstream is enabled
    if settings.checkout_upstream {
        let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, branch_name);
        output.step(&format!(
            "Checking for remote branch '{}/{}'...",
            config.remote_name, branch_name
        ));

        if git.show_ref_exists(&remote_branch_ref)? {
            output.step(&format!(
                "Setting upstream to '{}/{}'...",
                config.remote_name, branch_name
            ));

            if let Err(e) = git.set_upstream(&config.remote_name, branch_name) {
                output.warning(&format!(
                    "Failed to set upstream tracking: {}. Worktree created, but upstream may need manual configuration.",
                    e
                ));
            } else {
                output.step(&format!(
                    "Upstream tracking set to '{}/{}'",
                    config.remote_name, branch_name
                ));
            }
        } else {
            output.step(&format!(
                "Remote branch '{}/{}' not found, skipping upstream setup",
                config.remote_name, branch_name
            ));
        }
    } else {
        output.step("Skipping upstream setup (disabled in config)");
    }

    // Git-like result message (before hooks so it appears first)
    output.result(&format!("Prepared worktree '{}'", branch_name));

    // Run post-create hook
    run_post_create_hook(
        &project_root,
        &git_dir,
        &config.remote_name,
        &source_worktree,
        &worktree_path,
        branch_name,
        false, // not a new branch
        output,
    )?;

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&get_current_directory()?);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

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

#[allow(clippy::too_many_arguments)]
fn run_pre_create_hook(
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    source_worktree: &PathBuf,
    worktree_path: &PathBuf,
    branch_name: &str,
    is_new_branch: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let ctx = HookContext::new(
        HookType::PreCreate,
        "checkout",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_new_branch(is_new_branch);

    let result = executor.execute(&ctx, output)?;

    // Pre-create hooks with fail_mode=abort will return an error if they fail
    // If they succeed or are skipped, we continue
    if !result.success && !result.skipped {
        anyhow::bail!("Pre-create hook failed");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_post_create_hook(
    project_root: &PathBuf,
    git_dir: &PathBuf,
    remote_name: &str,
    source_worktree: &PathBuf,
    worktree_path: &PathBuf,
    branch_name: &str,
    is_new_branch: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let ctx = HookContext::new(
        HookType::PostCreate,
        "checkout",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_new_branch(is_new_branch);

    // Execute the hook (ignores if no hooks exist or not trusted)
    executor.execute(&ctx, output)?;

    Ok(())
}
