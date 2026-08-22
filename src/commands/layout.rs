use crate::{
    core::{
        CommandBridge,
        global_config::GlobalConfig,
        layout::{
            BuiltinLayout, DEFAULT_LAYOUT, Layout,
            detect::detect_layout,
            resolver::{DetectionResult, LayoutResolutionContext, LayoutSource, resolve_layout},
            transform,
        },
    },
    get_current_worktree_path, get_git_common_dir,
    git::GitCommand,
    hooks::{HookExecutor, TrustDatabase, yaml_config_loader},
    is_git_repository,
    output::{
        CliOutput, Output, OutputConfig,
        emit::{self, Cell, EmitArgs, EmitPayload, Table},
    },
    settings::DaftSettings,
    styles::{self, bold, dim, dim_underline},
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use tabled::{
    builder::Builder,
    settings::{Padding, Style, object::Columns},
};

#[derive(Parser)]
#[command(name = "layout")]
#[command(about = "Manage worktree layouts")]
#[command(long_about = r#"
Manage worktree layouts for daft repositories.

Layouts control where worktrees are placed relative to the bare repository.
Built-in layouts:

  contained           Worktrees inside the repo directory (bare required)
  contained-classic   Like contained but the main working tree is a regular clone
  contained-flat      Like contained but branch slashes flattened to dashes
  sibling             Worktrees next to the repo directory (default)
  nested              Worktrees in a hidden subdirectory
  centralized         Worktrees in a global ~/worktrees/ directory

Use `daft layout list` to see all available layouts including custom ones
defined in your global config (~/.config/daft/config.toml).

Use `daft layout show` to see the resolved layout for the current repo.

Use `daft layout transform <layout>` to convert a repo between layouts.

Use `daft layout default` to view or change the global default layout.
"#)]
pub struct LayoutArgs {
    #[command(subcommand)]
    command: Option<LayoutCommand>,
}

#[derive(Subcommand)]
enum LayoutCommand {
    /// List all available layouts
    List {
        #[command(flatten)]
        emit: EmitArgs,
    },
    /// Show the resolved layout for the current repo
    Show(ShowArgs),
    /// Transform the current repo to a different layout
    Transform(TransformArgs),
    /// View or set the global default layout
    Default(DefaultArgs),
    /// Clear the stored layout for a repo
    Reset(ResetArgs),
}

#[derive(Args)]
struct ShowArgs {
    /// Path to a git repository (defaults to current directory)
    path: Option<PathBuf>,
}

#[derive(Args)]
struct TransformArgs {
    /// Target layout name or template
    layout: String,
    /// Show plan without executing
    #[arg(long)]
    dry_run: bool,
    /// Directory name for a detached main working tree, when the target layout nests it
    #[arg(long = "as", value_name = "DIR", conflicts_with = "pivot")]
    as_dir: Option<String>,
    /// Worktree that becomes the main working tree when a bare repo gains one
    #[arg(long = "pivot", value_name = "BRANCH")]
    pivot: Option<String>,
    /// Also relocate this non-conforming worktree (repeatable)
    #[arg(long = "include", value_name = "BRANCH")]
    include: Vec<String>,
    /// Relocate all non-conforming worktrees
    #[arg(long)]
    include_all: bool,
    /// Auto-accept interactive prompts
    #[arg(short = 'y', long = "yes")]
    yes: bool,
    /// Operate quietly; suppress progress reporting
    #[arg(short, long)]
    quiet: bool,
    /// Show detailed hook execution
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct ResetArgs {
    /// Path to a git repository (defaults to current directory)
    path: Option<PathBuf>,
}

#[derive(Args)]
struct DefaultArgs {
    /// Layout name or template to set as the global default
    #[arg(conflicts_with = "reset")]
    layout: Option<String>,

    /// Remove the global default, reverting to built-in (sibling)
    #[arg(long)]
    reset: bool,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = crate::cli::argv().iter().skip(1).cloned().collect();
    let layout_args = LayoutArgs::parse_from(args);
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    match layout_args.command {
        Some(LayoutCommand::List { emit }) => cmd_list(&emit, &mut output),
        Some(LayoutCommand::Show(show_args)) => cmd_show(&show_args, &mut output),
        None => cmd_show(&ShowArgs { path: None }, &mut output),
        Some(LayoutCommand::Transform(transform_args)) => {
            let mut transform_output = CliOutput::new(OutputConfig::new(
                transform_args.quiet,
                transform_args.verbose,
            ));
            cmd_transform(&transform_args, &mut transform_output)
        }
        Some(LayoutCommand::Default(default_args)) => cmd_default(&default_args, &mut output),
        Some(LayoutCommand::Reset(reset_args)) => cmd_reset(&reset_args, &mut output),
    }
}

// ── layout list ────────────────────────────────────────────────────────────

fn cmd_list(emit_args: &EmitArgs, output: &mut dyn Output) -> Result<()> {
    let global_config = GlobalConfig::load().unwrap_or_default();
    let default_layout_name = global_config
        .defaults
        .layout
        .as_deref()
        .unwrap_or(DEFAULT_LAYOUT.name());

    let use_color = styles::colors_enabled();

    // Resolve current repo layout (best-effort, for "selected" indicator)
    let current_layout_name = resolve_current_layout_name(&global_config);

    // Collect all layouts: built-ins first, then custom
    let mut layouts: Vec<LayoutRow> = Vec::new();

    for builtin in BuiltinLayout::all() {
        let is_default = builtin.name() == default_layout_name;
        let is_selected = current_layout_name.as_deref() == Some(builtin.name());
        layouts.push(LayoutRow {
            name: builtin.name().to_string(),
            template: builtin.to_layout().template,
            is_default,
            is_selected,
        });
    }

    for (name, custom) in &global_config.layouts {
        if BuiltinLayout::from_name(name).is_some() {
            if let Some(row) = layouts.iter_mut().find(|r| r.name == *name) {
                row.template = custom.template.clone();
            }
            continue;
        }
        let is_default = name == default_layout_name;
        let is_selected = current_layout_name.as_deref() == Some(name.as_str());
        layouts.push(LayoutRow {
            name: name.clone(),
            template: custom.template.clone(),
            is_default,
            is_selected,
        });
    }

    if emit_args.is_structured() {
        let table = build_layout_table(&layouts);
        return emit::emit_and_handle(
            "layout list",
            EmitPayload::Tabular(table),
            emit_args,
            &mut std::io::stdout(),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    }

    // Build table with tabled (matches list command style)
    let mut builder = Builder::new();

    // Header: dim+underline (same as list command)
    builder.push_record(if use_color {
        vec![
            String::new(), // annotation column
            dim_underline("Layout"),
            dim_underline("Template"),
        ]
    } else {
        vec![String::new(), "Layout".into(), "Template".into()]
    });

    for row in &layouts {
        let annotation = if row.is_selected {
            if use_color {
                styles::green(styles::CURRENT_WORKTREE_SYMBOL)
            } else {
                styles::CURRENT_WORKTREE_SYMBOL.to_string()
            }
        } else {
            String::new()
        };

        let name_display = if use_color {
            let styled = if row.is_selected {
                bold(&row.name)
            } else {
                row.name.clone()
            };
            if row.is_default {
                format!("{styled} {}", dim("(default)"))
            } else {
                styled
            }
        } else if row.is_default {
            format!("{} (default)", row.name)
        } else {
            row.name.clone()
        };

        let template_display = if use_color {
            highlight_template(&row.template)
        } else {
            row.template.clone()
        };

        builder.push_record(vec![annotation, name_display, template_display]);
    }

    let mut table = builder.build();
    table.with(Style::blank());
    table.modify(Columns::first(), Padding::new(1, 0, 0, 0));

    output.info(&table.to_string());

    Ok(())
}

/// Build a structured `Table` payload from layout rows.
fn build_layout_table(layouts: &[LayoutRow]) -> Table {
    let mut table = Table::new(["name", "template", "is_default", "is_selected"]);
    for row in layouts {
        table = table.row([
            Cell::str(&row.name),
            Cell::str(&row.template),
            Cell::bool(row.is_default),
            Cell::bool(row.is_selected),
        ]);
    }
    table
}

/// Syntax-highlight a template string using the shared [`SYNTAX`] palette.
///
/// Uses semantic roles from the palette:
/// - `keyword`: delimiters `{{` `}}` (frames expressions)
/// - `identifier`: variable names `repo_path`, `repo`, `branch`
/// - `punctuation`: pipe operator `|`
/// - `string`: filter names `sanitize` (value-producing)
/// - Default: literal path text `/`, `.worktrees/`, `~/`
fn highlight_template(template: &str) -> String {
    use crate::styles::{RESET, SYNTAX};

    let mut result = String::new();
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        // Literal path text — default color (no styling)
        result.push_str(&rest[..start]);

        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let expr = &after_open[..end];
            if let Some(pipe_pos) = expr.find('|') {
                let var = expr[..pipe_pos].trim();
                let filter = expr[pipe_pos + 1..].trim();
                result.push_str(&format!(
                    "{kw}{{{{{RESET} {id}{var}{RESET} {p}|{RESET} {s}{filter}{RESET} {kw}}}}}{RESET}",
                    kw = SYNTAX.keyword,
                    id = SYNTAX.identifier,
                    p = SYNTAX.punctuation,
                    s = SYNTAX.string,
                ));
            } else {
                let var = expr.trim();
                result.push_str(&format!(
                    "{kw}{{{{{RESET} {id}{var}{RESET} {kw}}}}}{RESET}",
                    kw = SYNTAX.keyword,
                    id = SYNTAX.identifier,
                ));
            }
            rest = &after_open[end + 2..];
        } else {
            result.push_str(&rest[start..]);
            rest = "";
        }
    }
    // Remaining literal path text — default color
    result.push_str(rest);
    result
}

/// Try to resolve the current repo's layout name (best-effort).
fn resolve_current_layout_name(global_config: &GlobalConfig) -> Option<String> {
    let git_dir = get_git_common_dir().ok()?;
    let trust_db = TrustDatabase::load().ok()?;

    let yaml_layout: Option<String> = get_current_worktree_path()
        .ok()
        .and_then(|wt| yaml_config_loader::load_merged_config(&wt).ok().flatten())
        .and_then(|cfg| cfg.layout);

    let repo_store_layout = trust_db.get_layout(&git_dir).map(String::from);

    let (layout, _) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config,
        detection: None,
    });

    Some(layout.name)
}

