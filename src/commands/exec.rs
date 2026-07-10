use crate::{
    get_project_root,
    git::{GitCommand, should_show_gitoxide_notice},
    is_git_repository,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::{change_directory, get_current_directory},
};
use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "git-worktree-exec")]
#[command(version = crate::VERSION)]
#[command(about = "Run a command across one or more worktrees")]
#[command(long_about = r#"
Runs one or more commands against one or more selected worktrees without
changing the current directory.

Targets may be given as positional branch or worktree-directory names, or
globs against branch names (e.g. 'feat/*'). Use --all to target every
worktree in the repository. Positionals and --all are mutually exclusive.

Commands are expressed either as a literal argv after --, or as one or
more -x shell strings. The two forms are mutually exclusive. Multiple -x
values run sequentially per worktree; a failure stops that worktree but
does not stop other worktrees.

When a single worktree is targeted, stdio is fully inherited, making
interactive programs (claude, vim, fzf) work the same as if you had cd'd
into the worktree first.

By default, captured stdout/stderr is dumped only for failed or cancelled
worktrees. Pass --show-output to dump it for successful worktrees too. The
flag has no effect on single-target runs (stdio is already inherited).
"#)]
#[command(after_help = r#"EXAMPLES:
    Run a single command across all worktrees:
        daft exec --all -- npm test

    Run on specific branches (glob and exact mix):
        daft exec feat/auth 'feat/ui-*' -- cargo build

    Sequential with fail-fast:
        daft exec --all --sequential -- pnpm lint

    Pipeline of commands per worktree:
        daft exec --all -x 'mise install' -x 'pnpm build' -x 'pnpm test'

    Pass-through to an interactive program (single target):
        daft exec feat/auth -- claude

    Dump captured output for successful worktrees too:
        daft exec --all --show-output -- cargo build --timings
"#)]
pub struct Args {
    #[arg(help = "Target worktree(s) by branch name, directory name, or glob")]
    pub targets: Vec<String>,

    #[arg(
        long = "all",
        conflicts_with = "targets",
        help = "Target every worktree in the repository"
    )]
    pub all: bool,

    #[arg(
        long = "repo",
        value_name = "REPO",
        conflicts_with_all = ["all_repos", "related"],
        help = "Run in another cataloged repository (targets and --all apply there)"
    )]
    pub repo: Option<String>,

    #[arg(
        long = "all-repos",
        conflicts_with_all = ["repo", "related", "targets", "all"],
        help = "Run in every cataloged repository's default-branch worktree"
    )]
    pub all_repos: bool,

    #[arg(
        long = "related",
        conflicts_with_all = ["repo", "all_repos", "targets", "all"],
        help = "Run across this repo and its related repos (relations manifest), in each one's worktree for the current branch"
    )]
    pub related: bool,

    #[arg(
        short = 'x',
        long = "exec",
        value_name = "CMD",
        help = "Shell command to run (repeatable); runs via $SHELL -c"
    )]
    pub exec: Vec<String>,

    #[arg(
        long = "sequential",
        conflicts_with = "keep_going",
        help = "Run worktrees one at a time and stop on first failure"
    )]
    pub sequential: bool,

    #[arg(
        long = "keep-going",
        help = "Run worktrees one at a time and continue through failures"
    )]
    pub keep_going: bool,

    #[arg(
        long = "refresh-aliases",
        help = "Re-capture user shell aliases instead of using the cached snapshot"
    )]
    pub refresh_aliases: bool,

    #[arg(
        long = "show-output",
        help = "Dump captured stdout/stderr for successful worktrees too (no-op for single-target runs)"
    )]
    pub show_output: bool,

    /// Trailing command vector after `--`. Mutually exclusive with `-x`.
    #[arg(last = true, value_name = "CMD")]
    pub trailing: Vec<String>,
}

