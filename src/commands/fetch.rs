//! git-worktree-fetch - Update worktree branches from remote tracking branches
//!
//! This command fetches and pulls updates for one or more worktrees by navigating
//! to each target worktree and running `git pull` with configurable options.

use crate::{
    core::{
        worktree::fetch::{self, WorktreeFetchResult},
        OutputSink,
    },
    get_project_root,
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    styles, WorktreeConfig,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-fetch")]
#[command(version = crate::VERSION)]
#[command(about = "Update worktree branches from their remote tracking branches")]
#[command(long_about = r#"
Updates worktree branches from their remote tracking branches.

Targets can use refspec syntax (source:destination) to update a worktree
from a different remote branch:

  Same-branch:   daft update master        (pulls master via git pull --ff-only)
  Cross-branch:  daft update master:test   (fetches origin/master, resets test to it)
  Current:       daft update               (pulls current worktree's tracking branch)
  All:           daft update --all         (pulls all worktrees)

Same-branch mode uses `git pull` with configurable options (--rebase,
--ff-only, --autostash, -- PULL_ARGS). Cross-branch mode uses `git fetch`
+ `git reset --hard` and ignores pull flags.

Worktrees with uncommitted changes are skipped unless --force is specified.
Use --dry-run to preview what would be done without making changes.
"#)]
pub struct Args {
    /// Target worktree(s) by name or refspec (source:destination)
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

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-fetch"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::new(args.quiet, args.verbose);
    let mut output = CliOutput::new(config);

    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: args.quiet,
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    // Merge CLI flags with config-based args
    let config_args: Vec<&str> = settings.update_args.split_whitespace().collect();
    let config_has_rebase = config_args.contains(&"--rebase");
    let config_has_autostash = config_args.contains(&"--autostash");

    let params = fetch::FetchParams {
        targets: args.targets.clone(),
        all: args.all,
        force: args.force,
        dry_run: args.dry_run,
        rebase: args.rebase || (config_has_rebase && !args.ff_only),
        autostash: args.autostash || config_has_autostash,
        ff_only: args.ff_only,
        no_ff_only: args.no_ff_only,
        pull_args: args.pull_args.clone(),
        quiet: args.quiet,
        remote_name: wt_config.remote_name.clone(),
    };

    let mut sink = OutputSink(&mut output);
    let result = fetch::execute(&params, &git, &project_root, &mut sink)?;

    render_fetch_result(&result, &mut output);

    if result.failed_count() > 0 {
        anyhow::bail!("{} worktree(s) failed to update", result.failed_count());
    }

    Ok(())
}

fn render_fetch_result(result: &fetch::FetchResult, output: &mut dyn Output) {
    if result.results.is_empty() {
        output.info("No worktrees to update.");
        return;
    }

    // Header
    output.result(&format!("Updating from {}", result.remote_name));
    if let Some(ref url) = result.remote_url {
        output.info(&format!("URL: {url}"));
    }

    // Per-worktree status
    for r in &result.results {
        render_worktree_status(r, output);
    }

    // Summary
    print_summary(result, output);
}

fn render_worktree_status(r: &WorktreeFetchResult, output: &mut dyn Output) {
    if r.skipped {
        if r.message.contains("Dry run") {
            output.info(&format!(" * {} {}", tag_dry_run(), r.worktree_name));
        } else {
            output.info(&format!(" * {} {}", tag_skipped(), r.worktree_name));
        }
    } else if r.success {
        output.info(&format!(" * {} {}", tag_updated(), r.worktree_name));
    } else {
        output.error(&format!(
            "Failed to update '{}': {}",
            r.worktree_name, r.message
        ));
        output.info(&format!(" * {} {}", tag_failed(), r.worktree_name));
    }
}

fn print_summary(result: &fetch::FetchResult, output: &mut dyn Output) {
    let updated = result.updated_count();
    let skipped = result.skipped_count();
    let failed = result.failed_count();

    // Verbose details
    if output.is_verbose() {
        let updated_list: Vec<_> = result
            .results
            .iter()
            .filter(|r| r.success && !r.skipped)
            .collect();
        let skipped_list: Vec<_> = result.results.iter().filter(|r| r.skipped).collect();
        let failed_list: Vec<_> = result
            .results
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
            parts.push(format!("Updated {updated} {word}"));
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
            output.success(&parts.join(", "));
        }
    } else {
        let mut parts: Vec<String> = Vec::new();
        if updated > 0 {
            let word = if updated == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            parts.push(format!("{updated} {word} updated"));
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

// ── Colored status tags ─────────────────────────────────────────────────────

fn tag_updated() -> String {
    if styles::colors_enabled() {
        format!("{}[updated]{}", styles::GREEN, styles::RESET)
    } else {
        "[updated]".to_string()
    }
}

fn tag_skipped() -> String {
    if styles::colors_enabled() {
        format!("{}[skipped]{}", styles::YELLOW, styles::RESET)
    } else {
        "[skipped]".to_string()
    }
}

fn tag_failed() -> String {
    if styles::colors_enabled() {
        format!("{}[failed]{}", styles::RED, styles::RESET)
    } else {
        "[failed]".to_string()
    }
}

fn tag_dry_run() -> String {
    if styles::colors_enabled() {
        format!("{}[dry run]{}", styles::DIM, styles::RESET)
    } else {
        "[dry run]".to_string()
    }
}
