use crate::{
    core::{
        global_config::GlobalConfig,
        layout::{
            resolver::{resolve_layout, LayoutResolutionContext, LayoutSource},
            transform, BuiltinLayout, Layout, DEFAULT_LAYOUT,
        },
        OutputSink,
    },
    get_current_worktree_path, get_git_common_dir,
    git::GitCommand,
    hooks::{yaml_config_loader, TrustDatabase},
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

  contained           Worktrees inside the repo directory (bare required)
  contained-classic   Like contained but default branch is a regular clone
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
    List,
    /// Show the resolved layout for the current repo
    Show,
    /// Transform the current repo to a different layout
    Transform(TransformArgs),
    /// View or set the global default layout
    Default(DefaultArgs),
}

#[derive(Args)]
struct TransformArgs {
    /// Target layout name or template
    layout: String,
    /// Force transform even with uncommitted changes
    #[arg(short, long)]
    force: bool,
    /// Show plan without executing
    #[arg(long)]
    dry_run: bool,
    /// Also relocate this non-conforming worktree (repeatable)
    #[arg(long = "include", value_name = "BRANCH")]
    include: Vec<String>,
    /// Relocate all non-conforming worktrees
    #[arg(long)]
    include_all: bool,
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let layout_args = LayoutArgs::parse_from(args);
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    match layout_args.command {
        Some(LayoutCommand::List) => cmd_list(&mut output),
        Some(LayoutCommand::Show) | None => cmd_show(&mut output),
        Some(LayoutCommand::Transform(transform_args)) => {
            cmd_transform(&transform_args, &mut output)
        }
        Some(LayoutCommand::Default(default_args)) => cmd_default(&default_args, &mut output),
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
    if let Ok(db) = TrustDatabase::load() {
        if let Some(current_layout_name) = db.get_layout(&source.git_dir) {
            if let Some(current_layout) = global_config.resolve_layout_by_name(current_layout_name)
            {
                if current_layout.needs_wrapper() {
                    if let Some(wrapper) = source.project_root.parent() {
                        source.project_root = wrapper.to_path_buf();
                    }
                }
            }
        }
    }

    // Compute target state using the (possibly adjusted) project root
    let target = transform::compute_target_state(
        &target_layout,
        &source.project_root,
        &default_branch,
        &source.worktrees,
    )?;

    // Classify worktrees
    let classified =
        transform::classify_worktrees(&source, &target, &args.include, args.include_all);

    // Check for dirty worktrees (unless --force).
    // Only check the default branch worktree (the repo root for non-bare
    // layouts). Non-default worktrees are linked worktrees that git manages
    // independently — their dirty state is preserved via stash ops.
    // Layout artifacts (.gitignore with auto-added patterns, worktree
    // directories managed by the layout) are excluded from the check.
    if !args.force {
        let prev_dir = get_current_directory()?;
        for cw in &classified {
            if cw.disposition != transform::WorktreeDisposition::DefaultBranch {
                continue;
            }
            if cw.current_path.exists() {
                change_directory(&cw.current_path)?;
                if has_real_uncommitted_changes(&git, &source)? {
                    change_directory(&prev_dir)?;
                    anyhow::bail!(
                        "Worktree '{}' has uncommitted changes. Commit, stash, or use --force.",
                        cw.branch
                    );
                }
            }
        }
        change_directory(&prev_dir)?;
    }

    // Build the plan
    let mut plan = transform::build_plan(&source, &target, &classified, args.force)?;

    // Insert stash/pop ops for dirty worktrees when --force
    if args.force {
        let prev_dir = get_current_directory()?;
        let mut stash_ops = Vec::new();
        let mut pop_ops = Vec::new();
        for cw in &classified {
            if cw.disposition == transform::WorktreeDisposition::NonConforming {
                continue;
            }
            if cw.current_path.exists() {
                change_directory(&cw.current_path)?;
                if git.has_uncommitted_changes()? {
                    stash_ops.push(transform::TransformOp::StashChanges {
                        branch: cw.branch.clone(),
                        worktree_path: cw.current_path.clone(),
                    });
                    pop_ops.push(transform::TransformOp::PopStash {
                        branch: cw.branch.clone(),
                        worktree_path: cw.target_path.clone(),
                    });
                }
            }
        }
        change_directory(&prev_dir)?;

        // Prepend stash ops and append pop ops (before ValidateIntegrity)
        if !stash_ops.is_empty() {
            let validate = plan.ops.pop(); // Remove ValidateIntegrity
            let mut new_ops = stash_ops;
            new_ops.append(&mut plan.ops);
            new_ops.append(&mut pop_ops);
            if let Some(v) = validate {
                new_ops.push(v);
            }
            plan.ops = new_ops;
        }
    }

    // Dry run: print plan and exit
    if args.dry_run {
        transform::print_plan(&plan, output);
        return Ok(());
    }

    // Execute
    output.step(&format!(
        "Transforming to layout '{}'...",
        target_layout.name
    ));
    output.start_spinner("Transforming layout...");
    let exec_result = {
        let mut sink = OutputSink(output);
        transform::execute_plan(&plan, &git, &mut sink)
    };
    output.finish_spinner();
    exec_result?;

    // After transform, CWD may be in a directory that was moved or removed.
    // CD to a known-valid location: the target's default branch worktree if
    // it exists, or the project root.
    let target_default = target
        .worktrees
        .iter()
        .find(|wt| wt.is_default)
        .map(|wt| wt.path.clone())
        .unwrap_or_else(|| source.project_root.clone());
    if target_default.exists() {
        crate::utils::change_directory(&target_default)?;
    } else {
        crate::utils::change_directory(&source.project_root)?;
    }

    // Clean up layout artifacts from the source layout (e.g., .gitignore
    // entries auto-added by nested). Must happen after worktrees are moved
    // so that .worktrees/ is empty and can be ignored.
    revert_layout_gitignore(&source, &git, output);

    // Auto-add .gitignore entries if the target layout places worktrees
    // inside the repo (e.g. nested → .worktrees/). Only relevant for non-bare
    // layouts since bare repos don't have a working tree to conflict with.
    if !target_layout.needs_bare() {
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
    let mut trust_db = TrustDatabase::load().unwrap_or_default();
    trust_db.set_layout(&git_dir, target_layout.name.clone());
    trust_db
        .save()
        .context("Failed to save layout to repos.json")?;

    // CD to the worktree for the user's original branch (it may have moved)
    if let Some(ref branch) = user_branch {
        if let Ok(Some(wt_path)) = git.find_worktree_for_branch(branch) {
            output.cd_path(&wt_path);
        }
    }

    output.result(&format!("Transformed to layout '{}'.", target_layout.name));

    Ok(())
}

/// Check for uncommitted changes, ignoring layout artifacts.
///
/// Layout artifacts are: auto-generated .gitignore entries and worktree
/// directories managed by the layout system (e.g., `.worktrees/` for nested).
/// These would show up as untracked files but are not real user changes.
fn has_real_uncommitted_changes(
    _git: &GitCommand,
    source: &transform::LayoutState,
) -> Result<bool> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .context("Failed to execute git status")?;

    if !output.status.success() {
        return Ok(false);
    }

    let status = String::from_utf8_lossy(&output.stdout);

    // Compute layout artifact paths to ignore
    let mut ignore_patterns: Vec<String> = Vec::new();
    ignore_patterns.push(".gitignore".to_string());

    // Worktree parent directories (e.g., .worktrees/ for nested)
    for wt in &source.worktrees {
        if wt.is_default {
            continue;
        }
        if let Ok(rel) = wt.path.strip_prefix(&source.project_root) {
            if let Some(first) = rel.components().next() {
                let dir_name = first.as_os_str().to_string_lossy();
                ignore_patterns.push(format!("{dir_name}/"));
                ignore_patterns.push(dir_name.to_string());
            }
        }
    }

    for line in status.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Porcelain format: "XY path" where XY is 2-char status
        let path = if line.len() > 3 { &line[3..] } else { line };

        let is_artifact = ignore_patterns.iter().any(|p| path.starts_with(p));
        if !is_artifact {
            return Ok(true); // Real change found
        }
    }

