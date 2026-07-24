use crate::{
    CD_FILE_ENV,
    core::{
        CommandBridge, TimelineBridge,
        worktree::{branch_delete, rename},
    },
    git::GitCommand,
    hooks::HookExecutor,
    is_git_repository,
    logging::init_logging,
    output::{
        CliOutput, Output, OutputConfig,
        timeline::{Timeline, TimelineMode},
    },
    settings::{DaftSettings, PushVerify},
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

When remote deletion is enabled, the remote-branch delete pushes no content,
so the repo's pre-push hook is skipped by default (configurable via
daft.pushVerify: auto, always, or never; use always for hooks that gate
deletes by ref name). daft.pushVerify is the base setting every daft push
reads, so setting it also affects the branch-creation upstream push;
daft.checkout.pushVerify overrides it for that push alone. Pass --no-verify
to skip it unconditionally.

RENAME MODE (-m)

Renames a local branch and moves its associated worktree directory to match
the new branch name. If the branch has a remote tracking branch, the remote
branch is also renamed (push new name, delete old name) unless --no-remote
is specified. The new-name push honors the repo's pre-push hook; the old-name
delete pushes no content and skips it by default (configurable via
daft.pushVerify).

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

Use --repo <name> to remove branches in another cataloged repository without
leaving your current directory: `daft remove --repo api feature-x` deletes
`feature-x` in the `api` repo. --repo is a flag rather than a positional
because the positional slot is a required list of branches or paths; a repo
guessed there could delete a local branch and its remote by mistake. A --repo
removal never relocates your shell (your cwd stays valid) and runs
non-interactively (see the refined-daft-files note below). Combining --repo
with a worktree path is rejected — the path already identifies its own
repository.

Safety checks prevent accidental data loss. The command refuses to delete a
branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch
  - has refined untracked daft files (daft.yml / daft.local.yml edited since
    daft seeded them) that the default branch's worktree does not cover —
    consolidate with daft-file(1) merge, or (for a local removal) answer the
    interactive prompt; a --repo removal has no prompt and aborts instead,
    so consolidate or pass -f up front

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

When remote deletion is enabled, the remote-branch delete pushes no content,
so the repo's pre-push hook is skipped by default (configurable via
daft.pushVerify: auto, always, or never; use always for hooks that gate
deletes by ref name). daft.pushVerify is the base setting every daft push
reads, so setting it also affects the branch-creation upstream push;
daft.checkout.pushVerify overrides it for that push alone. Pass --no-verify
to skip it unconditionally.
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

    #[arg(
        long = "repo",
        value_name = "REPO",
        help = "Remove branches in another cataloged repository"
    )]
    repo: Option<String>,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// How a `daft remove` invocation reached its target repository.
///
/// Two behaviors key off this rather than off `--repo` directly, because
/// `--repo <the-repo-you-are-standing-in>` resolves back to [`Self::Local`]:
/// the flag names a repository, it does not by itself mean "elsewhere".
enum RemoveScope {
    /// The repository the cwd is in (reached directly, by worktree path, or
    /// by a `--repo` that names that same repository).
    Local,
    /// Another cataloged repository, entered via `--repo` (#749). Carries the
    /// directory the shell was in *before* we chdir'd into the target, so the
    /// cd-rescue can tell whether this removal deleted the ground the user is
    /// standing on (see the emission gate in `run_branch_delete`).
    CrossRepo {
        shell_cwd: Option<std::path::PathBuf>,
    },
}

/// Entry point for `daft remove`.
pub fn run_remove() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-remove");
    raw[0] = "daft remove".to_string();
    let mut remove_args = RemoveArgs::parse_from(raw);

    init_logging(remove_args.verbose);

    // Everything below is cwd-driven, so reaching another repository is done
    // by entering it first — the same trick `prepare_out_of_repo_paths` plays
    // for path arguments. Both must land before `DaftSettings::load()`, which
    // reads the repo-local config of whatever directory we end up in.
    let scope = match remove_args.repo.clone() {
        // `--repo <name>`: exec-shape targeting by catalog name (#749).
        Some(needle) => enter_repo_by_name(&needle, &remove_args)?,
        // No `--repo`. Inside a repo this is an ordinary local removal; from
        // outside one, the owning repo is discovered from a path argument.
        None => {
            if !is_git_repository()? {
                prepare_out_of_repo_paths(&mut remove_args.branches, "daft remove")?;
            }
            RemoveScope::Local
        }
    };

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
        scope,
        &mut output,
        &settings,
    )
}

