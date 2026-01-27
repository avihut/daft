//! Trust management commands for the hooks system.
//!
//! Provides `git daft hooks` subcommand with:
//! - `trust` - Trust a repository to run hooks automatically
//! - `prompt` - Trust a repository but prompt before each hook
//! - `untrust` - Revoke trust from a repository
//! - `status` - Show trust status and available hooks
//! - `list` - List all trusted repositories

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use daft::hooks::{TrustDatabase, TrustLevel, PROJECT_HOOKS_DIR};
use daft::styles::def;
use daft::{get_git_common_dir, is_git_repository};
use std::io::{self, Write};
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
    #[command(long_about = r#"
Grants full trust to the current repository, allowing hooks in .daft/hooks/
to be executed automatically during worktree operations.

Use 'git daft hooks prompt' instead if you want to be prompted before each
hook execution.

Trust settings are stored in ~/.config/daft/trust.json and persist across
sessions.
"#)]
    Trust {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        force: bool,
    },

    /// Trust repository but prompt before each hook
    #[command(long_about = r#"
Grants conditional trust to the current repository. Hooks in .daft/hooks/
will be executed, but you will be prompted for confirmation before each
hook runs.

Use 'git daft hooks trust' instead if you want hooks to run automatically
without prompting.

Trust settings are stored in ~/.config/daft/trust.json and persist across
sessions.
"#)]
    Prompt {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        force: bool,
    },

    /// Revoke trust from the current repository
    #[command(long_about = r#"
Revokes trust from the current repository. After this command, hooks will
no longer be executed for this repository until trust is granted again.
"#)]
    Untrust {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 'f', long, help = "Do not ask for confirmation")]
        force: bool,
    },

    /// Display trust status and available hooks
    Status {
        /// Path to check (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(short = 's', long, help = "Show compact one-line summary")]
        short: bool,
    },

    /// List all repositories with trust settings
    List {
        #[arg(long, help = "Include repositories with deny trust level")]
        all: bool,
    },
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);

    match args.command {
        Some(HooksCommand::Trust { path, force }) => cmd_set_trust(&path, TrustLevel::Allow, force),
        Some(HooksCommand::Prompt { path, force }) => {
            cmd_set_trust(&path, TrustLevel::Prompt, force)
        }
        Some(HooksCommand::Untrust { path, force }) => cmd_untrust(&path, force),
        Some(HooksCommand::Status { path, short }) => cmd_status(&path, short),
        Some(HooksCommand::List { all }) => cmd_list(all),
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
        println!("Current:");
        println!("{} (repository)", project_root.display());
        println!("{hooks_str} ({current_level})");

        if !force {
            print!("\nChange trust level to {new_level}? [y/N] ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                println!("Aborted.");
                return Ok(());
            }
        }

        // Update and save the trust database
        let mut db = db;
        db.set_trust_level(&git_dir, new_level);
        db.save().context("Failed to save trust database")?;

        // Show new status
        println!("{} (repository)", project_root.display());
        println!("{hooks_str} ({new_level})");

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

/// Revoke trust for the repository at the given path.
fn cmd_untrust(path: &Path, force: bool) -> Result<()> {
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
            println!("{} (repository)", project_root.display());
            println!("Not explicitly trusted");
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
        println!("Current:");
        println!("{} (repository)", project_root.display());
        println!("{hooks_str} ({current_level})");

        if !force {
            print!("\nChange trust level to deny? [y/N] ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();

            if input != "y" && input != "yes" {
                println!("Aborted.");
                return Ok(());
            }
        }

        let mut db = db;
        db.remove_trust(&git_dir);
        db.save().context("Failed to save trust database")?;

        // Show new status
        println!("{} (repository)", project_root.display());
        println!("{hooks_str} (deny)");

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
        let path_type = if is_repo_root {
            "repository"
        } else if abs_path.starts_with(project_root) {
            let relative = abs_path.strip_prefix(project_root).unwrap_or(&abs_path);
            let components: Vec<_> = relative.components().collect();
            if components.len() == 1 {
                "worktree"
            } else {
                "subdirectory"
            }
        } else {
            "unknown"
        };

        // Find hooks
        let hooks = find_project_hooks(&git_dir)?;

        if short {
            // Short format: PATH (type), optional repo line, then (LEVEL) hooks
            println!("{} ({path_type})", abs_path.display());
            if !is_repo_root {
                println!("{} (repository)", project_root.display());
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
            println!("{hooks_str} ({trust_level})");
        } else {
            // Full format
            println!("{} ({path_type})", abs_path.display());
            if !is_repo_root {
                println!("{} (repository)", project_root.display());
            }
            println!();

            // Trust status
            let trust_source = if is_explicit { "" } else { " (default)" };
            println!("Trust level: {trust_level}{trust_source}");
            println!("  {}", trust_level_description(trust_level));
            println!();

            if hooks.is_empty() {
                println!("No hooks found in {}:", PROJECT_HOOKS_DIR);
                println!("  (Create hooks by adding executable scripts to .daft/hooks/)");
            } else {
                println!("Hooks found in {}:", PROJECT_HOOKS_DIR);
                for hook in &hooks {
                    let name = hook.file_name().unwrap_or_default().to_string_lossy();
                    let executable = is_executable(hook);
                    let status = if executable { "" } else { " (not executable)" };
                    println!("  - {name}{status}");
                }
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
                    println!("To enable hooks:");
                    println!("  git daft hooks trust {path_arg}");
                    println!("  git daft hooks prompt {path_arg}");
                }
                TrustLevel::Prompt | TrustLevel::Allow => {
                    println!("To revoke trust:");
                    println!("  git daft hooks untrust {path_arg}");
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

/// List all trusted repositories.
fn cmd_list(show_all: bool) -> Result<()> {
    let db = TrustDatabase::load().context("Failed to load trust database")?;

    let repos: Vec<_> = if show_all {
        db.repositories
            .iter()
            .map(|(path, entry)| (path.as_str(), &entry.level, &entry.granted_at))
            .collect()
    } else {
        db.list_trusted()
            .into_iter()
            .map(|(path, entry)| (path, &entry.level, &entry.granted_at))
            .collect()
    };

    if repos.is_empty() {
        if show_all {
            println!("No repositories in trust database.");
        } else {
            println!("No trusted repositories.");
            println!();
            println!("To trust a repository, cd into it and run:");
            println!("  git daft hooks trust");
        }
        return Ok(());
    }

    let title = if show_all {
        "All repositories in trust database:"
    } else {
        "Trusted repositories:"
    };
    println!("{title}");
    println!();

    for (path, level, granted_at) in repos {
        // Truncate long paths
        let display_path = if path.len() > 60 {
            format!("...{}", &path[path.len() - 57..])
        } else {
            path.to_string()
        };
        println!("  {display_path}");
        println!("    Level: {level}  (trusted: {granted_at})");
    }

    // Show patterns if any
    if !db.patterns.is_empty() {
        println!();
        println!("Trust patterns:");
        for pattern in &db.patterns {
            let comment = pattern
                .comment
                .as_ref()
                .map(|c| format!(" # {c}"))
                .unwrap_or_default();
            println!("  {} -> {}{comment}", pattern.pattern, pattern.level);
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
