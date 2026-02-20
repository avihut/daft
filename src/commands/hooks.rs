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

use crate::hooks::yaml_executor::JobFilter;
use crate::hooks::{
    yaml_config, yaml_config_loader, yaml_config_validate, HookExecutor, HookType, HooksConfig,
    TrustDatabase, TrustEntry, TrustLevel, DEPRECATED_HOOK_REMOVAL_VERSION, PROJECT_HOOKS_DIR,
};
use crate::styles::{bold, cyan, def, dim, green, red, yellow};
use crate::{
    get_current_branch, get_current_worktree_path, get_git_common_dir, get_project_root,
    is_git_repository,
};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
        "  post-clone, post-init, worktree-pre-create, worktree-post-create,",
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

    /// Rename deprecated hook files to their new names
    #[command(long_about = migrate_long_about())]
    Migrate {
        /// Show what would be renamed without making changes
        #[arg(long, help = "Preview renames without making changes")]
        dry_run: bool,
    },

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

    /// Run a hook manually
    #[command(long_about = run_long_about())]
    Run(HooksRunArgs),
}

#[derive(clap::Args)]
struct HooksRunArgs {
    /// Hook type to run (e.g., worktree-post-create).
    /// Omit to list available hooks.
    #[arg(help = "Hook type to run (omit to list available hooks)")]
    hook_type: Option<String>,

    /// Run only the specified named job
    #[arg(long, help = "Run only the named job")]
    job: Option<String>,

    /// Run only jobs with this tag (repeatable, matches any)
    #[arg(long, help = "Run only jobs with this tag (repeatable)")]
    tag: Vec<String>,

    /// Preview what would run without executing
    #[arg(long, help = "Preview what would run without executing")]
    dry_run: bool,
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

    match args.command {
        Some(HooksCommand::Trust(trust_args)) => match trust_args.command {
            Some(trust_cmd::TrustSubcommand::List { all }) => cmd_list(all),
            Some(trust_cmd::TrustSubcommand::Reset(reset_args)) => match reset_args.command {
                Some(trust_cmd::ResetSubcommand::All { force }) => cmd_reset_trust(force),
                None => cmd_reset_trust_path(&reset_args.path, reset_args.force),
            },
            None => cmd_set_trust(&trust_args.path, TrustLevel::Allow, trust_args.force),
        },
        Some(HooksCommand::Prompt { path, force }) => {
            cmd_set_trust(&path, TrustLevel::Prompt, force)
        }
        Some(HooksCommand::Deny { path, force }) => cmd_deny(&path, force),
        Some(HooksCommand::Status { path, short }) => cmd_status(&path, short),
        Some(HooksCommand::Migrate { dry_run }) => cmd_migrate(dry_run),
        Some(HooksCommand::Install { hooks }) => cmd_install(&hooks),
        Some(HooksCommand::Validate) => cmd_validate(),
        Some(HooksCommand::Dump) => cmd_dump(),
        Some(HooksCommand::Run(run_args)) => cmd_run(&run_args),
        None => {
            cmd_status(&args.path, false)?;
            println!(
                "{}",
                dim("Run `git daft hooks --help` to see all available commands.")
            );
            Ok(())
        }
    }
}

