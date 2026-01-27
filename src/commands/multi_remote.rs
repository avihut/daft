//! Multi-remote management command.
//!
//! Provides `git daft multi-remote` subcommand for managing multi-remote mode.

use anyhow::Result;
use clap::{Parser, Subcommand};
use daft::{
    get_project_root,
    git::GitCommand,
    is_git_repository,
    multi_remote::{
        config::{set_multi_remote_default, set_multi_remote_enabled},
        migration::{list_worktrees, MigrationPlan},
    },
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
};
use std::io::{self, Write};

#[derive(Parser)]
#[command(name = "multi-remote")]
#[command(about = "Manage multi-remote worktree organization")]
#[command(long_about = r#"
Manages multi-remote mode, which organizes worktrees by remote when working
with multiple remotes (e.g., fork workflows with `origin` and `upstream`).

When multi-remote mode is disabled (default), worktrees are placed directly
under the project root:

    project/
    ├── .git/
    ├── main/
    └── feature/foo/

When multi-remote mode is enabled, worktrees are organized by remote:

    project/
    ├── .git/
    ├── origin/
    │   ├── main/
    │   └── feature/foo/
    └── upstream/
        └── main/

Use `git daft multi-remote enable` to migrate existing worktrees to the
multi-remote layout. Use `git daft multi-remote disable` to migrate back
to the flat layout.
"#)]
pub struct Args {
    #[command(subcommand)]
    command: Option<MultiRemoteCommand>,
}

#[derive(Subcommand)]
enum MultiRemoteCommand {
    /// Enable multi-remote mode and migrate existing worktrees
    #[command(long_about = r#"
Enables multi-remote mode and migrates existing worktrees to the remote-prefixed
directory structure.

Each worktree is moved from `project/branch` to `project/remote/branch`, where
the remote is determined by the branch's upstream tracking configuration or
defaults to the specified default remote.

Use --dry-run to preview the migration without making changes.
"#)]
    Enable {
        #[arg(long, help = "Default remote for new branches (defaults to 'origin')")]
        default: Option<String>,

        #[arg(long, help = "Preview changes without executing")]
        dry_run: bool,

        #[arg(short = 'f', long, help = "Skip confirmation")]
        force: bool,
    },

    /// Disable multi-remote mode and flatten worktree structure
    #[command(long_about = r#"
Disables multi-remote mode and migrates worktrees back to the flat directory
structure.

Each worktree is moved from `project/remote/branch` back to `project/branch`.
This command requires that only one remote is configured, as the flat structure
cannot distinguish between worktrees from different remotes.

Use --dry-run to preview the migration without making changes.
"#)]
    Disable {
        #[arg(long, help = "Preview changes without executing")]
        dry_run: bool,

        #[arg(short = 'f', long, help = "Skip confirmation")]
        force: bool,
    },

    /// Show current multi-remote configuration
    Status,

    /// Change the default remote for new branches
    SetDefault {
        #[arg(help = "Remote name to use as default")]
        remote: String,
    },
}

pub fn run() -> Result<()> {
    // Skip "daft" and "multi-remote" from args
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);

    match args.command {
        Some(MultiRemoteCommand::Enable {
            default,
            dry_run,
            force,
        }) => cmd_enable(default, dry_run, force),
        Some(MultiRemoteCommand::Disable { dry_run, force }) => cmd_disable(dry_run, force),
        Some(MultiRemoteCommand::Status) => cmd_status(),
        Some(MultiRemoteCommand::SetDefault { remote }) => cmd_set_default(&remote),
        None => cmd_status(), // Default to status
    }
}

/// Enable multi-remote mode.
fn cmd_enable(default_remote: Option<String>, dry_run: bool, skip_confirm: bool) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, true);
    let mut output = CliOutput::new(config);

    let project_root = get_project_root()?;
    let git = GitCommand::new(false);

    // Check if already enabled
    if settings.multi_remote_enabled {
        output.result("Multi-remote mode is already enabled");
        return Ok(());
    }

    // Get list of remotes
    let remotes = git.remote_list()?;
    if remotes.is_empty() {
        anyhow::bail!("No remotes configured. Add a remote before enabling multi-remote mode.");
    }

    let default = default_remote.unwrap_or_else(|| "origin".to_string());
    if !remotes.contains(&default) {
        output.warning(&format!(
            "Default remote '{}' does not exist. Available remotes: {}",
            default,
            remotes.join(", ")
        ));
    }

    output.step(&format!("Project root: {}", project_root.display()));
    output.step(&format!("Remotes: {}", remotes.join(", ")));
    output.step(&format!("Default remote: {}", default));

    // List existing worktrees
    let worktrees = list_worktrees(&git, &project_root)?;
    let worktree_count = worktrees
        .iter()
        .filter(|w| !w.path.ends_with(".git"))
        .count();

    output.step(&format!("Found {} worktrees to migrate", worktree_count));

    // Create migration plan
    let plan = MigrationPlan::for_enable(&project_root, &worktrees, &default);

    if plan.is_empty() {
        output.step("No migration needed");
    } else {
        plan.preview(&mut output);
    }

    if dry_run {
        output.result("Dry run complete - no changes made");
        return Ok(());
    }

    if !plan.is_empty() && !skip_confirm {
        print!("\nProceed with migration? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            output.result("Aborted");
            return Ok(());
        }
    }

    // Execute migration
    if !plan.is_empty() {
        output.step("Executing migration...");
        plan.execute(&git, &mut output)?;
    }

    // Update config
    output.step("Updating git config...");
    set_multi_remote_enabled(&git, true)?;
    set_multi_remote_default(&git, &default)?;

    output.result(&format!(
        "Multi-remote mode enabled (default remote: {})",
        default
    ));
    Ok(())
}