struct LayoutRow {
    name: String,
    template: String,
    is_default: bool,
    is_selected: bool,
}

// ── layout default ─────────────────────────────────────────────────────────

fn cmd_default(args: &DefaultArgs, output: &mut dyn Output) -> Result<()> {
    if args.reset {
        GlobalConfig::remove_default_layout()?;
        output.result("Default layout reset to built-in (sibling).");
        return Ok(());
    }

    if let Some(ref layout_name) = args.layout {
        if layout_name.is_empty() {
            anyhow::bail!("Layout name cannot be empty.");
        }
        GlobalConfig::set_default_layout(layout_name)?;
        output.result(&format!("Default layout set to '{layout_name}'."));
        return Ok(());
    }

    // Show current default
    let global_config = GlobalConfig::load().unwrap_or_default();
    let use_color = styles::colors_enabled();

    let (layout, source) = match global_config.defaults.layout {
        Some(_) => (global_config.default_layout().unwrap(), "global config"),
        None => (DEFAULT_LAYOUT.to_layout(), "default"),
    };
    let (name, template) = (layout.name, layout.template);

    let template_display = if use_color {
        highlight_template(&template)
    } else {
        template
    };

    output.info(&format!(
        "{} {} {}",
        bold(&name),
        template_display,
        dim(&format!("({source})"))
    ));

    Ok(())
}