/// Set trust level for the repository at the given path.
fn cmd_set_trust(path: &Path, new_level: TrustLevel, force: bool) -> Result<()> {
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;

        let hooks = find_project_hooks(&git_dir)?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let current_level = db.get_trust_level(&git_dir);
        let project_root = git_dir.parent().context("Invalid git directory")?;

        // Build hooks list string
        let hook_names: Vec<_> = hooks
            .iter()
            .filter_map(|h| h.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect();
        let hooks_str = if hook_names.is_empty() {
            "none".to_string()
        } else {
            hook_names.join(", ")
        };

        // Show current status
        println!("{}", bold("Current:"));
        println!("{} {}", project_root.display(), dim("(repository)"));
        println!("{hooks_str} ({})", styled_trust_level(current_level));

        if !force {
            print!(
                "\nChange trust level to {}? [y/N] ",
                styled_trust_level(new_level)
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                println!("{}", dim("Aborted."));
                return Ok(());
            }
        }

        // Update and save the trust database
        let mut db = db;
        db.set_trust_level(&git_dir, new_level);
        db.save().context("Failed to save trust database")?;

        // Show new status
        println!("{} {}", project_root.display(), dim("(repository)"));
        println!("{hooks_str} ({})", styled_trust_level(new_level));

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

/// Revoke trust for the repository at the given path.
fn cmd_deny(path: &Path, force: bool) -> Result<()> {
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let current_level = db.get_trust_level(&git_dir);
        let project_root = git_dir.parent().context("Invalid git directory")?;

        if !db.has_explicit_trust(&git_dir) {
            println!("{} {}", project_root.display(), dim("(repository)"));
            println!("{}", dim("Not explicitly trusted"));
            return Ok(());
        }

        let hooks = find_project_hooks(&git_dir)?;
        let hook_names: Vec<_> = hooks
            .iter()
            .filter_map(|h| h.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .collect();
        let hooks_str = if hook_names.is_empty() {
            "none".to_string()
        } else {
            hook_names.join(", ")
        };

        // Show current status
        println!("{}", bold("Current:"));
        println!("{} {}", project_root.display(), dim("(repository)"));
        println!("{hooks_str} ({})", styled_trust_level(current_level));

        if !force {
            print!(
                "\nChange trust level to {}? [y/N] ",
                styled_trust_level(TrustLevel::Deny)
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                println!("{}", dim("Aborted."));
                return Ok(());
            }
        }

        let mut db = db;
        db.remove_trust(&git_dir);
        db.save().context("Failed to save trust database")?;

        // Show new status
        println!("{} {}", project_root.display(), dim("(repository)"));
        println!("{hooks_str} ({})", styled_trust_level(TrustLevel::Deny));

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

/// Show trust status and available hooks.
fn cmd_status(path: &Path, short: bool) -> Result<()> {
    // Resolve the path to absolute
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    // Change to that directory temporarily to run git commands
    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    // Ensure we're in a git repository
    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let trust_level = db.get_trust_level(&git_dir);
        let is_explicit = db.has_explicit_trust(&git_dir);

        // Determine path type and display
        let project_root = git_dir.parent().context("Invalid git directory")?;
        let is_repo_root = abs_path == project_root;
        let worktree_root = get_current_worktree_path().ok();
        let path_type = if is_repo_root {
            "repository"
        } else if worktree_root.as_deref() == Some(&abs_path) {
            "worktree"
        } else if worktree_root.is_some() {
            "subdirectory"
        } else {
            "unknown"
        };

        // Find shell script hooks
        let hooks = find_project_hooks(&git_dir)?;

        // Find YAML-configured hooks
        let yaml_cfg = find_yaml_config_for_status(&git_dir, worktree_root.as_deref())
            .ok()
            .flatten();
        let yaml_hook_names: Vec<String> = yaml_cfg
            .as_ref()
            .map(|c| {
                let mut names: Vec<String> = c.hooks.keys().cloned().collect();
                names.sort();
                names
            })
            .unwrap_or_default();

        if short {
            // Short format: PATH (type), optional repo line, then (LEVEL) hooks
            println!("{} {}", abs_path.display(), dim(&format!("({path_type})")));
            if !is_repo_root {
                println!("{} {}", project_root.display(), dim("(repository)"));
            }
            // Combine shell hook names and YAML hook names (deduped)
            let mut all_names: Vec<String> = hooks
                .iter()
                .filter_map(|h| h.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .collect();
            for name in &yaml_hook_names {
                if !all_names.contains(name) {
                    all_names.push(name.clone());
                }
            }
            all_names.sort();
            let hooks_str = if all_names.is_empty() {
                "none".to_string()
            } else {
                all_names.join(", ")
            };
            println!("{hooks_str} ({})", styled_trust_level(trust_level));
        } else {
            // Full format
            println!("{} {}", abs_path.display(), dim(&format!("({path_type})")));
            if !is_repo_root {
                println!("{} {}", project_root.display(), dim("(repository)"));
            }
            println!();

            // Trust status
            let trust_source = if is_explicit {
                String::new()
            } else {
                format!(" {}", dim("(default)"))
            };
            println!(
                "{} {}{}",
                bold("Trust level:"),
                styled_trust_level(trust_level),
                trust_source
            );
            println!("  {}", dim(trust_level_description(trust_level)));
            println!();

            // YAML hooks section
            if !yaml_hook_names.is_empty() {
                println!("{} {}:", bold("Hooks configured in"), cyan("daft.yml"));
                for name in &yaml_hook_names {
                    println!("  - {}", cyan(name));
                }
                if !hooks.is_empty() {
                    println!();
                }
            }

            // Shell script hooks section
            if hooks.is_empty() {
                if yaml_hook_names.is_empty() {
                    println!("{} {}:", bold("No hooks found in"), cyan(PROJECT_HOOKS_DIR));
                    println!(
                        "  {}",
                        dim("(Create scripts in .daft/hooks/ or configure daft.yml)")
                    );
                }
            } else {
                println!("{} {}:", bold("Shell hooks in"), cyan(PROJECT_HOOKS_DIR));
                for hook in &hooks {
                    let name = hook.file_name().unwrap_or_default().to_string_lossy();
                    let executable = is_executable(hook);
                    let status = if executable {
                        String::new()
                    } else {
                        format!(" {}", red("(not executable)"))
                    };
                    println!("  - {}{status}", cyan(&name));
                }
            }

            // Check for deprecated hook filenames among discovered shell hooks
            let deprecated_hooks: Vec<_> = hooks
                .iter()
                .filter_map(|hook_path| {
                    let name = hook_path.file_name()?.to_str()?;
                    let hook_type = HookType::from_filename(name)?;
                    let dep = hook_type.deprecated_filename()?;
                    if name == dep {
                        Some((dep, hook_type.filename()))
                    } else {
                        None
                    }
                })
                .collect();

            if !deprecated_hooks.is_empty() {
                println!();
                println!("{}", yellow("Deprecated hook names detected:"));
                for (old_name, new_name) in &deprecated_hooks {
                    println!("  {} -> {}", red(old_name), green(new_name));
                }
                println!("  Run '{}' to rename them.", cyan("git daft hooks migrate"));
                println!(
                    "  {}",
                    dim(&format!(
                        "Deprecated names will stop working in daft v{}.",
                        DEPRECATED_HOOK_REMOVAL_VERSION
                    ))
                );
            }

            println!();

            // Show commands with relative path
            // If we're inside the repo, "." works since trust resolves the git common dir
            let path_arg = if original_dir.starts_with(project_root) || original_dir == project_root
            {
                ".".to_string()
            } else {
                relative_path(&original_dir, project_root)
                    .display()
                    .to_string()
            };

            match trust_level {
                TrustLevel::Deny => {
                    println!("{}", bold("To enable hooks:"));
                    println!("  {}", cyan(&format!("git daft hooks trust {path_arg}")));
                    println!("  {}", cyan(&format!("git daft hooks prompt {path_arg}")));
                }
                TrustLevel::Prompt | TrustLevel::Allow => {
                    println!("{}", bold("To revoke trust:"));
                    println!(
                        "  {}  {}",
                        cyan(&format!("git daft hooks deny {path_arg}")),
                        dim("(explicitly deny)")
                    );
                    println!(
                        "  {}  {}",
                        cyan(&format!("git daft hooks trust reset {path_arg}")),
                        dim("(remove trust entry)")
                    );
                }
            }
        }

        Ok(())
    })();

    // Restore original directory
    std::env::set_current_dir(&original_dir)?;

    result
}

/// Get a human-readable description for a trust level.
fn trust_level_description(level: TrustLevel) -> &'static str {
    match level {
        TrustLevel::Deny => "Hooks will NOT run for this repository.",
        TrustLevel::Prompt => "You will be prompted before each hook execution.",
        TrustLevel::Allow => "Hooks will run automatically without prompting.",
    }
}

/// Format a trust level with appropriate color.
fn styled_trust_level(level: TrustLevel) -> String {
    match level {
        TrustLevel::Deny => red(&level.to_string()),
        TrustLevel::Prompt => yellow(&level.to_string()),
        TrustLevel::Allow => green(&level.to_string()),
    }
}

/// List all trusted repositories.
fn cmd_list(show_all: bool) -> Result<()> {
    let db = TrustDatabase::load().context("Failed to load trust database")?;

    let repos: Vec<(&str, &TrustEntry)> = if show_all {
        db.repositories
            .iter()
            .map(|(p, e)| (p.as_str(), e))
            .collect()
    } else {
        db.list_trusted()
    };

    if repos.is_empty() {
        if show_all {
            println!("{}", dim("No repositories in trust database."));
        } else {
            println!("{}", dim("No trusted repositories."));
            println!();
            println!("{}", bold("To trust a repository, cd into it and run:"));
            println!("  {}", cyan("git daft hooks trust"));
        }
        return Ok(());
    }

    // Build output
    let mut output = String::new();

    let title = if show_all {
        bold("All repositories in trust database:")
    } else {
        bold("Trusted repositories:")
    };
    output.push_str(&title);
    output.push_str("\n\n");

    for (path, entry) in &repos {
        // Strip .git suffix if present to show repo path
        let repo_path = path.strip_suffix("/.git").unwrap_or(path);
        // Truncate long paths
        let display_path = if repo_path.len() > 60 {
            format!("...{}", &repo_path[repo_path.len() - 57..])
        } else {
            repo_path.to_string()
        };
        let display_time = entry.formatted_time();
        output.push_str(&format!("  {display_path}\n"));
        output.push_str(&format!(
            "    Level: {}  {}\n",
            styled_trust_level(entry.level),
            dim(&format!("(trusted: {display_time})"))
        ));
    }

    // Show patterns if any
    if !db.patterns.is_empty() {
        output.push_str(&format!("\n{}:\n", bold("Trust patterns")));
        for pattern in &db.patterns {
            let comment = pattern
                .comment
                .as_ref()
                .map(|c| format!(" {}", dim(&format!("# {c}"))))
                .unwrap_or_default();
            output.push_str(&format!(
                "  {} -> {}{comment}\n",
                cyan(&pattern.pattern),
                styled_trust_level(pattern.level)
            ));
        }
    }

    // Use pager if output is long and we're in a terminal
    let line_count = output.lines().count();
    if line_count > 20 && std::io::stdout().is_terminal() {
        output_with_pager(&output);
    } else {
        print!("{output}");
    }

    Ok(())
}

/// Output text through a pager if available.
fn output_with_pager(text: &str) {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());

    let result = Command::new("sh")
        .args(["-c", &format!("{pager} -R")])
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()
        });

    // Fall back to direct output if pager fails
    if result.is_err() {
        print!("{text}");
    }
}

