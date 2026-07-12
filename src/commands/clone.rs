use crate::{
    check_dependencies,
    core::{
        HookRunner, NullSink, OutputSink, ProgressSink, TimelineSink, TuiBridge,
        global_config::GlobalConfig,
        layout::{
            Layout, TemplateContext,
            resolver::{LayoutResolutionContext, resolve_layout},
        },
        ownership::OwnershipStrategy,
        stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec},
        worktree::{
            branch_source::{BranchPlan, BranchSource},
            clone,
            info_field::FieldSet,
            list::{EntryKind, Stat, WorktreeInfo},
            list_stream,
            sync_dag::{DagEvent, OperationPhase, PatchSource, TaskMessage, TaskStatus},
        },
    },
    executor::cli_presenter::CliPresenter,
    git::{GitCommand, should_show_gitoxide_notice},
    hints::{
        LayoutPromptResult, layout_prompt_applicable, maybe_prompt_layout_choice,
        maybe_show_shell_hint,
    },
    hooks::{
        HookContext, HookExecutor, HookType, TrustDatabase, TrustLevel, get_remote_url_for_git_dir,
        yaml_config_loader,
    },
    logging::init_logging,
    output::{
        CliOutput, Output, OutputConfig,
        timeline::{RegionOutput, Timeline, TimelineMode},
        tui::{
            Column,
            operation_table::{OperationTable, TableConfig},
        },
    },
    settings::{DaftSettings, HookOutputConfig},
    utils::*,
};
use anyhow::Result;
use clap::Parser;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "git-worktree-clone")]
#[command(version = crate::VERSION)]
#[command(about = "Clone a repository into a worktree-based directory structure")]
#[command(long_about = r#"
Clones a repository into a directory structure optimized for worktree-based
development. The resulting layout is:

    <repository-name>/.git    (bare repository metadata)
    <repository-name>/<branch>  (worktree for the checked-out branch)

The command first queries the remote to determine the default branch (main,
master, or other configured default), then performs a bare clone and creates
the initial worktree. This structure allows multiple worktrees to be created
as siblings, each containing a different branch.

If the repository contains a .daft/hooks/ directory and the repository is
trusted, lifecycle hooks are executed. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(help = "The repository URL to clone (HTTPS or SSH)")]
    repository_url: String,

    #[arg(
        short = 'b',
        long = "branch",
        value_name = "BRANCH",
        action = clap::ArgAction::Append,
        help = "Branch to check out (repeatable; use HEAD or @ for default branch)"
    )]
    branch: Vec<String>,

    #[arg(
        short = 'n',
        long = "no-checkout",
        help = "Perform a bare clone only; do not create any worktree (requires a bare layout: contained or contained-flat)"
    )]
    no_checkout: bool,

    #[arg(
        short = 'q',
        long = "quiet",
        help = "Operate quietly; suppress progress reporting"
    )]
    quiet: bool,

    #[arg(
        short = 'v',
        long = "verbose",
        action = clap::ArgAction::Count,
        help = "Increase verbosity (-v for hook details, -vv for full sequential output)"
    )]
    verbose: u8,

    #[arg(
        short = 'a',
        long = "all-branches",
        help = "Create a worktree for each remote branch, not just the default"
    )]
    all_branches: bool,

    #[arg(
        long = "trust-hooks",
        help = "Trust the repository and allow hooks to run without prompting"
    )]
    trust_hooks: bool,

    /// Skip hooks this run. Repeatable / comma-separated.
    /// Selectors: `all` (every hook), a hook name (`post-clone`,
    /// `worktree-post-create`, …), `tag:<tag>`, or a job name (plus its
    /// dependents). See daft-hooks(1).
    #[arg(
        long,
        value_name = "SELECTOR",
        value_delimiter = ',',
        help = "Skip hooks this run (all | <hook> | tag:<tag> | <job>); repeatable/comma-separated"
    )]
    skip_hooks: Vec<String>,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Organize worktree under this remote folder (enables multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    /// Worktree layout to use for this repository.
    ///
    /// Built-in layouts: contained, sibling, nested, centralized.
    /// Can also be a custom layout name from ~/.config/daft/config.toml
    /// or an inline template string.
    #[arg(long, value_name = "LAYOUT")]
    layout: Option<String>,

    #[arg(
        long,
        help = "Columns to display (comma-separated). Replace: branch,base,age. Modify defaults: +col,-col. Available: branch, path, size, base, changes, remote, age, annotation, owner, hash, last-commit"
    )]
    columns: Option<String>,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,

    #[arg(
        long = "install",
        help = "Run `daft install` in the new worktree(s) after a successful clone (implies --trust-hooks)"
    )]
    install: bool,

    #[arg(
        long = "git-exclude",
        help = "With --install: add /daft.yml to .git/info/exclude without prompting"
    )]
    git_exclude: bool,
}