// ── layout show ────────────────────────────────────────────────────────────

/// RAII guard that changes the current working directory and restores it on drop.
struct CwdGuard {
    original: Option<PathBuf>,
}

impl CwdGuard {
    fn new(target: &Path) -> Result<Self> {
        let original = std::env::current_dir().ok();
        std::env::set_current_dir(target)
            .with_context(|| format!("Cannot cd to {}", target.display()))?;
        Ok(Self { original })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        if let Some(ref old) = self.original {
            let _ = std::env::set_current_dir(old);
        }
    }
}

fn cmd_show(args: &ShowArgs, output: &mut dyn Output) -> Result<()> {
    // If a path was provided, switch CWD so all subsequent git calls work there.
    let _guard = if let Some(ref path) = args.path {
        Some(CwdGuard::new(path)?)
    } else {
        None
    };

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository. Run this command from within a repo.");
    }

    let global_config = GlobalConfig::load().unwrap_or_default();
    let git_dir = get_git_common_dir()?;
    let trust_db = TrustDatabase::load().unwrap_or_default();

    // Load daft.yml layout field
    let yaml_layout: Option<String> = get_current_worktree_path()
        .ok()
        .and_then(|wt| yaml_config_loader::load_merged_config(&wt).ok().flatten())
        .and_then(|cfg| cfg.layout);

    let repo_store_layout = trust_db.get_layout(&git_dir).map(String::from);

    // Run detection when no explicit layout is configured.
    let detection = if repo_store_layout.is_none() && yaml_layout.is_none() {
        Some(detect_layout(&git_dir, &global_config))
    } else {
        None
    };

    let (layout, source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection: detection.clone(),
    });

    let use_color = styles::colors_enabled();

    // Handle Unresolved source with differentiated messages based on detection result.
    if source == LayoutSource::Unresolved {
        let message = match detection {
            None | Some(DetectionResult::NoWorktrees) => {
                "No layout (no worktrees to detect from)".to_string()
            }
            Some(DetectionResult::NoMatch) => {
                "Unknown layout (worktrees don't match any known template)".to_string()
            }
            Some(DetectionResult::Ambiguous) => {
                "Unknown layout (worktrees match multiple templates)".to_string()
            }
            Some(DetectionResult::Detected(_)) => {
                // Should not happen: Detected is handled by the resolver before Unresolved.
                "Unknown layout".to_string()
            }
        };
        output.info(&if use_color { dim(&message) } else { message });
        return Ok(());
    }

    let source_display = match source {
        LayoutSource::Cli => "CLI flag",
        LayoutSource::RepoStore => "repo setting",
        LayoutSource::YamlConfig => "daft.yml",
        LayoutSource::GlobalConfig => "global config",
        LayoutSource::Detected => "detected",
        LayoutSource::Unresolved => unreachable!("handled above"),
    };

    let template_display = if use_color {
        highlight_template(&layout.template)
    } else {
        layout.template.clone()
    };

    // One-line output: <layout> <template> (<source>)
    output.info(&format!(
        "{} {} {}",
        bold(&layout.name),
        template_display,
        dim(&format!("({source_display})"))
    ));

    Ok(())
}

