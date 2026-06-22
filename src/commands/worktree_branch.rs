use crate::{
    CD_FILE_ENV,
    core::{
        CommandBridge,
        worktree::{branch_delete, rename},
    },
    git::GitCommand,
    git::should_show_gitoxide_notice,
    hooks::HookExecutor,
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
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
  - has refined untracked daft files (daft.yml / daft.local.yml edited since
    daft seeded them) that the default branch's worktree does not cover —
    consolidate with daft-file(1) merge, or answer the interactive prompt

Use -D to override these safety checks. Forcing DISCARDS refined untracked
daft files — they are stashed under `<git-common-dir>/.daft/discarded/<branch>/`
and never merged into another worktree. For the default branch (e.g. main),
-D removes its worktree only — the local branch ref and remote branch are
always preserved.

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

    #[arg(long, help = "Only delete locally, keep remote branch")]
    local: bool,

    #[arg(
        long,
        conflicts_with = "local",
        help = "Only delete the remote branch, keep local worktree and branch"
    )]
    remote: bool,

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

    #[arg(long, help = "Skip the repo's pre-push hook on remote operations")]
    no_verify: bool,

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
#[command(name = "daft remove")]
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
  - has refined untracked daft files (daft.yml / daft.local.yml edited since
    daft seeded them) that the default branch's worktree does not cover —
    consolidate with daft-file(1) merge, or answer the interactive prompt

Use -f to override these safety checks. Forcing DISCARDS refined untracked
daft files — they are stashed under `<git-common-dir>/.daft/discarded/<branch>/`
and never merged into another worktree. For the default branch (e.g. main),
-f removes its worktree only — the local branch ref and remote branch are
always preserved.

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

    #[arg(long, help = "Only delete locally, keep remote branch")]
    local: bool,

    #[arg(
        long,
        conflicts_with = "local",
        help = "Only delete the remote branch, keep local worktree and branch"
    )]
    remote: bool,

    #[arg(
        long,
        help = "Skip the repo's pre-push hook when deleting the remote branch"
    )]
    no_verify: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Entry point for `daft remove`.
pub fn run_remove() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-remove");
    raw[0] = "daft remove".to_string();
    let mut remove_args = RemoveArgs::parse_from(raw);

    init_logging(remove_args.verbose);

    // When invoked outside a git repository, allow operating on worktree paths
    // by discovering the owning repo from the first existing path argument and
    // chdir-ing to its project root. All path arguments are canonicalized first
    // so the subsequent chdir doesn't break their resolution.
    if !is_git_repository()? {
        prepare_out_of_repo_paths(&mut remove_args.branches, "daft remove")?;
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(remove_args.quiet, remove_args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_branch_delete(
        &remove_args.branches,
        remove_args.force,
        remove_args.quiet,
        remove_args.local,
        remove_args.remote,
        "-f/--force",
        remove_args.no_verify,
        &mut output,
        &settings,
    )
}

/// Prepare a path-accepting daft command (currently `daft remove` and `daft
/// rename`) to run when the user's current directory is not inside any git
/// repository. Resolves path arguments to absolute canonical paths, discovers
/// the owning repository from the first one, ensures all paths share the same
/// repository, and `chdir`s into the project root so the cwd-based pipeline
/// can take over unchanged.
///
/// `command_name` is used only in user-facing error messages (e.g. "Run
/// `daft remove` inside a repository"); the resolution logic is identical
/// across commands.
fn prepare_out_of_repo_paths(args: &mut Vec<String>, command_name: &str) -> Result<()> {
    // clap enforces `required = true` on the positional args of every caller,
    // so an empty `args` here would have already been rejected.
    let cwd = std::env::current_dir()
        .map_err(|e| anyhow::anyhow!("failed to read current working directory: {e}"))?;
    let mut absolute_paths: Vec<std::path::PathBuf> = Vec::with_capacity(args.len());
    for arg in args.iter() {
        let raw = std::path::PathBuf::from(arg);
        let candidate = if raw.is_absolute() {
            raw
        } else {
            cwd.join(&raw)
        };
        let canonical = std::fs::canonicalize(&candidate).map_err(|_| {
            anyhow::anyhow!(
                "Not inside a Git repository, and '{}' is not an existing path. \
                 Run `{}` inside a repository, or pass an existing worktree path.",
                arg,
                command_name
            )
        })?;
        absolute_paths.push(canonical);
    }

    let mut common_dir: Option<std::path::PathBuf> = None;
    for (path, original) in absolute_paths.iter().zip(args.iter()) {
        let repo = gix::discover(path).map_err(|_| {
            anyhow::anyhow!(
                "'{}' is not inside a Git repository. \
                 Run `{}` inside a repository, or pass a worktree path.",
                original,
                command_name
            )
        })?;
        let canonical_common = std::fs::canonicalize(repo.common_dir())
            .unwrap_or_else(|_| repo.common_dir().to_path_buf());
        match common_dir {
            None => common_dir = Some(canonical_common),
            Some(ref existing) if existing == &canonical_common => {}
            Some(_) => anyhow::bail!(
                "all paths must belong to the same repository when running outside a worktree"
            ),
        }
    }

    let common_dir = common_dir.expect("at least one path was processed");
    let project_root = common_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("could not determine project root for repository"))?;

    std::env::set_current_dir(project_root).map_err(|e| {
        anyhow::anyhow!(
            "failed to enter project root '{}': {e}",
            project_root.display()
        )
    })?;

    *args = absolute_paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    Ok(())
}

