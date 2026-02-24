use crate::{
    core::{
        worktree::{checkout, checkout_branch, previous},
        CommandBridge,
    },
    get_current_worktree_path, get_git_common_dir, get_project_root,
    git::{should_show_gitoxide_notice, GitCommand},
    hints::maybe_show_shell_hint,
    hooks::{HookExecutor, HooksConfig},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-checkout")]
#[command(version = crate::VERSION)]
#[command(about = "Create a worktree for an existing branch, or a new branch with -b")]
#[command(long_about = r#"
Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

With -b, creates a new branch and a corresponding worktree in a single
operation. The new branch is based on the current branch, or on <base-branch>
if specified. After creating the branch locally, it is pushed to the remote
and upstream tracking is configured.

With --start (or -s), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically, as if 'daft start'
had been called. This can also be enabled permanently with the daft.go.autoStart
git config option.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.
Cannot be combined with -b/--create-branch.

This command can be run from anywhere within the repository. If a worktree
for the specified branch already exists, no new worktree is created; the
working directory is changed to the existing worktree instead.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(
        help = "Name of the branch to check out (or create with -b); use '-' for previous worktree",
        allow_hyphen_values = true
    )]
    branch_name: String,

    #[arg(
        help = "Branch to use as the base for the new branch (only with -b); defaults to the current branch"
    )]
    base_branch_name: Option<String>,

    #[arg(
        short = 'b',
        long = "create-branch",
        help = "Create a new branch instead of checking out an existing one"
    )]
    create_branch: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
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

    #[arg(
        short = 's',
        long = "start",
        help = "Create a new worktree if the branch does not exist"
    )]
    start: bool,
}

/// Daft-style args for `daft go`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to navigating worktrees, with tailored about text.
#[derive(Parser)]
#[command(name = "daft go")]
#[command(version = crate::VERSION)]
#[command(about = "Open a worktree for an existing branch, or create one with -b")]
#[command(long_about = r#"
Opens a worktree for an existing local or remote branch. The worktree is
placed at the project root level as a sibling to other worktrees, using the
branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

If a worktree for the specified branch already exists, no new worktree is
created; the working directory is changed to the existing worktree instead.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.

With -b, creates a new branch and worktree in a single operation. The new
branch is based on the current branch, or on <base-branch> if specified.
Prefer 'daft start' for creating new branches.

With -s (--start), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically. This can also
be enabled permanently with the daft.go.autoStart git config option.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct GoArgs {
    #[arg(
        help = "Branch to open; use '-' for previous worktree",
        allow_hyphen_values = true
    )]
    branch_name: String,

    #[arg(
        help = "Base branch for -b (defaults to current branch)",
        requires = "create_branch"
    )]
    base_branch_name: Option<String>,

    #[arg(
        short = 'b',
        long = "create-branch",
        help = "Create a new branch (prefer 'daft start' instead)"
    )]
    create_branch: bool,

    #[arg(
        short = 's',
        long = "start",
        help = "Create a new worktree if the branch does not exist"
    )]
    start: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
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

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Daft-style args for `daft start`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to creating new branches, without `-b` or `--start`.
#[derive(Parser)]
#[command(name = "daft start")]
#[command(version = crate::VERSION)]
#[command(about = "Create a new branch and worktree")]
#[command(long_about = r#"
Creates a new branch and a corresponding worktree in a single operation. The
worktree is placed at the project root level as a sibling to other worktrees,
using the branch name as the directory name.

The new branch is based on the current branch, or on <base-branch> if
specified. After creating the branch locally, it is pushed to the remote and
upstream tracking is configured (unless disabled via daft.checkoutBranch.push).

This command can be run from anywhere within the repository.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct StartArgs {
    #[arg(help = "Name for the new branch")]
    new_branch_name: String,

    #[arg(help = "Branch to use as the base; defaults to the current branch")]
    base_branch_name: Option<String>,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
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

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Entry point for `git-worktree-checkout`.
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-checkout"));
    run_with_args(args)
}

