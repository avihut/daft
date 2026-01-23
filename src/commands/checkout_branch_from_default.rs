use anyhow::{Context, Result};
use clap::Parser;
use daft::{
    config::git::DEFAULT_EXIT_CODE,
    get_git_common_dir, is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::get_default_branch_local,
    settings::DaftSettings,
    utils::*,
};
use std::process::Command;

#[derive(Parser)]
#[command(name = "git-worktree-checkout-branch-from-default")]
#[command(version = daft::VERSION)]
#[command(about = "Create a worktree with a new branch based on the remote's default branch")]
#[command(long_about = r#"
Creates a new branch based on the remote's default branch (typically main or
master) and a corresponding worktree. This is equivalent to running
git-worktree-checkout-branch(1) with the default branch as the base.

The default branch is determined by querying the remote's HEAD reference.
This command is useful when the current branch has diverged from the mainline
and a fresh starting point is needed.

By default, uncommitted changes from the current worktree are carried to the
new worktree; use --no-carry to disable this. The worktree is placed at the
project root level as a sibling to other worktrees.
"#)]
pub struct Args {
    #[arg(help = "Name for the new branch (also used as the worktree directory name)")]
    new_branch_name: String,

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
}

pub fn run() -> Result<()> {
    let args = Args::parse();

    // Initialize logging based on verbosity flag
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    // Load settings from git config
    let settings = DaftSettings::load()?;

    let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_checkout_branch_from_default(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_checkout_branch_from_default(
    args: &Args,
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<()> {
    validate_branch_name(&args.new_branch_name)?;

    output.step(&format!(
        "Determining default branch for remote '{}'...",
        settings.remote
    ));

    let git_common_dir = get_git_common_dir()?;
    let default_branch = get_default_branch_local(&git_common_dir, &settings.remote)
        .context("Failed to determine default branch")?;

    output.step(&format!(
        "Detected default origin branch: '{default_branch}'"
    ));

    output.step(&format!(
        "Calling git-worktree-checkout-branch '{}' '{}'...",
        args.new_branch_name, default_branch
    ));

    let current_exe = std::env::current_exe()?;
    let exe_dir = current_exe
        .parent()
        .context("Failed to get executable directory")?;

    let checkout_branch_exe = exe_dir.join("git-worktree-checkout-branch");

    let mut cmd = Command::new(&checkout_branch_exe);
    cmd.arg(&args.new_branch_name).arg(&default_branch);

    if args.verbose {
        cmd.arg("--verbose");
    }
    if args.carry {
        cmd.arg("--carry");
    }
    if args.no_carry {
        cmd.arg("--no-carry");
    }

    let status = cmd
        .status()
        .context("Failed to execute git-worktree-checkout-branch")?;

    if !status.success() {
        anyhow::bail!(
            "git-worktree-checkout-branch failed with exit code: {}",
            status.code().unwrap_or(DEFAULT_EXIT_CODE)
        );
    }

    Ok(())
}