/// Clear all trust settings.
fn cmd_reset_trust(force: bool) -> Result<()> {
    let db = TrustDatabase::load().context("Failed to load trust database")?;

    let repo_count = db.repositories.len();
    let pattern_count = db.patterns.len();

    if repo_count == 0 && pattern_count == 0 {
        println!("{}", dim("Trust database is already empty."));
        return Ok(());
    }

    // Show current status
    println!("{}", bold("Current:"));
    println!(
        "{} repositories, {} patterns",
        yellow(&repo_count.to_string()),
        yellow(&pattern_count.to_string())
    );

    if !force {
        print!("\n{} all trust settings? [y/N] ", red("Clear"));
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_lowercase();

        if input != "y" && input != "yes" {
            println!("{}", dim("Aborted."));
            return Ok(());
        }
    }

    let mut db = db;
    db.clear();
    db.save().context("Failed to save trust database")?;

    println!("{} repositories, {} patterns", green("0"), green("0"));

    Ok(())
}

/// Remove the trust entry for a specific repository path.
fn cmd_reset_trust_path(path: &Path, force: bool) -> Result<()> {
    let abs_path = path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let original_dir = std::env::current_dir()?;
    std::env::set_current_dir(&abs_path)
        .with_context(|| format!("Cannot change to directory: {}", abs_path.display()))?;

    let result = (|| -> Result<()> {
        if !is_git_repository()? {
            anyhow::bail!("Not in a git repository: {}", abs_path.display());
        }

        let git_dir = get_git_common_dir()?;
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let project_root = git_dir.parent().context("Invalid git directory")?;

        if !db.has_explicit_trust(&git_dir) {
            println!("{} {}", project_root.display(), dim("(repository)"));
            println!("{}", dim("No explicit trust entry to remove."));
            return Ok(());
        }

        let current_level = db.get_trust_level(&git_dir);
        println!("{}", bold("Current:"));
        println!("{} {}", project_root.display(), dim("(repository)"));
        println!("Trust level: {}", styled_trust_level(current_level));

        if !force {
            print!(
                "\n{} trust entry for this repository? [y/N] ",
                red("Remove")
            );
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                println!("{}", dim("Aborted."));
                return Ok(());
            }
        }

        let mut db = db;
        db.remove_trust(&git_dir);
        db.save().context("Failed to save trust database")?;

        println!("{} {}", project_root.display(), dim("(repository)"));
        println!("{}", dim("Trust entry removed."));

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

/// Migrate deprecated hook filenames to their new canonical names.
///
/// Must be run from within a worktree. Only migrates hooks in the
/// current worktree's `.daft/hooks/` directory.
fn cmd_migrate(dry_run: bool) -> Result<()> {
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

/// Find YAML-configured hooks for the status display.
///
/// Checks the given worktree first, then falls back to searching worktree
/// subdirectories of the project root (for the bare-clone case where the
/// caller is at the repo root rather than inside a worktree).
fn find_yaml_config_for_status(
    git_dir: &Path,
    worktree_root: Option<&Path>,
) -> Result<Option<yaml_config::YamlConfig>> {
    if let Some(wt) = worktree_root {
        if let Ok(Some(config)) = yaml_config_loader::load_merged_config(wt) {
            return Ok(Some(config));
        }
    }

    // Fall back: search worktree subdirectories of the project root
    let project_root = git_dir.parent().context("Invalid git directory")?;
    for entry in std::fs::read_dir(project_root)
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() && path.file_name().map(|n| n != ".git").unwrap_or(false) {
            if let Ok(Some(config)) = yaml_config_loader::load_merged_config(&path) {
                return Ok(Some(config));
            }
        }
    }

    Ok(None)
}

/// Find project hooks in the current repository.
fn find_project_hooks(git_dir: &Path) -> Result<Vec<std::path::PathBuf>> {
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

/// Check if a file is executable.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true // Assume executable on non-Unix
}

/// Compute the shortest relative path from `from` to `to`.
///
/// Returns "." if they are the same directory.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    if from == to {
        return PathBuf::from(".");
    }

    // If `to` is a descendant of `from`, strip the prefix
    if let Ok(rel) = to.strip_prefix(from) {
        return rel.to_path_buf();
    }

    // If `from` is a descendant of `to`, go up with ".."
    if let Ok(rel) = from.strip_prefix(to) {
        let mut path = PathBuf::new();
        for _ in rel.components() {
            path.push("..");
        }
        return path;
    }

    // Find common ancestor and build relative path
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();

    // Find common prefix length
    let common_len = from_components
        .iter()
        .zip(to_components.iter())
        .take_while(|(a, b)| a == b)
        .count();

    // Build path: go up from `from` to common ancestor, then down to `to`
    let mut path = PathBuf::new();
    for _ in common_len..from_components.len() {
        path.push("..");
    }
    for component in &to_components[common_len..] {
        path.push(component);
    }

    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path
    }
}