pub fn run() -> Result<()> {
    let mut args = Args::parse_from(crate::get_clap_args("git-worktree-clone"));

    init_logging(args.verbose >= 2);

    validate_arg_combinations(&args)?;
    apply_install_trust(&mut args);

    let settings = DaftSettings::load_global()?;

    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose >= 2, autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    if let Err(e) = run_clone(&args, &settings, &mut output) {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    Ok(())
}

fn validate_arg_combinations(args: &Args) -> Result<()> {
    if args.no_checkout && args.all_branches {
        anyhow::bail!(
            "--no-checkout and --all-branches cannot be used together.\nUse --no-checkout to create only the bare repository, or --all-branches to create worktrees for all branches."
        );
    }
    if !args.branch.is_empty() && args.all_branches {
        anyhow::bail!(
            "--branch and --all-branches cannot be used together.\nUse --branch to checkout a specific branch, or --all-branches to create worktrees for all branches."
        );
    }
    if !args.branch.is_empty() && args.no_checkout {
        anyhow::bail!(
            "--branch and --no-checkout cannot be used together.\nUse --branch to checkout a specific branch, or --no-checkout to skip worktree creation."
        );
    }
    if args.remote.is_some() && args.branch.len() > 1 {
        anyhow::bail!("--remote cannot be used with multiple -b flags.");
    }
    if args.trust_hooks && skip_hooks_all(&args.skip_hooks) {
        anyhow::bail!("--trust-hooks and --skip-hooks all cannot be used together.");
    }
    if args.install && args.no_checkout {
        anyhow::bail!(
            "--install and --no-checkout cannot be used together.\n--install writes daft.yml into a worktree; --no-checkout creates none."
        );
    }
    if args.git_exclude && !args.install {
        anyhow::bail!("--git-exclude only applies together with --install.");
    }
    Ok(())
}

/// True when `--skip-hooks` requests skipping *every* hook (`all` / `*`) — the
/// uniform replacement for the old `--no-hooks`. Partial skips (`tag:`/`<job>`)
/// are NOT `all`: hooks still fire, just with some jobs excluded.
fn skip_hooks_all(skip_hooks: &[String]) -> bool {
    crate::hooks::job_adapter::parse_skip_selectors(skip_hooks).all
}

/// `--install` implies `--trust-hooks`: bootstrapping your own daft.yml in this
/// clone is an implicit trust decision — the hooks you'll run are your own, and
/// you shouldn't be prompted to trust your own config on the next worktree op.
/// `--skip-hooks all` opts out of hooks entirely, so it wins (and keeps us clear
/// of the `--trust-hooks`/`--skip-hooks all` conflict rejected in
/// `validate_arg_combinations`). A *partial* skip still runs your own hooks, so
/// it does NOT suppress the trust implication. Applied after validation so it
/// never trips that conflict check.
fn apply_install_trust(args: &mut Args) {
    if args.install && !skip_hooks_all(&args.skip_hooks) {
        args.trust_hooks = true;
    }
}

/// Reject `--no-checkout` for layouts where the resolved `repo_path` is also
/// the working tree. Without a separate bare directory, `-n` cannot leave an
/// "empty bare" state — it would silently degrade to a non-bare clone with no
/// working files, which is not what the user asked for.
fn check_no_checkout_compat(no_checkout: bool, layout: &Layout) -> Result<()> {
    if no_checkout && !layout.needs_bare() {
        anyhow::bail!(
            "--no-checkout requires a bare layout. The '{}' layout uses a non-bare repository.\n\
             Pick one of the bare-using layouts: contained or contained-flat.",
            layout.name
        );
    }
    Ok(())
}

fn run_clone(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    let global_config = GlobalConfig::load().unwrap_or_default();
    let original_dir = get_current_directory()?;

    let branch_source = BranchSource::from_args(&args.branch, args.all_branches);

    // Extract a single branch for backward compatibility with BareCloneParams.
    let bare_branch = match &branch_source {
        BranchSource::Single(b) => Some(b.clone()),
        _ => None,
    };

    // Phase 1: Always clone bare first
    let bare_params = clone::BareCloneParams {
        repository_url: args.repository_url.clone(),
        branch: bare_branch.clone(),
        no_checkout: args.no_checkout,
        all_branches: args.all_branches,
        remote: args.remote.clone(),
        remote_name: settings.remote.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_upstream: settings.checkout_upstream,
        use_gitoxide: settings.use_gitoxide,
    };

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    // Plan-execute rail timeline (#651): the rail opens the moment the
    // command starts. The header's repo name is pure URL parsing — the same
    // derivation the bare phase repeats — so a malformed URL still errors
    // before any region exists. The plan itself commits only once the
    // resolve span (bare clone, layout, branch resolution) has the facts;
    // until then the planning face carries the liveness, and the bare phase
    // later appears on the rail as a pre-completed row.
    let repo_name = crate::extract_repo_name(&args.repository_url)?;
    let mut timeline = Timeline::new(
        TimelineMode::auto(output.is_quiet()),
        output.is_verbose(),
        format!("Cloning {repo_name}"),
    );
    timeline.open_planning("Cloning repository");

    let bare_started = std::time::Instant::now();
    let bare_result = {
        let mut sink = TimelineSink::new(output, &mut timeline);
        clone::clone_bare_phase(&bare_params, &mut sink)
    };
    let bare_result = match bare_result {
        Ok(result) => result,
        Err(e) => {
            timeline.abandon_planning();
            return Err(e);
        }
    };
    let bare_elapsed = bare_started.elapsed();

    // The clone landed; the rest of the resolve span is branch work.
    timeline.set_planning_label("Resolving branches");

    // Phase 2: Read daft.yml from the bare repo (if no --layout flag)
    let yaml_layout = if args.layout.is_none() && !bare_result.is_empty {
        match yaml_config_loader::load_config_from_bare(&bare_result.git_dir) {
            Ok(Some(config)) => config.layout,
            Ok(None) => None,
            Err(e) => {
                TimelineSink::new(output, &mut timeline)
                    .on_warning(&format!("Could not read daft.yml: {e}"));
                None
            }
        }
    } else {
        None
    };

    // Phase 3: Resolve layout with full context. The first-clone layout
    // prompt owns the terminal while it draws — the planning face steps
    // aside for it (a pre-flight prompt, per the rail's contract) and
    // returns once answered. Gated on `layout_prompt_applicable` so the
    // silent-Default paths (hint already answered, hints disabled, non-TTY
    // stdin) never blink the face.
    let prompted_layout = if args.layout.is_none()
        && yaml_layout.is_none()
        && global_config.defaults.layout.is_none()
        && layout_prompt_applicable(output)
    {
        timeline.abandon_planning();
        match maybe_prompt_layout_choice(output, "Clone cancelled. Nothing was changed.") {
            LayoutPromptResult::Chosen(layout) => {
                timeline.open_planning("Resolving branches");
                Some(layout)
            }
            LayoutPromptResult::Default => {
                timeline.open_planning("Resolving branches");
                None
            }
            LayoutPromptResult::Cancelled => {
                // Clean up: we already cloned, so delete it
                change_directory(&original_dir).ok();
                remove_directory(&bare_result.parent_dir).ok();
                return Ok(());
            }
        }
    } else {
        None
    };

    let effective_cli_layout = args.layout.as_deref().or(prompted_layout.as_deref());

    let (layout, _source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: effective_cli_layout,
        repo_store_layout: None,
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection: None,
    });

    if let Err(e) = check_no_checkout_compat(args.no_checkout, &layout) {
        // The bare clone already landed on disk — remove it so a rejected
        // clone leaves no orphan directory behind.
        timeline.abandon_planning();
        change_directory(&original_dir).ok();
        remove_directory(&bare_result.parent_dir).ok();
        return Err(e);
    }

    // Report layout decision
    if layout.needs_bare() {
        TimelineSink::new(output, &mut timeline).on_step(&format!(
            "Using layout '{}' (worktrees inside repo)",
            layout.name
        ));
    } else {
        TimelineSink::new(output, &mut timeline)
            .on_step(&format!("Using layout '{}'", layout.name));
    }

    // Resolve branches against the remote
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let remote_branches = match git.list_remote_branches(&bare_params.remote_name) {
        Ok(branches) => branches,
        Err(e) => {
            timeline.abandon_planning();
            return Err(e);
        }
    };
    let remote_branch_refs: Vec<&str> = remote_branches.iter().map(|s| s.as_str()).collect();
    let branch_plan = branch_source.resolve(
        &bare_result.default_branch,
        layout.needs_bare(),
        &remote_branch_refs,
    );

    // Warn about missing branches
    for branch in &branch_plan.not_found {
        TimelineSink::new(output, &mut timeline)
            .on_warning(&format!("Branch '{}' not found on remote", branch));
    }

    // Determine if this is a multi-branch clone (Multiple or All source with
    // satellites to create beyond what Phase 4 handles).
    let is_multi_branch = matches!(branch_source, BranchSource::Multiple(_));

    // For multi-branch, override bare_result's target_branch so that Phase 4
    // creates the correct worktree. For non-bare layouts, this is the base
    // branch (`branch_plan.base`). For bare layouts, `branch_plan.base` is
    // None — Phase 4 must instead target the first valid requested branch
    // (`branch_plan.cd_target`), or skip worktree creation entirely when no
    // requested branch was found on the remote. Without this, Phase 4 keeps
    // the default-branch target set by `detect_branches` and creates an
    // unwanted worktree for the default branch (#451).
    let mut bare_result = bare_result;
    if is_multi_branch {
        if layout.needs_bare() {
            if let Some(ref first_valid) = branch_plan.cd_target {
                bare_result.target_branch = first_valid.clone();
                bare_result.branch_exists = true;
            } else if let Some(first_missing) = branch_plan.not_found.first() {
                bare_result.target_branch = first_missing.clone();
                bare_result.branch_exists = false;
            }
        } else if let Some(ref base) = branch_plan.base {
            bare_result.target_branch = base.clone();
            bare_result.branch_exists = remote_branches.contains(base);
        }
    }

    // After clone_bare_phase, cwd is inside the repo directory. Capture the
    // absolute path now — Phase 4 may change cwd (e.g., contained-classic moves
    // into a branch subdir), making the relative parent_dir unreachable.
    let canonical_parent_dir =
        std::env::current_dir().unwrap_or_else(|_| bare_result.parent_dir.clone());

    // Filter out the branch that Phase 4 already created (for bare layouts).
    // For bare layouts, Phase 4 creates a worktree for bare_result.target_branch,
    // but branch_plan.satellites includes it since branch_plan.base is None.
    // (Computed before Phase 4 so the timeline plan can account for the
    // satellite phase; only depends on the resolution done above.)
    let filtered_satellites: Vec<String> = if layout.needs_bare() {
        branch_plan
            .satellites
            .iter()
            .filter(|b| *b != &bare_result.target_branch)
            .cloned()
            .collect()
    } else {
        branch_plan.satellites.clone()
    };

    // Every prompt has fired and the resolve span has its facts; commit the
    // plan onto the planning face (it spans several core phases, so the
    // command owns it). When satellites will render on the TTY after the
    // base — the OperationTable, or the sequential legacy path under `-vv`
    // — the rail closes at the base milestone first and the post-clone/
    // post-create hooks render standalone after, exactly as today — so
    // their rows are only planned for the single-target journeys.
    let satellites_on_tty = is_multi_branch
        && !filtered_satellites.is_empty()
        && std::io::IsTerminal::is_terminal(&std::io::stderr());
    let will_use_satellite_tui = satellites_on_tty && args.verbose < 2;
    // Shared files the cloned config declares get a section between the hook
    // stages (post-clone hooks may seed shared storage, the links land before
    // post-create hooks). No worktree exists yet, so the probe reads the
    // config blob from the bare object store; on a clone into fresh storage
    // every row typically vanishes silently — planned, removed if unnecessary.
    let planned_shared = if satellites_on_tty {
        Vec::new()
    } else {
        crate::core::shared::read_shared_paths_from_ref(
            &canonical_parent_dir,
            &bare_result.target_branch,
        )
    };
    {
        let mut plan_rows = vec![
            Row::Step(
                StepSpec::new(StepKey::new(StageId::CloneBare))
                    .with_annotation(format!(
                        "\u{2190} {}",
                        crate::core::repo::display_url(&bare_params.repository_url)
                    ))
                    .pre_completed(bare_elapsed),
            ),
            Row::Step(
                StepSpec::new(StepKey::new(StageId::CreateBaseWorktree))
                    .with_annotation(bare_result.target_branch.clone()),
            ),
        ];
        if !satellites_on_tty {
            plan_rows.push(Row::Step(StepSpec::new(StepKey::new(
                StageId::PostCloneHooks,
            ))));
            crate::core::shared::push_shared_section(&mut plan_rows, &planned_shared);
            plan_rows.push(Row::Step(StepSpec::new(StepKey::new(
                StageId::PostCreateHooks,
            ))));
            if args.install {
                plan_rows.push(Row::Step(StepSpec::new(StepKey::new(StageId::Install))));
            }
        }
        timeline.commit_plan(PlanCommit::new(plan_rows));
    }

    // Phase 4: Set up repo in the correct layout. The rail region owns the
    // terminal now — the legacy spinners only run when it does not.
    let base_worktree_key = StepKey::new(StageId::CreateBaseWorktree);
    timeline.on_stage(&base_worktree_key, StageEvent::Started);
    let region_live = timeline.region_live();
    let result = if layout.needs_bare() {
        if !region_live {
            output.start_spinner("Setting up worktrees...");
        }
        let r = {
            let mut sink = TimelineSink::new(output, &mut timeline);
            clone::setup_bare_worktrees(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r
    } else if layout.needs_wrapper() {
        if !region_live {
            output.start_spinner("Setting up wrapped repository...");
        }
        let r = {
            let mut sink = TimelineSink::new(output, &mut timeline);
            clone::setup_wrapped_nonbare(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r
    } else {
        if !region_live {
            output.start_spinner("Setting up repository...");
        }
        let r = {
            let mut sink = TimelineSink::new(output, &mut timeline);
            clone::unbare_and_checkout(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r
    };
    let result = match result {
        Ok(result) => result,
        Err(e) => {
            timeline.abort(&format!("Failed after {}", timeline.elapsed_display()));
            return Err(e);
        }
    };
    if result.branch_not_found {
        timeline.on_stage(
            &base_worktree_key,
            StageEvent::SkippedAttention {
                reason: format!("branch '{}' not found on remote", result.target_branch),
            },
        );
    } else if result.no_checkout {
        timeline.on_stage(
            &base_worktree_key,
            StageEvent::SkippedExpected {
                reason: "--no-checkout".to_string(),
            },
        );
    } else {
        timeline.on_stage(
            &base_worktree_key,
            StageEvent::Completed { annotation: None },
        );
    }

    // For bare layouts, the "base" shown in the TUI is the Phase 4-created branch.
    // For non-bare layouts, it's branch_plan.base.
    let tui_base_branch: Option<String> = if layout.needs_bare() {
        // Phase 4 always creates a worktree for bare_result.target_branch
        Some(bare_result.target_branch.clone())
    } else {
        branch_plan.base.clone()
    };

    // Parse --columns for TUI table (default: branch, base, age, last-commit)
    use crate::core::columns::{ColumnSelection, CommandKind};

    let (tui_columns, columns_explicit) = match args.columns {
        Some(ref input) => {
            let resolved = match ColumnSelection::parse(input, CommandKind::Clone) {
                Ok(resolved) => resolved,
                Err(e) => return Err(fail_rail(&mut timeline, anyhow::anyhow!("{e}"))),
            };
            let tui_cols: Vec<Column> = resolved
                .columns
                .iter()
                .map(|c| Column::from_list_column(*c))
                .collect();
            (Some(tui_cols), resolved.explicit)
        }
        None => {
            // Clone-specific defaults: base, age, last-commit
            let defaults: Vec<Column> = crate::core::columns::ListColumn::clone_defaults()
                .iter()
                .map(|c| Column::from_list_column(*c))
                .collect();
            (Some(defaults), true)
        }
    };

    // Ensure parent_dir is canonical for satellite creation (Phase 4 may have
    // changed cwd, making the original relative parent_dir unreachable).
    let mut result = result;
    result.parent_dir = canonical_parent_dir;

    // Phase 5: Create satellite worktrees for multi-branch clone
    let mut used_tui = false;
    let result = if is_multi_branch && !filtered_satellites.is_empty() {
        if will_use_satellite_tui {
            used_tui = true;
            // The satellite OperationTable owns the terminal next — close
            // the rail first (base is done; hooks render after the table).
            timeline.finish(&format!(
                "Base worktree ready in {}",
                timeline.elapsed_display()
            ));
            create_satellite_worktrees_tui(
                &result,
                &branch_plan,
                &filtered_satellites,
                tui_base_branch.as_deref(),
                &bare_params,
                &layout,
                settings,
                &args.skip_hooks,
                args.trust_hooks,
                args.verbose,
                tui_columns.clone(),
                columns_explicit,
            )?
        } else {
            // The sequential satellite path (`-vv`, or a table-less TTY)
            // renders legacy spinners and hook blocks on the raw output —
            // the same terminal the live rail owns. Close the rail at the
            // base milestone first, exactly like the TUI arm above.
            if timeline.region_live() {
                timeline.finish(&format!(
                    "Base worktree ready in {}",
                    timeline.elapsed_display()
                ));
            }
            create_satellite_worktrees(
                &result,
                &branch_plan,
                &filtered_satellites,
                &bare_params,
                &layout,
                settings,
                &args.skip_hooks,
                args.trust_hooks,
                output,
            )?
        }
    } else {
        result
    };

    // On the rail the header + footer are the record; the result line stays
    // for Plain/Hidden, for the satellite-table path (whose rail closed
    // before the table), and for a redirected stdout, which never saw the
    // rail.
    if !timeline.replaces_stdout_record() || used_tui {
        render_clone_result(&result, &layout, output);
    }

    // While the region is live, stray writes must compose with it: warnings
    // route above the bars, stdout goes through a suspend. The hook helpers
    // and --install receive this region-aware output too.
    let mut region_output;
    let tail_output: &mut dyn Output = if timeline.region_live() {
        region_output =
            RegionOutput::new(timeline.handle(), output.is_quiet(), output.is_verbose());
        &mut region_output
    } else {
        output
    };

    // Remove stale trust entry if cloning to a path that was previously trusted.
    if !args.trust_hooks {
        let mut trust_db = TrustDatabase::load().unwrap_or_default();
        if trust_db.has_explicit_trust(&result.git_dir) {
            trust_db.remove_trust(&result.git_dir);
            if let Err(e) = trust_db.save() {
                tail_output.warning(&format!("Could not remove stale trust entry: {e}"));
            } else {
                tail_output.step("Removed stale trust entry for previous repository at this path");
            }
        }
    }

    // Run hooks and exec only if a worktree was created
    if result.worktree_dir.is_some() {
        // Surface untrusted-hook notices the multi-branch TUI deferred
        // (TuiBridge buffers warnings) BEFORE the post-clone fire: the
        // aggregated multi-worktree copy wins, and a post-clone Deny hit
        // then dedups against it instead of replacing it.
        crate::hooks::trust_skip::flush_pending_notice(&result.git_dir, tail_output);
        // Errors past this point happen with the plan committed and the
        // region live — close the rail as a failure (`fail_rail`), exactly
        // like go/start/remove, instead of leaving Drop to stamp the
        // receipt "interrupted".
        if let Err(e) = run_post_clone_hook(args, &result, tail_output, &mut timeline) {
            return Err(fail_rail(&mut timeline, e));
        }
        // For multi-branch TUI: hooks already ran inside TUI for all worktrees
        // (including the base). For everything else: run post-create hook for
        // the base worktree here.
        if !(is_multi_branch && used_tui)
            && let Err(e) =
                run_post_create_hook(args, &result, &planned_shared, tail_output, &mut timeline)
        {
            return Err(fail_rail(&mut timeline, e));
        }

        if args.install {
            let install_key = StepKey::new(StageId::Install);
            timeline.on_stage(&install_key, StageEvent::Started);
            // The install step prompts (the git-exclude offer) and prints
            // legacy lines — yield the terminal for the whole step via
            // `suspend_for_prompt`, on the raw output. Routing it through
            // the RegionOutput would deadlock: `suspend` holds the Inner
            // lock those writes need, and dialoguer's prompt would fight
            // the region's steady tick for the cursor.
            let handle = timeline.handle();
            match handle.suspend_for_prompt(|| run_clone_install(args, &result, output)) {
                Ok(None) => {
                    timeline.on_stage(&install_key, StageEvent::Completed { annotation: None });
                }
                Ok(Some(skip_reason)) => {
                    // The receipt must not claim "Installed daft" when the
                    // step skipped (daft.yml already present).
                    timeline.on_stage(
                        &install_key,
                        StageEvent::SkippedExpected {
                            reason: skip_reason.to_string(),
                        },
                    );
                }
                Err(e) => return Err(fail_rail(&mut timeline, e)),
            }
        }

        if timeline.region_live() {
            timeline.finish(&format!("Ready in {}", timeline.elapsed_display()));
        }

        let exec_result = crate::exec::run_exec_commands(&args.exec, output);

        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;

        exec_result?;
    } else if result.branch_not_found {
        if timeline.region_live() {
            timeline.finish(&format!("Finished in {}", timeline.elapsed_display()));
        }
        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;
    } else if result.no_checkout {
        if timeline.region_live() {
            timeline.finish(&format!("Ready in {}", timeline.elapsed_display()));
        }
        // No worktree exists for --no-checkout, but the user still ran a
        // clone — drop them into the project root (the directory holding
        // the bare .git) so they can immediately operate on the new repo.
        output.cd_path(&result.parent_dir);
        maybe_show_shell_hint(output)?;
    }

    Ok(())
}

/// Close the live rail as a failure before propagating `e`: an ordinary
/// error exit must read `Failed after <t>` — the wording go/start/remove
/// use — not the Drop safety net's "interrupted", which is Ctrl-C
/// vocabulary. No-op when no region is live.
fn fail_rail(timeline: &mut Timeline, e: anyhow::Error) -> anyhow::Error {
    timeline.abort(&format!("Failed after {}", timeline.elapsed_display()));
    e
}

/// Create satellite worktrees for a multi-branch clone.
///
/// After Phase 4 creates the base worktree, this function creates additional
/// worktrees for each satellite branch in the plan. Returns an updated
/// `CloneResult` with the cd_target adjusted to the branch plan's preference.
#[allow(clippy::too_many_arguments)]
fn create_satellite_worktrees(
    base_result: &clone::CloneResult,
    branch_plan: &crate::core::worktree::branch_source::BranchPlan,
    satellites: &[String],
    bare_params: &clone::BareCloneParams,
    layout: &crate::core::layout::Layout,
    settings: &DaftSettings,
    skip_hooks: &[String],
    trust_hooks: bool,
    output: &mut dyn Output,
) -> Result<clone::CloneResult> {
    // Use parent_dir (the repo root) as the base for path resolution.
    let repo_path = std::fs::canonicalize(&base_result.parent_dir)
        .unwrap_or_else(|_| base_result.parent_dir.clone());

    // cd to a directory where git can find the repo. For contained-classic,
    // .git lives inside the base worktree (e.g., repo/master/.git), so we must
    // cd there. For other layouts, the repo root has .git directly.
    if layout.needs_wrapper() {
        if let Some(ref wt) = base_result.worktree_dir {
            change_directory(wt)?;
        } else {
            change_directory(&repo_path)?;
        }
    } else {
        change_directory(&repo_path)?;
    }

    // Load once, clone per-iteration — loader is stable for the life of
    // this command, no need to re-read per branch. `_global` rather than
    // local even though `change_directory(&repo_path)` above puts us inside
    // the new repo: a freshly-cloned repo's `.git/config` is unlikely to
    // hold daft-specific keys yet, and `_global` keeps this consistent
    // with the orchestrator-thread site below — both pre-clone paths.
    let shared_hooks_config = if skip_hooks_all(skip_hooks) {
        None
    } else {
        Some(crate::core::settings::load_hooks_config_global()?)
    };

    let mut created_count = 0;
    for branch in satellites {
        let worktree_path = if layout.needs_bare() {
            // For bare layouts, worktrees are relative to parent_dir
            std::path::PathBuf::from(branch)
        } else {
            // For non-bare layouts, resolve via template
            let ctx = TemplateContext {
                repo_path: repo_path.clone(),
                repo: base_result.repo_name.clone(),
                branch: branch.clone(),
            };
            match layout.worktree_path(&ctx) {
                Ok(p) => p,
                Err(e) => {
                    output.warning(&format!(
                        "Could not resolve path for branch '{}': {}",
                        branch, e
                    ));
                    continue;
                }
            }
        };

        // For hooks, we need an absolute worktree path (bare layouts use
        // relative paths for git worktree add).
        let abs_worktree_path = if worktree_path.is_relative() {
            repo_path.join(&worktree_path)
        } else {
            worktree_path.clone()
        };

        // Run worktree-pre-create hook
        if let Some(ref hooks_config) = shared_hooks_config
            && let Ok(executor) = HookExecutor::new(hooks_config.clone())
        {
            let mut executor = executor
                .with_job_filter(crate::hooks::yaml_executor::JobFilter::skipping(skip_hooks));
            if trust_hooks {
                if let Some(fp) = get_remote_url_for_git_dir(&base_result.git_dir) {
                    let _ = executor.trust_repository_with_fingerprint(
                        &base_result.git_dir,
                        TrustLevel::Allow,
                        fp,
                    );
                } else {
                    let _ = executor.trust_repository(&base_result.git_dir, TrustLevel::Allow);
                }
            }

            let ctx = HookContext::new(
                HookType::PreCreate,
                "clone",
                &base_result.parent_dir,
                &base_result.git_dir,
                &base_result.remote_name,
                &abs_worktree_path,
                &abs_worktree_path,
                branch,
            )
            .with_new_branch(false);

            let presenter = CliPresenter::auto(&HookOutputConfig::default());
            if let Ok(outcome) = executor.execute(&ctx, output, presenter)
                && !outcome.success
                && !outcome.skipped
            {
                output.warning(&format!(
                    "pre-create hook failed for '{}', skipping",
                    branch
                ));
                continue;
            }
        }

        output.start_spinner(&format!("Creating worktree for '{}'...", branch));

        let satellite_result = {
            let mut sink = OutputSink(output);
            clone::create_satellite_worktree(
                branch,
                &worktree_path,
                &bare_params.remote_name,
                settings.checkout_upstream,
                settings.use_gitoxide,
                &mut sink,
            )
        };

        match satellite_result {
            Ok(_) => {
                output.finish_spinner();
                output.step(&format!("Created worktree for '{}'", branch));
                created_count += 1;

                // Run worktree-post-create hook
                if let Some(ref hooks_config) = shared_hooks_config {
                    // Link shared files before post-create hooks
                    crate::core::shared::render_link_results(
                        &crate::core::shared::link_shared_files_on_create(
                            &abs_worktree_path,
                            &base_result.git_dir,
                            &base_result.parent_dir,
                        ),
                    );

                    if let Ok(executor) = HookExecutor::new(hooks_config.clone()) {
                        let mut executor = executor.with_job_filter(
                            crate::hooks::yaml_executor::JobFilter::skipping(skip_hooks),
                        );
                        if trust_hooks {
                            if let Some(fp) = get_remote_url_for_git_dir(&base_result.git_dir) {
                                let _ = executor.trust_repository_with_fingerprint(
                                    &base_result.git_dir,
                                    TrustLevel::Allow,
                                    fp,
                                );
                            } else {
                                let _ = executor
                                    .trust_repository(&base_result.git_dir, TrustLevel::Allow);
                            }
                        }

                        let ctx = HookContext::new(
                            HookType::PostCreate,
                            "clone",
                            &base_result.parent_dir,
                            &base_result.git_dir,
                            &base_result.remote_name,
                            &abs_worktree_path,
                            &abs_worktree_path,
                            branch,
                        )
                        .with_new_branch(false);

                        let presenter = CliPresenter::auto(&HookOutputConfig::default());
                        let _ = executor.execute(&ctx, output, presenter);
                    }
                }
            }
            Err(e) => {
                output.finish_spinner();
                output.warning(&format!(
                    "Could not create worktree for branch '{}': {}",
                    branch, e
                ));
            }
        }
    }

    // Determine cd_target path
    let cd_target_path = if let Some(ref cd_branch) = branch_plan.cd_target {
        if layout.needs_bare() {
            // For bare layouts, worktrees are direct children of parent_dir
            let target = repo_path.join(cd_branch);
            if target.exists() {
                Some(target)
            } else {
                // Fall back to base worktree or parent_dir
                base_result.cd_target.clone()
            }
        } else {
            let ctx = TemplateContext {
                repo_path: repo_path.clone(),
                repo: base_result.repo_name.clone(),
                branch: cd_branch.clone(),
            };
            match layout.worktree_path(&ctx) {
                Ok(p) if p.exists() => Some(p),
                _ => base_result.cd_target.clone(),
            }
        }
    } else {
        base_result.cd_target.clone()
    };

    // cd to the target
    if let Some(ref target) = cd_target_path {
        change_directory(target)?;
    }

    let worktree_dir = cd_target_path.clone().or(base_result.worktree_dir.clone());

    Ok(clone::CloneResult {
        repo_name: base_result.repo_name.clone(),
        target_branch: branch_plan
            .cd_target
            .clone()
            .unwrap_or_else(|| base_result.target_branch.clone()),
        default_branch: base_result.default_branch.clone(),
        parent_dir: base_result.parent_dir.clone(),
        git_dir: base_result.git_dir.clone(),
        remote_name: base_result.remote_name.clone(),
        repository_url: base_result.repository_url.clone(),
        cd_target: cd_target_path,
        worktree_dir,
        branch_not_found: created_count == 0 && base_result.worktree_dir.is_none(),
        is_empty: base_result.is_empty,
        no_checkout: false,
    })
}

/// TUI table path for creating satellite worktrees during multi-branch clone.
///
/// Shows an `OperationTable` with per-worktree status and hook execution.
/// Falls back to sequential `create_satellite_worktrees()` when stderr is not
/// a TTY or verbose mode is enabled.
#[allow(clippy::too_many_arguments)]
fn create_satellite_worktrees_tui(
    base_result: &clone::CloneResult,
    branch_plan: &BranchPlan,
    satellites: &[String],
    base_branch: Option<&str>,
    bare_params: &clone::BareCloneParams,
    layout: &Layout,
    settings: &DaftSettings,
    skip_hooks: &[String],
    trust_hooks: bool,
    verbosity: u8,
    tui_columns: Option<Vec<Column>>,
    columns_explicit: bool,
) -> Result<clone::CloneResult> {
    use crate::core::worktree::list::Stat;

    // Use parent_dir (the repo root) not git_dir.parent(). For contained-classic,
    // git_dir moves into a branch subdirectory (e.g., repo/master/.git), so
    // git_dir.parent() would give repo/master instead of repo.
    let repo_path = std::fs::canonicalize(&base_result.parent_dir)
        .unwrap_or_else(|_| base_result.parent_dir.clone());

    // cd to a directory where git can find the repo. For contained-classic,
    // .git lives inside the base worktree (e.g., repo/master/.git), so we must
    // cd there. For other layouts, the repo root has .git directly.
    if layout.needs_wrapper() {
        if let Some(ref wt) = base_result.worktree_dir {
            change_directory(wt)?;
        } else {
            change_directory(&repo_path)?;
        }
    } else {
        change_directory(&repo_path)?;
    }

    // Build WorktreeInfo stubs — start with the base/Phase-4 worktree (if any),
    // then add each satellite branch.
    let mut worktree_infos: Vec<WorktreeInfo> = Vec::new();
    let mut satellite_paths: Vec<(String, std::path::PathBuf)> = Vec::new();

    // Add the base worktree as the first row (already created by Phase 4).
    // Use the actual worktree path from Phase 4, not a template guess.
    if let Some(base) = base_branch {
        let base_path = base_result
            .worktree_dir
            .clone()
            .unwrap_or_else(|| repo_path.join(base));
        let mut info = WorktreeInfo::empty(base);
        info.path = Some(base_path);
        worktree_infos.push(info);
    }

    for branch in satellites {
        let worktree_path = if layout.needs_bare() {
            repo_path.join(branch)
        } else {
            let ctx = TemplateContext {
                repo_path: repo_path.clone(),
                repo: base_result.repo_name.clone(),
                branch: branch.clone(),
            };
            match layout.worktree_path(&ctx) {
                Ok(p) => p,
                Err(_) => continue,
            }
        };

        let mut info = WorktreeInfo::empty(branch);
        info.path = Some(worktree_path.clone());
        worktree_infos.push(info);
        satellite_paths.push((branch.clone(), worktree_path));
    }

    let satellite_count = satellite_paths.len();
    if satellite_count == 0 {
        // No satellites resolved — return the base result unchanged.
        return Ok(clone::CloneResult {
            repo_name: base_result.repo_name.clone(),
            target_branch: base_result.target_branch.clone(),
            default_branch: base_result.default_branch.clone(),
            parent_dir: base_result.parent_dir.clone(),
            git_dir: base_result.git_dir.clone(),
            remote_name: base_result.remote_name.clone(),
            repository_url: base_result.repository_url.clone(),
            cd_target: base_result.cd_target.clone(),
            worktree_dir: base_result.worktree_dir.clone(),
            branch_not_found: base_result.branch_not_found,
            is_empty: base_result.is_empty,
            no_checkout: base_result.no_checkout,
        });
    }

    // Phases: Fetch (pre-completed) + Setup (active)
    let phases = vec![OperationPhase::Fetch, OperationPhase::Setup];

    // Use parent of repo_path as cwd so paths render as "repo/branch"
    let cwd = repo_path.parent().unwrap_or(&repo_path).to_path_buf();

    // Create channel for TUI events
    let (tx, rx) = std::sync::mpsc::channel();

    // Shared data for the worker thread
    let shared_remote_name = Arc::new(bare_params.remote_name.clone());
    let shared_checkout_upstream = settings.checkout_upstream;
    let shared_use_gitoxide = settings.use_gitoxide;
    let shared_ownership_strategy = settings.ownership_strategy;
    let shared_default_branch = Arc::new(base_result.default_branch.clone());
    let shared_satellite_paths = Arc::new(satellite_paths);
    let shared_git_dir = Arc::new(base_result.git_dir.clone());
    let shared_parent_dir = Arc::new(base_result.parent_dir.clone());
    let shared_remote_name_for_hooks = Arc::new(base_result.remote_name.clone());
    let shared_trust_hooks = trust_hooks;
    // Load once for the orchestrator thread; the base-post-create site and the
    // per-satellite loop each clone from this. Loader hits global git-config
    // files which are stable for the life of this command. `Option<None>`
    // when `--skip-hooks all` was passed — the gates below (`if let Some`)
    // replace the previous `if !no_hooks` checks. Partial skips still load the
    // config; the per-executor `JobFilter` (built from `shared_skip_hooks`)
    // excludes the requested jobs.
    let shared_hooks_config = if skip_hooks_all(skip_hooks) {
        None
    } else {
        Some(crate::core::settings::load_hooks_config_global()?)
    };
    // Owned copy moved into the orchestrator thread; each executor site builds
    // a `JobFilter` from it.
    let shared_skip_hooks = skip_hooks.to_vec();
    let shared_base_branch = base_branch.map(|s| s.to_string());
    let shared_base_path: Option<std::path::PathBuf> =
        worktree_infos.first().and_then(|info| info.path.clone());
    let orchestrator_handle = std::thread::spawn(move || {
        // Mark Fetch phase as already completed (bare clone happened before TUI)
        let _ = tx.send(DagEvent::TaskStarted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
        });
        let _ = tx.send(DagEvent::TaskCompleted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Ok("cloned".into()),
        });

        // Mark the base worktree as started, run post-create hook, then complete.
        // Phase 4 already created the worktree, so only worktree-post-create fires
        // (not worktree-pre-create).
        if let Some(ref base) = shared_base_branch {
            let _ = tx.send(DagEvent::TaskStarted {
                phase: OperationPhase::Setup,
                branch_name: base.clone(),
            });

            // Run worktree-post-create hook for the base worktree via TuiBridge
            if let Some(ref hooks_cfg) = shared_hooks_config
                && let Ok(executor) = HookExecutor::new(hooks_cfg.clone())
            {
                let mut executor = executor.with_job_filter(
                    crate::hooks::yaml_executor::JobFilter::skipping(&shared_skip_hooks),
                );
                if shared_trust_hooks {
                    if let Some(fp) = get_remote_url_for_git_dir(&shared_git_dir) {
                        let _ = executor.trust_repository_with_fingerprint(
                            &shared_git_dir,
                            TrustLevel::Allow,
                            fp,
                        );
                    } else {
                        let _ = executor.trust_repository(&shared_git_dir, TrustLevel::Allow);
                    }
                }
                let base_worktree_path = shared_base_path
                    .as_ref()
                    .expect("base path must exist when base branch is set");

                // Link shared files before post-create hooks
                crate::core::shared::render_link_results(
                    &crate::core::shared::link_shared_files_on_create(
                        base_worktree_path,
                        &shared_git_dir,
                        &shared_parent_dir,
                    ),
                );

                let mut bridge = TuiBridge::new(executor, tx.clone(), base.clone());

                let ctx = HookContext::new(
                    HookType::PostCreate,
                    "clone",
                    &*shared_parent_dir,
                    &*shared_git_dir,
                    &*shared_remote_name_for_hooks,
                    base_worktree_path,
                    base_worktree_path,
                    base,
                )
                .with_new_branch(false);

                let _ = bridge.run_hook(&ctx);
            }

            // Refresh last_commit + branch_age cells via PostTask patches so
            // the post-clone TUI rows show real values (not empty cells).
            if let Some(ref bp) = shared_base_path {
                spawn_post_clone_refresh(
                    base,
                    bp,
                    shared_use_gitoxide,
                    &shared_default_branch,
                    &shared_remote_name,
                    shared_ownership_strategy,
                    None,
                    &shared_git_dir,
                    &tx,
                );
            }

            let _ = tx.send(DagEvent::TaskCompleted {
                phase: OperationPhase::Setup,
                branch_name: base.clone(),
                status: TaskStatus::Succeeded,
                message: TaskMessage::BaseCreated,
            });
        }

        // Prepare hooks config for per-satellite executor creation
        let hooks_config = shared_hooks_config.clone();

        // Process each satellite branch
        for (branch, worktree_path) in shared_satellite_paths.iter() {
            // Send TaskStarted
            let _ = tx.send(DagEvent::TaskStarted {
                phase: OperationPhase::Setup,
                branch_name: branch.clone(),
            });

            // Run worktree-pre-create hook via TuiBridge
            let mut hook_failed = false;
            if let Some(ref config) = hooks_config {
                match HookExecutor::new(config.clone()) {
                    Err(e) => {
                        let _ = tx.send(DagEvent::TaskCompleted {
                            phase: OperationPhase::Setup,
                            branch_name: branch.clone(),
                            status: TaskStatus::Failed,
                            message: TaskMessage::Failed(format!(
                                "failed to initialize hook executor: {e}"
                            )),
                        });
                        continue;
                    }
                    Ok(executor) => {
                        let mut executor = executor.with_job_filter(
                            crate::hooks::yaml_executor::JobFilter::skipping(&shared_skip_hooks),
                        );
                        if shared_trust_hooks {
                            if let Some(fp) = get_remote_url_for_git_dir(&shared_git_dir) {
                                let _ = executor.trust_repository_with_fingerprint(
                                    &shared_git_dir,
                                    TrustLevel::Allow,
                                    fp,
                                );
                            } else {
                                let _ =
                                    executor.trust_repository(&shared_git_dir, TrustLevel::Allow);
                            }
                        }
                        let mut bridge = TuiBridge::new(executor, tx.clone(), branch.clone());

                        let ctx = HookContext::new(
                            HookType::PreCreate,
                            "clone",
                            &*shared_parent_dir,
                            &*shared_git_dir,
                            &*shared_remote_name_for_hooks,
                            worktree_path,
                            worktree_path,
                            branch,
                        )
                        .with_new_branch(false);

                        if let Ok(outcome) = bridge.run_hook(&ctx)
                            && !outcome.success
                            && !outcome.skipped
                        {
                            hook_failed = true;
                        }
                    }
                }
            }

            if hook_failed {
                let _ = tx.send(DagEvent::TaskCompleted {
                    phase: OperationPhase::Setup,
                    branch_name: branch.clone(),
                    status: TaskStatus::Failed,
                    message: TaskMessage::Failed("pre-create hook failed".into()),
                });
                continue;
            }

            // Create the worktree
            let result = {
                let mut sink = NullSink;
                clone::create_satellite_worktree(
                    branch,
                    worktree_path,
                    &shared_remote_name,
                    shared_checkout_upstream,
                    shared_use_gitoxide,
                    &mut sink,
                )
            };

            match result {
                Ok(_) => {
                    // Run worktree-post-create hook via TuiBridge
                    if let Some(ref config) = hooks_config {
                        match HookExecutor::new(config.clone()) {
                            Err(e) => {
                                let _ = tx.send(DagEvent::TaskCompleted {
                                    phase: OperationPhase::Setup,
                                    branch_name: branch.clone(),
                                    status: TaskStatus::Failed,
                                    message: TaskMessage::Failed(format!(
                                        "failed to initialize hook executor for post-create: {e}"
                                    )),
                                });
                            }
                            Ok(executor) => {
                                let mut executor = executor.with_job_filter(
                                    crate::hooks::yaml_executor::JobFilter::skipping(
                                        &shared_skip_hooks,
                                    ),
                                );
                                if shared_trust_hooks {
                                    if let Some(fp) = get_remote_url_for_git_dir(&shared_git_dir) {
                                        let _ = executor.trust_repository_with_fingerprint(
                                            &shared_git_dir,
                                            TrustLevel::Allow,
                                            fp,
                                        );
                                    } else {
                                        let _ = executor
                                            .trust_repository(&shared_git_dir, TrustLevel::Allow);
                                    }
                                }

                                // Link shared files before post-create hooks
                                crate::core::shared::render_link_results(
                                    &crate::core::shared::link_shared_files_on_create(
                                        worktree_path,
                                        &shared_git_dir,
                                        &shared_parent_dir,
                                    ),
                                );

                                let mut bridge =
                                    TuiBridge::new(executor, tx.clone(), branch.clone());

                                let ctx = HookContext::new(
                                    HookType::PostCreate,
                                    "clone",
                                    &*shared_parent_dir,
                                    &*shared_git_dir,
                                    &*shared_remote_name_for_hooks,
                                    worktree_path,
                                    worktree_path,
                                    branch,
                                )
                                .with_new_branch(false);

                                let _ = bridge.run_hook(&ctx);
                            }
                        }
                    }

                    // Refresh last_commit + branch_age cells via PostTask patches so
                    // the post-clone TUI rows show real values (not empty cells).
                    spawn_post_clone_refresh(
                        branch,
                        worktree_path,
                        shared_use_gitoxide,
                        &shared_default_branch,
                        &shared_remote_name,
                        shared_ownership_strategy,
                        None,
                        &shared_git_dir,
                        &tx,
                    );

                    let _ = tx.send(DagEvent::TaskCompleted {
                        phase: OperationPhase::Setup,
                        branch_name: branch.clone(),
                        status: TaskStatus::Succeeded,
                        message: TaskMessage::Created,
                    });
                }
                Err(e) => {
                    let _ = tx.send(DagEvent::TaskCompleted {
                        phase: OperationPhase::Setup,
                        branch_name: branch.clone(),
                        status: TaskStatus::Failed,
                        message: TaskMessage::Failed(format!("{e}")),
                    });
                }
            }
        }

        let _ = tx.send(DagEvent::AllDone);
    });

    // Run TUI on the main thread
    // Use parent of repo_path so paths render as "repo/branch" not just "branch"
    let display_root = repo_path.parent().unwrap_or(&repo_path).to_path_buf();
    let table = OperationTable::new(
        phases,
        worktree_infos,
        display_root,
        cwd,
        Stat::Summary,
        rx,
        TableConfig {
            columns: tui_columns,
            columns_explicit,
            sort_spec: None,
            extra_rows: 5 + (satellite_count as u16) * 8,
            verbosity,
            pin_default_branch: true,
            partition_by_owner: false, // Clone does not partition by owner.
            // Clone seeds bare `WorktreeInfo::empty(...)` stubs and streams
            // the populated fields via `list_stream::spawn` (see
            // `stream_post_setup_info`). Nothing is finalized at seed time.
            seeded_fields: FieldSet::EMPTY,
        },
        None,
    );
    let completed = table.run()?;

    // Wait for the worker thread to finish
    orchestrator_handle
        .join()
        .map_err(|_| anyhow::anyhow!("Clone worker thread panicked"))?;

    // Print hook summaries (warnings/failures)
    if !completed.hook_summaries.is_empty() {
        eprintln!();
        eprintln!("Hooks:");
        for entry in &completed.hook_summaries {
            let status_word = if entry.warned { "warned" } else { "failed" };
            let exit_str = entry
                .exit_code
                .map(|c| format!("exit {c}"))
                .unwrap_or_else(|| "error".to_string());
            eprintln!(
                "  {}: {} {} ({}, {}ms)",
                entry.branch_name,
                entry.hook_type.hook_name(),
                status_word,
                exit_str,
                entry.duration.as_millis(),
            );
            if let Some(ref output) = entry.output {
                for line in output.lines() {
                    eprintln!("    {line}");
                }
            }
        }
    }

    // Count successes and failures from completed rows
    let failed_rows: Vec<&crate::output::tui::WorktreeRow> = completed
        .rows
        .iter()
        .filter(|r| {
            matches!(
                &r.status,
                crate::output::tui::WorktreeStatus::Done(crate::output::tui::FinalStatus::Failed)
            )
        })
        .collect();
    let total_count = satellite_count;
    let failed_count = failed_rows.len();
    let created_count = total_count - failed_count;

    // Print partial failure summary if any worktrees failed
    if failed_count > 0 {
        eprintln!();
        eprintln!(
            "Created {} of {} worktrees ({} failed)",
            created_count, total_count, failed_count
        );
        for row in &failed_rows {
            let reason = row.failure_reason.as_deref().unwrap_or("unknown error");
            eprintln!("  \u{2717} {}: {}", row.info.name, reason);
        }
    }

    // Determine cd_target path (same logic as sequential path)
    let cd_target_path = if let Some(ref cd_branch) = branch_plan.cd_target {
        if layout.needs_bare() {
            let target = repo_path.join(cd_branch);
            if target.exists() {
                Some(target)
            } else {
                base_result.cd_target.clone()
            }
        } else {
            let ctx = TemplateContext {
                repo_path: repo_path.clone(),
                repo: base_result.repo_name.clone(),
                branch: cd_branch.clone(),
            };
            match layout.worktree_path(&ctx) {
                Ok(p) if p.exists() => Some(p),
                _ => base_result.cd_target.clone(),
            }
        }
    } else {
        base_result.cd_target.clone()
    };

    // cd to the target
    if let Some(ref target) = cd_target_path {
        change_directory(target)?;
    }

    let worktree_dir = cd_target_path.clone().or(base_result.worktree_dir.clone());

    Ok(clone::CloneResult {
        repo_name: base_result.repo_name.clone(),
        target_branch: branch_plan
            .cd_target
            .clone()
            .unwrap_or_else(|| base_result.target_branch.clone()),
        default_branch: base_result.default_branch.clone(),
        parent_dir: base_result.parent_dir.clone(),
        git_dir: base_result.git_dir.clone(),
        remote_name: base_result.remote_name.clone(),
        repository_url: base_result.repository_url.clone(),
        cd_target: cd_target_path,
        worktree_dir,
        branch_not_found: created_count == 0 && base_result.worktree_dir.is_none(),
        is_empty: base_result.is_empty,
        no_checkout: false,
    })
}

fn render_clone_result(
    result: &clone::CloneResult,
    layout: &crate::core::layout::Layout,
    output: &mut dyn Output,
) {
    if result.worktree_dir.is_some() {
        // For bare layouts, the worktree is a subdirectory: "repo/branch".
        // For regular layouts, the repo IS the worktree: just "repo".
        let display = if layout.needs_bare() {
            format!("{}/{}", result.repo_name, result.target_branch)
        } else {
            result.repo_name.clone()
        };
        output.result(&format!("Cloned into '{display}'"));
    } else if result.branch_not_found {
        output.result(&format!(
            "Cloned '{}' (branch '{}' not found, no worktree created)",
            result.repo_name, result.target_branch
        ));
    } else {
        output.result(&format!("Cloned '{}' (bare)", result.repo_name));
    }
}

fn run_post_clone_hook(
    args: &Args,
    result: &clone::CloneResult,
    output: &mut dyn Output,
    timeline: &mut Timeline,
) -> Result<()> {
    let step_key = StepKey::new(StageId::PostCloneHooks);
    if skip_hooks_all(&args.skip_hooks) {
        output.step("Skipping hooks (--skip-hooks all)");
        timeline.on_stage(
            &step_key,
            StageEvent::SkippedAttention {
                reason: "--skip-hooks all".to_string(),
            },
        );
        return Ok(());
    }

    // `_global` rather than `load_hooks_config()`: cwd at this point is
    // path-dependent (single-branch clone leaves cwd at the user's invocation
    // dir; multi-branch sequential leaves it inside the new repo via
    // `create_satellite_worktrees`). The local loader requires being inside a
    // repo and would error in the former case. Repo-local overrides on a
    // freshly-cloned repo are vanishingly rare in practice — a deliberate
    // tradeoff for cwd-tolerance.
    let hooks_config = crate::core::settings::load_hooks_config_global()?;
    // The loaded output settings reach the presenter (previously a default
    // that ignored the user's hooks.output config here); `-v` opts into the
    // full hook block on the rail (#651).
    let mut hook_output_config = hooks_config.output.clone();
    hook_output_config.verbose |= output.is_verbose();
    let mut executor = HookExecutor::new(hooks_config)?.with_job_filter(
        crate::hooks::yaml_executor::JobFilter::skipping(&args.skip_hooks),
    );

    if args.trust_hooks {
        output.step("Trusting repository for hooks (--trust-hooks flag)");
        if let Some(fp) = get_remote_url_for_git_dir(&result.git_dir) {
            executor.trust_repository_with_fingerprint(&result.git_dir, TrustLevel::Allow, fp)?;
        } else {
            executor.trust_repository(&result.git_dir, TrustLevel::Allow)?;
        }
    }

    let worktree_path = result.worktree_dir.as_ref().unwrap();

    let ctx = HookContext::new(
        HookType::PostClone,
        "clone",
        &result.parent_dir,
        &result.git_dir,
        &result.remote_name,
        worktree_path,
        worktree_path,
        &result.target_branch,
    )
    .with_repository_url(&result.repository_url)
    .with_default_branch(&result.default_branch)
    .with_new_branch(false);

    let presenter = if timeline.region_live() {
        CliPresenter::embedded(&hook_output_config, timeline.handle(), step_key.clone())
    } else {
        CliPresenter::auto(&hook_output_config)
    };
    let hook_result = executor.execute(&ctx, output, presenter)?;
    timeline.resolve_hook_step(
        &step_key,
        hook_result.skipped,
        hook_result.skip_reason.as_deref(),
    );

    Ok(())
}

fn run_post_create_hook(
    args: &Args,
    result: &clone::CloneResult,
    planned_shared: &[String],
    output: &mut dyn Output,
    timeline: &mut Timeline,
) -> Result<()> {
    let step_key = StepKey::new(StageId::PostCreateHooks);
    if skip_hooks_all(&args.skip_hooks) {
        timeline.on_stage(
            &step_key,
            StageEvent::SkippedAttention {
                reason: "--skip-hooks all".to_string(),
            },
        );
        return Ok(());
    }

    // `_global` for the same cwd-tolerance reason as `run_post_clone_hook` —
    // see the comment there.
    let hooks_config = crate::core::settings::load_hooks_config_global()?;
    let mut hook_output_config = hooks_config.output.clone();
    hook_output_config.verbose |= output.is_verbose();
    let mut executor = HookExecutor::new(hooks_config)?.with_job_filter(
        crate::hooks::yaml_executor::JobFilter::skipping(&args.skip_hooks),
    );

    if args.trust_hooks {
        if let Some(fp) = get_remote_url_for_git_dir(&result.git_dir) {
            executor.trust_repository_with_fingerprint(&result.git_dir, TrustLevel::Allow, fp)?;
        } else {
            executor.trust_repository(&result.git_dir, TrustLevel::Allow)?;
        }
    }

    let worktree_path = result.worktree_dir.as_ref().unwrap();

    // Link shared files before post-create hooks. Outcomes resolve the
    // planned shared section on the rail (post-clone hooks may have seeded
    // storage); with no live region they fall back to the legacy lines.
    let link_result = crate::core::shared::link_shared_files_on_create(
        worktree_path,
        &result.git_dir,
        &result.parent_dir,
    );
    {
        let mut sink = TimelineSink::new(output, timeline);
        crate::core::shared::report_link_results(&link_result, planned_shared, &mut sink);
    }

    let ctx = HookContext::new(
        HookType::PostCreate,
        "clone",
        &result.parent_dir,
        &result.git_dir,
        &result.remote_name,
        worktree_path,
        worktree_path,
        &result.target_branch,
    )
    .with_new_branch(false);

    let presenter = if timeline.region_live() {
        CliPresenter::embedded(&hook_output_config, timeline.handle(), step_key.clone())
    } else {
        CliPresenter::auto(&hook_output_config)
    };
    let hook_result = executor.execute(&ctx, output, presenter)?;
    timeline.resolve_hook_step(
        &step_key,
        hook_result.skipped,
        hook_result.skip_reason.as_deref(),
    );

    Ok(())
}

/// `--install`: bootstrap a starter daft.yml in the freshly-cloned worktree(s).
///
/// Installs once in the primary (cd-target) worktree — where the shell lands and
/// the `post-clone` hook runs — which writes daft.yml and (on a TTY, or
/// unconditionally with `--git-exclude`) offers the repo-wide
/// `.git/info/exclude` entry. The just-installed daft.yml is then copied
/// byte-for-byte into every other worktree this clone created, so multi-branch
/// clones are symmetric. It is a plain copy rather than a
/// `visitor_propagation::propagate` merge on purpose: the starter is a
/// comment-only skeleton, and propagate()'s YAML parse→serialize roundtrip would
/// strip every comment (turning it into canonical null-filled YAML). The exclude
/// lives in the shared common dir, so it already covers all worktrees — no
/// per-worktree offer is needed.
///
/// If the cloned repository already ships a daft.yml (a tracked team baseline),
/// there is nothing to bootstrap: skip with a note rather than failing.
fn run_clone_install(
    args: &Args,
    result: &clone::CloneResult,
    output: &mut dyn Output,
) -> Result<Option<&'static str>> {
    // The worktree the shell lands in; falls back to the created worktree.
    let Some(primary) = result
        .cd_target
        .as_deref()
        .or(result.worktree_dir.as_deref())
    else {
        // No worktree was created (e.g. requested branch not found) — nothing
        // to install into. --no-checkout is rejected up front.
        return Ok(Some("no worktree to install into"));
    };

    if primary.join("daft.yml").exists() {
        output.info("daft.yml already present in the repository — skipping --install.");
        return Ok(Some("daft.yml already present"));
    }

    // Decide interactivity here (TTY + not under DAFT_TESTING) and pass it in,
    // for the same reason install.rs does: the offer logic must never read the
    // terminal itself.
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::env::var("DAFT_TESTING").is_err();
    let opts = crate::core::install::InstallOptions {
        git_exclude: args.git_exclude,
    };
    crate::core::install::install_at(primary, output, &opts, interactive)?;

    // Copy the just-installed daft.yml into the other worktrees this clone
    // created (covers every layout and both the sequential and `--all-branches`
    // satellite paths). Shared with `daft install` run at a contained-layout
    // container root, which performs the same multi-worktree bootstrap.
    crate::core::install::propagate_starter_to_worktrees(primary, output);

    Ok(None)
}

/// Spawn a streaming-collector run that re-emits `LAST_COMMIT | BRANCH_AGE`
/// for the freshly-created worktree as `PatchSource::PostTask(Setup)`
/// patches. Blocks briefly so the patches land before the accompanying
/// `TaskCompleted` event, keeping the renderer state consistent (no empty
/// "Age"/"Commit" cells in the post-clone TUI).
///
/// Mirrors `sync.rs::spawn_post_task_refresh` but tailored to clone: the
/// only refreshed cells are commit metadata for the just-created worktree,
/// and the operation phase is always `Setup`.
#[allow(clippy::too_many_arguments)]
fn spawn_post_clone_refresh(
    branch_name: &str,
    path: &std::path::Path,
    use_gitoxide: bool,
    base_branch: &str,
    remote_name: &str,
    ownership_strategy: OwnershipStrategy,
    user_email: Option<&str>,
    git_common_dir: &std::path::Path,
    tx: &std::sync::mpsc::Sender<DagEvent>,
) {
    let target = list_stream::CollectorTarget {
        branch_name: branch_name.to_string(),
        path: Some(path.to_path_buf()),
        kind: EntryKind::Worktree,
        is_detached: false,
    };
    let ctx = Arc::new(list_stream::CollectorContext {
        use_gitoxide,
        base_branch: base_branch.to_string(),
        remote_name: remote_name.to_string(),
        ownership_strategy,
        user_email: user_email.map(|s| s.to_string()),
        git_common_dir: git_common_dir.to_path_buf(),
    });
    let handle = list_stream::spawn(
        list_stream::CollectorRequest {
            targets: vec![target],
            fields: FieldSet::LAST_COMMIT | FieldSet::BRANCH_AGE,
            stat: Stat::Summary,
            source: PatchSource::PostTask(OperationPhase::Setup),
            ctx,
        },
        tx.clone(),
    );
    handle.join();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout::BuiltinLayout;

    #[test]
    fn install_with_no_checkout_is_rejected() {
        // --no-checkout creates no worktree, so there's nowhere to install.
        let args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--install",
            "--no-checkout",
        ]);
        assert!(validate_arg_combinations(&args).is_err());
    }

    #[test]
    fn git_exclude_without_install_is_rejected() {
        let args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--git-exclude",
        ]);
        assert!(validate_arg_combinations(&args).is_err());
    }

    #[test]
    fn install_alone_is_ok() {
        let args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--install",
        ]);
        assert!(validate_arg_combinations(&args).is_ok());
    }

    #[test]
    fn install_with_git_exclude_is_ok() {
        let args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--install",
            "--git-exclude",
        ]);
        assert!(validate_arg_combinations(&args).is_ok());
    }

    #[test]
    fn install_implies_trust_hooks() {
        let mut args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--install",
        ]);
        assert!(!args.trust_hooks);
        apply_install_trust(&mut args);
        assert!(args.trust_hooks, "--install should imply --trust-hooks");
    }

    #[test]
    fn install_with_skip_hooks_all_does_not_trust() {
        // --skip-hooks all opts out of hooks entirely, so the trust implication
        // must not fire (and must not create a --trust-hooks/--skip-hooks all
        // conflict).
        let mut args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--install",
            "--skip-hooks",
            "all",
        ]);
        apply_install_trust(&mut args);
        assert!(!args.trust_hooks);
    }

    #[test]
    fn install_with_partial_skip_still_trusts() {
        // A *partial* skip (tag/name) still runs the user's own hooks — just
        // fewer jobs — so the --install trust implication must still fire.
        let mut args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--install",
            "--skip-hooks",
            "tag:heavy",
        ]);
        apply_install_trust(&mut args);
        assert!(
            args.trust_hooks,
            "--install with a partial --skip-hooks should still imply --trust-hooks"
        );
    }

    #[test]
    fn trust_hooks_with_skip_hooks_all_conflicts() {
        let args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--trust-hooks",
            "--skip-hooks",
            "all",
        ]);
        assert!(validate_arg_combinations(&args).is_err());
    }

    #[test]
    fn trust_hooks_with_partial_skip_is_ok() {
        let args = Args::parse_from([
            "git-worktree-clone",
            "https://example.com/r.git",
            "--trust-hooks",
            "--skip-hooks",
            "tag:heavy",
        ]);
        assert!(validate_arg_combinations(&args).is_ok());
    }

    #[test]
    fn clone_without_install_does_not_trust() {
        let mut args = Args::parse_from(["git-worktree-clone", "https://example.com/r.git"]);
        apply_install_trust(&mut args);
        assert!(!args.trust_hooks);
    }

    #[test]
    fn no_checkout_disabled_is_always_ok() {
        for builtin in BuiltinLayout::all() {
            let layout = builtin.to_layout();
            assert!(check_no_checkout_compat(false, &layout).is_ok());
        }
    }

    #[test]
    fn no_checkout_accepts_bare_layouts() {
        for builtin in [BuiltinLayout::Contained, BuiltinLayout::ContainedFlat] {
            let layout = builtin.to_layout();
            assert!(layout.needs_bare(), "{} should be bare", layout.name);
            assert!(
                check_no_checkout_compat(true, &layout).is_ok(),
                "{} should accept --no-checkout",
                layout.name
            );
        }
    }

    #[test]
    fn no_checkout_rejects_non_bare_layouts() {
        for builtin in [
            BuiltinLayout::ContainedClassic,
            BuiltinLayout::Sibling,
            BuiltinLayout::Nested,
            BuiltinLayout::Centralized,
        ] {
            let layout = builtin.to_layout();
            assert!(!layout.needs_bare(), "{} should be non-bare", layout.name);
            let err = check_no_checkout_compat(true, &layout)
                .expect_err(&format!("{} should reject --no-checkout", layout.name));
            let msg = err.to_string();
            assert!(
                msg.contains(&layout.name),
                "error message should name the layout '{}', got: {msg}",
                layout.name
            );
            assert!(
                msg.contains("--no-checkout"),
                "error message should mention --no-checkout, got: {msg}"
            );
            assert!(
                msg.contains("contained") && msg.contains("contained-flat"),
                "error message should suggest bare-using layouts, got: {msg}"
            );
        }
    }

    #[test]
    fn no_checkout_rejects_custom_non_bare_layout() {
        let layout = Layout {
            name: "my-custom".into(),
            template: "{{ repo }}/{{ branch | sanitize }}".into(),
            bare: None,
        };
        assert!(!layout.needs_bare());
        let err = check_no_checkout_compat(true, &layout).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'my-custom'"), "got: {msg}");
        assert!(msg.contains("--no-checkout"), "got: {msg}");
        assert!(
            msg.contains("contained") && msg.contains("contained-flat"),
            "got: {msg}"
        );
    }
}
