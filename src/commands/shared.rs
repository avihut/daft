//! Command: `daft shared` — manage shared files across worktrees.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;

use crate::core::layout;
use crate::core::repo;
use crate::core::shared;
use crate::output::{CliOutput, Output};

#[derive(Parser)]
#[command(name = "daft-shared")]
#[command(version = crate::VERSION)]
#[command(about = "Manage shared files across worktrees")]
#[command(long_about = r#"
Centralize untracked configuration files (.env, .idea/, .vscode/, etc.)
so they are shared across worktrees via symlinks.

Files are stored in .git/.daft/shared/ and symlinked into each worktree.
Use 'materialize' to make a worktree-local copy, and 'link' to rejoin
the shared version.
"#)]
pub struct Args {
    #[command(subcommand)]
    command: SharedCommand,
}

#[derive(Subcommand)]
enum SharedCommand {
    /// Collect file/dir from current worktree into shared storage
    Add(AddArgs),
    /// Stop sharing a file (materialize everywhere, then remove)
    Remove(RemoveArgs),
    /// Replace symlink with a local copy in current worktree
    Materialize(MaterializeArgs),
    /// Replace local copy with symlink to shared version
    Link(LinkArgs),
    /// Show shared files and per-worktree state
    Status(StatusArgs),
    /// Ensure all worktrees have symlinks for declared shared files
    Sync(SyncArgs),
}

#[derive(Parser)]
struct AddArgs {
    /// Paths to share (relative to worktree root)
    #[arg(required = true)]
    paths: Vec<String>,

    /// Only declare the path in daft.yml without collecting (file need not exist)
    #[arg(long)]
    declare: bool,
}

#[derive(Parser)]
struct RemoveArgs {
    /// Paths to stop sharing
    #[arg(required = true)]
    paths: Vec<String>,

    /// Delete shared file and all symlinks instead of materializing
    #[arg(long)]
    delete: bool,
}

#[derive(Parser)]
struct MaterializeArgs {
    /// Shared file path to materialize
    path: String,

    /// Target worktree name or path (defaults to current worktree)
    worktree: Option<String>,

    /// Force materialization even if a non-shared file exists
    #[arg(long = "override")]
    force_override: bool,
}

#[derive(Parser)]
struct LinkArgs {
    /// Shared file path to link back to shared version
    path: String,

    /// Target worktree name or path (defaults to current worktree)
    worktree: Option<String>,

    /// Replace local file even if it differs from shared version
    #[arg(long = "override")]
    force_override: bool,
}

#[derive(Parser)]
struct StatusArgs;

#[derive(Parser)]
struct SyncArgs;

pub fn run() -> Result<()> {
    // Skip argv[0] (binary name). When invoked as `daft shared <sub> <args>`,
    // env::args() is ["daft", "shared", ...] and skip(1) gives ["shared", ...]
    // so clap sees "shared" as the program name and parses the rest correctly.
    let args_raw: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args_raw);
    let mut output = CliOutput::default_output();

    match args.command {
        SharedCommand::Add(add_args) => run_add(add_args, &mut output),
        SharedCommand::Remove(remove_args) => run_remove(remove_args, &mut output),
        SharedCommand::Materialize(mat_args) => run_materialize(mat_args, &mut output),
        SharedCommand::Link(link_args) => run_link(link_args, &mut output),
        SharedCommand::Status(_) => run_status(&mut output),
        SharedCommand::Sync(_) => run_sync(&mut output),
    }
}