pub(crate) fn validate_args(args: &Args) -> anyhow::Result<()> {
    let has_repo_scope = args.repo.is_some() || args.all_repos || args.related;
    if args.targets.is_empty() && !args.all && !has_repo_scope {
        anyhow::bail!(
            "at least one target or --all is required (use `daft exec --help` for examples)"
        );
    }
    if args.exec.is_empty() && args.trailing.is_empty() {
        anyhow::bail!("no command given: pass `-x 'CMD'` one or more times, or `-- CMD ARGS…`");
    }
    if !args.exec.is_empty() && !args.trailing.is_empty() {
        anyhow::bail!("`-x` and `-- CMD` cannot be combined in one invocation");
    }
    Ok(())
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-exec"));
    validate_args(&args)?;

    let inside_repo = is_git_repository()?;
    if inside_repo {
        crate::catalog::touch_current_repo();
    }
    // --repo and --all-repos work from anywhere; everything else needs a repo.
    if !inside_repo && args.repo.is_none() && !args.all_repos {
        anyhow::bail!("Not inside a Git repository");
    }

    let config = OutputConfig::default();
    let mut output = CliOutput::new(config);

    use crate::core::worktree::exec as core;

    // --repo: everything below runs against the target repo, so enter it
    // before any cwd-derived work (settings, snapshot).
    if let Some(needle) = &args.repo {
        let row = crate::catalog::resolve_repo_arg(needle)?;
        change_directory(std::path::Path::new(&row.path))?;
    }

    let targets: Vec<core::ResolvedTarget> = if args.all_repos {
        collect_all_repos_targets(&mut output)?
    } else if args.related {
        collect_related_targets(&mut output)?
    } else {
        let settings = DaftSettings::load()?;
        let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
        if should_show_gitoxide_notice(settings.use_gitoxide) {
            output.warning("[experimental] Using gitoxide backend for git operations");
        }
        let snaps = core::collect_snapshot(&git)?;

        if args.repo.is_some() && args.targets.is_empty() && !args.all {
            // Bare `--repo X`: the repo's default-branch worktree.
            vec![default_branch_target(&snaps)?]
        } else {
            let (targets, orphans) =
                core::resolve_targets_with_orphans(&args.targets, args.all, &snaps)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
            if !orphans.is_empty() {
                output.warning(&format!(
                    "Skipped {} orphan branch(es) (no worktree): {}",
                    orphans.len(),
                    orphans.join(", ")
                ));
            }
            targets
        }
    };

    if targets.is_empty() {
        anyhow::bail!("no matching worktrees to run in");
    }

    let pipeline: Vec<core::CommandSpec> = if !args.trailing.is_empty() {
        vec![core::CommandSpec::Argv(args.trailing.clone())]
    } else {
        args.exec
            .iter()
            .map(|s| core::CommandSpec::Shell(s.clone()))
            .collect()
    };

    // Resolve once and reuse across all targets / commands. The first
    // capture costs a single rc-file load; subsequent invocations within
    // the TTL window read from disk and run at native speed.
    let shell_path = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let alias_cache = core::AliasCache::ensure(&shell_path, args.refresh_aliases);

    // Mode A: single-target pass-through. Inherit stdio; propagate exit
    // code verbatim; never render a UI. Handles `daft exec <single> -- claude`
    // and similar interactive cases without any flag ceremony.
    if targets.len() == 1 {
        let target = &targets[0];
        for spec in &pipeline {
            let mut cmd = core::build_command(spec, alias_cache.as_ref());
            cmd.current_dir(&target.worktree_path)
                .env("DAFT_WORKTREE_PATH", &target.worktree_path)
                .env("DAFT_BRANCH_NAME", &target.branch_name)
                .env("DAFT_COMMAND", "exec")
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit());

            let status = cmd.status()?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        std::process::exit(0);
    }

    let mode = if args.keep_going {
        core::ExecMode::KeepGoing
    } else if args.sequential {
        core::ExecMode::Sequential
    } else {
        core::ExecMode::Parallel
    };

    // Install a SIGINT handler that escalates the shared cancel flag:
    // first Ctrl-C soft-cancels (SIGTERM to children), second Ctrl-C
    // hard-cancels (SIGKILL). `ctrlc::set_handler` is process-global and
    // can only be installed once; swallow the error if something already
    // installed one so tests and nested invocations don't panic.
    let cancel = std::sync::Arc::new(core::CancelFlag::new());
    let handler_flag = std::sync::Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        handler_flag.escalate();
    });

    let report = core::progress_renderer::run_with_progress(
        &targets,
        &pipeline,
        mode,
        &cancel,
        alias_cache.as_ref(),
    )?;

    let dump_mode = if args.show_output {
        core::list_renderer::DumpMode::All
    } else {
        core::list_renderer::DumpMode::FailuresOnly
    };
    let stdout = std::io::stdout();
    let mut sink = stdout.lock();
    core::list_renderer::render_output_dump(&mut sink, &report, &pipeline, dump_mode)?;
    drop(sink);

    std::process::exit(report.aggregate_exit_code());
}

