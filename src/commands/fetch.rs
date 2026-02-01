//! git-worktree-fetch - Update worktree branches from remote tracking branches
//!
//! This command fetches and pulls updates for one or more worktrees by navigating
//! to each target worktree and running `git pull` with configurable options.

use crate::{
    get_project_root, git::GitCommand, is_git_repository, log_error, log_info, log_warning,
    logging::init_logging, settings::DaftSettings, utils::*, WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "git-worktree-fetch")]
#[command(version = crate::VERSION)]
#[command(about = "Update worktree branches from their remote tracking branches")]
#[command(long_about = r#"
Updates worktree branches by pulling from their remote tracking branches.

For each target worktree, the command navigates to that directory and runs
`git pull` with the configured options. By default, only fast-forward updates
are allowed (--ff-only).

Targets can be specified by worktree directory name or branch name. If no
targets are specified and --all is not used, the current worktree is updated.

Worktrees with uncommitted changes are skipped unless --force is specified.
Use --dry-run to preview what would be done without making changes.

Arguments after -- are passed directly to git pull, allowing full control
over the pull behavior.
"#)]
pub struct Args {
    /// Target worktree(s) by directory name or branch name
    #[arg(value_name = "TARGETS")]
    targets: Vec<String>,

    /// Update all worktrees
    #[arg(long, help = "Update all worktrees")]
    all: bool,

    /// Update even if worktree has uncommitted changes
    #[arg(short = 'f', long, help = "Update even with uncommitted changes")]
    force: bool,

    /// Show what would be done without making changes
    #[arg(long, help = "Show what would be done")]
    dry_run: bool,

    /// Use git pull --rebase
    #[arg(long, help = "Use git pull --rebase")]
    rebase: bool,

    /// Use git pull --autostash
    #[arg(long, help = "Use git pull --autostash")]
    autostash: bool,

    /// Only fast-forward (default behavior)
    #[arg(long, help = "Only fast-forward (default)")]
    ff_only: bool,

    /// Allow merge commits (disables --ff-only)
    #[arg(long, help = "Allow merge commits")]
    no_ff_only: bool,

    /// Be verbose; show detailed progress
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    /// Suppress non-error output
    #[arg(short, long, help = "Suppress non-error output")]
    quiet: bool,

    /// Additional arguments to pass to git pull
    #[arg(last = true, value_name = "PULL_ARGS")]
    pull_args: Vec<String>,
}

/// Result of a fetch operation for a single worktree
#[derive(Debug)]
struct FetchResult {
    worktree_name: String,
    success: bool,
    message: String,
    skipped: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-fetch"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    // Load settings from git config
    let settings = DaftSettings::load()?;

    run_fetch(&args, &settings)
}

fn run_fetch(args: &Args, settings: &DaftSettings) -> Result<()> {
    let config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: args.quiet,
    };
    let git = GitCommand::new(config.quiet);

    // Save original directory to return to
    let original_dir = get_current_directory()?;
    let project_root = get_project_root()?;

    // Determine targets
    let targets = determine_targets(args, &git, &project_root)?;

    if targets.is_empty() {
        if !args.quiet {
            println!("No worktrees to update.");
        }
        return Ok(());
    }

    // Build pull arguments from config and CLI flags
    let pull_args = build_pull_args(args, settings);

    if args.verbose && !args.quiet {
        println!("--> Pull arguments: {}", pull_args.join(" "));
    }

    // Process each target
    let mut results: Vec<FetchResult> = Vec::new();

    for target_path in &targets {
        let worktree_name = target_path
            .strip_prefix(&project_root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("unknown")
            .to_string();

        let result = process_worktree(
            &git,
            target_path,
            &worktree_name,
            &pull_args,
            args.force,
            args.dry_run,
            args.quiet,
            args.verbose,
        );

        results.push(result);
    }

    // Return to original directory
    change_directory(&original_dir)?;

    // Print summary
    print_summary(&results, args.quiet, args.verbose);

    // Check if any failures occurred
    let failures = results.iter().filter(|r| !r.success && !r.skipped).count();
    if failures > 0 {
        anyhow::bail!("{} worktree(s) failed to update", failures);
    }

    Ok(())
}

