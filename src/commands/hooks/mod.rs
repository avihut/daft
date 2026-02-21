//! Trust management and YAML hooks commands.
//!
//! Provides `git daft hooks` subcommand with:
//! - `trust` - Trust a repository to run hooks automatically
//!   - `trust list` - List all repositories with trust settings
//!   - `trust reset [path]` - Remove trust entry for a specific repository
//!   - `trust reset all` - Clear all trust settings from the database
//! - `prompt` - Trust a repository but prompt before each hook
//! - `deny` - Revoke trust from a repository
//! - `status` - Show trust status and available hooks
//! - `migrate` - Rename deprecated hook files to their new names
//! - `install` - Scaffold a daft.yml with hook definitions
//! - `validate` - Validate YAML hook configuration
//! - `dump` - Dump merged YAML hook configuration
//! - `run` - Manually run a hook (bypasses trust checks)

mod dump;
mod formatting;
mod install;
mod migrate;
mod run_cmd;
mod status;
mod trust;
mod validate;

use crate::hooks::{TrustLevel, PROJECT_HOOKS_DIR};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::styles::{bold, def, dim, green, red, yellow};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

fn hooks_long_about() -> String {
    [
        "Manage trust settings for repository hooks in .daft/hooks/.",
        "",
        "Trust levels:",
        &def("deny", "Do not run hooks (default)"),
        &def("prompt", "Prompt before each hook"),
        &def("allow", "Run hooks automatically"),
        "",
        "Trust applies to all worktrees. Without a subcommand, shows status.",
    ]
    .join("\n")
}

fn trust_long_about() -> String {
    [
        &format!(
            "Grants {} trust to the current repository, allowing hooks in",
            bold("full")
        ),
        ".daft/hooks/ to be executed automatically during worktree operations.",
        "",
        &format!(
            "Use '{}' instead if you want to be prompted before",
            bold("git daft hooks prompt")
        ),
        "each hook execution.",
        "",
        "Trust settings are stored in ~/.config/daft/trust.json and persist",
        "across sessions.",
    ]
    .join("\n")
}

fn prompt_long_about() -> String {
    [
        &format!(
            "Grants {} trust to the current repository. Hooks in",
            bold("conditional")
        ),
        ".daft/hooks/ will be executed, but you will be prompted for",
        "confirmation before each hook runs.",
        "",
        &format!(
            "Use '{}' instead if you want hooks to run",
            bold("git daft hooks trust")
        ),
        "automatically without prompting.",
        "",
        "Trust settings are stored in ~/.config/daft/trust.json and persist",
        "across sessions.",
    ]
    .join("\n")
}

fn deny_long_about() -> String {
    [
        &format!(
            "{} trust from the current repository. After this command,",
            bold("Revokes")
        ),
        "hooks will no longer be executed for this repository until trust",
        "is granted again.",
        "",
        &format!("This sets the trust level to {}.", bold("deny")),
    ]
    .join("\n")
}

fn status_long_about() -> String {
    [
        "Display trust status and available hooks for the current repository.",
        "",
        "Shows:",
        &def("level", "Current trust level (deny, prompt, or allow)"),
        &def("yaml", "Hooks defined in daft.yml"),
        &def("scripts", "Executable scripts in .daft/hooks/"),
        &def("commands", "Suggested commands to change trust"),
        "",
        &format!("Use {} for a compact one-line output.", bold("-s/--short")),
    ]
    .join("\n")
}

fn list_long_about() -> String {
    [
        "List all repositories with explicit trust settings.",
        "",
        &format!(
            "By default, only shows repositories with {} or {} trust.",
            bold("allow"),
            bold("prompt")
        ),
        &format!(
            "Use {} to include {} entries as well.",
            bold("--all"),
            bold("deny")
        ),
        "",
        "Output is paginated if it exceeds the terminal height.",
    ]
    .join("\n")
}

fn reset_long_about() -> String {
    [
        "Remove the trust entry for a repository, or clear all trust settings.",
        "",
        "Without a subcommand, removes the explicit trust record for the given",
        "repository path (defaults to the current directory). This returns the",
        "repository to the default trust level (deny).",
        "",
        &format!(
            "Use '{}' to clear all trust settings from the database.",
            bold("reset all")
        ),
    ]
    .join("\n")
}

fn reset_all_long_about() -> String {
    [
        &format!("{} all trust settings from the database.", bold("Clear")),
        "",
        "This removes all repository trust entries and patterns, resetting",
        "the trust database to its initial empty state.",
        "",
        &format!(
            "Use {} to skip the confirmation prompt.",
            bold("-f/--force")
        ),
    ]
    .join("\n")
}

