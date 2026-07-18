//! Multi-remote management command.
//!
//! Provides `git daft multi-remote` subcommand for managing multi-remote mode.

use crate::{
    core::OutputSink,
    core::worktree::ports::NoopStageRunner,
    core::worktree::push::{HookVerdict, PushAction, push_with_hooks},
    get_project_root,
    git::GitCommand,
    is_git_repository,
    multi_remote::{
        config::{set_multi_remote_default, set_multi_remote_enabled},
        migration::{MigrationPlan, list_worktrees},
        path::calculate_worktree_path,
    },
    output::{
        CliOutput, Output, OutputConfig,
        emit::{self, Cell, EmitArgs, EmitPayload, Section, Table},
    },
    settings::DaftSettings,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
    Status {
        #[command(flatten)]
        emit: EmitArgs,
    },

    /// Change the default remote for new branches
    SetDefault {
        #[arg(help = "Remote name to use as default")]
        remote: String,
    },

    /// Move a worktree to a different remote folder
    #[command(long_about = r#"
Moves a worktree from one remote folder to another. This is useful when:

- You forked a branch and want to organize it under a different remote
- You're transferring a feature branch from your fork to upstream
- You want to reorganize worktrees after adding a new remote

The worktree is physically moved on disk, and git's internal worktree
records are updated accordingly.

Options like --set-upstream can update the branch's tracking configuration
to match the new remote organization.
"#)]
    Move {
        #[arg(help = "Branch name or worktree path to move")]
        branch: String,

        #[arg(long, help = "Target remote folder")]
        to: String,

        #[arg(
            long,
            help = "Also update the branch's upstream tracking to the new remote"
        )]
        set_upstream: bool,

        #[arg(long, help = "Push the branch to the new remote")]
        push: bool,

        #[arg(long, help = "Delete the branch from the old remote after pushing")]
        delete_old: bool,

        #[arg(long, help = "Skip the repo's pre-push hook on remote operations")]
        no_verify: bool,

        #[arg(long, help = "Preview changes without executing")]
        dry_run: bool,

        #[arg(short = 'f', long, help = "Skip confirmation")]
        force: bool,
    },
}

pub fn run() -> Result<()> {
    // Skip "daft" and "multi-remote" from args
    let args: Vec<String> = crate::cli::argv().iter().skip(1).cloned().collect();
    let args = Args::parse_from(args);

    match args.command {
        Some(MultiRemoteCommand::Enable {
            default,
            dry_run,
            force,
        }) => cmd_enable(default, dry_run, force),
        Some(MultiRemoteCommand::Disable { dry_run, force }) => cmd_disable(dry_run, force),
        Some(MultiRemoteCommand::Status { emit }) => cmd_status(&emit),
        Some(MultiRemoteCommand::SetDefault { remote }) => cmd_set_default(&remote),
        Some(MultiRemoteCommand::Move {
            branch,
            to,
            set_upstream,
            push,
            delete_old,
            no_verify,
            dry_run,
            force,
        }) => cmd_move(
            &branch,
            &to,
            set_upstream,
            push,
            delete_old,
            no_verify,
            dry_run,
            force,
        ),
        None => cmd_status(&EmitArgs::default()), // Default to status
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
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

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
        plan.preview(&mut OutputSink(&mut output));
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
        plan.execute(&git, &mut OutputSink(&mut output))?;
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
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

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
        plan.preview(&mut OutputSink(&mut output));
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
        plan.execute(&git, &mut OutputSink(&mut output))?;
    }

    // Update config
    output.step("Updating git config...");
    set_multi_remote_enabled(&git, false)?;

    output.result("Multi-remote mode disabled");
    Ok(())
}

/// Show current status.
fn cmd_status(emit_args: &EmitArgs) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let settings = DaftSettings::load()?;
    let project_root = get_project_root()?;
    let git = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    // List remotes
    let remotes = git.remote_list()?;
    // List worktrees
    let worktrees = list_worktrees(&git, &project_root)?;
    let regular_worktrees: Vec<_> = worktrees
        .iter()
        .filter(|w| !w.path.ends_with(".git"))
        .collect();

    if emit_args.is_structured() {
        // Section 1: remotes (name + is_default; url not available from remote_list)
        let mut remotes_table = Table::new(["name", "is_default"]);
        for remote in &remotes {
            remotes_table = remotes_table.row([
                Cell::str(remote),
                Cell::bool(remote == &settings.multi_remote_default),
            ]);
        }

        // Section 2: worktrees (branch, remote, path)
        let mut worktrees_table = Table::new(["branch", "remote", "path"]);
        for wt in &regular_worktrees {
            worktrees_table = worktrees_table.row([
                Cell::str(wt.branch.as_deref().unwrap_or("")),
                Cell::str(wt.remote.as_deref().unwrap_or("")),
                Cell::str(wt.path.display().to_string()),
            ]);
        }

        let sections = vec![
            Section::new("remotes", EmitPayload::Tabular(remotes_table)),
            Section::new("worktrees", EmitPayload::Tabular(worktrees_table)),
        ];
        return emit::emit_and_handle(
            "multi-remote status",
            EmitPayload::Sectioned(sections),
            emit_args,
            &mut std::io::stdout(),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    }

    output.detail("Repository", &project_root.display().to_string());
    output.info("");

    // Multi-remote status
    let status = if settings.multi_remote_enabled {
        "enabled"
    } else {
        "disabled"
    };
    output.detail("Multi-remote mode", status);
    output.detail("Default remote", &settings.multi_remote_default);
    output.info("");

    // List remotes
    output.info("Configured remotes:");
    if remotes.is_empty() {
        output.info("  (none)");
    } else {
        for remote in &remotes {
            let marker = if remote == &settings.multi_remote_default {
                " (default)"
            } else {
                ""
            };
            output.list_item(&format!("{remote}{marker}"));
        }
    }
    output.info("");

    // List worktrees
    output.info("Worktrees:");
    if regular_worktrees.is_empty() {
        output.info("  (none)");
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

            output.info(&format!("  {relative}{branch_info}{remote_info}"));
        }
    }
    output.info("");

    // Commands
    let exe = crate::cli_label();
    if settings.multi_remote_enabled {
        output.info("To disable multi-remote mode:");
        output.info(&format!("  {exe} multi-remote disable"));
    } else {
        output.info("To enable multi-remote mode:");
        output.info(&format!("  {exe} multi-remote enable"));
    }

    Ok(())
}

