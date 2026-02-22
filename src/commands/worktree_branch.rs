use crate::{
    core::{worktree::branch_delete, CommandBridge},
    hooks::{HookExecutor, HooksConfig},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    CD_FILE_ENV,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-branch")]
#[command(version = crate::VERSION)]
#[command(about = "Delete branches and their worktrees")]
#[command(long_about = r#"
Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
git-worktree-checkout(1) -b.

Use -d for a safe delete that checks whether each branch has been merged.
Use -D to force-delete branches regardless of merge status. One of -d or -D
is required.

Arguments can be branch names or worktree paths. When a path is given
(absolute, relative, or "."), the branch checked out in that worktree is
resolved automatically. This is convenient when you are inside a worktree
and want to delete it without remembering the branch name.

Safety checks (with -d) prevent accidental data loss. The command refuses to
delete a branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch

Use -D to override these safety checks. The command always refuses to delete
the repository's default branch (e.g. main), even with -D.

All targeted branches are validated before any deletions begin. If any branch
fails validation without -D, the entire command aborts and no branches are
deleted.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(help = "Branches to delete (names or worktree paths)")]
    branches: Vec<String>,

    #[arg(short = 'd', long = "delete", help = "Delete branches (safe mode)")]
    delete: bool,

    #[arg(
        short = 'D',
        long = "force",
        help = "Force deletion even if not fully merged"
    )]
    force_delete: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Entry point for `git-worktree-branch`.
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-branch"));
    run_with_args(args)
}

/// Entry point for `daft remove` — injects `-d` before clap parsing.
pub fn run_remove() -> Result<()> {
    let mut raw = crate::get_clap_args("git-worktree-branch");
    // Insert `-d` right after the command name so clap sees it
    raw.insert(1, "-d".to_string());
    let args = Args::parse_from(raw);
    run_with_args(args)
}

fn run_with_args(args: Args) -> Result<()> {
    init_logging(args.verbose);

    if !args.delete && !args.force_delete {
        anyhow::bail!(
            "either -d (--delete) or -D (--force) is required.\n\n\
             Usage: git worktree-branch -d <branches...>\n\
             Usage: git worktree-branch -D <branches...>"
        );
    }

    if args.branches.is_empty() {
        anyhow::bail!(
            "at least one branch name is required.\n\n\
             Usage: git worktree-branch -d <branches...>\n\
             Usage: git worktree-branch -D <branches...>"
        );
    }

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_branch_delete(&args, &mut output, &settings)?;
    Ok(())
}

fn run_branch_delete(args: &Args, output: &mut dyn Output, settings: &DaftSettings) -> Result<()> {
    let params = branch_delete::BranchDeleteParams {
        branches: args.branches.clone(),
        force: args.force_delete,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: args.quiet,
        remote_name: settings.remote.clone(),
        prune_cd_target: settings.prune_cd_target,
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let result = {
        let mut bridge = CommandBridge::new(output, executor);
        branch_delete::execute(&params, &mut bridge)?
    };

    // Handle validation errors
    if !result.validation_errors.is_empty() {
        for err in &result.validation_errors {
            output.error(&format!("cannot delete '{}': {}", err.branch, err.message));
        }
        let total = result.requested_count;
        let failed = result.validation_errors.len();
        anyhow::bail!(
            "Aborting: {} of {} branch{} failed validation. No branches were deleted.",
            failed,
            total,
            if total == 1 { "" } else { "es" }
        );
    }

    if result.nothing_to_delete {
        output.info("No branches to delete");
        return Ok(());
    }

    // Render deletion results
    let mut had_errors = false;
    for deletion in &result.deletions {
        if deletion.has_errors() {
            had_errors = true;
            for err in &deletion.errors {
                output.error(err);
            }
        }
        let parts = deletion.deleted_parts();
        if !parts.is_empty() {
            output.result(&format!("Deleted {} ({})", deletion.branch, parts));
        }
    }

    // Write the cd target for the shell wrapper
    if let Some(ref cd_target) = result.cd_target {
        if std::env::var(CD_FILE_ENV).is_ok() {
            output.cd_path(cd_target);
        } else {
            output.result(&format!(
                "Run `cd {}` (your previous working directory was removed)",
                cd_target.display()
            ));
        }
    }

    if had_errors {
        anyhow::bail!("Some branches could not be fully deleted; see errors above");
    }

    Ok(())
}