/// Resolve `--repo <needle>` through the catalog, announce the destination,
/// and enter it unless it is the repository the user is already in.
///
/// Fails closed by construction: [`crate::catalog::resolve_repo_arg`] errors on
/// a store failure, an unknown name, a tombstoned entry, or a recorded path
/// that no longer exists. A cross-repo removal must never be reinterpreted as
/// a local one — that would turn a catalog outage into a deletion in the wrong
/// repository.
fn enter_repo_by_name(needle: &str, args: &RemoveArgs) -> Result<RemoveScope> {
    reject_path_args_with_repo(&args.branches, needle)?;

    let row = crate::catalog::resolve_repo_arg(needle)?;

    // Announce the resolved destination before any work happens — a removal
    // aimed somewhere other than "here" must be impossible to miss (`-q` opts
    // out). Its own CliOutput, with autocd off: this line is never a cd hint.
    let subject = if args.branches.len() == 1 {
        format!("branch '{}'", args.branches[0])
    } else {
        format!("{} branches", args.branches.len())
    };
    let mut announce = CliOutput::new(OutputConfig::with_autocd(args.quiet, args.verbose, false));
    announce.result(&format!(
        "Removing {} in '{}' ({})",
        subject, row.name, row.path
    ));

    if resolves_to_current_repo(&row) {
        return Ok(RemoveScope::Local);
    }

    // Capture where the shell is standing before we leave it: the cd-rescue in
    // `run_branch_delete` uses this to distinguish a genuine cross-repo removal
    // (cwd stays valid, never relocate) from `--repo` naming the repo the user
    // is actually in but misread as elsewhere (cwd removed, must rescue).
    let shell_cwd = std::env::current_dir().ok();
    crate::utils::change_directory(std::path::Path::new(&row.path))?;
    Ok(RemoveScope::CrossRepo { shell_cwd })
}

/// Whether `row` names the repository the cwd is already inside.
///
/// `--repo <current-repo>` must behave exactly like a plain local removal.
/// Entering the project root would move the cwd out of the worktree being
/// removed, and the cd-redirect that rescues the user's shell is derived from
/// the cwd — so treating "here" as cross-repo would strand them in a deleted
/// directory. Identity is keyed off the canonical git-common-dir via
/// [`crate::catalog::fleet::current_repo_git_common_dir`] — the same helper the
/// current-repo-last fleet ordering uses, so the two never disagree about which
/// catalog row is "here"; being outside any repository reads as "not current".
fn resolves_to_current_repo(row: &crate::store::models::CatalogRepoRow) -> bool {
    crate::catalog::fleet::current_repo_git_common_dir()
        .is_some_and(|canonical| canonical == row.git_common_dir)
}

/// Reject path-spelled positionals when `--repo` already names the target.
///
/// A worktree path identifies its own repository, so pairing the two is
/// contradictory — reject it rather than picking a winner. The test is
/// deliberately *spelling*-based rather than an on-disk probe: worktree
/// directories are named after their branches, so `daft remove --repo api
/// feat/foo` run from a repo that happens to have a `feat/foo` worktree would
/// otherwise be rejected for naming a perfectly ordinary branch. Bare names
/// stay branch names and are matched against the target repo's worktrees after
/// the chdir, where that resolution is meaningful.
fn reject_path_args_with_repo(branches: &[String], needle: &str) -> Result<()> {
    for arg in branches {
        let path_spelled = arg.starts_with('/')
            || arg.starts_with('~')
            || arg == "."
            || arg == ".."
            || arg.starts_with("./")
            || arg.starts_with("../");
        if path_spelled {
            anyhow::bail!(
                "'{arg}' is a worktree path, which already identifies its repository\n  \
                 tip: drop --repo to remove by path, or pass branch names to remove in '{needle}'"
            );
        }
    }
    Ok(())
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
        let canonical_common = crate::core::repo::git_common_dir_at(path).ok_or_else(|| {
            anyhow::anyhow!(
                "'{}' is not inside a Git repository. \
                 Run `{}` inside a repository, or pass a worktree path.",
                original,
                command_name
            )
        })?;
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
is specified. The new-name push honors the repo's pre-push hook; the old-name
delete pushes no content and skips it by default (configurable via
daft.pushVerify: auto, always, or never).

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
            // The legacy `git worktree-branch -d/-D` form has no `--repo`.
            RemoveScope::Local,
            &mut output,
            &settings,
        )?;
    }
    Ok(())
}