/// Resolve a worktree argument to an absolute path.
///
/// If `name` is None, returns the current worktree. Otherwise, matches
/// against known worktree paths by directory name or full path.
fn resolve_worktree(name: Option<&str>) -> Result<PathBuf> {
    let Some(name) = name else {
        return repo::get_current_worktree_path();
    };

    // If it's an absolute path that exists, use it directly
    let as_path = PathBuf::from(name);
    if as_path.is_absolute() && as_path.exists() {
        return Ok(as_path);
    }

    // Match by directory name against known worktrees
    let worktrees = shared::list_worktree_paths()?;
    for wt in &worktrees {
        let wt_name = wt.file_name().unwrap_or_default().to_string_lossy();
        if wt_name == name {
            return Ok(wt.clone());
        }
    }

    // Try as a relative path from current directory
    if as_path.exists() {
        return as_path
            .canonicalize()
            .context(format!("Failed to resolve worktree path: {name}"));
    }

    bail!(
        "Worktree '{}' not found. Known worktrees: {}",
        name,
        worktrees
            .iter()
            .filter_map(|w| w.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn run_add(args: AddArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let config_root = shared::resolve_config_root(&worktree_path);

    shared::ensure_shared_dir(&git_common_dir)?;

    let existing_shared = shared::read_shared_paths(&worktree_path)?;
    let mut added_paths = Vec::new();

    for rel_path in &args.paths {
        // Check if already shared
        if existing_shared.contains(rel_path) {
            if args.declare {
                output.info(&format!("'{}' is already declared as shared.", rel_path));
                continue;
            }
            bail!(
                "'{}' is already shared. Use `daft shared link {}` to symlink this worktree's copy.",
                rel_path,
                rel_path
            );
        }

        if args.declare {
            // --declare: just add to daft.yml and .gitignore
            layout::ensure_gitignore_entry(&config_root, rel_path)?;
            added_paths.push(rel_path.as_str());
            output.success(&format!("Declared: {}", rel_path));
            continue;
        }

        // Normal add: file must exist
        let full_path = worktree_path.join(rel_path);
        if !full_path.exists() {
            bail!(
                "'{}' does not exist in this worktree. Use `--declare` to declare without collecting.",
                rel_path
            );
        }

        // Must not be git-tracked
        if shared::is_git_tracked(&worktree_path, rel_path)? {
            bail!(
                "'{}' is tracked by git. Untrack it first with `git rm --cached {}`",
                rel_path,
                rel_path
            );
        }

        // Ensure gitignored
        layout::ensure_gitignore_entry(&config_root, rel_path)?;

        // Move to shared storage
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);
        if let Some(parent) = shared_target.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::rename(&full_path, &shared_target).is_err() {
            // rename fails across filesystems — fall back to copy + delete
            if full_path.is_dir() {
                shared::copy_dir_all(&full_path, &shared_target)?;
                fs::remove_dir_all(&full_path)?;
            } else {
                fs::copy(&full_path, &shared_target)?;
                fs::remove_file(&full_path)?;
            }
        }

        // Create symlink
        shared::create_shared_symlink(&worktree_path, rel_path, &git_common_dir)?;

        added_paths.push(rel_path.as_str());
        output.success(&format!(
            "Shared: {} → .git/.daft/shared/{}",
            rel_path, rel_path
        ));
    }

    // Update daft.yml
    if !added_paths.is_empty() {
        shared::add_to_daft_yml(&config_root, &added_paths)?;
    }

    Ok(())
}

fn run_remove(args: RemoveArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let config_root = shared::resolve_config_root(&worktree_path);
    let worktree_paths = shared::list_worktree_paths()?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    for rel_path in &args.paths {
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);

        if args.delete {
            // Delete mode: remove symlinks and shared storage
            for wt in &worktree_paths {
                let link = wt.join(rel_path);
                if link.is_symlink() {
                    fs::remove_file(&link)?;
                }
            }
            if shared_target.exists() {
                if shared_target.is_dir() {
                    fs::remove_dir_all(&shared_target)?;
                } else {
                    fs::remove_file(&shared_target)?;
                }
            }
            output.success(&format!(
                "Deleted: {} (shared storage + all symlinks)",
                rel_path
            ));
        } else {
            // Default: materialize everywhere, then delete shared storage
            if shared_target.exists() {
                for wt in &worktree_paths {
                    let link = wt.join(rel_path);
                    if link.is_symlink() {
                        fs::remove_file(&link)?;
                        if shared_target.is_dir() {
                            shared::copy_dir_all(&shared_target, &link)?;
                        } else {
                            fs::copy(&shared_target, &link)?;
                        }
                        output.info(&format!(
                            "  Materialized in {}",
                            wt.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                }
                if shared_target.is_dir() {
                    fs::remove_dir_all(&shared_target)?;
                } else {
                    fs::remove_file(&shared_target)?;
                }
            }
            output.success(&format!(
                "Removed: {} (materialized in all worktrees)",
                rel_path
            ));
        }

        materialized.remove_all(rel_path);
    }

    materialized.save(&git_common_dir)?;

    let path_refs: Vec<&str> = args.paths.iter().map(|s| s.as_str()).collect();
    shared::remove_from_daft_yml(&config_root, &path_refs)?;

    Ok(())
}

fn run_materialize(args: MaterializeArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = resolve_worktree(args.worktree.as_deref())?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    let rel_path = &args.path;
    let link = worktree_path.join(rel_path);
    let shared_target = shared::shared_file_path(&git_common_dir, rel_path);

    if !shared_target.exists() {
        bail!("'{}' has no shared file to materialize from.", rel_path);
    }

    if link.is_symlink() {
        // Replace symlink with copy
        fs::remove_file(&link)?;
        if shared_target.is_dir() {
            shared::copy_dir_all(&shared_target, &link)?;
        } else {
            fs::copy(&shared_target, &link)?;
        }
        materialized.add(rel_path, &worktree_path);
        output.success(&format!("Materialized: {} (copied from shared)", rel_path));
    } else if link.exists() {
        if args.force_override {
            if link.is_dir() {
                fs::remove_dir_all(&link)?;
            } else {
                fs::remove_file(&link)?;
            }
            if shared_target.is_dir() {
                shared::copy_dir_all(&shared_target, &link)?;
            } else {
                fs::copy(&shared_target, &link)?;
            }
            materialized.add(rel_path, &worktree_path);
            output.success(&format!("Materialized: {} (overridden)", rel_path));
        } else {
            output.info(&format!(
                "'{}' is already a local file in this worktree.",
                rel_path
            ));
        }
    } else {
        // No file at all — copy from shared
        if let Some(parent) = link.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        if shared_target.is_dir() {
            shared::copy_dir_all(&shared_target, &link)?;
        } else {
            fs::copy(&shared_target, &link)?;
        }
        materialized.add(rel_path, &worktree_path);
        output.success(&format!("Materialized: {} (copied from shared)", rel_path));
    }

    materialized.save(&git_common_dir)?;

    Ok(())
}

fn run_link(args: LinkArgs, output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = resolve_worktree(args.worktree.as_deref())?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    let rel_path = &args.path;
    let link = worktree_path.join(rel_path);
    let shared_target = shared::shared_file_path(&git_common_dir, rel_path);

    if !shared_target.exists() {
        bail!("'{}' has no shared file to link to.", rel_path);
    }

    // Already a correct symlink?
    if link.is_symlink() {
        let target = fs::read_link(&link)?;
        let expected = shared::relative_symlink_target(
            link.parent().unwrap_or(&worktree_path),
            &shared_target,
        )?;
        if target == expected {
            output.info(&format!(
                "'{}' is already linked to shared version.",
                rel_path
            ));
            materialized.save(&git_common_dir)?;
            return Ok(());
        }
    }

    // Real file exists — check for differences
    if link.exists() && !link.is_symlink() {
        if !args.force_override {
            // Compare contents
            let differs = if link.is_dir() {
                true // Directory diff is complex; require --override
            } else {
                let local = fs::read(&link)?;
                let shared_content = fs::read(&shared_target)?;
                local != shared_content
            };

            if differs {
                bail!(
                    "Local '{}' differs from shared version. Use `--override` to replace.",
                    rel_path
                );
            }
        }

        // Remove local file/dir to make way for symlink
        if link.is_dir() {
            fs::remove_dir_all(&link)?;
        } else {
            fs::remove_file(&link)?;
        }
    } else if link.is_symlink() {
        // Broken or wrong symlink — remove it
        fs::remove_file(&link)?;
    }

    shared::create_shared_symlink(&worktree_path, rel_path, &git_common_dir)?;
    materialized.remove(rel_path, &worktree_path);
    output.success(&format!("Linked: {} → shared", rel_path));

    materialized.save(&git_common_dir)?;

    Ok(())
}

fn run_status(output: &mut dyn Output) -> Result<()> {
    use crate::styles;

    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let shared_paths = shared::read_shared_paths(&worktree_path)?;
    let worktree_paths = shared::list_worktree_paths()?;
    let materialized = shared::MaterializedState::load(&git_common_dir)?;

    if shared_paths.is_empty() {
        output.info("No shared files declared.");
        return Ok(());
    }

    let use_color = styles::colors_enabled();

    println!("Shared files:\n");

    for rel_path in &shared_paths {
        let shared_target = shared::shared_file_path(&git_common_dir, rel_path);
        let has_source = shared_target.exists();

        if !has_source {
            if use_color {
                println!(
                    "  {}{}{} {}(declared, not yet collected){}",
                    styles::BOLD,
                    rel_path,
                    styles::RESET,
                    styles::DIM,
                    styles::RESET,
                );
            } else {
                println!("  {} (declared, not yet collected)", rel_path);
            }
            println!();
            continue;
        }

        if use_color {
            println!("  {}{}{}", styles::BOLD, rel_path, styles::RESET);
        } else {
            println!("  {}", rel_path);
        }

        for wt in &worktree_paths {
            let wt_name = wt.file_name().unwrap_or_default().to_string_lossy();
            let link = wt.join(rel_path);

            let state = if materialized.is_materialized(rel_path, wt) {
                "materialized"
            } else if link.is_symlink() {
                let target = fs::read_link(&link).ok();
                let expected =
                    shared::relative_symlink_target(link.parent().unwrap_or(wt), &shared_target)
                        .ok();
                if target == expected {
                    "linked"
                } else {
                    "broken"
                }
            } else if link.exists() {
                "conflict"
            } else {
                "missing"
            };

            let colored_state = if use_color {
                match state {
                    "linked" => format!("{}{}{}", styles::GREEN, state, styles::RESET),
                    "materialized" => format!("{}{}{}", styles::CYAN, state, styles::RESET),
                    "missing" => format!("{}{}{}", styles::DIM, state, styles::RESET),
                    "conflict" => format!("{}{}{}", styles::YELLOW, state, styles::RESET),
                    "broken" => format!("{}{}{}", styles::YELLOW, state, styles::RESET),
                    _ => state.to_string(),
                }
            } else {
                state.to_string()
            };

            println!("    {:<24}{}", wt_name, colored_state);
        }

        println!();
    }

    Ok(())
}

fn run_sync(output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let config_root = shared::resolve_config_root(&worktree_path);
    let shared_paths = shared::read_shared_paths(&worktree_path)?;
    let worktree_paths = shared::list_worktree_paths()?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    if shared_paths.is_empty() {
        output.info("No shared files declared.");
        return Ok(());
    }

    // Phase 1: Detect declared-but-uncollected files
    let uncollected = shared::detect_uncollected(&shared_paths, &worktree_paths, &git_common_dir);

    if !uncollected.is_empty() {
        let is_interactive = std::io::IsTerminal::is_terminal(&std::io::stderr())
            && std::env::var("DAFT_TESTING").is_err();

        if is_interactive {
            use crate::output::tui::collect_picker::{run_collect_picker, PickerOutcome};

            match run_collect_picker(uncollected)? {
                PickerOutcome::Decisions(decisions) => {
                    for decision in &decisions {
                        shared::execute_collect(
                            decision,
                            &worktree_paths,
                            &git_common_dir,
                            &config_root,
                            &mut materialized,
                        )?;
                        output.success(&format!("Collected: {}", decision.rel_path));
                    }
                    materialized.save(&git_common_dir)?;
                }
                PickerOutcome::Cancelled => {
                    output.info("Sync cancelled.");
                    return Ok(());
                }
            }
        } else {
            // Non-interactive: report what needs collection
            let count = uncollected.len();
            let files: Vec<&str> = uncollected.iter().map(|u| u.rel_path.as_str()).collect();
            output.info(&format!(
                "{} declared file{} not yet collected: {}",
                count,
                if count == 1 { "" } else { "s" },
                files.join(", ")
            ));
            output.info("Run `daft shared sync` interactively to collect them.");
        }
    }

    // Phase 2: Normal sync — ensure symlinks for all collected shared files
    for wt in &worktree_paths {
        let wt_name = wt
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        for rel_path in &shared_paths {
            if materialized.is_materialized(rel_path, wt) {
                continue;
            }

            match shared::create_shared_symlink(wt, rel_path, &git_common_dir)? {
                shared::LinkResult::Created => {
                    output.success(&format!("{}: {} \u{2192} symlinked", wt_name, rel_path));
                }
                shared::LinkResult::AlreadyLinked => {}
                shared::LinkResult::Conflict => {
                    output.warning(&format!(
                        "{}: {} exists (not shared) \u{2014} run `daft shared link {}` to replace",
                        wt_name, rel_path, rel_path
                    ));
                }
                shared::LinkResult::NoSource => {}
            }
        }
    }

    Ok(())
}
