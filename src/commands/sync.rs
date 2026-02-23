//! git-sync - Synchronize worktrees with remote
//!
//! Orchestrates pruning stale branches/worktrees and updating all remaining
//! worktrees in a single command.

use crate::{
    core::{
        worktree::{fetch, prune},
        CommandBridge, OutputSink,
    },
    get_project_root,
    git::GitCommand,
    hooks::{HookExecutor, HooksConfig},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    settings::DaftSettings,
    styles, WorktreeConfig, CD_FILE_ENV,
};
use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-sync")]
#[command(version = crate::VERSION)]
#[command(about = "Synchronize worktrees with remote (prune + update all)")]
#[command(long_about = r#"
Synchronizes all worktrees with the remote in a single command.

This is equivalent to running `daft prune` followed by `daft update --all`:

  1. Prune: fetches with --prune, removes worktrees and branches for deleted
     remote branches, executes lifecycle hooks for each removal.
  2. Update: pulls all remaining worktrees from their remote tracking branches.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
daft.prune.cdTarget).

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.
"#)]
pub struct Args {
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short,
        long,
        help = "Force removal of worktrees with uncommitted changes"
    )]
    force: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-sync"));

    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    // Phase 1: Prune stale branches and worktrees
    let prune_result = run_prune_phase(&mut output, &settings, args.force)?;

    // Phase 2: Update all remaining worktrees
    run_update_phase(&mut output, &settings, args.force)?;

    // Write the cd target for the shell wrapper (from prune phase)
    if let Some(ref cd_target) = prune_result.cd_target {
        if std::env::var(CD_FILE_ENV).is_ok() {
            output.cd_path(cd_target);
        } else {
            output.result(&format!(
                "Run `cd {}` (your previous working directory was removed)",
                cd_target.display()
            ));
        }
    }

    Ok(())
}

fn run_prune_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    force: bool,
) -> Result<prune::PruneResult> {
    let params = prune::PruneParams {
        force,
        use_gitoxide: settings.use_gitoxide,
        is_quiet: output.is_quiet(),
        remote_name: settings.remote.clone(),
        prune_cd_target: settings.prune_cd_target,
    };

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let result = {
        let mut bridge = CommandBridge::new(output, executor);
        prune::execute(&params, &mut bridge)?
    };

    if result.nothing_to_prune {
        return Ok(result);
    }

    // Print header
    output.result(&format!("Pruning {}", result.remote_name));
    if let Some(ref url) = result.remote_url {
        output.info(&format!("URL: {url}"));
    }

    // Per-branch detail lines
    for detail in &result.pruned_branches {
        render_pruned_branch(detail, output);
    }

    // Summary
    if result.branches_deleted > 0 || result.worktrees_removed > 0 {
        let branch_word = if result.branches_deleted == 1 {
            "branch"
        } else {
            "branches"
        };
        let mut summary = format!("Pruned {} {branch_word}", result.branches_deleted);
        if result.worktrees_removed > 0 {
            let wt_word = if result.worktrees_removed == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            summary.push_str(&format!(", removed {} {wt_word}", result.worktrees_removed));
        }
        output.success(&summary);
    }

    if result.has_prunable {
        output.warning(
            "Some prunable worktree data may exist. Run 'git worktree prune' to clean up.",
        );
    }

    Ok(result)
}

fn run_update_phase(output: &mut dyn Output, settings: &DaftSettings, force: bool) -> Result<()> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let git = GitCommand::new(wt_config.quiet).with_gitoxide(settings.use_gitoxide);
    let project_root = get_project_root()?;

    // Merge config-based args
    let config_args: Vec<&str> = settings.update_args.split_whitespace().collect();
    let config_has_rebase = config_args.contains(&"--rebase");
    let config_has_autostash = config_args.contains(&"--autostash");

    let params = fetch::FetchParams {
        targets: vec![],
        all: true,
        force,
        dry_run: false,
        rebase: config_has_rebase,
        autostash: config_has_autostash,
        ff_only: false,
        no_ff_only: false,
        pull_args: vec![],
        quiet: output.is_quiet(),
        remote_name: wt_config.remote_name.clone(),
    };

    let mut sink = OutputSink(output);
    let result = fetch::execute(&params, &git, &project_root, &mut sink)?;

    render_fetch_result(&result, output);

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

fn render_worktree_status(r: &fetch::WorktreeFetchResult, output: &mut dyn Output) {
    if r.skipped {
        output.info(&format!(" * {} {}", tag_skipped(), r.worktree_name));
    } else if r.success {
        if r.up_to_date {
            output.info(&format!(" * {} {}", tag_up_to_date(), r.worktree_name));
        } else {
            output.info(&format!(" * {} {}", tag_updated(), r.worktree_name));
            // Show captured pull output indented under the branch name
            if let Some(ref pull_output) = r.pull_output {
                for line in pull_output.lines() {
                    output.info(&format!("   {line}"));
                }
            }
        }
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
    let up_to_date = result.up_to_date_count();
    let skipped = result.skipped_count();
    let failed = result.failed_count();

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
        if up_to_date > 0 {
            let phrase = if up_to_date == 1 {
                "1 already up to date"
            } else {
                &format!("{up_to_date} already up to date")
            };
            parts.push(phrase.to_string());
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
        if up_to_date > 0 {
            parts.push(format!("{up_to_date} already up to date"));
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

fn render_pruned_branch(detail: &prune::PrunedBranchDetail, output: &mut dyn Output) {
    // Build a description of what was removed: the branch is one entity
    // with up to three manifestations (worktree, local branch, remote tracking branch).
    let mut removed = Vec::new();
    if detail.worktree_removed {
        removed.push("worktree");
    }
    if detail.branch_deleted {
        removed.push("local branch");
    }
    // The remote tracking branch is always removed (git fetch --prune did it)
    removed.push("remote tracking branch");

    output.info(&format!(
        " * {} {} — removed {}",
        tag_pruned(),
        detail.branch_name,
        removed.join(", ")
    ));
}

// -- Colored status tags --

fn tag_pruned() -> String {
    if styles::colors_enabled() {
        format!("{}[pruned]{}", styles::RED, styles::RESET)
    } else {
        "[pruned]".to_string()
    }
}

fn tag_updated() -> String {
    if styles::colors_enabled() {
        format!("{}[updated]{}", styles::GREEN, styles::RESET)
    } else {
        "[updated]".to_string()
    }
}

fn tag_up_to_date() -> String {
    if styles::colors_enabled() {
        format!("{}[up to date]{}", styles::DIM, styles::RESET)
    } else {
        "[up to date]".to_string()
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
