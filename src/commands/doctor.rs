//! `daft doctor` command — diagnose installation and configuration issues.
//!
//! Inspects daft installation, repository configuration, and hooks setup,
//! reporting issues with actionable suggestions and optional auto-fix.

use anyhow::Result;
use clap::Parser;

use crate::doctor::{
    hooks_checks, installation, repository, status_symbol, CheckCategory, CheckStatus,
    DoctorSummary,
};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::styles::{bold, dim, green, red, yellow};

fn long_about() -> String {
    [
        "Diagnose daft installation and configuration issues.",
        "",
        "Runs health checks on your daft installation, repository setup,",
        "and hooks configuration. Reports issues with actionable suggestions.",
        "",
        "When run outside a git repository, only installation checks are performed.",
        "Inside a daft-managed repository, repository and hooks checks run too.",
        "",
        "The --fix flag auto-repairs: missing command symlinks, missing shortcut",
        "symlinks for partially-installed styles, orphaned worktree entries,",
        "incorrect fetch refspecs, missing remote HEAD, non-executable hooks,",
        "and deprecated hook names. Issues requiring manual intervention (binary",
        "not in PATH, git not installed, shell integration) show suggestions only.",
        "",
        "Use --fix --dry-run to preview planned actions with pre-flight validation.",
        "Each action shows whether it would succeed or fail (e.g., directory not",
        "writable, conflicting files). Actions marked + would succeed; actions",
        "marked x would fail, with the reason shown below.",
    ]
    .join("\n")
}

#[derive(Parser)]
#[command(name = "daft-doctor")]
#[command(about = "Diagnose daft installation and configuration issues")]
#[command(long_about = long_about())]
pub struct Args {
    /// Show detailed output for each check
    #[arg(short, long, help = "Show detailed output for each check")]
    verbose: bool,

    /// Auto-fix issues that can be resolved automatically
    #[arg(long, help = "Auto-fix issues that can be resolved automatically")]
    fix: bool,

    /// Preview fixes without applying them (use with --fix)
    #[arg(
        long,
        requires = "fix",
        help = "Preview fixes without applying them (use with --fix)"
    )]
    dry_run: bool,

    /// Only show warnings and errors
    #[arg(short, long, help = "Only show warnings and errors")]
    quiet: bool,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);
    // Doctor manages its own quiet/verbose filtering; Output is just the print sink.
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    let mut categories = Vec::new();

    // Installation checks (always run)
    categories.push(run_installation_checks());

    // Repository checks (only inside a git repo)
    let repo_ctx = repository::get_repo_context();
    if let Some(ref ctx) = repo_ctx {
        categories.push(run_repository_checks(ctx));

        // Hooks checks (always when in a repo)
        categories.push(run_hooks_checks(ctx));
    }

    // Apply fixes if requested
    if args.fix {
        if args.dry_run {
            preview_fixes(&categories, &mut output);
            return Ok(());
        }
        apply_fixes(&categories, &mut output);
        // Re-run checks after fixes
        categories.clear();
        categories.push(run_installation_checks());
        if let Some(ref ctx) = repo_ctx {
            categories.push(run_repository_checks(ctx));
            categories.push(run_hooks_checks(ctx));
        }
    }

    // Display results
    print_results(&categories, args.verbose, args.quiet, &mut output);

    // Print summary
    let summary = DoctorSummary::from_categories(&categories);
    output.info("");
    print_summary(&summary, &mut output);

    if summary.has_failures() {
        std::process::exit(1);
    }

    Ok(())
}

fn run_installation_checks() -> CheckCategory {
    // Core checks
    let mut results = vec![
        installation::check_binary_in_path(),
        installation::check_command_symlinks(),
        installation::check_git(),
        installation::check_man_pages(),
        installation::check_shell_integration(),
    ];

    // Shell wrappers check (only if shell integration is configured)
    let shell_configured = results
        .iter()
        .any(|r| r.name == "Shell integration" && r.status == CheckStatus::Pass);
    if shell_configured {
        results.push(installation::check_shell_wrappers());
    }

    // Shortcut symlink checks
    results.extend(installation::check_shortcut_symlinks());

    CheckCategory {
        title: "Installation".to_string(),
        results,
    }
}

fn run_repository_checks(ctx: &repository::RepoContext) -> CheckCategory {
    CheckCategory {
        title: "Repository".to_string(),
        results: vec![
            repository::check_worktree_layout(ctx),
            repository::check_worktree_consistency(ctx),
            repository::check_fetch_refspec(ctx),
            repository::check_remote_head(ctx),
        ],
    }
}

fn run_hooks_checks(ctx: &repository::RepoContext) -> CheckCategory {
    let mut results = Vec::new();

    // Always check config source
    results.push(hooks_checks::check_hooks_config(
        &ctx.current_worktree,
        &ctx.project_root,
    ));

    // Shell hook checks (only when .daft/hooks/ exists)
    if hooks_checks::has_shell_hooks(&ctx.project_root) {
        results.push(hooks_checks::check_hooks_executable(&ctx.project_root));
        results.push(hooks_checks::check_deprecated_names(&ctx.project_root));
    }

    // Trust level (when any hooks are configured)
    if hooks_checks::has_any_hooks(&ctx.current_worktree, &ctx.project_root) {
        results.push(hooks_checks::check_trust_level(&ctx.git_common_dir));
    }

    CheckCategory {
        title: "Hooks".to_string(),
        results,
    }
}

