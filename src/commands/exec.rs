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

On an interactive terminal, multi-worktree runs render a live rail: one row
per worktree, filled in place, persisted as a receipt. Failed worktrees thread
their captured output under their row; pass -v to thread every worktree's
output (and, when stdout is redirected, dump successful worktrees' output too).
The flag has no effect on single-target runs (stdio is already inherited).
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

    Thread every worktree's output (and dump successes when redirected):
        daft exec --all -v -- cargo build --timings
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
        short = 'v',
        long = "verbose",
        help = "Thread each worktree's full output into the rail and dump captured output for successful worktrees too (no-op for single-target runs)"
    )]
    pub verbose: bool,

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
    let repo_row = match &args.repo {
        Some(needle) => {
            let row = crate::catalog::resolve_repo_arg(needle)?;
            change_directory(std::path::Path::new(&row.path))?;
            Some(row)
        }
        None => None,
    };

    let (targets, orphans): (Vec<core::ResolvedTarget>, Vec<String>) = if args.all_repos {
        (collect_all_repos_targets(&mut output)?, Vec::new())
    } else if args.related {
        (collect_related_targets(&mut output)?, Vec::new())
    } else {
        let settings = DaftSettings::load()?;
        let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
        if should_show_gitoxide_notice(settings.use_gitoxide) {
            output.warning("[experimental] Using gitoxide backend for git operations");
        }
        let snaps = core::collect_snapshot(&git)?;

        if args.repo.is_some() && args.targets.is_empty() && !args.all {
            // Bare `--repo X`: the repo's default-branch worktree, resolved
            // through the catalog like --all-repos/--related (not origin/HEAD
            // only), so a recorded default branch without origin/HEAD resolves.
            (
                vec![default_branch_target(&snaps, repo_row.as_ref())?],
                Vec::new(),
            )
        } else {
            // Orphans (matched branches with no worktree) become rail rows on
            // the interactive multi-target path, or a warning otherwise —
            // decided below, once the render path is known.
            core::resolve_targets_with_orphans(&args.targets, args.all, &snaps)
                .map_err(|e| anyhow::anyhow!("{e}"))?
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

    // The interactive multi-target run renders the rail (and shows orphans as
    // rows); the single-target passthrough and the non-interactive path warn
    // about orphans instead.
    let interactive = matches!(
        crate::output::timeline::TimelineMode::auto(false),
        crate::output::timeline::TimelineMode::Interactive { .. }
    );
    let will_rail = interactive && targets.len() >= 2;
    if !orphans.is_empty() && !will_rail {
        output.warning(&format!(
            "Skipped {} orphan branch(es) (no worktree): {}",
            orphans.len(),
            orphans.join(", ")
        ));
    }

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

    // Two-stage Ctrl-C: first ^C SIGTERMs children, second SIGKILLs. It rides
    // the interrupt dispatcher's slot (a live rail scaffolds its own
    // collapse-and-exit behavior; exec's escalation overrides it — see
    // `run_rail` — so the first ^C never tears the rail down) and re-arms
    // itself so the second ^C escalates again.
    let cancel = std::sync::Arc::new(core::CancelFlag::new());

    // The rail's output threads and the `-v` fold-in share the hook output
    // knobs. `--all-repos` runs from outside any repo, so tolerate a missing
    // config with the defaults.
    let hook_output = crate::core::settings::load_hooks_config()
        .map(|c| c.output)
        .unwrap_or_default()
        .with_cli_verbose(args.verbose);

    let report = if will_rail {
        run_rail(
            &targets,
            &orphans,
            &pipeline,
            mode,
            &cancel,
            alias_cache.as_ref(),
            &args,
            &hook_output,
        )?
    } else {
        arm_exec_interrupt(std::sync::Arc::clone(&cancel));
        core::progress_renderer::run_with_progress(
            &targets,
            &pipeline,
            mode,
            &cancel,
            alias_cache.as_ref(),
        )?
    };

    // The rail already showed every worktree's output on the terminal, so only
    // dump to stdout when it is redirected (or when no rail rendered). Non-rail
    // modes always dump, exactly as before.
    let dump_mode = if args.verbose {
        core::list_renderer::DumpMode::All
    } else {
        core::list_renderer::DumpMode::FailuresOnly
    };
    if !will_rail || !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        let stdout = std::io::stdout();
        let mut sink = stdout.lock();
        core::list_renderer::render_output_dump(&mut sink, &report, &pipeline, dump_mode)?;
        drop(sink);
    }

    std::process::exit(report.aggregate_exit_code());
}

/// Arm the two-stage Ctrl-C escalation on the interrupt dispatcher's slot,
/// re-arming after each fire so the next ^C escalates again (the slot is
/// one-shot). The behavior never exits the process — it only escalates the
/// shared cancel flag, which the scheduler's wait loop observes to SIGTERM
/// then SIGKILL the children. The rail's teardown (`finish`/`abort`) clears
/// the slot.
fn arm_exec_interrupt(cancel: std::sync::Arc<crate::core::worktree::exec::CancelFlag>) {
    crate::interrupt::set_behavior(move || {
        cancel.escalate();
        arm_exec_interrupt(std::sync::Arc::clone(&cancel));
    });
}