/// Scaffold a daft.yml configuration with hook definitions.
fn cmd_install(hooks: &[String]) -> Result<()> {
    let worktree_root = find_worktree_root()?;

    // Determine which hooks to scaffold
    let hook_names: Vec<&str> = if hooks.is_empty() {
        yaml_config::KNOWN_HOOK_NAMES.to_vec()
    } else {
        // Validate all provided names
        for name in hooks {
            if !yaml_config::KNOWN_HOOK_NAMES.contains(&name.as_str()) {
                anyhow::bail!(
                    "Unknown hook name: '{name}'. Valid hooks: {}",
                    yaml_config::KNOWN_HOOK_NAMES.join(", ")
                );
            }
        }
        hooks.iter().map(|s| s.as_str()).collect()
    };

    // Check if config already exists
    let existing_config_file = yaml_config_loader::find_config_file(&worktree_root);

    if let Some((config_path, _)) = existing_config_file {
        // Config file exists — don't modify it. Show what's missing and provide a snippet.
        let config = yaml_config_loader::load_merged_config(&worktree_root)
            .context("Failed to load YAML config")?;

        println!(
            "Config file already exists: {}",
            bold(&config_path.display().to_string())
        );

        let (existing, missing): (Vec<&str>, Vec<&str>) = if let Some(ref cfg) = config {
            hook_names
                .iter()
                .partition(|name| cfg.hooks.contains_key(**name))
        } else {
            (vec![], hook_names.clone())
        };

        if missing.is_empty() {
            println!("\n{}", dim("All requested hooks are already defined."));
            return Ok(());
        }

        if !existing.is_empty() {
            println!(
                "\nAlready defined: {}",
                existing
                    .iter()
                    .map(|n| green(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!(
            "Not yet defined: {}",
            missing
                .iter()
                .map(|n| cyan(n))
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!(
            "\nAdd them to your {} under the {} key:\n",
            bold(&config_path.file_name().unwrap().to_string_lossy()),
            cyan("hooks")
        );

        for name in &missing {
            println!("  {name}:");
            println!("    jobs:");
            println!("      - name: setup");
            println!("        run: echo \"TODO: add your {name} command\"");
        }

        println!();
    } else {
        // No config — create new file
        let config_path = worktree_root.join("daft.yml");
        let mut content = String::from(
            "# daft hooks configuration\n# See: https://github.com/avihut/daft\n\nhooks:\n",
        );

        for name in &hook_names {
            content.push_str(&format!(
                "  {name}:\n    jobs:\n      - name: setup\n        run: echo \"TODO: add your {name} command\"\n"
            ));
        }

        std::fs::write(&config_path, &content)
            .with_context(|| format!("Failed to write {}", config_path.display()))?;

        println!("{} {}", green("Created"), config_path.display());
        for name in &hook_names {
            println!("  {} {name}", green("added"));
        }
    }

    Ok(())
}

/// Validate the YAML hooks configuration.
fn cmd_validate() -> Result<()> {
    let worktree_root = find_worktree_root()?;

    let config = yaml_config_loader::load_merged_config(&worktree_root)
        .context("Failed to load YAML config")?;

    let config = match config {
        Some(c) => c,
        None => {
            println!("{}", dim("No daft.yml found."));
            return Ok(());
        }
    };

    let result = yaml_config_validate::validate_config(&config)?;

    for warning in &result.warnings {
        println!("  {} {warning}", yellow("warning:"));
    }

    for error in &result.errors {
        println!("  {} {error}", red("error:"));
    }

    if result.is_ok() {
        if result.warnings.is_empty() {
            println!("{}", green("Configuration is valid."));
        } else {
            println!(
                "\n{} ({} warning(s))",
                green("Configuration is valid"),
                result.warnings.len()
            );
        }
        Ok(())
    } else {
        println!(
            "\n{} ({} error(s), {} warning(s))",
            red("Configuration has errors"),
            result.errors.len(),
            result.warnings.len()
        );
        std::process::exit(1);
    }
}

/// Dump the merged YAML hooks configuration.
fn cmd_dump() -> Result<()> {
    let worktree_root = find_worktree_root()?;

    let config = yaml_config_loader::load_merged_config(&worktree_root)
        .context("Failed to load YAML config")?;

    let config = match config {
        Some(c) => c,
        None => {
            println!("{}", dim("No daft.yml found."));
            return Ok(());
        }
    };

    let value: serde_yaml::Value =
        serde_yaml::to_value(&config).context("Failed to convert config to YAML value")?;
    let stripped = strip_yaml_nulls(value);
    let yaml = serde_yaml::to_string(&stripped).context("Failed to serialize config")?;
    print!("{}", colorize_yaml_dump(&yaml));

    Ok(())
}

/// Run a hook manually.
fn cmd_run(args: &HooksRunArgs) -> Result<()> {
    use crate::hooks::yaml_config_loader::get_effective_jobs;
    use crate::hooks::HookContext;

    // Resolve worktree context
    let worktree_path = get_current_worktree_path()
        .context("Not in a git worktree. Run this command from within a worktree directory.")?;

    // Load YAML config (needed for both listing and execution)
    let yaml_config = yaml_config_loader::load_merged_config(&worktree_path)
        .context("Failed to load YAML config")?;
    let yaml_config = match yaml_config {
        Some(c) => c,
        None => {
            anyhow::bail!("No daft.yml found in this worktree");
        }
    };

    // If no hook type specified, list available hooks
    let hook_type_str = match args.hook_type {
        Some(ref s) => s.clone(),
        None => {
            return cmd_run_list_hooks(&yaml_config);
        }
    };

    // Parse hook type
    let hook_type = HookType::from_yaml_name(&hook_type_str).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown hook type: '{}'\nValid hook types: {}",
            hook_type_str,
            yaml_config::KNOWN_HOOK_NAMES.join(", ")
        )
    })?;

    let git_dir = get_git_common_dir().context("Could not determine git directory")?;
    let project_root = get_project_root().context("Could not determine project root")?;
    let branch_name = get_current_branch().unwrap_or_else(|_| "HEAD".to_string());

    let hook_name = hook_type.yaml_name();
    let hook_def = yaml_config.hooks.get(hook_name).ok_or_else(|| {
        let mut names: Vec<&str> = yaml_config.hooks.keys().map(|s| s.as_str()).collect();
        names.sort();
        if names.is_empty() {
            anyhow::anyhow!("No hooks defined in daft.yml")
        } else {
            anyhow::anyhow!(
                "Hook '{}' is not defined in daft.yml\nConfigured hooks: {}",
                hook_name,
                names.join(", ")
            )
        }
    })?;

    // Check trust level and show hint if not trusted
    let trust_db = TrustDatabase::load().unwrap_or_default();
    let trust_level = trust_db.get_trust_level(&git_dir);
    if trust_level != TrustLevel::Allow {
        println!(
            "{} this repository is not in your trust list ({}).",
            dim("Note:"),
            styled_trust_level(trust_level)
        );
        println!(
            "  {} run `{}` to allow hooks to run automatically.",
            dim("Tip:"),
            cyan("git daft hooks trust")
        );
        println!();
    }

    // Build job filter
    let filter = JobFilter {
        only_job_name: args.job.clone(),
        only_tags: args.tag.clone(),
    };

    // Dry-run: preview jobs without executing
    if args.dry_run {
        let mut jobs = get_effective_jobs(hook_def);

        // Apply exclude_tags from hook definition
        if let Some(ref exclude_tags) = hook_def.exclude_tags {
            jobs.retain(|job| {
                if let Some(ref tags) = job.tags {
                    !tags.iter().any(|t| exclude_tags.contains(t))
                } else {
                    true
                }
            });
        }

        // Apply inclusion filters
        if let Some(ref name) = filter.only_job_name {
            jobs.retain(|j| j.name.as_deref() == Some(name.as_str()));
            if jobs.is_empty() {
                anyhow::bail!("No job named '{}' found in hook '{}'", name, hook_name);
            }
        }
        if !filter.only_tags.is_empty() {
            jobs.retain(|job| {
                job.tags
                    .as_ref()
                    .is_some_and(|tags| tags.iter().any(|t| filter.only_tags.contains(t)))
            });
            if jobs.is_empty() {
                anyhow::bail!(
                    "No jobs matching tags {:?} in hook '{}'",
                    filter.only_tags,
                    hook_name
                );
            }
        }

        // Sort by priority
        jobs.sort_by_key(|j| j.priority.unwrap_or(0));

        if jobs.is_empty() {
            println!("{}", dim("No jobs to run."));
            return Ok(());
        }

        let job_count = jobs.len();
        let job_word = if job_count == 1 { "job" } else { "jobs" };
        println!(
            "{} {} ({} {})",
            bold("Hook:"),
            cyan(hook_name),
            job_count,
            job_word
        );
        println!();

        for (i, job) in jobs.iter().enumerate() {
            let name = job.name.as_deref().unwrap_or("(unnamed)");
            println!("  {}. {}", i + 1, bold(name));

            if let Some(ref run) = job.run {
                println!("     {}: {}", dim("run"), run);
            } else if let Some(ref script) = job.script {
                let runner_str = job
                    .runner
                    .as_ref()
                    .map(|r| format!("{r} "))
                    .unwrap_or_default();
                println!("     {}: {}{}", dim("script"), runner_str, script);
            } else if job.group.is_some() {
                println!("     {}", dim("(group)"));
            }

            if let Some(ref needs) = job.needs {
                if !needs.is_empty() {
                    println!("     {}: [{}]", dim("needs"), needs.join(", "));
                }
            }

            if let Some(ref tags) = job.tags {
                if !tags.is_empty() {
                    println!("     {}: [{}]", dim("tags"), tags.join(", "));
                }
            }

            if i + 1 < job_count {
                println!();
            }
        }

        return Ok(());
    }

    // Build HookContext for execution
    let ctx = HookContext::new(
        hook_type,
        "hooks-run",
        &project_root,
        &git_dir,
        "origin",
        &worktree_path,
        &worktree_path,
        &branch_name,
    );

    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?
        .with_bypass_trust(true)
        .with_job_filter(filter);

    let mut output = crate::output::CliOutput::default_output();
    let result = executor.execute(&ctx, &mut output)?;

    if result.skipped {
        if let Some(reason) = result.skip_reason {
            println!("{}", dim(&format!("Skipped: {reason}")));
        }
    } else if !result.success {
        std::process::exit(result.exit_code.unwrap_or(1));
    }

    Ok(())
}

/// List available hooks when `hooks run` is invoked with no arguments.
fn cmd_run_list_hooks(config: &yaml_config::YamlConfig) -> Result<()> {
    use crate::hooks::yaml_config_loader::get_effective_jobs;

    if config.hooks.is_empty() {
        println!("{}", dim("No hooks defined in daft.yml."));
        return Ok(());
    }

    let mut names: Vec<&String> = config.hooks.keys().collect();
    names.sort();

    println!("{}", bold("Available hooks:"));
    println!();

    for name in &names {
        let hook_def = &config.hooks[*name];
        let jobs = get_effective_jobs(hook_def);
        let job_count = jobs.len();
        let job_word = if job_count == 1 { "job" } else { "jobs" };
        println!("  {} ({} {})", cyan(name), job_count, job_word);
    }

    println!();
    println!(
        "Run a hook with: {}",
        cyan("git daft hooks run <hook-type>")
    );

    Ok(())
}

/// Recursively remove null values and empty mappings/sequences from a YAML value.
fn strip_yaml_nulls(value: serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let mut out = serde_yaml::Mapping::new();
            for (k, v) in map {
                if v.is_null() {
                    continue;
                }
                let stripped = strip_yaml_nulls(v);
                match &stripped {
                    serde_yaml::Value::Mapping(m) if m.is_empty() => continue,
                    serde_yaml::Value::Sequence(s) if s.is_empty() => continue,
                    _ => {}
                }
                out.insert(k, stripped);
            }
            serde_yaml::Value::Mapping(out)
        }
        serde_yaml::Value::Sequence(seq) => serde_yaml::Value::Sequence(
            seq.into_iter()
                .filter(|v| !v.is_null())
                .map(strip_yaml_nulls)
                .collect(),
        ),
        other => other,
    }
}

