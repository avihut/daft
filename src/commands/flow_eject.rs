use crate::{
    core::{worktree::flow_eject, CommandBridge},
    git::should_show_gitoxide_notice,
    hooks::{HookExecutor, HooksConfig},
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
};
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-flow-eject")]
#[command(version = crate::VERSION)]
#[command(about = "Convert a worktree-based repository back to traditional layout")]
#[command(long_about = r#"
WHAT THIS COMMAND DOES

Converts your worktree-based repository back to a traditional Git layout.
This removes all worktrees except one, and moves that worktree's files
back to the repository root.

  Before:                    After:
  my-project/                my-project/
  ├── .git/                  ├── .git/
  ├── main/                  ├── src/
  │   ├── src/               └── README.md
  │   └── README.md
  └── feature/auth/
      └── ...

By default, the remote's default branch (main, master, etc.) is kept.
Use --branch to specify a different branch.

HANDLING UNCOMMITTED CHANGES

- Changes in the target branch's worktree are preserved
- Other worktrees with uncommitted changes cause the command to fail
- Use --force to delete dirty worktrees (changes will be lost!)

EXAMPLES

  git worktree-flow-eject
      Eject to the default branch

  git worktree-flow-eject -b feature/auth
      Eject, keeping the feature/auth branch

  git worktree-flow-eject --force
      Eject even if other worktrees have uncommitted changes
"#)]
pub struct Args {
    #[arg(help = "Path to the repository to convert (defaults to current directory)")]
    repository_path: Option<PathBuf>,

    #[arg(
        short = 'b',
        long = "branch",
        help = "Branch to keep (defaults to remote's default branch)"
    )]
    branch: Option<String>,

    #[arg(
        short = 'f',
        long = "force",
        help = "Delete worktrees with uncommitted changes (changes will be lost!)"
    )]
    force: bool,

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
        long = "dry-run",
        help = "Show what would be done without making any changes"
    )]
    dry_run: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-flow-eject"));

    init_logging(args.verbose);

    let settings = DaftSettings::load_global()?;

    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_eject(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn run_eject(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let params = flow_eject::EjectParams {
        repository_path: args.repository_path.clone(),
        branch: args.branch.clone(),
        force: args.force,
        dry_run: args.dry_run,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: args.quiet,
        remote_name: settings.remote.clone(),
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    if !params.dry_run {
        output.start_spinner("Converting to traditional layout...");
    }
    let result = {
        let mut bridge = CommandBridge::new(output, executor);
        flow_eject::execute(&params, &mut bridge)?
    };
    output.finish_spinner();

    if result.dry_run {
        output.result(&format!(
            "Would convert to traditional layout with branch '{}'",
            result.target_branch
        ));
        return Ok(());
    }

    output.result(&format!(
        "Converted to traditional layout on branch '{}'",
        result.target_branch
    ));

    output.cd_path(&result.project_root);

    Ok(())
}
