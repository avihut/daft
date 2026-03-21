use crate::{
    core::{
        global_config::GlobalConfig,
        layout::{
            resolver::{resolve_layout, LayoutResolutionContext, LayoutSource},
            BuiltinLayout, Layout, DEFAULT_LAYOUT,
        },
        worktree::{flow_adopt, flow_eject},
        CommandBridge, OutputSink,
    },
    get_current_worktree_path, get_git_common_dir,
    git::GitCommand,
    hooks::{yaml_config_loader, HookExecutor, HooksConfig, TrustDatabase},
    is_git_repository,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    styles::{self, bold, dim, dim_underline},
    utils::*,
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use tabled::{
    builder::Builder,
    settings::{object::Columns, Padding, Style},
};

#[derive(Parser)]
#[command(name = "layout")]
#[command(about = "Manage worktree layouts")]
#[command(long_about = r#"
Manage worktree layouts for daft repositories.

Layouts control where worktrees are placed relative to the bare repository.
Built-in layouts:

  contained     Worktrees inside the repo directory (bare required)
  sibling       Worktrees next to the repo directory (default)
  nested        Worktrees in a hidden subdirectory
  centralized   Worktrees in a global ~/worktrees/ directory

Use `daft layout list` to see all available layouts including custom ones
defined in your global config (~/.config/daft/config.toml).

Use `daft layout show` to see the resolved layout for the current repo.

Use `daft layout transform <layout>` to convert a repo between layouts.
"#)]
pub struct LayoutArgs {
    #[command(subcommand)]
    command: Option<LayoutCommand>,
}

#[derive(Subcommand)]
enum LayoutCommand {
    /// List all available layouts
    List,
    /// Show the resolved layout for the current repo
    Show,
    /// Transform the current repo to a different layout
    Transform(TransformArgs),
}

#[derive(Args)]
struct TransformArgs {
    /// Target layout name or template
    layout: String,
    /// Force transform even with uncommitted changes
    #[arg(short, long)]
    force: bool,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let layout_args = LayoutArgs::parse_from(args);
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    match layout_args.command {
        Some(LayoutCommand::List) => cmd_list(&mut output),
        Some(LayoutCommand::Show) | None => cmd_show(&mut output),
        Some(LayoutCommand::Transform(transform_args)) => {
            cmd_transform(&transform_args, &mut output)
        }
    }
}

// ── layout list ────────────────────────────────────────────────────────────

fn cmd_list(output: &mut dyn Output) -> Result<()> {
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
    });

    Some(layout.name)
}

struct LayoutRow {
    name: String,
    template: String,
    is_default: bool,
    is_selected: bool,
}

// ── layout show ────────────────────────────────────────────────────────────

fn cmd_show(output: &mut dyn Output) -> Result<()> {
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

    let (layout, source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None,
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
    });

    let source_display = match source {
        LayoutSource::Cli => "CLI flag",
        LayoutSource::RepoStore => "repo setting",
        LayoutSource::YamlConfig => "daft.yml",
        LayoutSource::GlobalConfig => "global config",
        LayoutSource::Default => "default",
    };

    let use_color = styles::colors_enabled();
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

// ── layout transform ───────────────────────────────────────────────────────

fn cmd_transform(args: &TransformArgs, output: &mut dyn Output) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository. Run this command from within a repo.");
    }

    let settings = DaftSettings::load_global()?;
    let global_config = GlobalConfig::load().unwrap_or_default();
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

    // Resolve target layout
    let target_layout = match global_config.resolve_layout_by_name(&args.layout) {
        Some(layout) => layout,
        None => Layout {
            name: args.layout.clone(),
            template: args.layout.clone(),
            bare: None,
        },
    };

    let is_currently_bare = git.rev_parse_is_bare_repository().unwrap_or(false);
    let target_needs_bare = target_layout.needs_bare();

    match (is_currently_bare, target_needs_bare) {
        // non-bare -> bare (adopt + relocate)
        (false, true) => {
            output.step(&format!(
                "Transforming to layout '{}' (non-bare -> bare)...",
                target_layout.name
            ));
            transform_to_bare(&settings, output)?;
            // After adopt, relocate any linked worktrees to the new layout paths
            relocate_worktrees(&target_layout, &git, output)?;
        }
        // bare -> non-bare (eject)
        (true, false) => {
            output.step(&format!(
                "Transforming to layout '{}' (bare -> non-bare)...",
                target_layout.name
            ));
            transform_to_non_bare(&settings, args.force, output)?;
            // After eject, relocate any linked worktrees to the new layout paths
            relocate_worktrees(&target_layout, &git, output)?;
        }
        // non-bare -> non-bare (relocate only)
        (false, false) => {
            output.step(&format!(
                "Transforming to layout '{}'...",
                target_layout.name
            ));
            relocate_worktrees(&target_layout, &git, output)?;
        }
        // bare -> bare (relocate only)
        (true, true) => {
            output.step(&format!(
                "Transforming to layout '{}'...",
                target_layout.name
            ));
            relocate_worktrees(&target_layout, &git, output)?;
        }
    }

    // Update repos.json with the new layout
    let git_dir = get_git_common_dir()?;
    let mut trust_db = TrustDatabase::load().unwrap_or_default();
    trust_db.set_layout(&git_dir, target_layout.name.clone());
    trust_db
        .save()
        .context("Failed to save layout to repos.json")?;

    output.result(&format!(
        "Transformed to layout '{}'. Layout saved to repos.json.",
        target_layout.name
    ));

    Ok(())
}