// ── layout reset ──────────────────────────────────────────────────────────

fn cmd_reset(args: &ResetArgs, output: &mut dyn Output) -> Result<()> {
    let _guard = if let Some(ref path) = args.path {
        let resolved = std::fs::canonicalize(path)
            .with_context(|| format!("Cannot resolve path: {}", path.display()))?;
        Some(CwdGuard::new(&resolved)?)
    } else {
        None
    };

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository. Run this command from within a repo.");
    }

    let git_dir = get_git_common_dir()?;
    let mut cleared = false;
    TrustDatabase::update_if(|db| {
        cleared = db.remove_layout(&git_dir);
        Ok(cleared)
    })?;

    if cleared {
        output.info("Layout setting cleared for this repository.");
    } else {
        output.info("No layout setting found for this repository.");
    }

    Ok(())
}

// ── layout transform ───────────────────────────────────────────────────────

fn cmd_transform(args: &TransformArgs, output: &mut dyn Output) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository. Run this command from within a repo.");
    }

    // In-repo already (guarded above), so local-or-global resolves the repo
    // and reads its config — honoring a repo-local `daft.gitoxide = false`
    // opt-out on this layout-mutating command (#733).
    let settings = DaftSettings::load_local_or_global()?;
    let global_config = GlobalConfig::load().unwrap_or_default();
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

    // Remember which branch the user is on, for CD target after transform
    let user_branch = crate::get_current_branch().ok();

    // Resolve target layout ("default" is reserved — resolves to the system default)
    let target_layout = if args.layout == "default" {
        global_config
            .default_layout()
            .unwrap_or_else(|| DEFAULT_LAYOUT.to_layout())
    } else {
        match global_config.resolve_layout_by_name(&args.layout) {
            Some(layout) => layout,
            None => Layout {
                name: args.layout.clone(),
                template: args.layout.clone(),
                bare: None,
            },
        }
    };

    // Detect default branch
    let default_branch = crate::remote::get_default_branch_local(
        &get_git_common_dir()?,
        &settings.remote,
        settings.use_gitoxide,
    )
    .unwrap_or_else(|_| "main".to_string());

    // Read current state
    let mut source = transform::read_source_state(&git, &default_branch)?;

    // For wrapped non-bare layouts (contained-classic), project_root from
    // read_source_state is the clone subdir (e.g., repo/master/). The actual
    // wrapper directory is one level up. Detect this by checking the current
    // layout stored in repos.json.
    if let Ok(db) = TrustDatabase::load()
        && let Some(current_layout_name) = db.get_layout(&source.git_dir)
        && let Some(current_layout) = global_config.resolve_layout_by_name(current_layout_name)
        && current_layout.needs_wrapper()
    {
        // The main working tree sits at `<wrapper>/<template path>`, where the
        // template path can have several components (`task/local-docker`) and
        // was evaluated for whatever branch the clone was on when it was
        // placed — the branch it is on now, or the default branch it was
        // cloned on and has since switched away from. Match the directory
        // against each before falling back to one level up.
        let main_path = source.project_root.clone();
        let root_branch = source
            .worktrees
            .iter()
            .find(|wt| wt.is_root)
            .and_then(|wt| wt.branch.clone());
        let candidates: Vec<&str> = root_branch
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(default_branch.as_str()))
            .collect();
        let wrapper = candidates
            .iter()
            .find_map(|name| {
                let rel =
                    transform::compute_target_worktree_path(&current_layout, Path::new("/"), name)
                        .ok()?;
                let rel = rel.strip_prefix("/").ok()?.to_path_buf();
                if rel.as_os_str().is_empty() || !main_path.ends_with(&rel) {
                    return None;
                }
                main_path
                    .ancestors()
                    .nth(rel.components().count())
                    .map(Path::to_path_buf)
            })
            .or_else(|| main_path.parent().map(Path::to_path_buf));
        if let Some(wrapper) = wrapper {
            source.project_root = wrapper;
        }
    }

    // Prompts need a terminal and a human; the YAML harness (`DAFT_TESTING`)
    // has neither, and a dry run must never block on one. Without a terminal
    // every question has a flag twin, and the refusal names it.
    let interactive = std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::env::var("DAFT_TESTING").is_err()
        && !args.dry_run;
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let mut blockers: Vec<transform::Blocker> = Vec::new();
    let mut probed: Vec<PathBuf> = Vec::new();
    let mut probe_role_change =
        |wts: Vec<(PathBuf, Option<String>)>, blockers: &mut Vec<transform::Blocker>| {
            let fresh: Vec<(PathBuf, Option<String>)> = wts
                .into_iter()
                .filter(|(p, _)| !probed.contains(p))
                .collect();
            probed.extend(fresh.iter().map(|(p, _)| p.clone()));
            blockers.extend(transform::preflight::role_change_blockers(&fresh));
        };

    // ── The two questions daft cannot answer alone ──────────────────────
    let mut over = transform::PivotOverride {
        branch: args.pivot.clone(),
        dirname: args.as_dir.clone(),
    };
    if let Some(name) = &over.dirname {
        if let Err(why) = crate::core::worktree::sandbox::validate_dirname(name) {
            anyhow::bail!("--as {name}: {why}");
        }
        if source.project_root.join(name).exists() {
            anyhow::bail!(
                "--as {name}: {} already exists",
                source.project_root.join(name).display()
            );
        }
    }
    if over.branch.is_some() && !source.is_bare {
        anyhow::bail!(
            "--pivot chooses which worktree takes the repository root when a bare \
             repository gains a main working tree; this repository already has one."
        );
    }
    let bare_flips = source.is_bare != target_layout.needs_bare();
    let situation = transform::root_situation(&target_layout, &source);

    // Probe for in-progress operations *before* asking anything: asking the
    // user to name a directory and then refusing because of a rebase is the
    // contradiction this ordering exists to avoid.
    if bare_flips {
        let early: Vec<(PathBuf, Option<String>)> = match &situation {
            transform::RootSituation::AmbiguousPivot { candidates } if over.branch.is_none() => {
                candidates
                    .iter()
                    .map(|&i| {
                        (
                            source.worktrees[i].path.clone(),
                            source.worktrees[i].branch.clone(),
                        )
                    })
                    .collect()
            }
            _ if !source.is_bare => source
                .worktrees
                .iter()
                .filter(|wt| wt.is_root)
                .map(|wt| (wt.path.clone(), wt.branch.clone()))
                .collect(),
            _ => Vec::new(),
        };
        probe_role_change(early, &mut blockers);
    }

    match &situation {
        transform::RootSituation::DetachedMain { head } => {
            let commit = head.clone().unwrap_or_default();
            let derived = crate::core::worktree::sandbox::derived_dirname(&commit);
            match transform::decide::decide_dirname(
                over.dirname.as_deref(),
                true,
                &derived,
                interactive,
                args.yes,
            ) {
                transform::DirnameDecision::Settled => {}
                transform::DirnameDecision::Use(name) => over.dirname = Some(name),
                transform::DirnameDecision::Ask(default) if blockers.is_empty() => {
                    over.dirname = Some(prompt_dirname(
                        output,
                        &commit,
                        &target_layout.name,
                        &source.project_root,
                        &default,
                    )?);
                }
                transform::DirnameDecision::Ask(derived)
                | transform::DirnameDecision::Blocked(derived) => {
                    blockers.push(transform::Blocker::repo_wide(
                        transform::BlockerKind::MissingAs {
                            derived,
                            commit: commit.clone(),
                        },
                    ));
                }
            }
        }
        transform::RootSituation::AmbiguousPivot { candidates } => {
            let rows: Vec<transform::PivotCandidate> = candidates
                .iter()
                .map(|&i| transform::PivotCandidate {
                    branch: source.worktrees[i].label().to_string(),
                    path: source.worktrees[i].path.clone(),
                    counts: crate::core::worktree::list::count_changed_files(
                        &source.worktrees[i].path,
                    ),
                })
                .collect();
            match transform::decide::decide_pivot(
                over.branch.as_deref(),
                true,
                interactive,
                args.yes,
            ) {
                transform::PivotDecision::Settled => {}
                transform::PivotDecision::Ask if blockers.is_empty() => {
                    over.branch = Some(prompt_pivot(
                        output,
                        &rows,
                        &default_branch,
                        &source.project_root,
                        cwd.as_deref(),
                    )?);
                }
                transform::PivotDecision::Ask | transform::PivotDecision::Blocked => {
                    blockers.push(transform::Blocker::repo_wide(
                        transform::BlockerKind::MissingPivot { candidates: rows },
                    ));
                }
            }
        }
        transform::RootSituation::Settled => {}
    }
    // Without an answer to one of the two questions there is no target to
    // plan against; report what is known so far. Every other blocker waits
    // for the plan, so the report is one pass.
    let undecided = blockers.iter().any(|b| {
        matches!(
            b.kind,
            transform::BlockerKind::MissingAs { .. } | transform::BlockerKind::MissingPivot { .. }
        )
    });
    if undecided {
        anyhow::bail!(transform::report::render_blockers(
            &blockers,
            &target_layout.name,
            &source,
            cwd.as_deref()
        ));
    }

    // Compute target state using the (possibly adjusted) project root. This is
    // where an impossible transform is refused — before any mutation.
    let target = transform::compute_target_state(&target_layout, &source, &over)?;

    // Classify worktrees
    let classified =
        transform::classify_worktrees(&source, &target, &args.include, args.include_all);

    // ── Everything that can block, in one pass ──────────────────────────
    if bare_flips
        && let Some(cw) = classified
            .iter()
            .find(|cw| cw.disposition == transform::WorktreeDisposition::Root)
    {
        probe_role_change(
            vec![(cw.current_path.clone(), cw.branch.clone())],
            &mut blockers,
        );
    }
    let moving: Vec<(PathBuf, Option<String>)> = classified
        .iter()
        .filter(|cw| cw.disposition == transform::WorktreeDisposition::Conforming)
        .filter(|cw| !transform::paths_equivalent(&cw.current_path, &cw.target_path))
        .map(|cw| (cw.current_path.clone(), cw.branch.clone()))
        .collect();
    blockers.extend(transform::preflight::relocation_blockers(
        &source.git_dir,
        &moving,
    ));

    // Snapshot every worktree's status: the plan line reports what is carried,
    // and `ValidateIntegrity` holds the transform to it.
    let artifacts = transform::Artifacts::for_transform(&source, &target);
    let snapshots = transform::status_snapshot::capture(&classified, &artifacts)?;

    // Build the plan
    let plan = transform::build_plan(&source, &target, &classified, &snapshots)?;

    // A move across volumes is a copy — say how much, and get a yes.
    let copy_moves = plan.cross_volume_moves();
    let copy_bytes = (!copy_moves.is_empty()).then(|| {
        let roots: Vec<PathBuf> = copy_moves.iter().map(|(from, _)| from.clone()).collect();
        let jobs = crate::core::size_walk::resolve_jobs(None);
        crate::core::size_walk::walk_all(&roots, None, jobs)
            .into_iter()
            .map(|b| b.unwrap_or(0))
            .sum::<u64>()
    });
    let mut confirm_copy = false;
    match transform::decide::decide_copy_confirm(!copy_moves.is_empty(), interactive, args.yes) {
        transform::ConfirmDecision::Accept => {}
        transform::ConfirmDecision::Ask => confirm_copy = true,
        transform::ConfirmDecision::Blocked => blockers.push(transform::Blocker::repo_wide(
            transform::BlockerKind::NeedsCopyConfirm {
                bytes: copy_bytes.unwrap_or(0),
                moves: copy_moves.clone(),
            },
        )),
    }

    // The plan line — always, before anything happens. It absorbs the #873
    // notice: the root role follows the main working tree, so when it is on a
    // branch other than the default, say what the default is left with.
    output.notice(&transform::report::plan_line(
        &source,
        &classified,
        &plan,
        &target_layout.name,
        copy_bytes,
    ));

    if !blockers.is_empty() {
        if args.dry_run {
            transform::print_plan(&plan, output);
        }
        anyhow::bail!(transform::report::render_blockers(
            &blockers,
            &target_layout.name,
            &source,
            cwd.as_deref()
        ));
    }

    // Dry run: print plan and exit
    if args.dry_run {
        transform::print_plan(&plan, output);
        return Ok(());
    }

    if confirm_copy
        && !confirm_cross_volume_copy(
            output,
            &copy_moves,
            copy_bytes.unwrap_or(0),
            &source.project_root,
            cwd.as_deref(),
        )?
    {
        output.notice("Transform cancelled; nothing was changed.");
        return Ok(());
    }

    // Execute
    output.step(&format!(
        "Transforming to layout '{}'...",
        target_layout.name
    ));
    output.start_spinner("Transforming layout...");
    let exec_ctx = transform::ExecutionContext {
        project_root: source.project_root.clone(),
        git_dir: source.git_dir.clone(),
        target_git_dir: target.git_dir.clone(),
        remote: settings.remote.clone(),
        source_worktree: source.project_root.clone(),
        default_branch: default_branch.clone(),
        status_snapshots: snapshots,
        artifacts,
    };
    let mut hooks_config = crate::core::settings::load_hooks_config()?;
    if args.verbose {
        hooks_config.output.verbose = true;
    }
    let executor = HookExecutor::new(hooks_config.clone())?;
    let exec_result = {
        let mut bridge =
            CommandBridge::with_output_config(output, executor, hooks_config.output.clone());
        transform::execute_plan(&plan, &git, &exec_ctx, &mut bridge)
    };
    output.finish_spinner();
    exec_result?;

    // After transform, CWD may be in a directory that was moved or removed.
    // CD to a known-valid location: the target's root worktree if it exists,
    // or the project root.
    let target_root = target
        .worktrees
        .iter()
        .find(|wt| wt.is_root)
        .map(|wt| wt.path.clone())
        .unwrap_or_else(|| source.project_root.clone());
    let landing = if target_root.exists() {
        target_root
    } else {
        source.project_root.clone()
    };
    crate::utils::change_directory(&landing)?;

    // Clean up layout artifacts from the source layout (e.g., .gitignore
    // entries auto-added by nested). Must happen after worktrees are moved
    // so that .worktrees/ is empty and can be ignored. The file is looked up
    // where it *landed*: a NestFromRoot has already carried it out of the
    // project root and into the nested worktree.
    revert_layout_gitignore(&source, &landing, &git, output);

    // Auto-add .gitignore entries if the target layout places worktrees
    // inside the repo (e.g. nested → .worktrees/). Only relevant for non-bare
    // layouts that aren't wrapped. Bare repos don't have a working tree to
    // conflict with, and wrapped layouts (contained-classic) place worktrees
    // at the wrapper level, not inside the working tree.
    if !target_layout.needs_bare() && !target_layout.needs_wrapper() {
        let project_root = crate::get_project_root()?;
        // Compute a sample worktree path to derive the gitignore pattern
        let ctx = crate::core::multi_remote::path::build_template_context(&project_root, "sample");
        if let Ok(sample_path) = target_layout.worktree_path(&ctx) {
            crate::core::layout::auto_gitignore_if_needed(
                &project_root,
                &sample_path,
                Some(&target_layout),
            )?;
        }
    }

    // Update repos.json with the new layout
    let git_dir = get_git_common_dir()?;
    TrustDatabase::update(|db| {
        db.set_layout(&git_dir, target_layout.name.clone());
        Ok(())
    })
    .context("Failed to save layout to repos.json")?;

    // The catalog keys on the git common dir and the project root; both can
    // move under a transform (contained-classic puts `.git` inside the main
    // working tree). Best-effort, like every other ambient registration.
    crate::catalog::touch_current_repo();

    // CD to the worktree for the user's original branch (it may have moved)
    if let Some(ref branch) = user_branch
        && let Ok(Some(wt_path)) = git.find_worktree_for_branch(branch)
    {
        output.cd_path(&wt_path);
    }

    output.result(&format!("Transformed to layout '{}'.", target_layout.name));

    Ok(())
}