/// Determine which worktrees to update based on arguments
fn determine_targets(args: &Args, git: &GitCommand, project_root: &Path) -> Result<Vec<PathBuf>> {
    if args.all {
        // Get all worktrees
        get_all_worktrees(git)
    } else if args.targets.is_empty() {
        // Current worktree
        let current = git.get_current_worktree_path()?;
        Ok(vec![current])
    } else {
        // Resolve specified targets
        let mut resolved: Vec<PathBuf> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for target in &args.targets {
            match git.resolve_worktree_path(target, project_root) {
                Ok(path) => resolved.push(path),
                Err(e) => errors.push(format!("'{}': {}", target, e)),
            }
        }

        if !errors.is_empty() {
            for error in &errors {
                log_error!("Failed to resolve target {}", error);
            }
            anyhow::bail!("Failed to resolve {} target(s)", errors.len());
        }

        Ok(resolved)
    }
}

/// Get all worktrees from git worktree list
fn get_all_worktrees(git: &GitCommand) -> Result<Vec<PathBuf>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut worktrees: Vec<PathBuf> = Vec::new();
    let mut current_worktree: Option<PathBuf> = None;
    let mut is_bare = false;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            // Save previous worktree if it wasn't bare
            if let Some(path) = current_worktree.take() {
                if !is_bare {
                    worktrees.push(path);
                }
            }
            // Start tracking new worktree
            current_worktree = Some(PathBuf::from(worktree_path));
            is_bare = false;
        } else if line == "bare" {
            // Mark current worktree as bare (will be skipped)
            is_bare = true;
        }
    }

    // Don't forget the last worktree
    if let Some(path) = current_worktree {
        if !is_bare {
            worktrees.push(path);
        }
    }

    Ok(worktrees)
}

/// Build pull arguments from config and CLI flags
fn build_pull_args(args: &Args, settings: &DaftSettings) -> Vec<String> {
    let mut pull_args: Vec<String> = Vec::new();

    // Parse config args into a set for easy checking
    let config_args: Vec<&str> = settings.fetch_args.split_whitespace().collect();
    let config_has_rebase = config_args.contains(&"--rebase");
    let config_has_ff_only = config_args.contains(&"--ff-only");
    let config_has_autostash = config_args.contains(&"--autostash");

    // Determine rebase behavior
    // CLI --rebase always adds --rebase
    // CLI --no-ff-only without --rebase keeps config rebase setting
    if args.rebase {
        pull_args.push("--rebase".to_string());
    } else if config_has_rebase && !args.ff_only {
        // Config has rebase and user didn't explicitly request --ff-only
        pull_args.push("--rebase".to_string());
    }

    // Determine ff-only behavior
    // --no-ff-only disables ff-only
    // --ff-only or config ff-only enables it (unless rebase is used)
    if !args.no_ff_only && !args.rebase && !config_has_rebase {
        if args.ff_only || config_has_ff_only {
            pull_args.push("--ff-only".to_string());
        } else {
            // Default to ff-only if no other merge strategy specified
            pull_args.push("--ff-only".to_string());
        }
    }

    // Autostash
    if args.autostash || config_has_autostash {
        pull_args.push("--autostash".to_string());
    }

    // Add pass-through arguments
    for arg in &args.pull_args {
        pull_args.push(arg.clone());
    }

    pull_args
}

