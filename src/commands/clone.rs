use crate::{
    check_dependencies,
    core::{
        global_config::GlobalConfig,
        layout::{
            resolver::{resolve_layout, LayoutResolutionContext},
            Layout, TemplateContext,
        },
        worktree::{
            branch_source::{BranchPlan, BranchSource},
            clone,
            list::WorktreeInfo,
            sync_dag::{DagEvent, OperationPhase, TaskMessage, TaskStatus},
        },
        HookRunner, NullSink, OutputSink, TuiBridge,
    },
    executor::cli_presenter::CliPresenter,
    git::{should_show_gitoxide_notice, GitCommand},
    hints::{maybe_prompt_layout_choice, maybe_show_shell_hint, LayoutPromptResult},
    hooks::{
        get_remote_url_for_git_dir, yaml_config_loader, HookContext, HookExecutor, HookType,
        HooksConfig, TrustDatabase, TrustLevel,
    },
    logging::init_logging,
    output::{
        tui::{
            operation_table::{OperationTable, TableConfig},
            Column,
        },
        CliOutput, Output, OutputConfig,
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
        help = "Perform a bare clone only; do not create any worktree"
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

    #[arg(long = "no-hooks", help = "Do not run any hooks from the repository")]
    no_hooks: bool,

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
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-clone"));

    init_logging(args.verbose >= 2);

    validate_arg_combinations(&args)?;

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
        anyhow::bail!("--no-checkout and --all-branches cannot be used together.\nUse --no-checkout to create only the bare repository, or --all-branches to create worktrees for all branches.");
    }
    if !args.branch.is_empty() && args.all_branches {
        anyhow::bail!("--branch and --all-branches cannot be used together.\nUse --branch to checkout a specific branch, or --all-branches to create worktrees for all branches.");
    }
    if !args.branch.is_empty() && args.no_checkout {
        anyhow::bail!("--branch and --no-checkout cannot be used together.\nUse --branch to checkout a specific branch, or --no-checkout to skip worktree creation.");
    }
    if args.remote.is_some() && args.branch.len() > 1 {
        anyhow::bail!("--remote cannot be used with multiple -b flags.");
    }
    if args.trust_hooks && args.no_hooks {
        anyhow::bail!("--trust-hooks and --no-hooks cannot be used together.");
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

    output.start_spinner("Cloning repository...");
    let bare_result = {
        let mut sink = OutputSink(output);
        clone::clone_bare_phase(&bare_params, &mut sink)
    };
    output.finish_spinner();
    let bare_result = bare_result?;

    // Phase 2: Read daft.yml from the bare repo (if no --layout flag)
    let yaml_layout = if args.layout.is_none() && !bare_result.is_empty {
        match yaml_config_loader::load_config_from_bare(&bare_result.git_dir) {
            Ok(Some(config)) => config.layout,
            Ok(None) => None,
            Err(e) => {
                output.warning(&format!("Could not read daft.yml: {e}"));
                None
            }
        }
    } else {
        None
    };

    // Phase 3: Resolve layout with full context
    let prompted_layout = if args.layout.is_none()
        && yaml_layout.is_none()
        && global_config.defaults.layout.is_none()
    {
        match maybe_prompt_layout_choice(output) {
            LayoutPromptResult::Chosen(layout) => Some(layout),
            LayoutPromptResult::Default => None,
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

    // Report layout decision
    if layout.needs_bare() {
        output.step(&format!(
            "Using layout '{}' (worktrees inside repo)",
            layout.name
        ));
    } else {
        output.step(&format!("Using layout '{}'", layout.name));
    }

    // Resolve branches against the remote
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let remote_branches = git.list_remote_branches(&bare_params.remote_name)?;
    let remote_branch_refs: Vec<&str> = remote_branches.iter().map(|s| s.as_str()).collect();
    let branch_plan = branch_source.resolve(
        &bare_result.default_branch,
        layout.needs_bare(),
        &remote_branch_refs,
    );

    // Warn about missing branches
    for branch in &branch_plan.not_found {
        output.warning(&format!("Branch '{}' not found on remote", branch));
    }

    // Determine if this is a multi-branch clone (Multiple or All source with
    // satellites to create beyond what Phase 4 handles).
    let is_multi_branch = matches!(branch_source, BranchSource::Multiple(_));

    // For multi-branch, override bare_result's target_branch to the base branch
    // so that Phase 4 creates the correct base worktree.
    let mut bare_result = bare_result;
    if is_multi_branch {
        if let Some(ref base) = branch_plan.base {
            bare_result.target_branch = base.clone();
            bare_result.branch_exists = remote_branches.contains(base);
        }
    }

    // After clone_bare_phase, cwd is inside the repo directory. Capture the
    // absolute path now — Phase 4 may change cwd (e.g., contained-classic moves
    // into a branch subdir), making the relative parent_dir unreachable.
    let canonical_parent_dir =
        std::env::current_dir().unwrap_or_else(|_| bare_result.parent_dir.clone());

    // Phase 4: Set up repo in the correct layout
    let result = if layout.needs_bare() {
        output.start_spinner("Setting up worktrees...");
        let r = {
            let mut sink = OutputSink(output);
            clone::setup_bare_worktrees(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r?
    } else if layout.needs_wrapper() {
        output.start_spinner("Setting up wrapped repository...");
        let r = {
            let mut sink = OutputSink(output);
            clone::setup_wrapped_nonbare(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r?
    } else {
        output.start_spinner("Setting up repository...");
        let r = {
            let mut sink = OutputSink(output);
            clone::unbare_and_checkout(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r?
    };

    // Filter out the branch that Phase 4 already created (for bare layouts).
    // For bare layouts, Phase 4 creates a worktree for bare_result.target_branch,
    // but branch_plan.satellites includes it since branch_plan.base is None.
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
            let resolved = ColumnSelection::parse(input, CommandKind::Clone)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
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
        if std::io::IsTerminal::is_terminal(&std::io::stderr()) && args.verbose < 2 {
            used_tui = true;
            create_satellite_worktrees_tui(
                &result,
                &branch_plan,
                &filtered_satellites,
                tui_base_branch.as_deref(),
                &bare_params,
                &layout,
                settings,
                args.no_hooks,
                args.trust_hooks,
                args.verbose,
                tui_columns.clone(),
                columns_explicit,
            )?
        } else {
            create_satellite_worktrees(
                &result,
                &branch_plan,
                &filtered_satellites,
                &bare_params,
                &layout,
                settings,
                args.no_hooks,
                args.trust_hooks,
                output,
            )?
        }
    } else {
        result
    };

    render_clone_result(&result, &layout, output);

    // Remove stale trust entry if cloning to a path that was previously trusted.
    if !args.trust_hooks {
        let mut trust_db = TrustDatabase::load().unwrap_or_default();
        if trust_db.has_explicit_trust(&result.git_dir) {
            trust_db.remove_trust(&result.git_dir);
            if let Err(e) = trust_db.save() {
                output.warning(&format!("Could not remove stale trust entry: {e}"));
            } else {
                output.step("Removed stale trust entry for previous repository at this path");
            }
        }
    }

    // Run hooks and exec only if a worktree was created
    if result.worktree_dir.is_some() {
        run_post_clone_hook(args, &result, output)?;
        // For multi-branch TUI: hooks already ran inside TUI for all worktrees
        // (including the base). For everything else: run post-create hook for
        // the base worktree here.
        if !(is_multi_branch && used_tui) {
            run_post_create_hook(args, &result, output)?;
        }

        let exec_result = crate::exec::run_exec_commands(&args.exec, output);

        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;

        exec_result?;
    } else if result.branch_not_found {
        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;
    }

    Ok(())
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
    no_hooks: bool,
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
        if !no_hooks {
            let hooks_config = HooksConfig::default();
            if let Ok(mut executor) = HookExecutor::new(hooks_config) {
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
                if let Ok(outcome) = executor.execute(&ctx, output, presenter) {
                    if !outcome.success && !outcome.skipped {
                        output.warning(&format!(
                            "pre-create hook failed for '{}', skipping",
                            branch
                        ));
                        continue;
                    }
                }
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
                if !no_hooks {
                    let hooks_config = HooksConfig::default();
                    if let Ok(mut executor) = HookExecutor::new(hooks_config) {
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
    no_hooks: bool,
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
    let shared_satellite_paths = Arc::new(satellite_paths);
    let shared_git_dir = Arc::new(base_result.git_dir.clone());
    let shared_parent_dir = Arc::new(base_result.parent_dir.clone());
    let shared_remote_name_for_hooks = Arc::new(base_result.remote_name.clone());
    let shared_no_hooks = no_hooks;
    let shared_trust_hooks = trust_hooks;
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
            updated_info: None,
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
            if !shared_no_hooks {
                let hooks_cfg = HooksConfig::default();
                if let Ok(mut executor) = HookExecutor::new(hooks_cfg) {
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
            }

            // Populate commit metadata for the base worktree
            let base_updated = shared_base_path.as_ref().map(|bp| {
                let mut info = WorktreeInfo::empty(base);
                info.path = Some(bp.clone());
                if let Ok((ts, hash, subj)) = crate::git::oxide::get_commit_metadata_for_head(bp) {
                    info.last_commit_timestamp = Some(ts);
                    info.last_commit_hash = Some(hash);
                    info.last_commit_subject = subj;
                }
                info.branch_creation_timestamp = info.last_commit_timestamp;
                Box::new(info)
            });

            let _ = tx.send(DagEvent::TaskCompleted {
                phase: OperationPhase::Setup,
                branch_name: base.clone(),
                status: TaskStatus::Succeeded,
                message: TaskMessage::BaseCreated,
                updated_info: base_updated,
            });
        }

        // Prepare hooks config for per-satellite executor creation
        let hooks_config = if !shared_no_hooks {
            Some(HooksConfig::default())
        } else {
            None
        };

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
                            updated_info: None,
                        });
                        continue;
                    }
                    Ok(mut executor) => {
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

                        if let Ok(outcome) = bridge.run_hook(&ctx) {
                            if !outcome.success && !outcome.skipped {
                                hook_failed = true;
                            }
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
                    updated_info: None,
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
                                    updated_info: None,
                                });
                            }
                            Ok(mut executor) => {
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

                    // Populate commit metadata for the newly created worktree
                    let mut updated = WorktreeInfo::empty(branch);
                    updated.path = Some(worktree_path.clone());
                    if let Ok((ts, hash, subj)) =
                        crate::git::oxide::get_commit_metadata_for_head(worktree_path)
                    {
                        updated.last_commit_timestamp = Some(ts);
                        updated.last_commit_hash = Some(hash);
                        updated.last_commit_subject = subj;
                    }
                    updated.branch_creation_timestamp = updated.last_commit_timestamp;

                    let _ = tx.send(DagEvent::TaskCompleted {
                        phase: OperationPhase::Setup,
                        branch_name: branch.clone(),
                        status: TaskStatus::Succeeded,
                        message: TaskMessage::Created,
                        updated_info: Some(Box::new(updated)),
                    });
                }
                Err(e) => {
                    let _ = tx.send(DagEvent::TaskCompleted {
                        phase: OperationPhase::Setup,
                        branch_name: branch.clone(),
                        status: TaskStatus::Failed,
                        message: TaskMessage::Failed(format!("{e}")),
                        updated_info: None,
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
                entry.hook_type.filename(),
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
) -> Result<()> {
    if args.no_hooks {
        output.step("Skipping hooks (--no-hooks flag)");
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

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

    let presenter = CliPresenter::auto(&HookOutputConfig::default());
    let hook_result = executor.execute(&ctx, output, presenter)?;

    if hook_result.skipped {
        if let Some(reason) = &hook_result.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(worktree_path, &result.git_dir, output);
            }
        }
    }

    Ok(())
}

fn run_post_create_hook(
    args: &Args,
    result: &clone::CloneResult,
    output: &mut dyn Output,
) -> Result<()> {
    if args.no_hooks {
        return Ok(());
    }

    let hooks_config = HooksConfig::default();
    let mut executor = HookExecutor::new(hooks_config)?;

    if args.trust_hooks {
        if let Some(fp) = get_remote_url_for_git_dir(&result.git_dir) {
            executor.trust_repository_with_fingerprint(&result.git_dir, TrustLevel::Allow, fp)?;
        } else {
            executor.trust_repository(&result.git_dir, TrustLevel::Allow)?;
        }
    }

    let worktree_path = result.worktree_dir.as_ref().unwrap();

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

    let presenter = CliPresenter::auto(&HookOutputConfig::default());
    executor.execute(&ctx, output, presenter)?;

    Ok(())
}
