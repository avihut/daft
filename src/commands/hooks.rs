//! Trust management and YAML hooks commands.
//!
//! Provides `git daft hooks` subcommand with:
//! - `trust` - Trust a repository to run hooks automatically
//! - `prompt` - Trust a repository but prompt before each hook
//! - `deny` - Revoke trust from a repository
//! - `status` - Show trust status and available hooks
//! - `list` - List all trusted repositories
//! - `migrate` - Rename deprecated hook files to their new names
//! - `install` - Scaffold a daft.yml with hook definitions
//! - `validate` - Validate YAML hook configuration
//! - `dump` - Dump merged YAML hook configuration

use crate::hooks::{
    yaml_config, yaml_config_loader, yaml_config_validate, HookType, TrustDatabase, TrustEntry,
    TrustLevel, DEPRECATED_HOOK_REMOVAL_VERSION, PROJECT_HOOKS_DIR,
};
use crate::styles::{bold, cyan, def, dim, green, red, yellow};
use crate::{get_current_worktree_path, get_git_common_dir, is_git_repository};
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
        &def("hooks", "Available hooks in .daft/hooks/"),
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

fn reset_trust_long_about() -> String {
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
        "If daft.yml already exists, only hooks not already defined are added.",
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

#[derive(Parser)]
#[command(name = "hooks")]
#[command(about = "Manage repository trust for hook execution")]
#[command(long_about = hooks_long_about())]
pub struct Args {
    #[command(subcommand)]
    command: Option<HooksCommand>,
}

#[derive(Subcommand)]
enum HooksCommand {
    /// Trust repository to run hooks automatically
    #[command(long_about = trust_long_about())]
    Trust {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        force: bool,
    },

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

    /// List all repositories with trust settings
    #[command(long_about = list_long_about())]
    List {
        #[arg(long, help = "Include repositories with deny trust level")]
        all: bool,
    },

    /// Clear all trust settings
    #[command(long_about = reset_trust_long_about())]
    ResetTrust {
        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        force: bool,
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
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);

    match args.command {
        Some(HooksCommand::Trust { path, force }) => cmd_set_trust(&path, TrustLevel::Allow, force),
        Some(HooksCommand::Prompt { path, force }) => {
            cmd_set_trust(&path, TrustLevel::Prompt, force)
        }
        Some(HooksCommand::Deny { path, force }) => cmd_deny(&path, force),
        Some(HooksCommand::Status { path, short }) => cmd_status(&path, short),
        Some(HooksCommand::List { all }) => cmd_list(all),
        Some(HooksCommand::ResetTrust { force }) => cmd_reset_trust(force),
        Some(HooksCommand::Migrate { dry_run }) => cmd_migrate(dry_run),
        Some(HooksCommand::Install { hooks }) => cmd_install(&hooks),
        Some(HooksCommand::Validate) => cmd_validate(),
        Some(HooksCommand::Dump) => cmd_dump(),
        None => cmd_status(&std::path::PathBuf::from("."), false), // Default to status if no subcommand
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

        // Find hooks
        let hooks = find_project_hooks(&git_dir)?;

        if short {
            // Short format: PATH (type), optional repo line, then (LEVEL) hooks
            println!("{} {}", abs_path.display(), dim(&format!("({path_type})")));
            if !is_repo_root {
                println!("{} {}", project_root.display(), dim("(repository)"));
            }
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

            if hooks.is_empty() {
                println!("{} {}:", bold("No hooks found in"), cyan(PROJECT_HOOKS_DIR));
                println!(
                    "  {}",
                    dim("(Create hooks by adding executable scripts to .daft/hooks/)")
                );
            } else {
                println!("{} {}:", bold("Hooks found in"), cyan(PROJECT_HOOKS_DIR));
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

            // Check for deprecated hook filenames among discovered hooks
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
                    println!("  {}", cyan(&format!("git daft hooks deny {path_arg}")));
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
    let existing_config = yaml_config_loader::load_merged_config(&worktree_root)
        .context("Failed to load YAML config")?;

    let config_path = worktree_root.join("daft.yml");

    if let Some(ref config) = existing_config {
        // Config exists — add only hooks not already defined
        let mut added = Vec::new();
        let mut skipped = Vec::new();

        for &name in &hook_names {
            if config.hooks.contains_key(name) {
                skipped.push(name);
            } else {
                added.push(name);
            }
        }

        if added.is_empty() {
            for name in &skipped {
                println!(
                    "  {} {name} {}",
                    yellow("skipped"),
                    dim("(already defined)")
                );
            }
            println!("\n{}", dim("No new hooks to add."));
            return Ok(());
        }

        // Append new hooks to the existing file
        let mut content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;

        // Ensure trailing newline before appending
        if !content.ends_with('\n') {
            content.push('\n');
        }

        for name in &added {
            content.push_str(&format!(
                "\n  {name}:\n    jobs:\n      - name: setup\n        run: echo \"TODO: add your {name} command\"\n"
            ));
        }

        std::fs::write(&config_path, &content)
            .with_context(|| format!("Failed to write {}", config_path.display()))?;

        for name in &added {
            println!("  {} {name}", green("added"));
        }
        for name in &skipped {
            println!(
                "  {} {name} {}",
                yellow("skipped"),
                dim("(already defined)")
            );
        }
    } else {
        // No config — create new file
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

    let yaml = serde_yaml::to_string(&config).context("Failed to serialize config")?;
    print!("{yaml}");

    Ok(())
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