// ── transform prompts ─────────────────────────────────────────────────────
//
// One-shot prompts, so `dialoguer` (the daft-tui boundary: ratatui is for
// panels and live state). Esc / Ctrl-C leave through the prompt module's
// cancel path — exit 130, nothing changed.

const TRANSFORM_CANCELLED: &str = "Transform cancelled; nothing was changed.";

fn prompt_theme() -> dialoguer::theme::ColorfulTheme {
    dialoguer::theme::ColorfulTheme::default()
}

/// Ask for the directory name of a detached main working tree.
fn prompt_dirname(
    output: &mut dyn Output,
    commit: &str,
    layout_name: &str,
    project_root: &Path,
    default: &str,
) -> Result<String> {
    let short = &commit[..commit.len().min(7)];
    output.notice(&format!(
        "The main working tree is detached at {short}, so the '{layout_name}' layout has no \
         branch to name its directory."
    ));
    let root = project_root.to_path_buf();
    let name: String = dialoguer::Input::with_theme(&prompt_theme())
        .with_prompt("Directory name for the detached main working tree")
        .with_initial_text(default)
        .validate_with(move |s: &String| -> std::result::Result<(), String> {
            crate::core::worktree::sandbox::validate_dirname(s)?;
            if root.join(s).exists() {
                return Err(format!("{} already exists.", root.join(s).display()));
            }
            Ok(())
        })
        .interact_text()
        .unwrap_or_else(|_| crate::prompt::exit_cancelled_with(TRANSFORM_CANCELLED));
    Ok(name)
}