fn migrate_long_about() -> String {
    [
        "Rename deprecated hook files to their new canonical names.",
        "",
        "In daft v1.x, worktree-scoped hooks were renamed with a 'worktree-' prefix:",
        &def("pre-create", "worktree-pre-create"),
        &def("post-create", "worktree-post-create"),
        &def("pre-remove", "worktree-pre-remove"),
        &def("post-remove", "worktree-post-remove"),
        "",
        "This command must be run from within a worktree. It renames deprecated",
        "hook files in the current worktree's .daft/hooks/ directory.",
        "",
        "If both old and new names exist, the old file is skipped (conflict).",
        "Resolve conflicts manually before re-running.",
        "",
        &format!(
            "Use {} to preview changes without renaming.",
            bold("--dry-run")
        ),
    ]
    .join("\n")
}

fn install_long_about() -> String {
    [
        "Scaffold a daft.yml configuration with hook definitions.",
        "",
        "Creates a daft.yml file with placeholder jobs for the specified hooks.",
        "If no hook names are provided, all daft lifecycle hooks are scaffolded.",
        "",
        "If a config file already exists, it is not modified. Instead, a YAML",
        "snippet is printed for any missing hooks so you can add them manually.",
        "",
        "Valid hook names:",
        "  post-clone, worktree-pre-create, worktree-post-create,",
        "  worktree-pre-remove, worktree-post-remove",
    ]
    .join("\n")
}

fn validate_long_about() -> String {
    [
        "Validate the YAML hooks configuration file.",
        "",
        "Loads and parses daft.yml (or equivalent), then runs semantic",
        "validation checks including:",
        &def("version", "min_version compatibility check"),
        &def("modes", "Mutually exclusive execution modes"),
        &def("jobs", "Each job has a run, script, or group"),
        &def("groups", "Group definitions are valid"),
        "",
        "Exits with code 1 if there are validation errors.",
    ]
    .join("\n")
}

fn dump_long_about() -> String {
    [
        "Load and display the fully merged YAML hooks configuration.",
        "",
        "Merges all config sources (main file, extends, per-hook files,",
        "local overrides) and outputs the final effective configuration",
        "as YAML.",
    ]
    .join("\n")
}

fn run_long_about() -> String {
    [
        "Manually run a hook by name.",
        "",
        "Executes the specified hook type as if it were triggered by a",
        "worktree lifecycle event. Trust checks are bypassed since the",
        "user is explicitly invoking the hook.",
        "",
        "Use cases:",
        &def("re-run", "Re-run a hook after a previous failure"),
        &def("develop", "Iterate on hook scripts during development"),
        &def(
            "bootstrap",
            "Set up worktrees that predate the hooks config",
        ),
        "",
        &format!("Use {} to preview which jobs would run.", bold("--dry-run")),
        &format!("Use {} to run a single job by name.", bold("--job <name>")),
        &format!(
            "Use {} to run only jobs with a specific tag.",
            bold("--tag <tag>")
        ),
    ]
    .join("\n")
}

#[derive(Parser)]
#[command(name = "hooks")]
#[command(about = "Manage repository trust for hook execution")]
#[command(long_about = hooks_long_about())]
pub struct Args {
    /// Path to check (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    #[command(subcommand)]
    command: Option<HooksCommand>,
}

#[derive(Subcommand)]
enum HooksCommand {
    /// Trust repository to run hooks automatically
    #[command(long_about = trust_long_about())]
    Trust(trust_cmd::TrustArgs),

    /// Trust repository but prompt before each hook
    #[command(long_about = prompt_long_about())]
    Prompt {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        force: bool,
    },

    /// Revoke trust from the current repository
    #[command(long_about = deny_long_about())]
    Deny {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        force: bool,
    },

    /// Display trust status and available hooks
    #[command(long_about = status_long_about())]
    Status {
        /// Path to check (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 's', long, help = "Show compact one-line summary")]
        short: bool,
    },

    /// Run a hook manually
    #[command(long_about = run_long_about())]
    Run(HooksRunArgs),

    /// Scaffold a daft.yml configuration with hook definitions
    #[command(long_about = install_long_about())]
    Install {
        /// Hook names to scaffold (e.g., post-clone worktree-post-create).
        /// If omitted, scaffolds all hooks.
        #[arg(help = "Hook names to add (omit for all hooks)")]
        hooks: Vec<String>,
    },

    /// Validate the YAML hooks configuration
    #[command(long_about = validate_long_about())]
    Validate,

    /// Dump the merged YAML hooks configuration
    #[command(long_about = dump_long_about())]
    Dump,

    /// Rename deprecated hook files to their new names
    #[command(long_about = migrate_long_about())]
    Migrate {
        /// Show what would be renamed without making changes
        #[arg(long, help = "Preview renames without making changes")]
        dry_run: bool,
    },
}

#[derive(clap::Args)]
pub(super) struct HooksRunArgs {
    /// Hook type to run (e.g., worktree-post-create).
    /// Omit to list available hooks.
    #[arg(help = "Hook type to run (omit to list available hooks)")]
    pub hook_type: Option<String>,

    /// Run only the specified named job
    #[arg(long, help = "Run only the named job")]
    pub job: Option<String>,

    /// Run only jobs with this tag (repeatable, matches any)
    #[arg(long, help = "Run only jobs with this tag (repeatable)")]
    pub tag: Vec<String>,