/// Daft-style args for `daft rename`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to renaming, without `-d`/`-D`.
#[derive(Parser)]
#[command(name = "daft rename")]
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

    #[arg(long, help = "Skip the repo's pre-push hook on remote operations")]
    no_verify: bool,

    #[arg(long, help = "Preview changes without executing")]
    dry_run: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Entry point for `daft rename`.
pub fn run_rename() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-rename");
    raw[0] = "daft rename".to_string();
    let mut rename_args = RenameArgs::parse_from(raw);

    init_logging(rename_args.verbose);

    // When invoked outside a git repository, allow renaming a worktree by
    // path: discover the owning repo from the source argument and chdir into
    // its project root so the cwd-based rename pipeline runs unchanged. Only
    // the `source` arg is treated as a path; `new_branch` is always a branch
    // name, never a path.
    if !is_git_repository()? {
        let mut source_arg = vec![rename_args.source.clone()];
        prepare_out_of_repo_paths(&mut source_arg, "daft rename")?;
        rename_args.source = source_arg
            .into_iter()
            .next()
            .expect("one arg in, one arg out");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(rename_args.quiet, rename_args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_rename_inner(
        &rename_args.source,
        &rename_args.new_branch,
        rename_args.no_remote,
        rename_args.no_verify,
        rename_args.dry_run,
        rename_args.verbose,
        &mut output,
        &settings,
    )
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
        if args.branches.len() != 2 {
            anyhow::bail!(
                "rename mode requires exactly 2 arguments: <source> <new-branch>.\n\n\
                 Usage: git worktree-branch -m <source> <new-branch>"
            );
        }
        run_rename_inner(
            &args.branches[0],
            &args.branches[1],
            args.no_remote,
            args.no_verify,
            args.dry_run,
            args.verbose,
            &mut output,
            &settings,
        )?;
    } else {
        if args.branches.is_empty() {
            anyhow::bail!(
                "at least one branch name is required.\n\n\
                 Usage: git worktree-branch -d <branches...>\n\
                 Usage: git worktree-branch -D <branches...>"
            );
        }
        run_branch_delete(
            &args.branches,
            args.force_delete,
            args.quiet,
            args.local,
            args.remote,
            "-D/--force",
            args.no_verify,
            &mut output,
            &settings,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_branch_delete(
    branches: &[String],
    force: bool,
    quiet: bool,
    local_only: bool,
    remote_only: bool,
    force_flag_label: &str,
    no_verify: bool,
    output: &mut dyn Output,
    settings: &DaftSettings,
) -> Result<()> {
    let params = branch_delete::BranchDeleteParams {
        branches: branches.to_vec(),
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: quiet,
        remote_name: settings.remote.clone(),
        delete_remote: if local_only {
            false
        } else if remote_only {
            true
        } else {
            settings.branch_delete_remote
        },
        remote_only,
        keep_local_branch: false,
        no_verify,
        prune_cd_target: settings.prune_cd_target,
        command_label: "branch-delete".to_string(),
        skip_merge_validation: false,
        force_flag_label: force_flag_label.to_string(),
    };

    let hooks_config = crate::core::settings::load_hooks_config()?;
    let hook_output_config = hooks_config.output.clone();
    let executor = HookExecutor::new(hooks_config)?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    // The pre-push hook run on the remote-branch delete renders through
    // this presenter — keep the spinner off when it will fire (#599).
    let probe_git = GitCommand::new(quiet).with_gitoxide(settings.use_gitoxide);
    let push_hook_will_render = params.delete_remote
        && !params.no_verify
        && std::env::current_dir()
            .map(|cwd| probe_git.pre_push_hook_exists(&cwd))
            .unwrap_or(false);
    let push_presenter: Option<std::sync::Arc<dyn crate::executor::presenter::JobPresenter>> =
        if push_hook_will_render {
            let p: std::sync::Arc<dyn crate::executor::presenter::JobPresenter> =
                crate::executor::cli_presenter::CliPresenter::auto(&hook_output_config);
            Some(p)
        } else {
            None
        };

    if !push_hook_will_render {
        output.start_spinner("Deleting branches...");
    }
    let exec_result = {
        let mut bridge = CommandBridge::new(output, executor);
        branch_delete::execute(&params, push_presenter.as_ref(), &mut bridge)
    };
    if !push_hook_will_render {
        output.finish_spinner();
    }
    let result = exec_result?;

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

#[allow(clippy::too_many_arguments)]
fn run_rename_inner(
    source: &str,
    new_branch: &str,
    no_remote: bool,
    no_verify: bool,
    dry_run: bool,
    verbose: bool,
    output: &mut dyn Output,
    settings: &DaftSettings,
) -> Result<()> {
    let params = rename::RenameParams {
        source: source.to_string(),
        new_branch: new_branch.to_string(),
        no_remote,
        dry_run,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: output.is_quiet(),
        remote_name: settings.remote.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        no_verify,
    };

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    let mut hooks_config = crate::core::settings::load_hooks_config()?;
    if verbose {
        hooks_config.output.verbose = true;
    }
    let executor = HookExecutor::new(hooks_config.clone())?;

    // The pre-push hook run on the remote rename renders through this
    // presenter — keep the spinner off when it will fire (#599).
    let probe_git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
    let push_hook_will_render = !params.dry_run
        && !params.no_remote
        && !params.no_verify
        && std::env::current_dir()
            .map(|cwd| probe_git.pre_push_hook_exists(&cwd))
            .unwrap_or(false);
    let push_presenter: Option<std::sync::Arc<dyn crate::executor::presenter::JobPresenter>> =
        if push_hook_will_render {
            let p: std::sync::Arc<dyn crate::executor::presenter::JobPresenter> =
                crate::executor::cli_presenter::CliPresenter::auto(&hooks_config.output);
            Some(p)
        } else {
            None
        };

    if !params.dry_run && !push_hook_will_render {
        output.start_spinner("Renaming branch...");
    }
    let exec_result = {
        let mut bridge =
            CommandBridge::with_output_config(output, executor, hooks_config.output.clone());
        rename::execute(&params, push_presenter.as_ref(), &mut bridge)
    };
    output.finish_spinner();
    let result = exec_result?;

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
    if let Some(ref cd_target) = result.cd_target
        && let Ok(cd_file) = std::env::var(CD_FILE_ENV)
    {
        std::fs::write(&cd_file, cd_target.to_string_lossy().as_bytes()).ok();
    }

    // The worktree moved and the shell has been re-pointed — now surface a
    // deferred pre-push gate refusal as the command's failure (#599).
    if let Some(message) = result.push_gate_error {
        anyhow::bail!(message);
    }

    Ok(())
}