    Ok(false) // Only artifacts, no real changes
}

/// Revert layout-specific .gitignore entries that were auto-added by the source
/// layout (e.g., `.worktrees/` for nested). Removes only the specific pattern,
/// preserving any user-added entries. If the .gitignore becomes empty, deletes it.
fn revert_layout_gitignore(
    source: &transform::LayoutState,
    _git: &GitCommand,
    output: &mut dyn Output,
) {
    // Only relevant for non-bare source layouts that place worktrees inside
    // the project root (nested is the main case).
    let gitignore_path = source.project_root.join(".gitignore");
    if !gitignore_path.exists() {
        return;
    }

    // Compute the auto-gitignore pattern the source layout would have added.
    // This is the first path component of a worktree path relative to the root.
    let sample_wt = source.worktrees.iter().find(|wt| !wt.is_default);
    let pattern = sample_wt.and_then(|wt| {
        wt.path
            .strip_prefix(&source.project_root)
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
                .current_dir(&source.project_root)
                .output();
            output.step("Reverted layout-specific .gitignore");
        }
    } else {
        // Write back without the pattern
        if std::fs::write(&gitignore_path, format!("{new_contents}\n")).is_ok() {
            // If it was tracked, stage the change
            let _ = std::process::Command::new("git")
                .args(["add", "--ignore-errors", ".gitignore"])
                .current_dir(&source.project_root)
                .output();
            output.step("Reverted layout-specific .gitignore entry");
        }
    }
}