/// A command label for a plan row / header, capped so a long command line
/// can't blow out the rail's width.
fn truncate_cmd(s: &str) -> String {
    const MAX: usize = 64;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let head: String = s.chars().take(MAX - 1).collect();
        format!("{head}\u{2026}")
    }
}

/// Render a multi-target run on the plan-then-execute rail: build the plan and
/// the matching presenter, override the region's Ctrl-C collapse with exec's
/// two-stage escalation, run the scheduler, and close the rail with the
/// outcome footer. Returns the aggregated report for the caller's stdout dump.
#[allow(clippy::too_many_arguments)]
fn run_rail(
    targets: &[crate::core::worktree::exec::ResolvedTarget],
    orphans: &[String],
    pipeline: &[crate::core::worktree::exec::CommandSpec],
    mode: crate::core::worktree::exec::ExecMode,
    cancel: &std::sync::Arc<crate::core::worktree::exec::CancelFlag>,
    alias_cache: Option<&crate::core::worktree::exec::AliasCache>,
    args: &Args,
    hook_output: &crate::settings::HookOutputConfig,
) -> Result<crate::core::worktree::exec::ExecReport> {
    use crate::core::stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec};
    use crate::core::worktree::exec::rail_presenter::{RailExecPresenter, command_key};
    use crate::core::worktree::exec::{self as core, ExecReport};
    use crate::executor::presenter::JobPresenter;
    use crate::output::timeline::{RowOutputConfig, Timeline, TimelineMode};
    use std::collections::HashMap;
    use std::sync::Arc;

    let m = pipeline.len();
    let scope_noun = if args.all_repos {
        "repos"
    } else if args.related {
        "related worktrees"
    } else {
        "worktrees"
    };
    let cmd_phrase = if m == 1 {
        format!("Running {}", truncate_cmd(&pipeline[0].display()))
    } else {
        format!("Running {m} commands")
    };
    // The rail only renders for >= 2 targets (single-target is passthrough),
    // so the count is always plural.
    let header = format!("{cmd_phrase} in {} {scope_noun}", targets.len());

    let mut timeline = Timeline::new(TimelineMode::auto(false), hook_output.verbose, header);
    timeline.set_ordered_receipts(true);
    timeline.set_row_output(RowOutputConfig {
        verbose: hook_output.verbose,
        tail_lines: hook_output.tail_lines as usize,
        buffer_cap: Some(core::OUTPUT_CAP_BYTES),
    });

    // Build the plan and the presenter's row keys together — both index by the
    // same `command_key`. Orphan rows lead the plan and resolve immediately.
    let mut plan_rows: Vec<Row> = Vec::new();
    let orphan_keys: Vec<StepKey> = orphans
        .iter()
        .map(|branch| {
            let key = StepKey::scoped(StageId::ExecCommand, format!("orphan\u{1f}{branch}"));
            plan_rows.push(Row::Step(
                StepSpec::new(key.clone()).with_label(branch.clone()),
            ));
            key
        })
        .collect();

    let mut rows: HashMap<String, Vec<StepKey>> = HashMap::new();
    for target in targets {
        let label = target.label().to_string();
        let keys: Vec<StepKey> = (0..m).map(|i| command_key(&label, i, m)).collect();
        if m == 1 {
            plan_rows.push(Row::Step(
                StepSpec::new(keys[0].clone()).with_label(label.clone()),
            ));
        } else {
            plan_rows.push(Row::Group {
                label: label.clone(),
            });
            for (i, key) in keys.iter().enumerate() {
                plan_rows.push(Row::Step(
                    StepSpec::new(key.clone()).with_label(truncate_cmd(&pipeline[i].display())),
                ));
            }
        }
        rows.insert(label, keys);
    }

    timeline.commit_plan(PlanCommit::new(plan_rows));

    // Orphans never run — resolve their rows to the yellow `↓ … no worktree`.
    for key in &orphan_keys {
        timeline.on_stage(
            key,
            StageEvent::SkippedAttention {
                reason: "no worktree".to_string(),
            },
        );
    }

    // Override the region's collapse behavior with exec's escalation, now that
    // the plan (and thus the region's own Ctrl-C handler) has committed.
    arm_exec_interrupt(Arc::clone(cancel));

    let presenter: Arc<dyn JobPresenter> =
        Arc::new(RailExecPresenter::new(timeline.handle(), rows));
    let scheduler_result = core::progress_renderer::run_fleet(
        targets,
        pipeline,
        mode,
        &presenter,
        cancel,
        alias_cache,
        core::progress_renderer::NameStyle::Label,
    );

    // Close the rail with the outcome footer before returning (finish/abort
    // clear the interrupt slot and restore the terminal).
    let elapsed = timeline.elapsed_display();
    match &scheduler_result {
        Ok(outcomes) => {
            let cancelled = cancel.is_cancelled() || outcomes.iter().any(|o| o.cancelled);
            let stopped = outcomes.len() < targets.len();
            let any_failed = outcomes.iter().any(|o| !o.succeeded());
            if cancelled {
                timeline.abort(&format!("Cancelled after {elapsed}"));
            } else if stopped {
                timeline.abort(&format!("Failed after {elapsed}"));
            } else if any_failed {
                timeline.finish(&format!("Finished with failures in {elapsed}"));
            } else {
                timeline.finish(&format!("Done in {elapsed}"));
            }
        }
        Err(_) => timeline.abort(&format!("Failed after {elapsed}")),
    }

    Ok(ExecReport {
        outcomes: scheduler_result?,
        orphan_branches_skipped: orphans.to_vec(),
    })
}