/// Set the default remote.
fn cmd_set_default(remote: &str) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let settings = DaftSettings::load()?;
    let git = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let mut output = CliOutput::new(OutputConfig::new(false, false));

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
    output.result(&format!("Default remote set to: {remote}"));

    Ok(())
}

/// Move a worktree to a different remote folder.
#[allow(clippy::fn_params_excessive_bools)]
#[allow(clippy::too_many_arguments)]
fn cmd_move(
    branch: &str,
    to_remote: &str,
    set_upstream: bool,
    push: bool,
    delete_old: bool,
    no_verify: bool,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(false, true);
    let mut output = CliOutput::new(config);

    let project_root = get_project_root()?;
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

    // Verify multi-remote mode is enabled
    if !settings.multi_remote_enabled {
        anyhow::bail!(
            "Multi-remote mode is not enabled.\n\
             Run '{}' first, or use regular git commands.",
            crate::daft_cmd("multi-remote enable")
        );
    }

    // Verify target remote exists
    let remotes = git.remote_list()?;
    if !remotes.contains(&to_remote.to_string()) {
        anyhow::bail!(
            "Remote '{}' does not exist. Available remotes: {}",
            to_remote,
            remotes.join(", ")
        );
    }

    // Find the worktree for this branch
    let worktree_path = git
        .resolve_worktree_path(branch, &project_root)
        .context(format!("Could not find worktree for branch '{}'", branch))?;

    // Get branch name from worktree
    let branch_name =
        get_branch_name_for_worktree(&git, &worktree_path)?.unwrap_or_else(|| branch.to_string());

    // Determine current remote (from path structure)
    let current_remote = worktree_path
        .strip_prefix(&project_root)
        .ok()
        .and_then(|p| p.components().next())
        .and_then(|c| c.as_os_str().to_str())
        .map(String::from);

    output.step(&format!("Branch: {}", branch_name));
    output.step(&format!("Current location: {}", worktree_path.display()));

    if let Some(ref remote) = current_remote {
        output.step(&format!("Current remote folder: {}", remote));
    }
    output.step(&format!("Target remote folder: {}", to_remote));

    // Check if already in target location
    if current_remote.as_deref() == Some(to_remote) {
        output.result("Worktree is already in the target remote folder");
        return Ok(());
    }

    // Calculate new path
    let new_path = calculate_worktree_path(&project_root, &branch_name, to_remote, true);
    output.step(&format!("New location: {}", new_path.display()));

    // Check if target path already exists
    if new_path.exists() {
        anyhow::bail!(
            "Target path already exists: {}\n\
             Remove the existing worktree first or choose a different remote.",
            new_path.display()
        );
    }

    // Summary of actions
    output.step("");
    output.step("Actions to perform:");
    output.step(&format!(
        "  1. Move worktree: {} -> {}",
        worktree_path.display(),
        new_path.display()
    ));

    if set_upstream {
        output.step(&format!("  2. Set upstream: {}/{}", to_remote, branch_name));
    }

    if push {
        output.step(&format!("  3. Push to remote: {}", to_remote));
    }

    if delete_old && let Some(ref old_remote) = current_remote {
        output.step(&format!(
            "  4. Delete from old remote: {}/{}",
            old_remote, branch_name
        ));
    }

    if dry_run {
        output.result("Dry run complete - no changes made");
        return Ok(());
    }

    if !skip_confirm {
        print!("\nProceed? [y/N] ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            output.result("Aborted");
            return Ok(());
        }
    }

    // Execute the move
    output.step("Moving worktree...");

    // Ensure parent directory exists
    if let Some(parent) = new_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    git.worktree_move(&worktree_path, &new_path)
        .context("Failed to move worktree")?;

    // Set upstream if requested
    if set_upstream {
        output.step(&format!(
            "Setting upstream to {}/{}...",
            to_remote, branch_name
        ));

        // Need to change to the worktree directory to set upstream
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&new_path)?;

        let result = git.set_upstream(to_remote, &branch_name);

        // Restore original directory
        std::env::set_current_dir(&original_dir)?;

        if let Err(e) = result {
            output.warning(&format!("Failed to set upstream: {}", e));
        }
    }

    // Remote pushes honor the repo's pre-push hook (#599); a failure with
    // the hook in effect escalates after the move's local steps complete.
    let push_presenter: Option<std::sync::Arc<dyn crate::executor::presenter::JobPresenter>> =
        if (push || delete_old) && !no_verify && git.pre_push_hook_exists(&new_path) {
            let p: std::sync::Arc<dyn crate::executor::presenter::JobPresenter> =
                crate::executor::cli_presenter::CliPresenter::auto(
                    &crate::settings::HookOutputConfig::default(),
                );
            Some(p)
        } else {
            None
        };
    let mut push_gate_error: Option<String> = None;

    // Push if requested
    if push {
        output.step(&format!("Pushing to {}...", to_remote));

        match push_with_hooks(
            &git,
            PushAction::SetUpstream {
                remote: to_remote,
                branch: &branch_name,
                force_with_lease: false,
            },
            &new_path,
            !no_verify,
            &NoopStageRunner,
            push_presenter.as_ref(),
            None,
        ) {
            Ok(outcome) => {
                if let Some(msg) = outcome.failure {
                    if matches!(outcome.hook, HookVerdict::Rejected | HookVerdict::Passed) {
                        let hint = if outcome.hook.no_verify_might_help() {
                            " (or re-run with --no-verify to bypass the hook)"
                        } else {
                            ""
                        };
                        push_gate_error = Some(format!(
                            "Could not push '{to_remote}/{branch_name}': {msg} ({}). \
                             The worktree was moved; push manually with: \
                             git push --set-upstream {to_remote} {branch_name}{hint}",
                            outcome.hook.failure_cause(),
                        ));
                    } else {
                        output.warning(&format!("Failed to push: {}", msg));
                    }
                }
            }
            Err(e) => {
                output.warning(&format!("Failed to push: {}", e));
            }
        }
    }

    // Delete from old remote if requested (skipped if the push was gated —
    // the same hook would gate this push too).
    if delete_old
        && push_gate_error.is_none()
        && let Some(old_remote) = current_remote
    {
        output.step(&format!(
            "Deleting from old remote {}/{}...",
            old_remote, branch_name
        ));

        match push_with_hooks(
            &git,
            PushAction::Delete {
                remote: &old_remote,
                branch: &branch_name,
            },
            &new_path,
            !no_verify,
            &NoopStageRunner,
            push_presenter.as_ref(),
            None,
        ) {
            Ok(outcome) => {
                if let Some(msg) = outcome.failure {
                    if matches!(outcome.hook, HookVerdict::Rejected | HookVerdict::Passed) {
                        let hint = if outcome.hook.no_verify_might_help() {
                            " (or re-run with --no-verify to bypass the hook)"
                        } else {
                            ""
                        };
                        push_gate_error = Some(format!(
                            "Could not delete '{old_remote}/{branch_name}': {msg} ({}). \
                             Delete it manually with: \
                             git push {old_remote} --delete {branch_name}{hint}",
                            outcome.hook.failure_cause(),
                        ));
                    } else {
                        output.warning(&format!("Failed to delete from old remote: {}", msg));
                    }
                }
            }
            Err(e) => {
                output.warning(&format!("Failed to delete from old remote: {}", e));
            }
        }
    }

    // Clean up empty remote directories
    if let Some(old_remote) = worktree_path
        .strip_prefix(&project_root)
        .ok()
        .and_then(|p| p.components().next())
        .and_then(|c| c.as_os_str().to_str())
    {
        let old_remote_dir = project_root.join(old_remote);
        if old_remote_dir.exists()
            && let Ok(mut entries) = std::fs::read_dir(&old_remote_dir)
            && entries.next().is_none()
        {
            // Directory is empty, remove it
            let _ = std::fs::remove_dir(&old_remote_dir);
        }
    }

    output.result(&format!("Worktree moved to {}/{}", to_remote, branch_name));

    // The move's local steps are complete — surface a deferred pre-push
    // gate refusal as the command's failure (#599).
    if let Some(message) = push_gate_error {
        anyhow::bail!(message);
    }

    Ok(())
}

/// Get the branch name for a worktree.
///
/// Delegates the porcelain parse to the shared
/// [`crate::core::worktree::porcelain::parse_worktree_list_porcelain`] and
/// returns the matching worktree's (short) branch, or `None` when the path is
/// not found or is detached/bare.
fn get_branch_name_for_worktree(
    git: &GitCommand,
    worktree_path: &std::path::Path,
) -> Result<Option<String>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    Ok(
        crate::core::worktree::porcelain::parse_worktree_list_porcelain(&porcelain_output)
            .into_iter()
            .find(|e| e.path.as_path() == worktree_path)
            .and_then(|e| e.branch),
    )
}