/// Bare `--repo X` target: X's default-branch worktree. cwd is already X.
fn default_branch_target(
    snaps: &[crate::core::worktree::exec::WorktreeSnapshot],
) -> Result<crate::core::worktree::exec::ResolvedTarget> {
    let root = get_project_root()?;
    let branch = crate::core::remote::local_default_branch(&root, "origin").ok_or_else(|| {
        anyhow::anyhow!("could not determine the default branch; pass a target or --all")
    })?;
    snaps
        .iter()
        .filter(|w| w.has_worktree())
        .find(|w| w.branch.as_deref() == Some(branch.as_str()))
        .map(|w| crate::core::worktree::exec::ResolvedTarget {
            worktree_path: w.path.clone(),
            branch_name: branch.clone(),
            display: None,
        })
        .ok_or_else(|| {
            anyhow::anyhow!("no worktree for default branch '{branch}'; pass a target or --all")
        })
}

/// `--all-repos`: one target per live catalog repo — its default-branch
/// worktree. Unusable repos are skipped with a warning, never silently.
fn collect_all_repos_targets(
    output: &mut dyn Output,
) -> Result<Vec<crate::core::worktree::exec::ResolvedTarget>> {
    let rows = match crate::catalog::Catalog::open_ro()? {
        Some(catalog) => catalog.list(false)?,
        None => Vec::new(),
    };
    if rows.is_empty() {
        anyhow::bail!(
            "the repo catalog is empty — clone a repo or run `{}` first",
            crate::daft_cmd("repo add")
        );
    }

    let original = get_current_directory()?;
    let mut targets = Vec::new();
    for row in &rows {
        let path = std::path::Path::new(&row.path);
        if !path.is_dir() {
            output.warning(&format!(
                "skipped '{}' (path missing: {})",
                row.name, row.path
            ));
            continue;
        }
        change_directory(path)?;
        let found = repo_branch_target(row, None);
        change_directory(&original)?;
        match found {
            Ok(Some(target)) => targets.push(target),
            Ok(None) => output.warning(&format!(
                "skipped '{}' (no default-branch worktree)",
                row.name
            )),
            Err(e) => output.warning(&format!("skipped '{}' ({e})", row.name)),
        }
    }
    Ok(targets)
}

/// `--related`: the current repo's current-branch worktree plus, for every
/// relations-manifest edge, that repo's worktree for the same branch.
/// Repos without that worktree (or not cloned) are skipped with a notice —
/// the coordinated-change set is whatever actually carries the branch.
fn collect_related_targets(
    output: &mut dyn Output,
) -> Result<Vec<crate::core::worktree::exec::ResolvedTarget>> {
    use crate::core::worktree::exec as core;

    let resolved = crate::catalog::relations::current_repo_resolved_relations()?;
    if resolved.is_empty() {
        anyhow::bail!(
            "this repo declares no relations — add a `relations:` section to daft.yml \
             (each entry: `- url: <remote-url>`)"
        );
    }

    let current_branch = crate::core::repo::get_current_branch()?;
    let current_worktree = crate::get_current_worktree_path()?;
    let mut targets = vec![core::ResolvedTarget {
        worktree_path: current_worktree,
        branch_name: current_branch.clone(),
        display: Some(format!(
            "{}:{}",
            current_repo_catalog_name(),
            current_branch
        )),
    }];

    let original = get_current_directory()?;
    for relation in &resolved {
        let Some(row) = &relation.repo else {
            output.warning(&format!(
                "related repo '{}' is not cloned locally — `{}`",
                relation.entry.label(),
                crate::daft_cmd(&format!("clone {}", relation.entry.url))
            ));
            continue;
        };
        let path = std::path::Path::new(&row.path);
        if !path.is_dir() {
            output.warning(&format!(
                "skipped '{}' (path missing: {})",
                row.name, row.path
            ));
            continue;
        }
        change_directory(path)?;
        let found = repo_branch_target(row, Some(&current_branch));
        change_directory(&original)?;
        match found {
            Ok(Some(target)) => targets.push(target),
            Ok(None) => output.notice(&format!(
                "skipped '{}' (no worktree for '{}')",
                row.name, current_branch
            )),
            Err(e) => output.warning(&format!("skipped '{}' ({e})", row.name)),
        }
    }
    Ok(targets)
}