/// The live worktree checked out on `branch`, as an exec target.
fn find_worktree_for_branch(
    snaps: &[crate::core::worktree::exec::WorktreeSnapshot],
    branch: &str,
    display: Option<String>,
) -> Option<crate::core::worktree::exec::ResolvedTarget> {
    snaps
        .iter()
        .filter(|w| w.has_worktree())
        .find(|w| w.branch.as_deref() == Some(branch))
        .map(|w| crate::core::worktree::exec::ResolvedTarget {
            worktree_path: w.path.clone(),
            branch_name: branch.to_string(),
            display,
        })
}

/// Bare `--repo X` target: X's default-branch worktree. cwd is already X. The
/// branch comes from the catalog row (recorded default branch, else
/// origin/HEAD) — the same resolution --all-repos/--related use — so bare
/// `--repo` can't diverge from them on a repo with no origin/HEAD.
fn default_branch_target(
    snaps: &[crate::core::worktree::exec::WorktreeSnapshot],
    row: Option<&crate::store::CatalogRepoRow>,
) -> Result<crate::core::worktree::exec::ResolvedTarget> {
    let branch = row
        .and_then(crate::catalog::effective_default_branch)
        .or_else(|| {
            get_project_root()
                .ok()
                .and_then(|root| crate::core::remote::local_default_branch(&root, "origin"))
        })
        .ok_or_else(|| {
            anyhow::anyhow!("could not determine the default branch; pass a target or --all")
        })?;
    find_worktree_for_branch(snaps, &branch, None).ok_or_else(|| {
        anyhow::anyhow!("no worktree for default branch '{branch}'; pass a target or --all")
    })
}

/// `--all-repos`: one target per live catalog repo — its default-branch
/// worktree. Unusable repos are skipped with a warning, never silently.
fn collect_all_repos_targets(
    output: &mut dyn Output,
) -> Result<Vec<crate::core::worktree::exec::ResolvedTarget>> {
    // open_ro contract: a transient open error degrades to "no catalog" (the
    // empty-catalog bail below), never a hard failure.
    let rows = match crate::catalog::Catalog::open_ro().ok().flatten() {
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
    Ok(find_worktree_for_branch(
        &snaps,
        &branch,
        Some(format!("{}:{}", row.name, branch)),
    ))
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
    fn default_branch_target_prefers_the_catalog_row() {
        // #357 C5: bare `--repo X` resolves the default branch from the catalog
        // row (like --all-repos/--related), not origin/HEAD — so it can't
        // diverge from them on a repo lacking origin/HEAD. A row with a
        // recorded default_branch short-circuits the origin/HEAD fallback.
        use crate::core::worktree::exec::WorktreeSnapshot;
        use std::path::PathBuf;

        let row = crate::store::CatalogRepoRow {
            uuid: "u1".into(),
            name: "api".into(),
            path: "/w/api".into(),
            git_common_dir: "/w/api/.git".into(),
            remote_url: None,
            remote_url_normalized: None,
            default_branch: Some("trunk".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            removed_at: None,
        };
        let snaps = vec![
            WorktreeSnapshot {
                path: PathBuf::from("/w/api/other"),
                branch: Some("other".into()),
            },
            WorktreeSnapshot {
                path: PathBuf::from("/w/api/trunk"),
                branch: Some("trunk".into()),
            },
        ];
        let target = default_branch_target(&snaps, Some(&row)).unwrap();
        assert_eq!(target.branch_name, "trunk");
        assert_eq!(target.worktree_path, PathBuf::from("/w/api/trunk"));
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
    fn accepts_verbose_flag() {
        // -v / --verbose replaced --show-output: it threads output onto the
        // rail and dumps successes when stdout is redirected.
        let long = parse(&["--all", "--verbose", "--", "echo"]).unwrap();
        assert!(long.verbose);
        let short = parse(&["--all", "-v", "--", "echo"]).unwrap();
        assert!(short.verbose);
        let off = parse(&["--all", "--", "echo"]).unwrap();
        assert!(!off.verbose);
    }
}