/// Relocate linked worktrees to match the target layout template.
///
/// Parses `git worktree list --porcelain`, computes the expected path for
/// each worktree using the target layout, and moves any that are out of place.
/// Skips the bare root entry, detached HEAD worktrees, and any branch in
/// `skip_branches` (used to preserve the default branch for eject).
fn relocate_worktrees(
    target_layout: &Layout,
    git: &GitCommand,
    output: &mut dyn Output,
    skip_branches: &[&str],
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

    // Canonicalize project root for comparison with worktree paths
    let project_root_canonical = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.clone());

    for (current_path, branch) in &worktrees {
        if skip_branches.iter().any(|s| s == branch) {
            continue;
        }

        // Skip the main working tree (non-bare repo root). It's not a linked
        // worktree and cannot be moved with `git worktree move`.
        let current_canonical = current_path
            .canonicalize()
            .unwrap_or_else(|_| current_path.clone());
        if current_canonical == project_root_canonical {
            continue;
        }

        let ctx = build_template_context(&project_root, branch);
        let expected_path = target_layout
            .worktree_path(&ctx)
            .with_context(|| format!("Failed to compute path for branch '{branch}'"))?;

        // Canonicalize for comparison (handles symlinks, /tmp vs /private/tmp)
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

/// Public entry point for worktree relocation (used by post-clone reconciliation).
///
/// Thin wrapper around the internal `relocate_worktrees` that accepts `Output`
/// so callers outside this module can use it.
pub fn relocate_worktrees_public(
    target_layout: &Layout,
    git: &GitCommand,
    output: &mut dyn Output,
) -> Result<()> {
    relocate_worktrees(target_layout, git, output, &[])
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

/// Convert non-bare -> bare using the layout transform module.
#[allow(dead_code)]
fn transform_to_bare(settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let params = transform::ConvertToBareParams {
        use_gitoxide: settings.use_gitoxide,
    };

    output.start_spinner("Converting to bare (worktree) layout...");
    let exec_result = {
        let mut sink = OutputSink(output);
        transform::convert_to_bare(&params, &mut sink)
    };
    output.finish_spinner();
    let result = exec_result?;

    output.step(&format!(
        "Converted to worktree layout: '{}/{}'",
        result.repo_display_name, result.current_branch
    ));

    Ok(())
}

/// Collapse a bare repo to non-bare, keeping linked worktrees (for layout transform).
#[allow(dead_code)]
fn collapse_bare_to_non_bare(
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<transform::CollapseBareResult> {
    let params = transform::CollapseBareParams {
        use_gitoxide: settings.use_gitoxide,
        remote_name: settings.remote.clone(),
    };

    output.start_spinner("Converting bare repository to non-bare...");
    let exec_result = {
        let mut sink = OutputSink(output);
        transform::collapse_bare_to_non_bare(&params, &mut sink)
    };
    output.finish_spinner();
    let result = exec_result?;

    output.step(&format!(
        "Converted to non-bare on branch '{}'",
        result.default_branch
    ));

    Ok(result)
}