/// Whether the shell that launched a cross-repo removal needs relocating.
///
/// True only when we captured its pre-chdir directory *and* that directory is
/// now gone — the removal deleted the ground it was standing on. A shell we
/// couldn't locate (`None`), or one whose directory still exists, is left where
/// it is: a cross-repo removal must never relocate a still-valid cwd (that is
/// the whole reason cross-repo suppresses the redirect), and the rescue is
/// reserved for `--repo` naming the repo the user is actually in, misread as
/// elsewhere by a `GIT_*`-perturbed probe (#749). Checking the cwd directly,
/// rather than trusting the scope classification, is what makes a
/// misclassification unable to strand the user.
fn cross_repo_shell_stranded(shell_cwd: Option<&std::path::Path>) -> bool {
    shell_cwd.is_some_and(|cwd| matches!(cwd.try_exists(), Ok(false)))
}

/// Hand the shell a new working directory after its own was removed: through
/// the wrapper's `DAFT_CD_FILE` when active, otherwise a `Run \`cd …\`` hint so
/// users without the wrapper still recover.
fn emit_cd_redirect(output: &mut dyn Output, cd_target: &std::path::Path) {
    if std::env::var(CD_FILE_ENV).is_ok() {
        output.cd_path(cd_target);
    } else {
        output.result(&format!(
            "Run `cd {}` (your previous working directory was removed)",
            cd_target.display()
        ));
    }
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
    scope: RemoveScope,
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
        push_verify: settings.push_verify,
        prune_cd_target: settings.prune_cd_target,
        command_label: "branch-delete".to_string(),
        skip_merge_validation: false,
        force_flag_label: force_flag_label.to_string(),
    };

    let hooks_config = crate::core::settings::load_hooks_config()?;
    let hook_output_config = hooks_config.output.with_cli_verbose(output.is_verbose());
    let executor = HookExecutor::new(hooks_config)?;

    // Plan-execute rail timeline (#651). This header is a seed built from
    // raw args; the core replaces it at plan commit with the resolved
    // targets (`daft remove .` → `Removing <branch>`, count as validated).
    let header = if branches.len() == 1 {
        format!("Removing {}", branches[0])
    } else {
        format!("Removing {} branches", branches.len())
    };
    let mut timeline = Timeline::new(TimelineMode::auto(quiet), output.is_verbose(), header);
    timeline.set_verbose_density(hook_output_config.verbose);
    let interactive = timeline.is_interactive();

    // Presenter for the pre-push hook run on remote-branch deletes. On the
    // rail it embeds under each branch's active DeleteRemote row (re-resolved
    // per phase from the push target). Off the rail, keep the legacy #599
    // behavior: presenter only when the hook will fire, spinner otherwise.
    //
    // Unlike checkout's probe-dependent upper bound, a delete's *verify*
    // decision is static (#747) — the hook can only fire under
    // `pushVerify = always` — so that half of the gate is exact. The
    // hook-presence half is not: the delete probes each branch's own worktree
    // while this runs before the branches are resolved and can only ask about
    // the cwd, and a relative `core.hooksPath` resolves per worktree. The
    // residue is cosmetic — a miss renders the hook through the fallback
    // spinner path instead of the reported pre-push phase.
    let probe_git = GitCommand::new(quiet).with_gitoxide(settings.use_gitoxide);
    let push_hook_will_render = params.delete_remote
        && !params.no_verify
        && params.push_verify == PushVerify::Always
        && std::env::current_dir()
            .map(|cwd| probe_git.pre_push_hook_exists(&cwd))
            .unwrap_or(false);
    let push_presenter: Option<std::sync::Arc<dyn crate::executor::presenter::JobPresenter>> =
        if interactive {
            Some(
                crate::executor::cli_presenter::CliPresenter::embedded_for_stage(
                    &hook_output_config,
                    timeline.handle(),
                    crate::core::stage::StageId::DeleteRemote,
                ),
            )
        } else if push_hook_will_render {
            Some(crate::executor::cli_presenter::CliPresenter::auto(
                &hook_output_config,
            ))
        } else {
            None
        };

    if interactive {
        // The rail opens immediately; validation runs under the planning
        // face (the consolidation prompts suspend it for their duration)
        // and the plan replaces the face in place when validation commits.
        timeline.open_planning("Validating branches");
    } else if !push_hook_will_render {
        output.start_spinner("Deleting branches...");
    }
    let exec_result = {
        let mut bridge =
            TimelineBridge::new(output, &mut timeline, executor, hook_output_config.clone());
        // A cross-repo removal must not stop on daft's own consolidation
        // prompt: the Repo-Aware Command Grammar routes fleet-style forms
        // non-interactively, and a keypress here would ask about refined daft
        // files in a repo the user isn't standing in. Suppressing hands that
        // decision to the `ConsolidationPrompter` default — always Abort — so
        // the removal refuses with the usual `daft file merge` / `-f` guidance
        // instead. This governs daft's prompt only: a remote-delete pre-push
        // hook or a git credential helper can still prompt, exactly as one
        // would for a local removal.
        if matches!(scope, RemoveScope::CrossRepo { .. }) {
            bridge = bridge.without_prompts();
        }
        let witness = crate::commands::forge_cache::merged_witness(
            &GitCommand::new(true).with_gitoxide(settings.use_gitoxide),
        );
        branch_delete::execute(
            &params,
            push_presenter.as_ref(),
            witness.as_ref(),
            &mut bridge,
        )
    };
    // A run that never committed a plan (validation errors, nothing to
    // delete) collapses the face here, before its legacy error/info lines
    // print — raw writes must not tear through live bars.
    timeline.abandon_planning();
    if interactive && exec_result.is_err() {
        timeline.abort(&format!("Failed after {}", timeline.elapsed_display()));
    }
    if !interactive && !push_hook_will_render {
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

    // Render deletion results. On the rail, the footer + rows are the
    // record; the per-branch result lines stay for Plain/Hidden so the
    // non-TTY and DAFT_TESTING output is byte-identical to before.
    let had_errors = result.deletions.iter().any(|d| d.has_errors());
    if interactive {
        let footer = if had_errors {
            format!("Finished with failures in {}", timeline.elapsed_display())
        } else if result.deletions.len() == 1 {
            format!("Removed in {}", timeline.elapsed_display())
        } else {
            format!(
                "Removed {} branches in {}",
                result.deletions.len(),
                timeline.elapsed_display()
            )
        };
        if had_errors {
            timeline.abort(&footer);
        } else {
            timeline.finish(&footer);
        }
    }
    for deletion in &result.deletions {
        if deletion.has_errors() {
            for err in &deletion.errors {
                output.error(err);
            }
        }
        if !timeline.replaces_stdout_record() {
            let parts = deletion.deleted_parts();
            if !parts.is_empty() {
                output.result(&format!("Deleted {} ({})", deletion.branch, parts));
            }
        }
    }

    // Write the cd target for the shell wrapper.
    //
    // Rescue the shell only when the directory it is standing in was actually
    // removed. For a local removal that is exactly what `result.cd_target`
    // reports: the core detects its own current worktree and resolves a
    // fallback before deleting it. A cross-repo removal differs on both counts
    // — the shell is normally in another repository whose cwd stays valid (so
    // relocating it would be user-hostile), and the core ran from the *target*
    // root, so its current-worktree detection (which also matches by branch
    // name) is no proxy for the shell's fate and `cd_target` is usually `None`.
    // The one cross-repo case that still needs rescuing is `--repo` naming the
    // repo the user is actually in, misread as elsewhere by a `GIT_*`-perturbed
    // probe (#749): there the shell's own directory is gone. Detect that
    // directly — has the captured pre-chdir cwd survived? — rather than trusting
    // the scope, so a misclassification can never strand the user.
    match &scope {
        RemoveScope::Local => {
            if let Some(ref cd_target) = result.cd_target {
                emit_cd_redirect(output, cd_target);
            }
        }
        RemoveScope::CrossRepo { shell_cwd } => {
            if cross_repo_shell_stranded(shell_cwd.as_deref())
                && let Some(target) = result
                    .cd_target
                    .clone()
                    .or_else(|| std::env::current_dir().ok())
            {
                emit_cd_redirect(output, &target);
            }
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
        push_verify: settings.push_verify,
    };

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

#[cfg(test)]
mod tests {
    use super::cross_repo_shell_stranded;

    // The cross-repo cd-rescue keys off whether the shell's own directory
    // survived the removal, not off the scope classification. That is what
    // guarantees the two opposing failures never happen (#749): a genuine
    // cross-repo removal must not relocate a still-valid shell, and a same-repo
    // removal misread as cross-repo must still rescue a shell whose worktree it
    // just deleted.
    #[test]
    fn cross_repo_shell_stranded_only_when_captured_cwd_is_gone() {
        // No captured cwd → nothing to rescue, and never relocate on a guess.
        assert!(!cross_repo_shell_stranded(None));

        let scratch = tempfile::tempdir().unwrap();

        // The shell's directory still exists → leave it put (anti-teleport):
        // this is the ordinary cross-repo case, cwd in another repo, untouched.
        let intact = scratch.path().join("still-here");
        std::fs::create_dir(&intact).unwrap();
        assert!(!cross_repo_shell_stranded(Some(&intact)));

        // The shell's directory was removed under it → rescue (anti-strand):
        // `--repo <self>` misclassified as cross-repo, worktree now gone.
        let removed = scratch.path().join("deleted-worktree");
        std::fs::create_dir(&removed).unwrap();
        assert!(!cross_repo_shell_stranded(Some(&removed)));
        std::fs::remove_dir(&removed).unwrap();
        assert!(cross_repo_shell_stranded(Some(&removed)));
    }
}