    /// Preview what would run without executing
    #[arg(long, help = "Preview what would run without executing")]
    pub dry_run: bool,

    /// Show verbose output including skipped jobs
    #[arg(short, long, help = "Show verbose output including skipped jobs")]
    pub verbose: bool,
}

mod trust_cmd {
    use super::{list_long_about, reset_all_long_about, reset_long_about};
    use clap::{Args, Subcommand};
    use std::path::PathBuf;

    #[derive(Args)]
    pub struct TrustArgs {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        pub path: PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        pub force: bool,

        #[command(subcommand)]
        pub command: Option<TrustSubcommand>,
    }

    #[derive(Subcommand)]
    pub enum TrustSubcommand {
        /// List all repositories with trust settings
        #[command(long_about = list_long_about())]
        List {
            #[arg(long, help = "Include repositories with deny trust level")]
            all: bool,
        },

        /// Remove trust entry for a repository or clear all trust settings
        #[command(long_about = reset_long_about())]
        Reset(ResetArgs),
    }

    #[derive(Args)]
    pub struct ResetArgs {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        pub path: PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        pub force: bool,

        #[command(subcommand)]
        pub command: Option<ResetSubcommand>,
    }

    #[derive(Subcommand)]
    pub enum ResetSubcommand {
        /// Clear all trust settings from the database
        #[command(long_about = reset_all_long_about())]
        All {
            #[arg(short = 'f', long, help = "Do not ask for confirmation")]
            force: bool,
        },
    }
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);
    let mut output = CliOutput::new(OutputConfig::new(false, false));

    match args.command {
        Some(HooksCommand::Trust(trust_args)) => match trust_args.command {
            Some(trust_cmd::TrustSubcommand::List { all }) => trust::cmd_list(all, &mut output),
            Some(trust_cmd::TrustSubcommand::Reset(reset_args)) => match reset_args.command {
                Some(trust_cmd::ResetSubcommand::All { force }) => {
                    trust::cmd_reset_trust(force, &mut output)
                }
                None => {
                    trust::cmd_reset_trust_path(&reset_args.path, reset_args.force, &mut output)
                }
            },
            None => trust::cmd_set_trust(
                &trust_args.path,
                TrustLevel::Allow,
                trust_args.force,
                &mut output,
            ),
        },
        Some(HooksCommand::Prompt { path, force }) => {
            trust::cmd_set_trust(&path, TrustLevel::Prompt, force, &mut output)
        }
        Some(HooksCommand::Deny { path, force }) => trust::cmd_deny(&path, force, &mut output),
        Some(HooksCommand::Status { path, short }) => status::cmd_status(&path, short, &mut output),
        Some(HooksCommand::Migrate { dry_run }) => migrate::cmd_migrate(dry_run, &mut output),
        Some(HooksCommand::Install { hooks }) => install::cmd_install(&hooks, &mut output),
        Some(HooksCommand::Validate) => validate::cmd_validate(&mut output),
        Some(HooksCommand::Dump) => dump::cmd_dump(&mut output),
        Some(HooksCommand::Run(run_args)) => run_cmd::cmd_run(&run_args, &mut output),
        None => {
            status::cmd_status(&args.path, false, &mut output)?;
            output.info(&dim(
                "Run `git daft hooks --help` to see all available commands.",
            ));
            Ok(())
        }
    }
}

/// Format a trust level with appropriate color.
pub(super) fn styled_trust_level(level: TrustLevel) -> String {
    match level {
        TrustLevel::Deny => red(&level.to_string()),
        TrustLevel::Prompt => yellow(&level.to_string()),
        TrustLevel::Allow => green(&level.to_string()),
    }
}

/// Find project hooks in the current repository.
pub(super) fn find_project_hooks(git_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    // Get the worktree path (parent of .git for bare repos, or current for regular)
    // For daft's worktree structure, we need to find any worktree and check its .daft/hooks
    let project_root = git_dir.parent().context("Invalid git directory")?;

    let mut hooks = Vec::new();

    // Check all worktrees for hooks
    for entry in std::fs::read_dir(project_root)
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() && path.file_name().map(|n| n != ".git").unwrap_or(false) {
            let hooks_dir = path.join(PROJECT_HOOKS_DIR);
            if hooks_dir.exists() {
                for hook_entry in std::fs::read_dir(&hooks_dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let hook_path = hook_entry.path();
                    if hook_path.is_file() {
                        hooks.push(hook_path);
                    }
                }
                break; // Found hooks in one worktree, that's enough
            }
        }
    }

    // Sort by filename for consistent output
    hooks.sort_by(|a, b| {
        a.file_name()
            .unwrap_or_default()
            .cmp(b.file_name().unwrap_or_default())
    });

    Ok(hooks)
}

/// Find the worktree root directory.
pub(super) fn find_worktree_root() -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to execute git rev-parse")?;

    if !output.status.success() {
        anyhow::bail!("Not in a git worktree");
    }

    Ok(PathBuf::from(
        String::from_utf8(output.stdout)
            .context("Invalid UTF-8 in git output")?
            .trim(),
    ))
}
