use crate::{
    core::{
        global_config::GlobalConfig,
        layout::{
            resolver::{resolve_layout, LayoutResolutionContext, LayoutSource},
            BuiltinLayout, Layout,
        },
        worktree::{checkout, checkout_branch, previous},
        CommandBridge,
    },
    get_current_worktree_path, get_git_common_dir, get_project_root,
    git::{should_show_gitoxide_notice, GitCommand},
    hints::maybe_show_shell_hint,
    hooks::{yaml_config_loader, HookExecutor, HooksConfig, TrustDatabase},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;
use std::io::IsTerminal;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "git-worktree-checkout")]
#[command(version = crate::VERSION)]
#[command(about = "Create a worktree for an existing branch, or a new branch with -b")]
#[command(long_about = r#"
Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

With -b, creates a new branch and a corresponding worktree in a single
operation. The new branch is based on the current branch, or on <base-branch>
if specified. After creating the branch locally, it is pushed to the remote
and upstream tracking is configured.

With --start (or -s), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically, as if 'daft start'
had been called. This can also be enabled permanently with the daft.go.autoStart
git config option.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.
Cannot be combined with -b/--create-branch.

This command can be run from anywhere within the repository. If a worktree
for the specified branch already exists, no new worktree is created; the
working directory is changed to the existing worktree instead.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(
        help = "Name of the branch to check out (or create with -b); use '-' for previous worktree",
        allow_hyphen_values = true
    )]
    branch_name: String,

    #[arg(
        help = "Branch to use as the base for the new branch (only with -b); defaults to the current branch"
    )]
    base_branch_name: Option<String>,

    #[arg(
        short = 'b',
        long = "create-branch",
        help = "Create a new branch instead of checking out an existing one"
    )]
    create_branch: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,

    #[arg(
        short = 's',
        long = "start",
        help = "Create a new worktree if the branch does not exist"
    )]
    start: bool,

    /// Place the worktree at a specific path instead of using the layout template.
    #[arg(short = '@', long, value_name = "PATH")]
    at: Option<PathBuf>,
}

/// Daft-style args for `daft go`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to navigating worktrees, with tailored about text.
#[derive(Parser)]
#[command(name = "daft go")]
#[command(version = crate::VERSION)]
#[command(about = "Open a worktree for an existing branch, or create one with -b")]
#[command(long_about = r#"
Opens a worktree for an existing local or remote branch. The worktree is
placed at the project root level as a sibling to other worktrees, using the
branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

If a worktree for the specified branch already exists, no new worktree is
created; the working directory is changed to the existing worktree instead.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.

With -b, creates a new branch and worktree in a single operation. The new
branch is based on the current branch, or on <base-branch> if specified.
Prefer 'daft start' for creating new branches.

With -s (--start), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically. This can also
be enabled permanently with the daft.go.autoStart git config option.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct GoArgs {
    #[arg(
        help = "Branch to open; use '-' for previous worktree",
        allow_hyphen_values = true
    )]
    branch_name: String,

    #[arg(
        help = "Base branch for -b (defaults to current branch)",
        requires = "create_branch"
    )]
    base_branch_name: Option<String>,

    #[arg(
        short = 'b',
        long = "create-branch",
        help = "Create a new branch (prefer 'daft start' instead)"
    )]
    create_branch: bool,

    #[arg(
        short = 's',
        long = "start",
        help = "Create a new worktree if the branch does not exist"
    )]
    start: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    /// Place the worktree at a specific path instead of using the layout template.
    #[arg(short = '@', long, value_name = "PATH")]
    at: Option<PathBuf>,
}

/// Daft-style args for `daft start`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to creating new branches, without `-b` or `--start`.
#[derive(Parser)]
#[command(name = "daft start")]
#[command(version = crate::VERSION)]
#[command(about = "Create a new branch and worktree")]
#[command(long_about = r#"
Creates a new branch and a corresponding worktree in a single operation. The
worktree is placed at the project root level as a sibling to other worktrees,
using the branch name as the directory name.

The new branch is based on the current branch, or on <base-branch> if
specified. After creating the branch locally, it is pushed to the remote and
upstream tracking is configured (unless disabled via daft.checkoutBranch.push).