/// Ask which worktree takes the repository root of a bare repository.
fn prompt_pivot(
    output: &mut dyn Output,
    candidates: &[transform::PivotCandidate],
    default_branch: &str,
    project_root: &Path,
    cwd: Option<&Path>,
) -> Result<String> {
    output.notice(&format!(
        "The default branch '{default_branch}' has no worktree, so one of these becomes the \
         main working tree at {}.",
        crate::output::format::tilde_path(&project_root.to_string_lossy())
    ));
    let _ = cwd;
    output.notice(&dim("j/k move · Enter select · Esc cancel"));
    let paths: Vec<String> = candidates
        .iter()
        .map(|c| crate::output::format::tilde_path(&c.path.to_string_lossy()))
        .collect();
    let bw = candidates.iter().map(|c| c.branch.len()).max().unwrap_or(0);
    let pw = paths.iter().map(String::len).max().unwrap_or(0);
    let rows: Vec<String> = candidates
        .iter()
        .zip(paths.iter())
        .map(|(c, p)| {
            format!(
                "{:<bw$}  {:<pw$}  {}",
                c.branch,
                p,
                transform::report::tree_summary_or_clean(&c.counts)
            )
        })
        .collect();
    let selection = dialoguer::Select::with_theme(&prompt_theme())
        .with_prompt("Which worktree becomes the main working tree?")
        .items(&rows)
        .default(0)
        .interact_opt()
        .unwrap_or_else(|_| crate::prompt::exit_cancelled_with(TRANSFORM_CANCELLED));
    match selection {
        Some(i) => Ok(candidates[i].branch.clone()),
        None => crate::prompt::exit_cancelled_with(TRANSFORM_CANCELLED),
    }
}

