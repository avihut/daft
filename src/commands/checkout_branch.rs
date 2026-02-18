use crate::{
    config::git::{COMMITS_AHEAD_THRESHOLD, DEFAULT_COMMIT_COUNT},
    get_current_branch, get_git_common_dir, get_project_root,
    git::GitCommand,
    hints::maybe_show_shell_hint,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig},
    is_git_repository, logging,
    multi_remote::path::{calculate_worktree_path, resolve_remote_for_branch},
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-checkout-branch")]
#[command(version = crate::VERSION)]
#[command(about = "Create a worktree with a new branch")]
#[command(long_about = r#"
Creates a new branch and a corresponding worktree in a single operation. The
new branch is based on the current branch, or on <base-branch> if specified.
The worktree is placed at the project root level as a sibling to other
worktrees.

After creating the branch locally, this command pushes it to the remote and
configures upstream tracking. By default, uncommitted changes from the current
worktree are carried to the new worktree; use --no-carry to disable this.

This command can be run from anywhere within the repository. Lifecycle hooks
from .daft/hooks/ are executed if the repository is trusted. See git-daft(1)
for hook management.
"#)]
pub struct Args {
    #[arg(help = "Name for the new branch (also used as the worktree directory name)")]
    new_branch_name: String,

    #[arg(help = "Branch to use as the base for the new branch; defaults to the current branch")]
    base_branch_name: Option<String>,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes to the new worktree (this is the default)"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes to the new worktree")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-checkout-branch"));

    // Initialize logging based on verbosity flags
    logging::init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    // Load settings from git config
    let settings = DaftSettings::load()?;

    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout_branch(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout_branch(
    args: &Args,
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<()> {
    validate_branch_name(&args.new_branch_name)?;

    let base_branch = match &args.base_branch_name {
        Some(branch) => {
            output.step(&format!(
                "Using explicitly provided base branch: '{branch}'"
            ));
            branch.clone()
        }
        None => {
            output.step("Base branch not specified, using current branch...");
            let current = get_current_branch()?;
            output.step(&format!("Using current branch as base: '{current}'"));
            current
        }
    };

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
        &args.new_branch_name,
        args.remote.as_deref(),
        &settings.multi_remote_default,
    )?;

    // Calculate worktree path based on multi-remote mode
    let worktree_path = calculate_worktree_path(
        &project_root,
        &args.new_branch_name,
        &remote_for_path,
        settings.multi_remote_enabled,
    );

    // Fetch latest changes from remote to ensure we have the latest version of the base branch
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

    // Ensure remote tracking branches are created (needed for bare repositories)
    output.step("Setting up remote tracking branches...");
    if let Err(e) = git.fetch_refspec(
        &config.remote_name,
        &format!("+refs/heads/*:refs/remotes/{}/*", config.remote_name),
    ) {
        output.warning(&format!("Failed to set up remote tracking branches: {e}"));
    }

    // Three-way branch selection algorithm for optimal worktree base branch
    let local_branch_ref = format!("refs/heads/{base_branch}");
    let remote_branch_ref = format!("refs/remotes/{}/{}", config.remote_name, base_branch);

    let checkout_base =
        if git.show_ref_exists(&remote_branch_ref)? && git.show_ref_exists(&local_branch_ref)? {
            // Both local and remote exist - use commit comparison
            let local_ahead = git
                .rev_list_count(&format!(
                    "{}..{}",
                    &format!("{}/{}", config.remote_name, base_branch),
                    &base_branch
                ))
                .unwrap_or(DEFAULT_COMMIT_COUNT)
                > COMMITS_AHEAD_THRESHOLD;

            if local_ahead {
                output.step(&format!(
                    "Using local branch '{base_branch}' as base (has local commits)"
                ));
                base_branch.clone()
            } else {
                output.step(&format!(
                    "Using remote branch '{}/{}' as base (has latest changes)",
                    config.remote_name, base_branch
                ));
                format!("{}/{}", config.remote_name, base_branch)
            }
        } else if git.show_ref_exists(&local_branch_ref)? {
            output.step(&format!("Using local branch '{base_branch}' as base"));
            base_branch.clone()
        } else if git.show_ref_exists(&remote_branch_ref)? {
            output.step(&format!(
                "Local branch '{}' not found, using remote branch '{}/{}'",
                base_branch, config.remote_name, base_branch
            ));
            format!("{}/{}", config.remote_name, base_branch)
        } else {
            output.step(&format!(
                "Neither local nor remote branch found for '{base_branch}', using as-is"
            ));
            base_branch.clone()
        };

    // Determine carry behavior:
    // 1. --carry flag explicitly set -> carry
    // 2. --no-carry flag explicitly set -> don't carry
    // 3. Neither flag set -> use settings.checkout_branch_carry
    let should_carry = if args.carry {
        true
    } else if args.no_carry {
        false
    } else {
        settings.checkout_branch_carry
    };

    // Determine the carry source worktree:
    // - If base_branch_name was explicitly provided, carry from that branch's worktree
    //   (if it exists; if not, silently skip carry)
    // - If no base branch specified, carry from the current worktree
    let carry_source = if should_carry {
        if args.base_branch_name.is_some() {
            // Explicit base branch: find its worktree
            match git.find_worktree_for_branch(&base_branch) {
                Ok(Some(path)) => {
                    output.step(&format!(
                        "Found worktree for base branch '{}' at '{}'",
                        base_branch,
                        path.display()
                    ));
                    Some(path)
                }
                Ok(None) => {
                    output.step(&format!(
                        "No worktree found for base branch '{}', skipping carry",
                        base_branch
                    ));
                    None
                }
                Err(e) => {
                    output.warning(&format!(
                        "Could not look up worktree for base branch '{}': {e}",
                        base_branch
                    ));
                    None
                }
            }
        } else {
            // No explicit base branch: carry from current worktree
            let in_worktree = git.rev_parse_is_inside_work_tree().unwrap_or(false);
            if in_worktree {
                Some(get_current_directory()?)
            } else {
                output.step("Skipping carry (not inside a worktree)");
                None
            }
        }
    } else {
        output.step("Skipping carry (--no-carry flag set or carry disabled in config)");
        None
    };

    // Check for uncommitted changes and stash them from the carry source worktree
    let stash_created = if let Some(carry_path) = &carry_source {
        // cd to the carry source worktree to check and stash changes
        change_directory(carry_path)?;
        match git.has_uncommitted_changes() {
            Ok(true) => {
                output.step(&format!(
                    "Stashing uncommitted changes from '{}'...",
                    carry_path.display()
                ));
                if let Err(e) = git.stash_push_with_untracked("daft: carry changes to new worktree")
                {
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
        &args.new_branch_name,
        Some(&base_branch),
        output,
    )?;

    output.step(&format!(
        "Creating worktree at '{}' with new branch '{}' from '{}'",
        worktree_path.display(),
        args.new_branch_name,
        checkout_base
    ));

    if let Err(e) =
        git.worktree_add_new_branch(&worktree_path, &args.new_branch_name, &checkout_base)
    {
        // If worktree creation fails and we stashed changes, restore them to the carry source
        if stash_created {
            if let Some(carry_path) = &carry_source {
                change_directory(carry_path).ok();
            }
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
        "Changing directory to worktree: {}",
        worktree_path.display()
    ));
    change_directory(&worktree_path)?;

    // Apply stashed changes to the new worktree
    if stash_created {
        output.step("Applying stashed changes to new worktree...");
        if let Err(e) = git.stash_pop() {
            output.warning(&format!(
                "Stash could not be applied cleanly. Resolve conflicts and run 'git stash pop'. Error: {e}"
            ));
        } else {
            output.step("Changes successfully applied to new worktree");
        }
    }

    // Push and set upstream only if checkout_push is enabled
    if settings.checkout_push {
        output.step(&format!(
            "Pushing and setting upstream to '{}/{}'...",
            config.remote_name, args.new_branch_name
        ));

        if let Err(e) = git.push_set_upstream(&config.remote_name, &args.new_branch_name) {
            output.warning(&format!(
                "Could not push '{}' to '{}': {}. The worktree is ready locally. Push manually with: git push -u {} {}",
                args.new_branch_name, config.remote_name, e,
                config.remote_name, args.new_branch_name
            ));
        } else {
            output.step(&format!(
                "Push to '{}' and upstream tracking set successfully",
                config.remote_name
            ));
        }
    } else {
        output.step("Skipping push (disabled in config)");
    }

    // Git-like result message (before hooks so it appears first)
    output.result(&format!(
        "Created worktree '{}' from '{}'",
        args.new_branch_name, checkout_base
    ));

    // Run post-create hook
    run_post_create_hook(
        &project_root,
        &git_dir,
        &config.remote_name,
        &source_worktree,
        &worktree_path,
        &args.new_branch_name,
        Some(&base_branch),
        output,
    )?;

    output.cd_path(&get_current_directory()?);
    maybe_show_shell_hint(output)?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_pre_create_hook(
    project_root: &std::path::PathBuf,
    git_dir: &std::path::PathBuf,
    remote_name: &str,
    source_worktree: &std::path::PathBuf,
    worktree_path: &std::path::PathBuf,
    branch_name: &str,
    base_branch: Option<&str>,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let mut ctx = HookContext::new(
        HookType::PreCreate,
        "checkout-branch",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_new_branch(true);

    if let Some(base) = base_branch {
        ctx = ctx.with_base_branch(base);
    }

    let result = executor.execute(&ctx, output)?;

    if !result.success && !result.skipped {
        anyhow::bail!("Pre-create hook failed");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_post_create_hook(
    project_root: &std::path::PathBuf,
    git_dir: &std::path::PathBuf,
    remote_name: &str,
    source_worktree: &std::path::PathBuf,
    worktree_path: &std::path::PathBuf,
    branch_name: &str,
    base_branch: Option<&str>,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let mut ctx = HookContext::new(
        HookType::PostCreate,
        "checkout-branch",
        project_root,
        git_dir,
        remote_name,
        source_worktree,
        worktree_path,
        branch_name,
    )
    .with_new_branch(true);

    if let Some(base) = base_branch {
        ctx = ctx.with_base_branch(base);
    }

    executor.execute(&ctx, output)?;

    Ok(())
}