/// Disable multi-remote mode.
fn cmd_disable(dry_run: bool, skip_confirm: bool) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, true);
    let mut output = CliOutput::new(config);

    let project_root = get_project_root()?;
    let git = GitCommand::new(false);

    // Check if already disabled
    if !settings.multi_remote_enabled {
        output.result("Multi-remote mode is already disabled");
        return Ok(());
    }

    // Check number of remotes
    let remotes = git.remote_list()?;
    if remotes.len() > 1 {
        output.warning(&format!(
            "Multiple remotes configured: {}",
            remotes.join(", ")
        ));
        output.warning("Disabling multi-remote mode with multiple remotes may cause confusion.");
        output.warning("Consider removing unused remotes first.");
    }

    // List existing worktrees
    let worktrees = list_worktrees(&git, &project_root)?;
    let worktree_count = worktrees
        .iter()
        .filter(|w| !w.path.ends_with(".git"))
        .count();

    output.step(&format!("Found {} worktrees to migrate", worktree_count));

    // Create migration plan
    let plan = MigrationPlan::for_disable(&project_root, &worktrees)?;

    if plan.is_empty() {
        output.step("No migration needed");
    } else {
        plan.preview(&mut output);
    }

    if dry_run {
        output.result("Dry run complete - no changes made");
        return Ok(());
    }

    if !plan.is_empty() && !skip_confirm {
        print!("\nProceed with migration? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            output.result("Aborted");
            return Ok(());
        }
    }

    // Execute migration
    if !plan.is_empty() {
        output.step("Executing migration...");
        plan.execute(&git, &mut output)?;
    }

    // Update config
    output.step("Updating git config...");
    set_multi_remote_enabled(&git, false)?;

    output.result("Multi-remote mode disabled");
    Ok(())
}

/// Show current status.
fn cmd_status() -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let settings = DaftSettings::load()?;
    let project_root = get_project_root()?;
    let git = GitCommand::new(true);

    println!("Repository: {}", project_root.display());
    println!();

    // Multi-remote status
    let status = if settings.multi_remote_enabled {
        "enabled"
    } else {
        "disabled"
    };
    println!("Multi-remote mode: {status}");
    println!("Default remote: {}", settings.multi_remote_default);
    println!();

    // List remotes
    let remotes = git.remote_list()?;
    println!("Configured remotes:");
    if remotes.is_empty() {
        println!("  (none)");
    } else {
        for remote in &remotes {
            let marker = if remote == &settings.multi_remote_default {
                " (default)"
            } else {
                ""
            };
            println!("  - {remote}{marker}");
        }
    }
    println!();

    // List worktrees
    let worktrees = list_worktrees(&git, &project_root)?;
    let regular_worktrees: Vec<_> = worktrees
        .iter()
        .filter(|w| !w.path.ends_with(".git"))
        .collect();

    println!("Worktrees:");
    if regular_worktrees.is_empty() {
        println!("  (none)");
    } else {
        for wt in &regular_worktrees {
            let relative = wt
                .path
                .strip_prefix(&project_root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| wt.path.display().to_string());

            let branch_info = wt
                .branch
                .as_ref()
                .map(|b| format!(" [{b}]"))
                .unwrap_or_default();

            let remote_info = wt
                .remote
                .as_ref()
                .map(|r| format!(" ({})", r))
                .unwrap_or_default();

            println!("  {relative}{branch_info}{remote_info}");
        }
    }
    println!();

    // Commands
    if settings.multi_remote_enabled {
        println!("To disable multi-remote mode:");
        println!("  git daft multi-remote disable");
    } else {
        println!("To enable multi-remote mode:");
        println!("  git daft multi-remote enable");
    }

    Ok(())
}

/// Set the default remote.
fn cmd_set_default(remote: &str) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let git = GitCommand::new(true);

    // Verify remote exists
    let remotes = git.remote_list()?;
    if !remotes.contains(&remote.to_string()) {
        anyhow::bail!(
            "Remote '{}' does not exist. Available remotes: {}",
            remote,
            remotes.join(", ")
        );
    }

    set_multi_remote_default(&git, remote)?;
    println!("Default remote set to: {remote}");

    Ok(())
}