/// Confirm a copy across volumes. `Ok(false)` is an explicit "no".
fn confirm_cross_volume_copy(
    output: &mut dyn Output,
    moves: &[(PathBuf, PathBuf)],
    bytes: u64,
    project_root: &Path,
    cwd: Option<&Path>,
) -> Result<bool> {
    let _ = cwd;
    let show = |p: &Path| crate::output::format::tilde_path(&p.to_string_lossy());
    let dest = moves
        .first()
        .and_then(|(_, to)| to.parent())
        .map(show)
        .unwrap_or_else(|| "the destination".to_string());
    output.notice(&format!(
        "{dest} is a different volume from {}, so the worktree{} below {} copied, not renamed — \
         interruptible, and the source is kept until the copy is verified.",
        show(project_root),
        if moves.len() == 1 { "" } else { "s" },
        if moves.len() == 1 { "is" } else { "are" },
    ));
    let size = crate::core::copy_paths::format_bytes(bytes);
    let question = match moves {
        [(from, to)] => format!(
            "Copy {} ({size}) to {}?",
            from.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| show(from)),
            show(to)
        ),
        _ => format!("Copy {} worktrees ({size}) to {dest}?", moves.len()),
    };
    let answer = dialoguer::Confirm::with_theme(&prompt_theme())
        .with_prompt(question)
        .default(false)
        .interact_opt()
        .unwrap_or_else(|_| crate::prompt::exit_cancelled_with(TRANSFORM_CANCELLED));
    match answer {
        Some(yes) => Ok(yes),
        None => crate::prompt::exit_cancelled_with(TRANSFORM_CANCELLED),
    }
}