This command can be run from anywhere within the repository.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct StartArgs {
    #[arg(help = "Name for the new branch")]
    new_branch_name: String,

    #[arg(help = "Branch to use as the base; defaults to the current branch")]
    base_branch_name: Option<String>,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    /// Place the worktree at a specific path instead of using the layout template.
    #[arg(short = '@', long, value_name = "PATH")]
    at: Option<PathBuf>,
}

/// Entry point for `git-worktree-checkout`.
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-checkout"));
    run_with_args(args)
}

/// Entry point for `daft go`.
pub fn run_go() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-go");
    raw[0] = "daft go".to_string();
    let go_args = GoArgs::parse_from(raw);

    let args = Args {
        branch_name: go_args.branch_name,
        base_branch_name: go_args.base_branch_name,
        create_branch: go_args.create_branch,
        start: go_args.start,
        carry: go_args.carry,
        no_carry: go_args.no_carry,
        remote: go_args.remote,
        no_cd: go_args.no_cd,
        exec: go_args.exec,
        quiet: go_args.quiet,
        verbose: go_args.verbose,
        at: go_args.at,
    };
    run_with_args(args)
}

/// Entry point for `daft start`.
pub fn run_start() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-start");
    raw[0] = "daft start".to_string();
    let start_args = StartArgs::parse_from(raw);

    let args = Args {
        branch_name: start_args.new_branch_name,
        base_branch_name: start_args.base_branch_name,
        create_branch: true,
        start: false,
        carry: start_args.carry,
        no_carry: start_args.no_carry,
        remote: start_args.remote,
        no_cd: start_args.no_cd,
        exec: start_args.exec,
        quiet: start_args.quiet,
        verbose: start_args.verbose,
        at: start_args.at,
    };
    run_with_args(args)
}

