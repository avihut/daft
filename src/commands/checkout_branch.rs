use crate::{
    core::{worktree::checkout_branch, CommandBridge},
    get_project_root,
    git::GitCommand,
    hints::maybe_show_shell_hint,
    hooks::{HookExecutor, HooksConfig},
    is_git_repository, logging,
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

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-checkout-branch"));

    logging::init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

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
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let params = checkout_branch::CheckoutBranchParams {
        new_branch_name: args.new_branch_name.clone(),
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

    let result = {
        let mut bridge = CommandBridge::new(output, executor);
        checkout_branch::execute(&params, &git, &project_root, &mut bridge)?
    };

    render_result(&result, output);

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(())
}

fn render_result(result: &checkout_branch::CheckoutBranchResult, output: &mut dyn Output) {
    output.result(&format!(
        "Created worktree '{}' from '{}'",
        result.new_branch_name, result.base_branch
    ));
}
