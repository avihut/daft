use crate::hooks::{HookType, PROJECT_HOOKS_DIR};
use crate::styles::{bold, dim, green, red, yellow};
use crate::{get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Migrate deprecated hook filenames to their new canonical names.
///
/// Must be run from within a worktree. Only migrates hooks in the
/// current worktree's `.daft/hooks/` directory.
pub(super) fn cmd_migrate(dry_run: bool) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let git_dir = get_git_common_dir()?;
    let project_root = git_dir.parent().context("Invalid git directory")?;

    // Determine the current worktree using git rev-parse --show-toplevel
    let toplevel_output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to execute git rev-parse")?;

    if !toplevel_output.status.success() {
        anyhow::bail!("Failed to determine current worktree");
    }

    let worktree_path = PathBuf::from(
        String::from_utf8(toplevel_output.stdout)
            .context("Failed to parse worktree path")?
            .trim(),
    );

    // Verify we're inside a worktree, not at the project root
    if worktree_path == project_root {
        anyhow::bail!(
            "Must be run from within a worktree, not the project root.\n\
             cd into a worktree directory first (e.g., cd main/)."
        );
    }

    let hooks_dir = worktree_path.join(PROJECT_HOOKS_DIR);

    if !hooks_dir.exists() || !hooks_dir.is_dir() {
        println!("{}", dim("No .daft/hooks/ directory in this worktree."));
        return Ok(());
    }

    // Build the rename mapping: (old_name, new_name) for hooks that have deprecated names
    let rename_map: Vec<(&str, &str)> = HookType::all()
        .iter()
        .filter_map(|ht| ht.deprecated_filename().map(|old| (old, ht.filename())))
        .collect();

    let mut renamed = 0u32;
    let mut skipped = 0u32;
    let mut conflicts = 0u32;

    if dry_run {
        println!("{}", bold("Dry run - no files will be changed"));
        println!();
    }

    for &(old_name, new_name) in &rename_map {
        let old_path = hooks_dir.join(old_name);
        let new_path = hooks_dir.join(new_name);

        if !old_path.exists() {
            continue;
        }

        if new_path.exists() {
            // Conflict: both exist
            println!(
                "  {} {}: both '{}' and '{}' exist",
                red("conflict"),
                bold(old_name),
                old_name,
                new_name,
            );
            conflicts += 1;
            continue;
        }

        if dry_run {
            println!("  {} {} -> {}", yellow("would rename"), old_name, new_name,);
            renamed += 1;
        } else {
            match std::fs::rename(&old_path, &new_path) {
                Ok(()) => {
                    println!("  {} {} -> {}", green("renamed"), old_name, new_name,);
                    renamed += 1;
                }
                Err(e) => {
                    println!("  {} {} -> {}: {}", red("error"), old_name, new_name, e);
                    skipped += 1;
                }
            }
        }
    }

    println!();
    if dry_run {
        println!(
            "{} would be renamed, {} conflicts",
            bold(&renamed.to_string()),
            bold(&conflicts.to_string())
        );
    } else if renamed == 0 && conflicts == 0 {
        println!("{}", dim("No deprecated hook files found."));
    } else {
        println!(
            "{} renamed, {} skipped, {} conflicts",
            bold(&renamed.to_string()),
            bold(&skipped.to_string()),
            bold(&conflicts.to_string())
        );
        if renamed > 0 {
            println!(
                "{}",
                dim("Remember to 'git add' the renamed files if they are tracked.")
            );
        }
    }

    Ok(())
}
