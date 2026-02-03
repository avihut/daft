//! git-worktree-fetch - Update worktree branches from remote tracking branches
//!
//! This command fetches and pulls updates for one or more worktrees by navigating
//! to each target worktree and running `git pull` with configurable options.

use crate::{
    get_project_root,
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    utils::*,
    WorktreeConfig,
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

    let config = OutputConfig::new(args.quiet, args.verbose);
    let mut output = CliOutput::new(config);

    run_fetch(&args, &settings, &mut output)
}

fn run_fetch(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    let config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: args.quiet,
    };
    let git = GitCommand::new(config.quiet);

    // Save original directory to return to
    let original_dir = get_current_directory()?;
    let project_root = get_project_root()?;

    // Determine targets
    let targets = determine_targets(args, &git, &project_root, output)?;

    if targets.is_empty() {
        output.info("No worktrees to update.");
        return Ok(());
    }

    // Build pull arguments from config and CLI flags
    let pull_args = build_pull_args(args, settings);

    output.step(&format!("Pull arguments: {}", pull_args.join(" ")));

    // Print header
    output.info(&format!("Fetching {}", config.remote_name));
    if let Ok(url) = git.remote_get_url(&config.remote_name) {
        output.info(&format!("URL: {url}"));
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

        let result = process_worktree(&git, target_path, &worktree_name, &pull_args, args, output);

        results.push(result);
    }

    // Return to original directory
    change_directory(&original_dir)?;

    // Print summary
    print_summary(&results, output);

    // Check if any failures occurred
    let failures = results.iter().filter(|r| !r.success && !r.skipped).count();
    if failures > 0 {
        anyhow::bail!("{} worktree(s) failed to update", failures);
    }

    Ok(())
}

/// Determine which worktrees to update based on arguments
fn determine_targets(
    args: &Args,
    git: &GitCommand,
    project_root: &Path,
    output: &mut dyn Output,
) -> Result<Vec<PathBuf>> {
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
                output.error(&format!("Failed to resolve target {error}"));
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
fn process_worktree(
    git: &GitCommand,
    target_path: &Path,
    worktree_name: &str,
    pull_args: &[String],
    args: &Args,
    output: &mut dyn Output,
) -> FetchResult {
    output.step(&format!("Processing '{worktree_name}'..."));

    // Change to worktree directory
    if let Err(e) = change_directory(target_path) {
        output.error(&format!(
            "Failed to change to directory for '{worktree_name}': {e}"
        ));
        output.info(&format!(" * [failed] {worktree_name}"));
        return FetchResult {
            worktree_name: worktree_name.to_string(),
            success: false,
            message: format!("Failed to change to directory: {e}"),
            skipped: false,
        };
    }

    // Check for uncommitted changes
    match git.has_uncommitted_changes() {
        Ok(has_changes) => {
            if has_changes && !args.force {
                output.warning(&format!(
                    "Skipping '{worktree_name}': has uncommitted changes (use --force to update anyway)"
                ));
                output.info(&format!(
                    " * [skipped] {worktree_name} (uncommitted changes)"
                ));
                return FetchResult {
                    worktree_name: worktree_name.to_string(),
                    success: true,
                    message: "Skipped: uncommitted changes".to_string(),
                    skipped: true,
                };
            }
        }
        Err(e) => {
            output.error(&format!(
                "Failed to check status for '{worktree_name}': {e}"
            ));
            output.info(&format!(" * [failed] {worktree_name}"));
            return FetchResult {
                worktree_name: worktree_name.to_string(),
                success: false,
                message: format!("Failed to check status: {e}"),
                skipped: false,
            };
        }
    }

    // Check if branch has an upstream
    if check_has_upstream(git).is_err() {
        output.warning(&format!(
            "Skipping '{worktree_name}': no tracking branch configured"
        ));
        output.info(&format!(
            " * [skipped] {worktree_name} (no tracking branch)"
        ));
        return FetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: "Skipped: no tracking branch".to_string(),
            skipped: true,
        };
    }

    // Dry run mode
    if args.dry_run {
        output.info(&format!(
            " * [dry run] {worktree_name} (would pull with: git pull {})",
            pull_args.join(" ")
        ));
        return FetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: "Dry run: would update".to_string(),
            skipped: true,
        };
    }

    // Run git pull
    let pull_args_refs: Vec<&str> = pull_args.iter().map(|s| s.as_str()).collect();

    let pull_result = if output.is_quiet() {
        // In quiet mode, capture output (suppress git's progress)
        git.pull(&pull_args_refs).map(|_| ())
    } else {
        // In normal/verbose mode, let git's output flow to the terminal
        git.pull_passthrough(&pull_args_refs)
    };

    match pull_result {
        Ok(()) => {
            output.info(&format!(" * [fetched] {worktree_name}"));
            FetchResult {
                worktree_name: worktree_name.to_string(),
                success: true,
                message: "Updated successfully".to_string(),
                skipped: false,
            }
        }
        Err(e) => {
            output.error(&format!("Failed to update '{worktree_name}': {e}"));
            output.info(&format!(" * [failed] {worktree_name}"));
            FetchResult {
                worktree_name: worktree_name.to_string(),
                success: false,
                message: format!("Failed: {e}"),
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
fn print_summary(results: &[FetchResult], output: &mut dyn Output) {
    let updated = results.iter().filter(|r| r.success && !r.skipped).count();
    let skipped = results.iter().filter(|r| r.skipped).count();
    let failed = results.iter().filter(|r| !r.success && !r.skipped).count();

    // Verbose details
    if output.is_verbose() {
        let updated_list: Vec<_> = results.iter().filter(|r| r.success && !r.skipped).collect();
        let skipped_list: Vec<_> = results.iter().filter(|r| r.skipped).collect();
        let failed_list: Vec<_> = results
            .iter()
            .filter(|r| !r.success && !r.skipped)
            .collect();

        if !updated_list.is_empty() {
            output.step("Updated:");
            for r in &updated_list {
                output.step(&format!("  {} - {}", r.worktree_name, r.message));
            }
        }
        if !skipped_list.is_empty() {
            output.step("Skipped:");
            for r in &skipped_list {
                output.step(&format!("  {} - {}", r.worktree_name, r.message));
            }
        }
        if !failed_list.is_empty() {
            output.step("Failed:");
            for r in &failed_list {
                output.step(&format!("  {} - {}", r.worktree_name, r.message));
            }
        }
    }

    // Pluralized summary line
    if failed == 0 {
        let mut parts: Vec<String> = Vec::new();

        if updated > 0 {
            let word = if updated == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            parts.push(format!("Fetched {updated} {word}"));
        }

        if skipped > 0 {
            let word = if skipped == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            if parts.is_empty() {
                parts.push(format!("Skipped {skipped} {word}"));
            } else {
                parts.push(format!("skipped {skipped} {word}"));
            }
        }

        if parts.is_empty() {
            output.info("Nothing to update");
        } else {
            output.info(&parts.join(", "));
        }
    } else {
        let mut parts: Vec<String> = Vec::new();

        if updated > 0 {
            let word = if updated == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            parts.push(format!("{updated} {word} fetched"));
        }
        if skipped > 0 {
            let word = if skipped == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            parts.push(format!("{skipped} {word} skipped"));
        }
        let word = if failed == 1 { "worktree" } else { "worktrees" };
        parts.push(format!("{failed} {word} failed"));

        output.error(&parts.join(", "));
    }
}