/// Find `row`'s worktree for `branch` (default branch when `None`),
/// labeled `repo:branch`. Assumes cwd is already inside `row`'s repo.
fn repo_branch_target(
    row: &crate::store::CatalogRepoRow,
    branch: Option<&str>,
) -> Result<Option<crate::core::worktree::exec::ResolvedTarget>> {
    use crate::core::worktree::exec as core;
    let settings = DaftSettings::load()?;
    let git = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let snaps = core::collect_snapshot(&git)?;
    let branch = match branch {
        Some(b) => b.to_string(),
        None => match crate::catalog::effective_default_branch(row) {
            Some(b) => b,
            None => return Ok(None),
        },
    };
    Ok(snaps
        .iter()
        .filter(|w| w.has_worktree())
        .find(|w| w.branch.as_deref() == Some(branch.as_str()))
        .map(|w| core::ResolvedTarget {
            worktree_path: w.path.clone(),
            branch_name: branch.clone(),
            display: Some(format!("{}:{}", row.name, branch)),
        }))
}

/// The current repo's catalog name, for `repo:branch` labels; falls back
/// to the project directory's name.
fn current_repo_catalog_name() -> String {
    if let Ok(git_dir) = crate::get_git_common_dir()
        && let Ok(canonical) = git_dir.canonicalize()
        && let Ok(Some(catalog)) = crate::catalog::Catalog::open_ro()
        && let Ok(Some(row)) = catalog.resolve(&canonical.to_string_lossy())
    {
        return row.name;
    }
    get_project_root()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "repo".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(argv: &[&str]) -> Result<Args, clap::Error> {
        let mut full = vec!["git-worktree-exec"];
        full.extend_from_slice(argv);
        Args::try_parse_from(full)
    }

    #[test]
    fn parses_argv_after_double_dash() {
        let args = parse(&["--all", "--", "cargo", "test"]).unwrap();
        assert!(args.all);
        assert_eq!(args.trailing, vec!["cargo", "test"]);
        assert!(args.exec.is_empty());
    }

    #[test]
    fn parses_repeated_dash_x() {
        let args = parse(&["feat/a", "-x", "mise install", "-x", "pnpm test"]).unwrap();
        assert_eq!(args.targets, vec!["feat/a"]);
        assert_eq!(args.exec, vec!["mise install", "pnpm test"]);
    }

    #[test]
    fn positionals_conflict_with_all() {
        let err = parse(&["feat/a", "--all", "--", "echo"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"), "{err}");
    }

    #[test]
    fn sequential_conflicts_with_keep_going() {
        let err = parse(&["--all", "--sequential", "--keep-going", "--", "echo"]).unwrap_err();
        assert!(err.to_string().contains("cannot be used with"), "{err}");
    }

    #[test]
    fn accepts_glob_positionals() {
        let args = parse(&["feat/*", "fix/crash", "--", "echo"]).unwrap();
        assert_eq!(args.targets, vec!["feat/*", "fix/crash"]);
    }

    fn validate(args: &Args) -> anyhow::Result<()> {
        super::validate_args(args)
    }

    #[test]
    fn rejects_empty_targets_and_no_all() {
        let args = parse(&["--", "echo"]).unwrap();
        let err = validate(&args).unwrap_err();
        assert!(
            err.to_string().contains("at least one target"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_empty_command_forms() {
        let args = parse(&["--all"]).unwrap();
        let err = validate(&args).unwrap_err();
        assert!(
            err.to_string().contains("-x") || err.to_string().contains("--"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_both_command_forms() {
        let args = parse(&["--all", "-x", "echo", "--", "echo"]).unwrap();
        let err = validate(&args).unwrap_err();
        assert!(err.to_string().contains("cannot be combined"), "got: {err}");
    }

    #[test]
    fn accepts_minimal_valid_argv_form() {
        let args = parse(&["--all", "--", "echo"]).unwrap();
        validate(&args).unwrap();
    }

    #[test]
    fn accepts_minimal_valid_x_form() {
        let args = parse(&["--all", "-x", "echo"]).unwrap();
        validate(&args).unwrap();
    }

    #[test]
    fn rejects_unknown_verbose_flag() {
        let err = parse(&["--all", "--verbose", "--", "echo"]).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "expected UnknownArgument for --verbose, got: kind={:?}, msg={err}",
            err.kind()
        );
    }
}
