use crate::{
    check_dependencies,
    core::{worktree::clone, OutputSink},
    git::should_show_gitoxide_notice,
    hints::maybe_show_shell_hint,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, TrustLevel},
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
};
use anyhow::Result;
use clap::Parser;

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

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-clone"));

    init_logging(args.verbose);

    validate_arg_combinations(&args)?;

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

fn validate_arg_combinations(args: &Args) -> Result<()> {
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
    Ok(())
}

fn run_clone(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    let params = clone::CloneParams {
        repository_url: args.repository_url.clone(),
        branch: args.branch.clone(),
        no_checkout: args.no_checkout,
        all_branches: args.all_branches,
        remote: args.remote.clone(),
        remote_name: settings.remote.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_upstream: settings.checkout_upstream,
        use_gitoxide: settings.use_gitoxide,
    };

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Cloning repository...");
    let result = {
        let mut sink = OutputSink(output);
        clone::execute(&params, &mut sink)?
    };
    output.finish_spinner();

    render_clone_result(&result, output);

    // Run hooks and exec only if a worktree was created
    if result.worktree_dir.is_some() {
        run_post_clone_hook(args, &result, output)?;
        run_post_create_hook(args, &result, output)?;

        let exec_result = crate::exec::run_exec_commands(&args.exec, output);

        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;

        exec_result?;
    } else if result.branch_not_found {
        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;
    }

    Ok(())
}

fn render_clone_result(result: &clone::CloneResult, output: &mut dyn Output) {
    if result.worktree_dir.is_some() {
        output.result(&format!(
            "Cloned into '{}/{}'",
            result.repo_name, result.target_branch
        ));
    } else if result.branch_not_found {
        output.result(&format!(
            "Cloned '{}' (branch '{}' not found, no worktree created)",
            result.repo_name, result.target_branch
        ));
    } else {
        output.result(&format!("Cloned '{}' (bare)", result.repo_name));
    }
}

fn run_post_clone_hook(
    args: &Args,
    result: &clone::CloneResult,
    output: &mut dyn Output,
) -> Result<()> {
    if args.no_hooks {
        output.step("Skipping hooks (--no-hooks flag)");
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    if args.trust_hooks {
        output.step("Trusting repository for hooks (--trust-hooks flag)");
        executor.trust_repository(&result.git_dir, TrustLevel::Allow)?;
    }

    let worktree_path = result.worktree_dir.as_ref().unwrap();

    let ctx = HookContext::new(
        HookType::PostClone,
        "clone",
        &result.parent_dir,
        &result.git_dir,
        &result.remote_name,
        worktree_path,
        worktree_path,
        &result.target_branch,
    )
    .with_repository_url(&result.repository_url)
    .with_default_branch(&result.default_branch)
    .with_new_branch(false);

    let hook_result = executor.execute(&ctx, output)?;

    if hook_result.skipped {
        if let Some(reason) = &hook_result.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(worktree_path, &result.git_dir, output);
            }
        }
    }

    Ok(())
}

fn run_post_create_hook(
    args: &Args,
    result: &clone::CloneResult,
    output: &mut dyn Output,
) -> Result<()> {
    if args.no_hooks {
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    if args.trust_hooks {
        executor.trust_repository(&result.git_dir, TrustLevel::Allow)?;
    }

    let worktree_path = result.worktree_dir.as_ref().unwrap();

    let ctx = HookContext::new(
        HookType::PostCreate,
        "clone",
        &result.parent_dir,
        &result.git_dir,
        &result.remote_name,
        worktree_path,
        worktree_path,
        &result.target_branch,
    )
    .with_new_branch(false);

    executor.execute(&ctx, output)?;

    Ok(())
}