fn run_with_args(args: Args) -> Result<()> {
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    // Handle `daft go -` (previous worktree navigation)
    if args.branch_name == "-" {
        if args.create_branch {
            anyhow::bail!("Cannot use '-' with -b/--create-branch");
        }

        let settings = DaftSettings::load()?;
        let autocd = settings.autocd && !args.no_cd;
        let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
        let mut output = CliOutput::new(config);
        return run_go_previous(&mut output);
    }

    // Validate: base_branch_name only valid with -b
    if args.base_branch_name.is_some() && !args.create_branch {
        anyhow::bail!("<BASE_BRANCH_NAME> can only be used with -b/--create-branch");
    }

    let settings = DaftSettings::load()?;

    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
    let mut output = CliOutput::new(config);

    let original_dir = get_current_directory()?;

    // Capture source worktree before the operation (best-effort)
    let source_worktree = get_current_worktree_path().ok();

    let result = if args.create_branch {
        run_create_branch(&args, &settings, &mut output)
    } else {
        match run_checkout(&args, &settings, &mut output) {
            Ok(already_existed) => {
                // --at is invalid when navigating to an existing worktree
                // (it only applies when creating a new one)
                if args.at.is_some() && already_existed {
                    change_directory(&original_dir).ok();
                    anyhow::bail!(
                        "--at cannot be used: worktree already exists for '{}'. \
                         Use 'daft go {}' without --at to navigate to it.",
                        args.branch_name,
                        args.branch_name
                    );
                }
                Ok(())
            }
            Err(checkout::CheckoutError::BranchNotFound {
                ref branch,
                ref remote,
                fetch_failed,
            }) => {
                let auto_start = args.start || settings.go_auto_start;
                if auto_start {
                    change_directory(&original_dir).ok();
                    output.result(&format!(
                        "Branch '{branch}' not found, creating new worktree..."
                    ));
                    run_create_branch(&args, &settings, &mut output)
                } else {
                    change_directory(&original_dir).ok();
                    // --at with a non-existent branch requires --start or autoStart
                    if args.at.is_some() {
                        anyhow::bail!(
                            "--at requires --start (or daft.go.autoStart=true) \
                             when branch '{branch}' does not exist"
                        );
                    }
                    render_branch_not_found_error(branch, remote, fetch_failed, &settings);
                    std::process::exit(1);
                }
            }
            Err(checkout::CheckoutError::Other(e)) => Err(e),
        }
    };

    if let Err(e) = result {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    // Save the source worktree as previous (best-effort, after success)
    if let Some(src) = source_worktree {
        if let Ok(git_dir) = get_git_common_dir() {
            let _ = previous::save(&git_dir, &src);
        }
    }

    Ok(())
}

/// Navigate to the previous worktree (`daft go -`).
fn run_go_previous(output: &mut dyn Output) -> Result<()> {
    let git_dir = get_git_common_dir()?;

    let previous_path = previous::load(&git_dir)?
        .ok_or_else(|| anyhow::anyhow!("No previous worktree to switch to"))?;

    if !previous_path.exists() {
        anyhow::bail!(
            "Previous worktree no longer exists: '{}'",
            previous_path.display()
        );
    }

    // Save current worktree as the new previous before switching
    if let Ok(current) = get_current_worktree_path() {
        let _ = previous::save(&git_dir, &current);
    }

    change_directory(&previous_path)?;

    // Try to get the branch name for display
    let branch_display =
        crate::get_current_branch().unwrap_or_else(|_| previous_path.display().to_string());
    output.result(&format!("Switched to worktree '{branch_display}'"));

    output.cd_path(&previous_path);
    maybe_show_shell_hint(output)?;

    Ok(())
}

/// Resolve the layout for checkout operations.
///
/// Loads the layout from the config chain: repo store > daft.yml > global config > detection > default.
/// Also checks if the resolved layout requires a bare repo and warns if the current repo
/// is not bare.
fn resolve_checkout_layout(
    git: &GitCommand,
    output: &mut dyn Output,
) -> (crate::core::layout::Layout, LayoutSource) {
    let global_config = GlobalConfig::load().unwrap_or_default();
    let git_dir = get_git_common_dir().ok();
    let trust_db = TrustDatabase::load().unwrap_or_default();

    // Load daft.yml layout field from the current worktree (best-effort)
    let yaml_layout: Option<String> = get_current_worktree_path()
        .ok()
        .and_then(|wt| yaml_config_loader::load_merged_config(&wt).ok().flatten())
        .and_then(|cfg| cfg.layout);

    let repo_store_layout = git_dir
        .as_ref()
        .and_then(|d| trust_db.get_layout(d).map(String::from));

    // Run detection when no explicit layout is set.
    let detection = if repo_store_layout.is_none() && yaml_layout.is_none() {
        git_dir
            .as_ref()
            .map(|d| crate::core::layout::detect::detect_layout(d, &global_config))
    } else {
        None
    };

    let (layout, source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None, // checkout doesn't have --layout yet
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection,
    });

    // Graceful degradation: warn if layout needs bare but repo is not bare.
    // Use config_get("core.bare") instead of rev_parse_is_bare_repository()
    // because the latter returns false from inside a linked worktree of a
    // bare repo — which is exactly where users run checkout from.
    let is_bare = git
        .config_get("core.bare")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_lowercase() == "true");
    if layout.needs_bare() && !is_bare {
        output.warning(&format!(
            "Layout '{}' works best with a bare repository. \
             Consider running `daft layout transform` to convert.",
            layout.name
        ));
    }

    (layout, source)
}

