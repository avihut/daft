use crate::{
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    DaftSettings,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-branch-delete")]
#[command(version = crate::VERSION)]
#[command(about = "Delete branches and their worktrees")]
#[command(long_about = r#"
Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
git-worktree-checkout-branch(1).

Safety checks prevent accidental data loss. The command refuses to delete a
branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch

Use -D (--force) to override these safety checks. The command always refuses
to delete the repository's default branch (e.g. main), even with --force.

All targeted branches are validated before any deletions begin. If any branch
fails validation without --force, the entire command aborts and no branches
are deleted.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(required = true, help = "Branch names to delete")]
    branches: Vec<String>,

    #[arg(short = 'D', long, help = "Force deletion even if not fully merged")]
    force: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(long, help = "Do not change directory after deletion")]
    no_cd: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-branch-delete"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(!args.no_cd, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_branch_delete(&args, &mut output, &settings)?;
    Ok(())
}

fn run_branch_delete(
    _args: &Args,
    output: &mut dyn Output,
    _settings: &DaftSettings,
) -> Result<()> {
    output.info("git-worktree-branch-delete: not yet implemented");
    Ok(())
}