fn preview_fixes(categories: &[CheckCategory], output: &mut dyn Output) {
    let fixable: Vec<_> = categories
        .iter()
        .flat_map(|c| &c.results)
        .filter(|r| r.fixable() && matches!(r.status, CheckStatus::Warning | CheckStatus::Fail))
        .collect();

    if fixable.is_empty() {
        output.info(&dim("No fixable issues found."));
        return;
    }

    output.info(&bold(&format!("Would fix {} issue(s):", fixable.len())));
    output.info("");

    let mut any_would_fail = false;

    for result in &fixable {
        let symbol = status_symbol(result.status);
        output.info(&format!(
            "  {symbol} {} {} {}",
            result.name,
            dim("\u{2014}"),
            result.message
        ));

        if let Some(ref dry_run) = result.dry_run_fix {
            let actions = dry_run();
            for action in &actions {
                if action.would_succeed {
                    output.info(&format!("      {} {}", green("+"), action.description));
                } else {
                    any_would_fail = true;
                    output.info(&format!("      {} {}", red("x"), action.description));
                    if let Some(ref reason) = action.failure_reason {
                        output.info(&format!("        {}", dim(reason)));
                    }
                }
            }
        } else if let Some(ref suggestion) = result.suggestion {
            output.info(&format!("      {}", dim(&format!("Action: {suggestion}"))));
        }
    }

    output.info("");
    if any_would_fail {
        output.warning(&yellow(
            "Some fixes would fail. Resolve the issues above first.",
        ));
        output.info(&dim(
            "Run 'daft doctor --fix' to apply fixes that can succeed.",
        ));
    } else {
        output.info(&dim("Run 'daft doctor --fix' to apply these fixes."));
    }
}

fn apply_fixes(categories: &[CheckCategory], output: &mut dyn Output) {
    output.info(&bold("Applying fixes..."));
    output.info("");

    for category in categories {
        for result in &category.results {
            if result.fixable() && matches!(result.status, CheckStatus::Warning | CheckStatus::Fail)
            {
                output.info(&format!("  Fixing: {} ... ", result.name));
                if let Some(ref fix) = result.fix {
                    match fix() {
                        Ok(()) => output.info(&green("done")),
                        Err(e) => output.info(&red(&format!("failed: {e}"))),
                    }
                }
            }
        }
    }
    output.info("");
}

fn print_results(
    categories: &[CheckCategory],
    verbose: bool,
    quiet: bool,
    output: &mut dyn Output,
) {
    // Header with verbose hint
    if verbose {
        output.info(&bold("Doctor summary:"));
    } else {
        output.info(&bold("Doctor summary (run daft doctor -v for details):"));
    }

    for category in categories {
        // Check if category has any visible results
        let has_visible = category.results.iter().any(|r| {
            if quiet {
                matches!(r.status, CheckStatus::Warning | CheckStatus::Fail)
            } else if verbose {
                true
            } else {
                !matches!(r.status, CheckStatus::Skipped)
            }
        });

        if !has_visible {
            continue;
        }

        output.info("");
        output.info(&bold(&category.title));

        for result in &category.results {
            // In quiet mode, skip passing and skipped checks
            if quiet && matches!(result.status, CheckStatus::Pass | CheckStatus::Skipped) {
                continue;
            }

            // Hide skipped checks unless verbose
            if !verbose && matches!(result.status, CheckStatus::Skipped) {
                continue;
            }

            let symbol = status_symbol(result.status);

            // Format: pass uses parens, warning/fail uses dash
            match result.status {
                CheckStatus::Pass => {
                    output.info(&format!("  {symbol} {} ({})", result.name, result.message));
                }
                CheckStatus::Warning | CheckStatus::Fail => {
                    output.info(&format!(
                        "  {symbol} {} {} {}",
                        result.name,
                        dim("\u{2014}"),
                        result.message
                    ));
                }
                CheckStatus::Skipped => {
                    output.info(&format!(
                        "  {symbol} {} {} {}",
                        result.name,
                        dim("\u{2014}"),
                        dim(&result.message)
                    ));
                }
            }

            // Show details for warnings/failures (indented)
            if matches!(result.status, CheckStatus::Warning | CheckStatus::Fail) {
                for detail in &result.details {
                    output.info(&format!("        {}", detail));
                }
            }

            // Show suggestion for warnings and failures
            if let Some(ref suggestion) = result.suggestion {
                if matches!(result.status, CheckStatus::Warning | CheckStatus::Fail) {
                    output.info(&format!("        {}", dim(suggestion)));
                }
            }

            // Show details in verbose mode for passing checks
            if verbose && matches!(result.status, CheckStatus::Pass) && !result.details.is_empty() {
                for detail in &result.details {
                    output.info(&format!("        {}", dim(detail)));
                }
            }
        }
    }
}

fn print_summary(summary: &DoctorSummary, output: &mut dyn Output) {
    if summary.warnings == 0 && summary.failures == 0 {
        output.info(&green(&format!(
            "No issues found! ({} checks passed)",
            summary.passed
        )));
        return;
    }

    let mut parts = Vec::new();
    let mut names = Vec::new();

    if summary.failures > 0 {
        let label = if summary.failures == 1 {
            "failure"
        } else {
            "failures"
        };
        parts.push(red(&format!("{} {label}", summary.failures)));
        names.extend(summary.failure_names.iter().cloned());
    }

    if summary.warnings > 0 {
        let label = if summary.warnings == 1 {
            "warning"
        } else {
            "warnings"
        };
        parts.push(yellow(&format!("{} {label}", summary.warnings)));
        names.extend(summary.warning_names.iter().cloned());
    }

    // Deduplicate names
    names.sort();
    names.dedup();

    let names_str = if names.is_empty() {
        String::new()
    } else {
        format!(" ({})", names.join(", "))
    };

    output.info(&format!("{}{names_str}", parts.join(", ")));
}
