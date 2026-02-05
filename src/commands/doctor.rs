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

    /// Only show warnings and errors
    #[arg(short, long, help = "Only show warnings and errors")]
    quiet: bool,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);

    let mut categories = Vec::new();

    // Installation checks (always run)
    categories.push(run_installation_checks());

    // Repository checks (only inside a daft-managed repo)
    if let Some(ctx) = repository::get_repo_context() {
        categories.push(run_repository_checks(&ctx));

        // Hooks checks (only when hooks exist)
        if hooks_checks::has_hooks(&ctx.project_root) {
            categories.push(run_hooks_checks(&ctx));
        }
    }

    // Apply fixes if requested
    if args.fix {
        apply_fixes(&categories);
        // Re-run checks after fixes
        categories.clear();
        categories.push(run_installation_checks());
        if let Some(ctx) = repository::get_repo_context() {
            categories.push(run_repository_checks(&ctx));
            if hooks_checks::has_hooks(&ctx.project_root) {
                categories.push(run_hooks_checks(&ctx));
            }
        }
    }

    // Display results
    print_results(&categories, args.verbose, args.quiet);

    // Print summary
    let summary = DoctorSummary::from_categories(&categories);
    println!();
    print_summary(&summary);

    if summary.has_failures() {
        std::process::exit(1);
    }

    Ok(())
}

fn run_installation_checks() -> CheckCategory {
    CheckCategory {
        title: "Installation".to_string(),
        results: vec![
            installation::check_binary_in_path(),
            installation::check_command_symlinks(),
            installation::check_git(),
            installation::check_man_pages(),
            installation::check_shell_integration(),
        ],
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
    CheckCategory {
        title: "Hooks".to_string(),
        results: vec![
            hooks_checks::check_hooks_executable(&ctx.project_root),
            hooks_checks::check_deprecated_names(&ctx.project_root),
            hooks_checks::check_trust_level(&ctx.git_common_dir),
        ],
    }
}

fn apply_fixes(categories: &[CheckCategory]) {
    println!("{}", bold("Applying fixes..."));
    println!();

    for category in categories {
        for result in &category.results {
            if result.fixable && matches!(result.status, CheckStatus::Warning | CheckStatus::Fail) {
                print!("  Fixing: {} ... ", result.name);
                let fix_result = apply_single_fix(&result.name);
                match fix_result {
                    Ok(()) => println!("{}", green("done")),
                    Err(e) => println!("{}", red(&format!("failed: {e}"))),
                }
            }
        }
    }
    println!();
}

fn apply_single_fix(check_name: &str) -> Result<(), String> {
    match check_name {
        "Command symlinks" => installation::fix_command_symlinks(),
        "Worktree consistency" => repository::fix_worktree_consistency(),
        "Fetch refspec" => repository::fix_fetch_refspec(),
        "Remote HEAD" => repository::fix_remote_head(),
        "Hooks executable" => {
            if let Some(ctx) = repository::get_repo_context() {
                hooks_checks::fix_hooks_executable(&ctx.project_root)
            } else {
                Err("Not in a git repository".to_string())
            }
        }
        "Hook names" => {
            if let Some(ctx) = repository::get_repo_context() {
                hooks_checks::fix_deprecated_names(&ctx.project_root)
            } else {
                Err("Not in a git repository".to_string())
            }
        }
        _ => Err(format!("No fix available for '{check_name}'")),
    }
}

fn print_results(categories: &[CheckCategory], verbose: bool, quiet: bool) {
    println!("{}", bold("Doctor summary:"));

    for category in categories {
        println!();
        println!("{}", bold(&category.title));

        for result in &category.results {
            // In quiet mode, skip passing and skipped checks
            if quiet && matches!(result.status, CheckStatus::Pass | CheckStatus::Skipped) {
                continue;
            }

            let symbol = status_symbol(result.status);
            println!("  {symbol} {}", result.message);

            // Show suggestion for warnings and failures
            if let Some(ref suggestion) = result.suggestion {
                if matches!(result.status, CheckStatus::Warning | CheckStatus::Fail) {
                    println!("      {}", dim(suggestion));
                }
            }

            // Show details in verbose mode
            if verbose && !result.details.is_empty() {
                for detail in &result.details {
                    println!("      {}", dim(detail));
                }
            }
        }
    }
}

fn print_summary(summary: &DoctorSummary) {
    let parts: Vec<String> = [
        (summary.passed, "passed", green as fn(&str) -> String),
        (summary.warnings, "warnings", yellow),
        (summary.failures, "failures", red),
    ]
    .iter()
    .filter(|(count, _, _)| *count > 0)
    .map(|(count, label, style)| style(&format!("{count} {label}")))
    .collect();

    let summary_text = if parts.is_empty() {
        dim("No checks performed")
    } else {
        parts.join(", ")
    };

    println!("Summary: {summary_text}");
}