/// Decide whether to use the resolved layout as-is or to prompt the user.
///
/// Returns `(layout, should_persist)`:
/// - `should_persist` is true when the layout should be saved to the repo store.
fn interactive_layout_resolution(
    layout: &Layout,
    source: LayoutSource,
    output: &mut dyn Output,
) -> Result<(Layout, bool)> {
    let is_testing = std::env::var("DAFT_TESTING").is_ok();
    let is_interactive = std::io::stdin().is_terminal() && !is_testing;

    match source {
        // Explicitly configured — use as-is, never persist again.
        LayoutSource::Cli
        | LayoutSource::RepoStore
        | LayoutSource::YamlConfig
        | LayoutSource::GlobalConfig => Ok((layout.clone(), false)),

        // Detection found a match — ask the user to confirm (interactive only).
        LayoutSource::Detected => {
            if !is_interactive {
                // Non-interactive: use detected layout and persist it.
                return Ok((layout.clone(), true));
            }

            output.info(&format!("Detected layout: {}", layout.name));

            let confirmed =
                dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
                    .with_prompt("Use this layout?")
                    .default(true)
                    .interact()?;

            if confirmed {
                Ok((layout.clone(), true))
            } else {
                let picked = show_layout_picker(Some(layout))?;
                maybe_consolidate(&picked, output)?;
                Ok((picked, true))
            }
        }

        // Nothing was detected — check if this is a repo with linked worktrees.
        LayoutSource::Unresolved => {
            // Check for linked worktrees (Flow A vs Flow C).
            let git = GitCommand::new(true);
            let has_linked_worktrees = git
                .worktree_list_porcelain()
                .ok()
                .map(|porcelain| {
                    crate::core::layout::detect::parse_worktree_list(&porcelain)
                        .into_iter()
                        .any(|w| !w.is_main)
                })
                .unwrap_or(false);

            if !has_linked_worktrees {
                // Flow A: plain git clone — silently use default and persist.
                return Ok((layout.clone(), true));
            }

            // Flow C: worktrees exist in an unrecognized arrangement.
            if !is_interactive {
                // Non-interactive: use default layout, do not persist.
                return Ok((layout.clone(), false));
            }

            output.info("Found worktrees in unrecognized arrangement.");
            let picked = show_layout_picker(Some(layout))?;
            maybe_consolidate(&picked, output)?;
            Ok((picked, true))
        }
    }
}

/// Show a layout picker and return the selected layout.
fn show_layout_picker(preselect: Option<&Layout>) -> Result<Layout> {
    let global_config = GlobalConfig::load().unwrap_or_default();

    // Build list: builtins first, then custom layouts.
    let mut items: Vec<Layout> = BuiltinLayout::all().iter().map(|b| b.to_layout()).collect();
    items.extend(global_config.custom_layouts());

    // Format each item as "{name:<20}{template}"
    let display: Vec<String> = items
        .iter()
        .map(|l| format!("{:<20}{}", l.name, l.template))
        .collect();

    // Pre-select the provided layout, or fall back to index 0.
    let default_idx = preselect
        .and_then(|pre| items.iter().position(|l| l.name == pre.name))
        .unwrap_or(0);

    let selection = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Select a layout")
        .items(&display)
        .default(default_idx)
        .interact()?;

    Ok(items.remove(selection))
}

/// Ask whether to consolidate existing worktrees to the chosen layout.
fn maybe_consolidate(chosen_layout: &Layout, output: &mut dyn Output) -> Result<()> {
    if !std::io::stdin().is_terminal() || std::env::var("DAFT_TESTING").is_ok() {
        return Ok(());
    }

    let git = GitCommand::new(true);
    let porcelain = git.worktree_list_porcelain()?;
    let worktrees = crate::core::layout::detect::parse_worktree_list(&porcelain);
    let linked_count = worktrees.iter().filter(|wt| !wt.is_main).count();

    if linked_count == 0 {
        return Ok(());
    }

    let prompt = format!(
        "Consolidate {} existing worktree{} to match \"{}\" layout?",
        linked_count,
        if linked_count == 1 { "" } else { "s" },
        chosen_layout.name,
    );

    let consolidate = dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt(prompt)
        .default(false)
        .interact()?;

    if consolidate {
        output.info(&format!(
            "Run `daft layout transform {}` to consolidate.",
            chosen_layout.name,
        ));
    }

    Ok(())
}

/// Returns `Ok(already_existed)` — true if the worktree already existed
/// (navigation only, no creation).
fn run_checkout(
    args: &Args,
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<bool, checkout::CheckoutError> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let (resolved_layout, source) = resolve_checkout_layout(&git, output);
    let (layout, should_persist) = interactive_layout_resolution(&resolved_layout, source, output)?;

    if should_persist {
        if let Ok(git_dir) = get_git_common_dir() {
            let mut trust_db = TrustDatabase::load().unwrap_or_default();
            trust_db.set_layout(&git_dir, layout.name.clone());
            let _ = trust_db.save();
        }
    }

    let params = checkout::CheckoutParams {
        branch_name: args.branch_name.clone(),
        carry: args.carry,
        no_carry: args.no_carry,
        remote: args.remote.clone(),
        remote_name: wt_config.remote_name.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_carry: settings.checkout_carry,
        checkout_upstream: settings.checkout_upstream,
        checkout_fetch: settings.checkout_fetch,
        layout: Some(layout),
        at_path: args.at.clone(),
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Preparing worktree...");
    let (checkout_result, executor) = {
        let mut bridge = CommandBridge::new(output, executor);
        let r = checkout::execute(&params, &git, &project_root, &mut bridge);
        (r, bridge.into_executor())
    };
    output.finish_spinner();
    let result = checkout_result?;

    render_checkout_result(&result, output);

    // Show hooks notice if skipped due to trust
    if result.post_hook_outcome.skipped {
        if let Some(reason) = &result.post_hook_outcome.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(&result.worktree_path, &result.git_dir, output);
            }
        }
    }

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(result.already_existed)
}

