use crate::{
    core::{worktree::checkout, CommandBridge},
    get_project_root,
    git::GitCommand,
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

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

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

    let result = {
        let mut bridge = CommandBridge::new(output, executor);
        checkout::execute(&params, &git, &project_root, &mut bridge)?
    };

    render_checkout_result(&result, output);

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(())
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
