use crate::{
    core::{
        worktree::{branch_delete, rename},
        CommandBridge, OutputSink,
    },
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
#[command(about = "Delete or rename branches and their worktrees")]
#[command(long_about = r#"
Manage branches and their associated worktrees. Supports deletion (-d/-D)
and renaming (-m).

DELETE MODE (-d / -D)

Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
git-worktree-checkout(1) -b.

Use -d for a safe delete that checks whether each branch has been merged.
Use -D to force-delete branches regardless of merge status.

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

RENAME MODE (-m)

Renames a local branch and moves its associated worktree directory to match
the new branch name. If the branch has a remote tracking branch, the remote
branch is also renamed (push new name, delete old name) unless --no-remote
is specified.

The source can be specified as a branch name or a path to an existing
worktree (absolute or relative).

If you are currently inside the worktree being renamed, the shell is
redirected to the new worktree location after the rename completes.

Empty parent directories left behind by the move are automatically cleaned up.
"#)]
pub struct Args {
    #[arg(help = "Branches (delete mode) or source + new-name (rename mode)")]
    branches: Vec<String>,

    #[arg(short = 'd', long = "delete", help = "Delete branches (safe mode)")]
    delete: bool,

    #[arg(
        short = 'D',
        long = "force",
        help = "Force deletion even if not fully merged"
    )]
    force_delete: bool,

    #[arg(
        short = 'm',
        long = "move",
        help = "Rename a branch and move its worktree"
    )]
    rename: bool,

    #[arg(
        long,
        help = "Skip remote branch rename (only with -m)",
        requires = "rename"
    )]
    no_remote: bool,

    #[arg(
        long,
        help = "Preview changes without executing (only with -m)",
        requires = "rename"
    )]
    dry_run: bool,

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

/// Daft-style args for `daft remove`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to removal, with `-f` instead of git-style `-D`.
#[derive(Parser)]
#[command(name = "daft-remove")]
#[command(version = crate::VERSION)]
#[command(about = "Delete branches and their worktrees")]
#[command(long_about = r#"
Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
`daft start` / `daft go`.

By default, performs a safe delete that checks whether each branch has been
merged. Use -f (--force) to force-delete branches regardless of merge status.

Arguments can be branch names or worktree paths. When a path is given
(absolute, relative, or "."), the branch checked out in that worktree is
resolved automatically. This is convenient when you are inside a worktree
and want to delete it without remembering the branch name.

Safety checks prevent accidental data loss. The command refuses to delete a
branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch

Use -f to override these safety checks. The command always refuses to delete
the repository's default branch (e.g. main), even with -f.

All targeted branches are validated before any deletions begin. If any branch
fails validation without -f, the entire command aborts and no branches are
deleted.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See daft-hooks(1) for hook management.
"#)]
pub struct RemoveArgs {
    #[arg(required = true, help = "Branches or worktree paths to delete")]
    branches: Vec<String>,

    #[arg(
        short = 'f',
        long = "force",
        help = "Force deletion even if not fully merged"
    )]
    force: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Entry point for `daft remove`.
pub fn run_remove() -> Result<()> {
    let raw = crate::get_clap_args("daft-remove");
    let remove_args = RemoveArgs::parse_from(raw);
    let args = Args {
        branches: remove_args.branches,
        delete: !remove_args.force,
        force_delete: remove_args.force,
        rename: false,
        no_remote: false,
        dry_run: false,
        quiet: remove_args.quiet,
        verbose: remove_args.verbose,
    };
    run_with_args(args)
}

/// Daft-style args for `daft rename`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to renaming, without `-d`/`-D`.
#[derive(Parser)]
#[command(name = "daft-rename")]
#[command(version = crate::VERSION)]
#[command(about = "Rename a branch and move its worktree")]
#[command(long_about = r#"
Renames a local branch and moves its associated worktree directory to match
the new branch name. If the branch has a remote tracking branch, the remote
branch is also renamed (push new name, delete old name) unless --no-remote
is specified.