/// Colorize a serialized YAML string for terminal output.
///
/// Skips the `---` document separator, colors top-level keys bold,
/// hook names bold+yellow, sub-keys cyan, quoted strings green, and
/// booleans/numbers yellow.
fn colorize_yaml_dump(yaml: &str) -> String {
    use crate::styles::{colors_enabled, BOLD, CYAN, DIM, RESET, YELLOW};

    let use_colors = colors_enabled();
    let mut in_hooks = false;
    let mut result = String::new();

    for line in yaml.lines() {
        if line == "---" {
            continue;
        }

        if line.is_empty() {
            result.push('\n');
            continue;
        }

        let indent_len = line.len() - line.trim_start().len();
        let rest = &line[indent_len..];
        let indent = &line[..indent_len];

        // Track entry into the hooks: section (top-level key)
        if indent_len == 0 {
            in_hooks = rest == "hooks:" || rest.starts_with("hooks: ");
        }

        if !use_colors {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        let colored = if let Some(after_dash) = rest.strip_prefix("- ") {
            format!("{DIM}-{RESET} {}", yaml_colorize_value_part(after_dash))
        } else if rest == "-" {
            format!("{DIM}-{RESET}")
        } else if let Some(colon_pos) = yaml_key_colon(rest) {
            let key = &rest[..colon_pos];
            let after_colon = &rest[colon_pos + 1..];

            let colored_key = if in_hooks && indent_len == 2 {
                // Hook name: bold + yellow
                format!("{BOLD}{YELLOW}{key}{RESET}")
            } else if indent_len == 0 {
                // Top-level config key: bold
                format!("{BOLD}{key}{RESET}")
            } else {
                // Sub-key: cyan
                format!("{CYAN}{key}{RESET}")
            };

            if after_colon.is_empty() {
                format!("{colored_key}:")
            } else {
                let val = after_colon.trim_start();
                format!("{colored_key}: {}", yaml_colorize_scalar(val))
            }
        } else {
            yaml_colorize_scalar(rest)
        };

        result.push_str(indent);
        result.push_str(&colored);
        result.push('\n');
    }

    result
}

/// Find the byte position of the `:` separating a YAML key from its value.
///
/// Returns `Some(pos)` for `key: value` (pos of `:`) or for a bare
/// mapping header `key:` (pos of trailing `:`). Returns `None` for
/// plain strings that contain no key-value separator.
fn yaml_key_colon(s: &str) -> Option<usize> {
    if let Some(pos) = s.find(": ") {
        return Some(pos);
    }
    if s.ends_with(':') {
        return Some(s.len() - 1);
    }
    None
}

/// Colorize the content after `- ` in a YAML list item.
///
/// If the content is `key: value`, the key is colored cyan. Otherwise
/// the whole string is treated as a scalar value.
fn yaml_colorize_value_part(s: &str) -> String {
    use crate::styles::{CYAN, RESET};
    if let Some(pos) = yaml_key_colon(s) {
        let key = &s[..pos];
        let after = &s[pos + 1..];
        let colored_key = format!("{CYAN}{key}{RESET}");
        if after.is_empty() {
            format!("{colored_key}:")
        } else {
            format!(
                "{colored_key}: {}",
                yaml_colorize_scalar(after.trim_start())
            )
        }
    } else {
        yaml_colorize_scalar(s)
    }
}

/// Colorize a scalar YAML value.
///
/// - Booleans and null → yellow
/// - Quoted strings → green
/// - Numbers → yellow
/// - Everything else → plain
fn yaml_colorize_scalar(value: &str) -> String {
    use crate::styles::{GREEN, RESET, YELLOW};
    if matches!(value, "true" | "false" | "null" | "~") {
        return format!("{YELLOW}{value}{RESET}");
    }
    if value.starts_with('"') || value.starts_with('\'') {
        return format!("{GREEN}{value}{RESET}");
    }
    if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        return format!("{YELLOW}{value}{RESET}");
    }
    value.to_string()
}

/// Find the worktree root directory.
fn find_worktree_root() -> Result<PathBuf> {
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