/// Relocate all linked worktrees to match the target layout template.
///
/// Parses `git worktree list --porcelain`, computes the expected path for
/// each worktree using the target layout, and moves any that are out of place.
/// Skips the bare root entry and detached HEAD worktrees.
fn relocate_worktrees(
    target_layout: &Layout,
    git: &GitCommand,
    output: &mut dyn Output,
) -> Result<()> {
    use crate::core::multi_remote::path::build_template_context;
    use std::path::PathBuf;

    let project_root = crate::get_project_root()?;
    let porcelain = git.worktree_list_porcelain()?;

    // Parse porcelain output into (path, branch) pairs
    let mut worktrees: Vec<(PathBuf, String)> = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut is_bare = false;

    for line in porcelain.lines() {
        if let Some(wt_path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(wt_path));
            is_bare = false;
        } else if line == "bare" {
            is_bare = true;
        } else if line.starts_with("detached") {
            // Skip detached HEAD worktrees (sandboxes)
            current_path = None;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if !is_bare {
                if let (Some(path), Some(branch)) =
                    (current_path.take(), branch_ref.strip_prefix("refs/heads/"))
                {
                    worktrees.push((path, branch.to_string()));
                }
            }
        } else if line.is_empty() {
            is_bare = false;
        }
    }

    let mut moved_count = 0;

    for (current_path, branch) in &worktrees {
        let ctx = build_template_context(&project_root, branch);
        let expected_path = target_layout
            .worktree_path(&ctx)
            .with_context(|| format!("Failed to compute path for branch '{branch}'"))?;

        // Canonicalize for comparison (handles symlinks, /tmp vs /private/tmp)
        let current_canonical = current_path
            .canonicalize()
            .unwrap_or_else(|_| current_path.clone());
        let expected_canonical = expected_path
            .canonicalize()
            .unwrap_or_else(|_| expected_path.clone());

        if current_canonical == expected_canonical {
            continue; // Already in the right place
        }

        // Create parent directory for the target path
        if let Some(parent) = expected_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }

        output.step(&format!(
            "Moving '{}': {} -> {}",
            branch,
            current_path.display(),
            expected_path.display()
        ));

        git.worktree_move(current_path, &expected_path)
            .with_context(|| {
                format!(
                    "Failed to move worktree '{}' from {} to {}",
                    branch,
                    current_path.display(),
                    expected_path.display()
                )
            })?;

        moved_count += 1;

        // Clean up empty parent directories left behind
        if let Some(parent) = current_path.parent() {
            let _ = cleanup_empty_parents(parent, &project_root);
        }
    }

    if moved_count > 0 {
        output.step(&format!(
            "Relocated {} worktree{}",
            moved_count,
            if moved_count == 1 { "" } else { "s" }
        ));
    }

    Ok(())
}

/// Remove empty parent directories up to (but not including) the stop directory.
fn cleanup_empty_parents(mut dir: &std::path::Path, stop: &std::path::Path) -> Result<()> {
    while dir != stop {
        if dir
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            std::fs::remove_dir(dir)?;
        } else {
            break;
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    Ok(())
}

/// Delegate to flow_adopt core logic (non-bare -> bare).
fn transform_to_bare(settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let params = flow_adopt::AdoptParams {
        repository_path: None,
        dry_run: false,
        use_gitoxide: settings.use_gitoxide,
    };

    output.start_spinner("Converting to bare (worktree) layout...");
    let exec_result = {
        let mut sink = OutputSink(output);
        flow_adopt::execute(&params, &mut sink)
    };
    output.finish_spinner();
    let result = exec_result?;

    output.step(&format!(
        "Converted to worktree layout. Working directory: '{}/{}'",
        result.repo_display_name, result.current_branch
    ));

    output.cd_path(&get_current_directory()?);

    Ok(())
}

/// Delegate to flow_eject core logic (bare -> non-bare).
fn transform_to_non_bare(
    settings: &DaftSettings,
    force: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let params = flow_eject::EjectParams {
        repository_path: None,
        branch: None,
        force,
        dry_run: false,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: false,
        remote_name: settings.remote.clone(),
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    output.start_spinner("Converting to traditional layout...");
    let exec_result = {
        let mut bridge = CommandBridge::new(output, executor);
        flow_eject::execute(&params, &mut bridge)
    };
    output.finish_spinner();
    let result = exec_result?;

    output.step(&format!(
        "Converted to traditional layout on branch '{}'",
        result.target_branch
    ));

    output.cd_path(&result.project_root);

    Ok(())
}
