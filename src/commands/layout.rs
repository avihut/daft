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
    styles::{bold, dim},
    utils::*,
};
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

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
    command: LayoutCommand,
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
        LayoutCommand::List => cmd_list(&mut output),
        LayoutCommand::Show => cmd_show(&mut output),
        LayoutCommand::Transform(transform_args) => cmd_transform(&transform_args, &mut output),
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

    // Collect all layouts: built-ins first, then custom
    let mut layouts: Vec<LayoutRow> = Vec::new();

    for builtin in BuiltinLayout::all() {
        let layout = builtin.to_layout();
        let is_default = builtin.name() == default_layout_name;
        layouts.push(LayoutRow {
            name: builtin.name().to_string(),
            template: builtin.to_layout().template,
            bare: layout.needs_bare(),
            is_default,
        });
    }

    for (name, custom) in &global_config.layouts {
        // Skip if a custom layout overrides a built-in name (already shown)
        if BuiltinLayout::from_name(name).is_some() {
            // Find and update the built-in entry with the custom template
            if let Some(row) = layouts.iter_mut().find(|r| r.name == *name) {
                let layout = Layout {
                    name: name.clone(),
                    template: custom.template.clone(),
                    bare: custom.bare,
                };
                row.template = custom.template.clone();
                row.bare = layout.needs_bare();
            }
            continue;
        }
        let layout = Layout {
            name: name.clone(),
            template: custom.template.clone(),
            bare: custom.bare,
        };
        let is_default = name == default_layout_name;
        layouts.push(LayoutRow {
            name: name.clone(),
            template: custom.template.clone(),
            bare: layout.needs_bare(),
            is_default,
        });
    }

    // Calculate column widths
    let name_width = layouts
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(0)
        .max(6); // "Layout" header
    let template_width = layouts
        .iter()
        .map(|r| r.template.len())
        .max()
        .unwrap_or(0)
        .max(8); // "Template" header

    // Print header
    output.info(&format!(
        "{:<name_width$}  {:<template_width$}  {}",
        bold("Layout"),
        bold("Template"),
        bold("Bare"),
    ));

    // Print rows
    for row in &layouts {
        let bare_str = if row.bare { "yes" } else { "no" };
        let default_marker = if row.is_default { "  (default)" } else { "" };
        output.info(&format!(
            "{:<name_width$}  {:<template_width$}  {bare_str}{default_marker}",
            row.name, row.template,
        ));
    }

    Ok(())
}

struct LayoutRow {
    name: String,
    template: String,
    bare: bool,
    is_default: bool,
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
        LayoutSource::RepoStore => "repos.json (per-repo)",
        LayoutSource::YamlConfig => "daft.yml (team convention)",
        LayoutSource::GlobalConfig => "global config (~/.config/daft/config.toml)",
        LayoutSource::Default => "built-in default",
    };

    let bare_str = if layout.needs_bare() { "yes" } else { "no" };

    output.info(&format!("{} {}", bold("Layout:"), layout.name));
    output.info(&format!("{} {}", bold("Template:"), layout.template));
    output.info(&format!("{} {}", bold("Bare:"), bare_str));
    output.info(&format!("{} {}", bold("Source:"), source_display));

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
        // non-bare -> bare (adopt)
        (false, true) => {
            output.step(&format!(
                "Transforming to layout '{}' (non-bare -> bare)...",
                target_layout.name
            ));
            transform_to_bare(&settings, output)?;
        }
        // bare -> non-bare (eject)
        (true, false) => {
            output.step(&format!(
                "Transforming to layout '{}' (bare -> non-bare)...",
                target_layout.name
            ));
            transform_to_non_bare(&settings, args.force, output)?;
        }
        // non-bare -> non-bare (worktree move)
        (false, false) => {
            output.warning(&format!(
                "Transform from non-bare to non-bare layout '{}' is not yet supported.",
                target_layout.name
            ));
            output.info(&dim(
                "Worktree relocation between non-bare layouts will be added in a future release.",
            ));
            return Ok(());
        }
        // bare -> bare (worktree move within bare)
        (true, true) => {
            output.warning(&format!(
                "Transform from bare to bare layout '{}' is not yet supported.",
                target_layout.name
            ));
            output.info(&dim(
                "Worktree relocation between bare layouts will be added in a future release.",
            ));
            return Ok(());
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