/// Revert layout-specific .gitignore entries that were auto-added by the source
/// layout (e.g., `.worktrees/` for nested). Removes only the specific pattern,
/// preserving any user-added entries. If the .gitignore becomes empty, deletes it.
fn revert_layout_gitignore(
    source: &transform::LayoutState,
    root_dir: &Path,
    _git: &GitCommand,
    output: &mut dyn Output,
) {
    // Only relevant for non-bare source layouts that place worktrees inside
    // the project root (nested is the main case). A bare source has no working
    // tree to have auto-ignored anything, and the pattern derived below would
    // name an ordinary branch directory — which is a line the user may well
    // have written themselves.
    if source.is_bare {
        return;
    }

    // `root_dir` is where the main working tree ended up, which is not
    // `source.project_root` whenever the transform nested it.
    let gitignore_path = root_dir.join(".gitignore");
    if !gitignore_path.exists() {
        return;
    }

    // Compute the auto-gitignore pattern the source layout would have added.
    // This is the first path component of a worktree path relative to the main
    // working tree — `nested`'s `.worktrees/` is the case that exists.
    //
    // Relative to the main working tree, *not* `project_root`: for a wrapper
    // layout (`contained-classic`) those differ, and measuring from the wrapper
    // yields an ordinary branch directory name — `dev/` for a worktree at
    // `repo/dev` — which is a line the user may well have written themselves
    // in `repo/main/.gitignore`. Stripping that would be data loss. Measuring
    // from the main working tree makes the sibling worktree fail
    // `strip_prefix`, so nothing is derived and the file is left alone.
    let Some(source_root) = source.worktrees.iter().find(|wt| wt.is_root) else {
        return;
    };
    let sample_wt = source.worktrees.iter().find(|wt| !wt.is_root);
    let pattern = sample_wt.and_then(|wt| {
        wt.path
            .strip_prefix(&source_root.path)
            .ok()
            .and_then(|rel| rel.components().next())
            .map(|c| format!("{}/", c.as_os_str().to_string_lossy()))
    });

    let Some(pattern) = pattern else {
        return;
    };

    // Read .gitignore and remove the specific pattern
    let Ok(contents) = std::fs::read_to_string(&gitignore_path) else {
        return;
    };

    let new_contents: String = contents
        .lines()
        .filter(|line| line.trim() != pattern.trim())
        .collect::<Vec<_>>()
        .join("\n");

    if new_contents == contents {
        return; // Pattern wasn't there, nothing to revert
    }

    let new_contents = new_contents.trim().to_string();

    if new_contents.is_empty() {
        // File only had the layout pattern — remove it entirely
        if std::fs::remove_file(&gitignore_path).is_ok() {
            // If it was tracked, stage the removal
            let _ = std::process::Command::new("git")
                .args([
                    "rm",
                    "--cached",
                    "--quiet",
                    "--ignore-unmatch",
                    ".gitignore",
                ])
                .current_dir(root_dir)
                .output();
            output.step("Reverted layout-specific .gitignore");
        }
    } else {
        // Write back without the pattern
        if std::fs::write(&gitignore_path, format!("{new_contents}\n")).is_ok() {
            // If it was tracked, stage the change
            let _ = std::process::Command::new("git")
                .args(["add", "--ignore-errors", ".gitignore"])
                .current_dir(root_dir)
                .output();
            output.step("Reverted layout-specific .gitignore entry");
        }
    }
}