fn run_create_branch(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(output.is_quiet()).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    let (resolved_layout, source) = resolve_checkout_layout(&git, output);
    let (layout, should_persist) = interactive_layout_resolution(&resolved_layout, source, output)?;

    if should_persist {
        if let Ok(git_dir) = get_git_common_dir() {
            let mut trust_db = TrustDatabase::load().unwrap_or_default();
            trust_db.set_layout(&git_dir, layout.name.clone());
            let _ = trust_db.save();
        }
    }

    let params = checkout_branch::CheckoutBranchParams {
        new_branch_name: args.branch_name.clone(),
        base_branch_name: args.base_branch_name.clone(),
        carry: args.carry,
        no_carry: args.no_carry,
        remote: args.remote.clone(),
        remote_name: wt_config.remote_name.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_branch_carry: settings.checkout_branch_carry,
        checkout_push: settings.checkout_push,
        checkout_fetch: settings.checkout_fetch,
        layout: Some(layout),
        at_path: args.at.clone(),
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Creating worktree...");
    let (checkout_result, executor) = {
        let mut bridge = CommandBridge::new(output, executor);
        let r = checkout_branch::execute(&params, &git, &project_root, &mut bridge);
        (r, bridge.into_executor())
    };
    output.finish_spinner();
    let result = checkout_result?;

    render_create_result(&result, output);

    // Show hooks notice if skipped due to trust
    if result.post_hook_outcome.skipped {
        if let Some(reason) = &result.post_hook_outcome.skip_reason {
            if reason == "Repository not trusted" {
                executor.check_hooks_notice(&result.worktree_path, &result.git_dir, output);
            }
        }
    }

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(())
}

fn render_branch_not_found_error(
    branch: &str,
    remote: &str,
    fetch_failed: bool,
    settings: &DaftSettings,
) {
    // Section 1: Diagnosis
    if fetch_failed {
        eprintln!(
            "error: Branch '{branch}' not found -- could not reach remote '{remote}' to check"
        );
    } else {
        eprintln!(
            "error: Branch '{branch}' not found -- it does not exist locally or on remote '{remote}'"
        );
    }

    // Section 2: Start suggestion (skip if fetch failed since start would also likely fail)
    if !fetch_failed {
        eprintln!();
        eprintln!("  tip: Use `daft go --start {branch}` or `daft start {branch}` to create it");
    }

    // Section 3: Fuzzy matches
    let git = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let all_branches = checkout::collect_branch_names(&git, remote);
    let suggestions = crate::suggest::find_similar(branch, &all_branches, 5);
    if !suggestions.is_empty() {
        eprintln!();
        if suggestions.len() == 1 {
            eprintln!("  Did you mean this?");
        } else {
            eprintln!("  Did you mean one of these?");
        }
        for s in &suggestions {
            eprintln!("    {s}");
        }
    }
}

fn render_checkout_result(result: &checkout::CheckoutResult, output: &mut dyn Output) {
    if result.already_existed {
        output.result(&format!(
            "Switched to existing worktree '{}'",
            result.branch_name
        ));
    } else {
        output.result(&format!("Prepared worktree '{}'", result.branch_name));
    }
}

fn render_create_result(result: &checkout_branch::CheckoutBranchResult, output: &mut dyn Output) {
    output.result(&format!(
        "Created worktree '{}' from '{}'",
        result.new_branch_name, result.base_branch
    ));
}