/// Entry point for `daft go`.
pub fn run_go() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-go");
    raw[0] = "daft go".to_string();
    let go_args = GoArgs::parse_from(raw);

    let args = Args {
        branch_name: go_args.branch_name,
        base_branch_name: go_args.base_branch_name,
        create_branch: go_args.create_branch,
        start: go_args.start,
        carry: go_args.carry,
        no_carry: go_args.no_carry,
        remote: go_args.remote,
        no_cd: go_args.no_cd,
        exec: go_args.exec,
        quiet: go_args.quiet,
        verbose: go_args.verbose,
    };
    run_with_args(args)
}

/// Entry point for `daft start`.
pub fn run_start() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-start");
    raw[0] = "daft start".to_string();
    let start_args = StartArgs::parse_from(raw);

    let args = Args {
        branch_name: start_args.new_branch_name,
        base_branch_name: start_args.base_branch_name,
        create_branch: true,
        start: false,
        carry: start_args.carry,
        no_carry: start_args.no_carry,
        remote: start_args.remote,
        no_cd: start_args.no_cd,
        exec: start_args.exec,
        quiet: start_args.quiet,
        verbose: start_args.verbose,
    };
    run_with_args(args)
}

fn run_with_args(args: Args) -> Result<()> {
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    // Handle `daft go -` (previous worktree navigation)
    if args.branch_name == "-" {
        if args.create_branch {
            anyhow::bail!("Cannot use '-' with -b/--create-branch");
        }

        let settings = DaftSettings::load()?;
        let autocd = settings.autocd && !args.no_cd;
        let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
        let mut output = CliOutput::new(config);
        return run_go_previous(&mut output);
    }

    // Validate: base_branch_name only valid with -b
    if args.base_branch_name.is_some() && !args.create_branch {
        anyhow::bail!("<BASE_BRANCH_NAME> can only be used with -b/--create-branch");
    }

    let settings = DaftSettings::load()?;

    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    // Capture source worktree before the operation (best-effort)
    let source_worktree = get_current_worktree_path().ok();

    let result = if args.create_branch {
        run_create_branch(&args, &settings, &mut output)
    } else {
        match run_checkout(&args, &settings, &mut output) {
            Ok(()) => Ok(()),
            Err(checkout::CheckoutError::BranchNotFound {
                ref branch,
                ref remote,
                fetch_failed,
            }) => {
                let auto_start = args.start || settings.go_auto_start;
                if auto_start {
                    change_directory(&original_dir).ok();
                    output.result(&format!(
                        "Branch '{branch}' not found, creating new worktree..."
                    ));
                    run_create_branch(&args, &settings, &mut output)
                } else {
                    change_directory(&original_dir).ok();
                    render_branch_not_found_error(branch, remote, fetch_failed, &settings);
                    // Exit directly: the error is already rendered to stderr with
                    // custom formatting. Returning an anyhow error here would cause
                    // main() to print a redundant "Error:" line.
                    std::process::exit(1);
                }
            }
            Err(checkout::CheckoutError::Other(e)) => Err(e),
        }
    };

    if let Err(e) = result {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    // Save the source worktree as previous (best-effort, after success)
    if let Some(src) = source_worktree {
        if let Ok(git_dir) = get_git_common_dir() {
            let _ = previous::save(&git_dir, &src);
        }
    }

    Ok(())
}

/// Navigate to the previous worktree (`daft go -`).
fn run_go_previous(output: &mut dyn Output) -> Result<()> {
    let git_dir = get_git_common_dir()?;

    let previous_path = previous::load(&git_dir)?
        .ok_or_else(|| anyhow::anyhow!("No previous worktree to switch to"))?;

    if !previous_path.exists() {
        anyhow::bail!(
            "Previous worktree no longer exists: '{}'",
            previous_path.display()
        );
    }

    // Save current worktree as the new previous before switching
    if let Ok(current) = get_current_worktree_path() {
        let _ = previous::save(&git_dir, &current);
    }

    change_directory(&previous_path)?;

    // Try to get the branch name for display
    let branch_display =
        crate::get_current_branch().unwrap_or_else(|_| previous_path.display().to_string());
    output.result(&format!("Switched to worktree '{branch_display}'"));

    output.cd_path(&previous_path);
    maybe_show_shell_hint(output)?;

    Ok(())
}

fn run_checkout(
    args: &Args,
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<(), checkout::CheckoutError> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let params = checkout::CheckoutParams {
        branch_name: args.branch_name.clone(),
        carry: args.carry,
        no_carry: args.no_carry,
        remote: args.remote.clone(),
        remote_name: wt_config.remote_name.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_carry: settings.checkout_carry,
        checkout_upstream: settings.checkout_upstream,
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Preparing worktree...");
    let (result, executor) = {
        let mut bridge = CommandBridge::new(output, executor);
        let result = checkout::execute(&params, &git, &project_root, &mut bridge)?;
        (result, bridge.into_executor())
    };
    output.finish_spinner();

    render_checkout_result(&result, output);

    // Show hooks notice if skipped due to trust
    if result.post_hook_outcome.skipped {
        if let Some(reason) = &result.post_hook_outcome.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(&result.worktree_path, &result.git_dir, output);
            }
        }
    }

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(())
}