The source can be specified as a branch name or a path to an existing
worktree (absolute or relative).

If you are currently inside the worktree being renamed, the shell is
redirected to the new worktree location after the rename completes.

Empty parent directories left behind by the move are automatically cleaned up.
"#)]
pub struct RenameArgs {
    #[arg(required = true, help = "Source branch or worktree path")]
    source: String,

    #[arg(required = true, help = "New branch name")]
    new_branch: String,

    #[arg(long, help = "Skip remote branch rename")]
    no_remote: bool,

    #[arg(long, help = "Preview changes without executing")]
    dry_run: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Entry point for `daft rename`.
pub fn run_rename() -> Result<()> {
    let raw = crate::get_clap_args("daft-rename");
    let rename_args = RenameArgs::parse_from(raw);
    let args = Args {
        branches: vec![rename_args.source, rename_args.new_branch],
        delete: false,
        force_delete: false,
        rename: true,
        no_remote: rename_args.no_remote,
        dry_run: rename_args.dry_run,
        quiet: rename_args.quiet,
        verbose: rename_args.verbose,
    };
    run_with_args(args)
}

fn run_with_args(args: Args) -> Result<()> {
    init_logging(args.verbose);

    let mode_count = args.delete as u8 + args.force_delete as u8 + args.rename as u8;
    if mode_count == 0 {
        anyhow::bail!(
            "one of -d (--delete), -D (--force), or -m (--move) is required.\n\n\
             Usage: git worktree-branch -d <branches...>\n\
             Usage: git worktree-branch -D <branches...>\n\
             Usage: git worktree-branch -m <source> <new-branch>"
        );
    }
    if mode_count > 1 {
        anyhow::bail!(
            "only one of -d (--delete), -D (--force), or -m (--move) can be used at a time."
        );
    }

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    if args.rename {
        run_rename_inner(&args, &mut output, &settings)?;
    } else {
        if args.branches.is_empty() {
            anyhow::bail!(
                "at least one branch name is required.\n\n\
                 Usage: git worktree-branch -d <branches...>\n\
                 Usage: git worktree-branch -D <branches...>"
            );
        }
        run_branch_delete(&args, &mut output, &settings)?;
    }
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

fn run_rename_inner(args: &Args, output: &mut dyn Output, settings: &DaftSettings) -> Result<()> {
    if args.branches.len() != 2 {
        anyhow::bail!(
            "rename mode requires exactly 2 arguments: <source> <new-branch>.\n\n\
             Usage: git worktree-branch -m <source> <new-branch>"
        );
    }

    let params = rename::RenameParams {
        source: args.branches[0].clone(),
        new_branch: args.branches[1].clone(),
        no_remote: args.no_remote,
        dry_run: args.dry_run,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: output.is_quiet(),
        remote_name: settings.remote.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
    };

    let result = {
        let mut sink = OutputSink(output);
        rename::execute(&params, &mut sink)?
    };

    // Render result
    if result.dry_run {
        output.info("Dry run complete. No changes were made.");
    } else {
        let mut summary_parts = Vec::new();
        if result.branch_renamed {
            summary_parts.push(format!(
                "branch '{}' -> '{}'",
                result.old_branch, result.new_branch
            ));
        }
        if result.worktree_moved {
            summary_parts.push(format!(
                "worktree '{}' -> '{}'",
                result.old_path.display(),
                result.new_path.display()
            ));
        }
        if result.remote_renamed {
            summary_parts.push("remote branch updated".to_string());
        }

        output.success(&format!("Renamed: {}", summary_parts.join(", ")));

        for warning in &result.warnings {
            output.warning(warning);
        }
    }

    // Handle cd_target via DAFT_CD_FILE
    if let Some(ref cd_target) = result.cd_target {
        if let Ok(cd_file) = std::env::var(CD_FILE_ENV) {
            std::fs::write(&cd_file, cd_target.to_string_lossy().as_bytes()).ok();
        }
    }

    Ok(())
}
