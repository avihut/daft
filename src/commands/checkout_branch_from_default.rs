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
#[command(about = "Creates a git worktree and branch based on the remote's default branch")]
#[command(long_about = r#"
Creates a git worktree and branch based on the REMOTE'S DEFAULT branch
(e.g., main, master). It determines the default branch and then calls 'git-worktree-checkout-branch'.
"#)]
pub struct Args {
    #[arg(help = "The name for the new branch and the worktree directory")]
    new_branch_name: String,

    #[arg(short, long, help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Carry uncommitted changes to the new worktree (default)"
    )]
    carry: bool,

    #[arg(long, help = "Don't carry uncommitted changes to the new worktree")]
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