fn run_create_branch(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let params = checkout_branch::CheckoutBranchParams {
        new_branch_name: args.branch_name.clone(),
        base_branch_name: args.base_branch_name.clone(),
        carry: args.carry,
        no_carry: args.no_carry,
        remote: args.remote.clone(),
        remote_name: wt_config.remote_name.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_branch_carry: settings.checkout_branch_carry,
        checkout_push: settings.checkout_push,
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Creating worktree...");
    let (result, executor) = {
        let mut bridge = CommandBridge::new(output, executor);
        let result = checkout_branch::execute(&params, &git, &project_root, &mut bridge)?;
        (result, bridge.into_executor())
    };
    output.finish_spinner();

    render_create_result(&result, output);

    // Show hooks notice if skipped due to trust
    if result.post_hook_outcome.skipped {
        if let Some(reason) = &result.post_hook_outcome.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(&result.worktree_path, &result.git_dir, output);
            }
        }
    }

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(())
}

fn render_branch_not_found_error(
    branch: &str,
    remote: &str,
    fetch_failed: bool,
    settings: &DaftSettings,
) {
    // Section 1: Diagnosis
    if fetch_failed {
        eprintln!(
            "error: Branch '{branch}' not found -- could not reach remote '{remote}' to check"
        );
    } else {
        eprintln!(
            "error: Branch '{branch}' not found -- it does not exist locally or on remote '{remote}'"
        );
    }

    // Section 2: Start suggestion (skip if fetch failed since start would also likely fail)
    if !fetch_failed {
        eprintln!();
        eprintln!("  tip: Use `daft go --start {branch}` or `daft start {branch}` to create it");
    }

    // Section 3: Fuzzy matches
    let git = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let all_branches = checkout::collect_branch_names(&git, remote);
    let suggestions = crate::suggest::find_similar(branch, &all_branches, 5);
    if !suggestions.is_empty() {
        eprintln!();
        if suggestions.len() == 1 {
            eprintln!("  Did you mean this?");
        } else {
            eprintln!("  Did you mean one of these?");
        }
        for s in &suggestions {
            eprintln!("    {s}");
        }
    }
}

fn render_checkout_result(result: &checkout::CheckoutResult, output: &mut dyn Output) {
    if result.already_existed {
        output.result(&format!(
            "Switched to existing worktree '{}'",
            result.branch_name
        ));
    } else {
        output.result(&format!("Prepared worktree '{}'", result.branch_name));
    }
}

fn render_create_result(result: &checkout_branch::CheckoutBranchResult, output: &mut dyn Output) {
    output.result(&format!(
        "Created worktree '{}' from '{}'",
        result.new_branch_name, result.base_branch
    ));
}