/// Process a single worktree
#[allow(clippy::too_many_arguments)]
fn process_worktree(
    git: &GitCommand,
    target_path: &Path,
    worktree_name: &str,
    pull_args: &[String],
    force: bool,
    dry_run: bool,
    quiet: bool,
    verbose: bool,
) -> FetchResult {
    if !quiet {
        println!("--> Processing '{}'...", worktree_name);
    }

    // Change to worktree directory
    if let Err(e) = change_directory(target_path) {
        return FetchResult {
            worktree_name: worktree_name.to_string(),
            success: false,
            message: format!("Failed to change to directory: {}", e),
            skipped: false,
        };
    }

    // Check for uncommitted changes
    match git.has_uncommitted_changes() {
        Ok(has_changes) => {
            if has_changes && !force {
                if !quiet {
                    log_warning!(
                        "Skipping '{}': has uncommitted changes (use --force to update anyway)",
                        worktree_name
                    );
                }
                return FetchResult {
                    worktree_name: worktree_name.to_string(),
                    success: true,
                    message: "Skipped: uncommitted changes".to_string(),
                    skipped: true,
                };
            }
        }
        Err(e) => {
            return FetchResult {
                worktree_name: worktree_name.to_string(),
                success: false,
                message: format!("Failed to check status: {}", e),
                skipped: false,
            };
        }
    }

    // Check if branch has an upstream
    if check_has_upstream(git).is_err() {
        if !quiet {
            log_warning!(
                "Skipping '{}': no tracking branch configured",
                worktree_name
            );
        }
        return FetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: "Skipped: no tracking branch".to_string(),
            skipped: true,
        };
    }

    // Dry run mode
    if dry_run {
        if !quiet {
            log_info!(
                "Would update '{}' with: git pull {}",
                worktree_name,
                pull_args.join(" ")
            );
        }
        return FetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: "Dry run: would update".to_string(),
            skipped: true,
        };
    }

    // Run git pull
    let pull_args_refs: Vec<&str> = pull_args.iter().map(|s| s.as_str()).collect();
    match git.pull(&pull_args_refs) {
        Ok(output) => {
            if verbose && !quiet && !output.trim().is_empty() {
                println!("{}", output);
            }
            log_info!("Updated '{}'", worktree_name);
            FetchResult {
                worktree_name: worktree_name.to_string(),
                success: true,
                message: "Updated successfully".to_string(),
                skipped: false,
            }
        }
        Err(e) => {
            log_error!("Failed to update '{}': {}", worktree_name, e);
            FetchResult {
                worktree_name: worktree_name.to_string(),
                success: false,
                message: format!("Failed: {}", e),
                skipped: false,
            }
        }
    }
}

/// Check if the current branch has an upstream tracking branch
fn check_has_upstream(git: &GitCommand) -> Result<()> {
    // Get current branch
    let branch = git.symbolic_ref_short_head()?;

    // Check if upstream is configured by looking for the tracking info
    // We use git config to check branch.<name>.remote
    let remote_key = format!("branch.{}.remote", branch);
    if git.config_get(&remote_key)?.is_none() {
        anyhow::bail!("No upstream configured for branch '{}'", branch);
    }

    Ok(())
}

/// Print summary of fetch operations
fn print_summary(results: &[FetchResult], quiet: bool, verbose: bool) {
    if quiet {
        return;
    }

    let updated: Vec<_> = results.iter().filter(|r| r.success && !r.skipped).collect();
    let skipped: Vec<_> = results.iter().filter(|r| r.skipped).collect();
    let failed: Vec<_> = results
        .iter()
        .filter(|r| !r.success && !r.skipped)
        .collect();

    // In verbose mode, show details for each worktree
    if verbose {
        if !updated.is_empty() {
            println!("Updated:");
            for r in &updated {
                println!("  {} - {}", r.worktree_name, r.message);
            }
        }
        if !skipped.is_empty() {
            println!("Skipped:");
            for r in &skipped {
                println!("  {} - {}", r.worktree_name, r.message);
            }
        }
        if !failed.is_empty() {
            println!("Failed:");
            for r in &failed {
                println!("  {} - {}", r.worktree_name, r.message);
            }
        }
    }

    println!("---");
    if failed.is_empty() {
        if updated.is_empty() && !skipped.is_empty() {
            println!("Done! {} worktree(s) skipped, none updated.", skipped.len());
        } else if !skipped.is_empty() {
            println!(
                "Done! {} worktree(s) updated, {} skipped.",
                updated.len(),
                skipped.len()
            );
        } else {
            println!("Done! {} worktree(s) updated.", updated.len());
        }
    } else {
        eprintln!(
            "Completed with {} updated, {} skipped, {} failed.",
            updated.len(),
            skipped.len(),
            failed.len()
        );
    }
}
