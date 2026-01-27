//! Trust management commands for the hooks system.
//!
//! Provides `git daft hooks` subcommand with:
//! - `trust` - Trust a repository to run hooks
//! - `untrust` - Revoke trust from a repository
//! - `status` - Show trust status and available hooks
//! - `list` - List all trusted repositories

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use daft::hooks::{TrustDatabase, TrustLevel, PROJECT_HOOKS_DIR};
use daft::styles::def;
use daft::{get_git_common_dir, is_git_repository};
use std::io::{self, Write};
use std::path::Path;

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
    /// Grant trust to the current repository
    #[command(long_about = r#"
Grants trust to the current repository, allowing hooks in .daft/hooks/ to be
executed during worktree operations.

By default, sets the trust level to "allow", which runs hooks without
prompting. Use --prompt to set the trust level to "prompt" instead, which
requires confirmation before each hook execution.

Trust settings are stored in ~/.config/daft/trust.json and persist across
sessions.
"#)]
    Trust {
        /// Path to repository (defaults to current directory)
        #[arg(default_value = ".")]
        path: std::path::PathBuf,

        #[arg(
            long,
            help = "Set trust level to prompt; require confirmation before each hook"
        )]
        prompt: bool,

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
        Some(HooksCommand::Trust { path, prompt, force }) => cmd_trust(&path, prompt, force),
        Some(HooksCommand::Untrust { path, force }) => cmd_untrust(&path, force),
        Some(HooksCommand::Status { path }) => cmd_status(&path),
        Some(HooksCommand::List { all }) => cmd_list(all),
        None => cmd_status(&std::path::PathBuf::from(".")), // Default to status if no subcommand
    }
}

/// Trust the repository at the given path.
fn cmd_trust(path: &Path, prompt_level: bool, force: bool) -> Result<()> {
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
        let new_level = if prompt_level {
            TrustLevel::Prompt
        } else {
            TrustLevel::Allow
        };

        // Show hooks that exist
        let hooks = find_project_hooks(&git_dir)?;
        if hooks.is_empty() {
            println!("No hooks found in .daft/hooks/");
            println!();
            println!("To create hooks, add executable scripts to .daft/hooks/");
            println!("Available hook types: post-clone, post-init, pre-create, post-create, pre-remove, post-remove");
            return Ok(());
        }

        // Load database to show current trust level
        let db = TrustDatabase::load().context("Failed to load trust database")?;
        let current_level = db.get_trust_level(&git_dir);
        let is_explicit = db.has_explicit_trust(&git_dir);
        let project_root = git_dir.parent().context("Invalid git directory")?;

        println!("Repository: {}", project_root.display());
        println!();
        println!("Hooks found in {}:", PROJECT_HOOKS_DIR);
        for hook in &hooks {
            let name = hook.file_name().unwrap_or_default().to_string_lossy();
            println!("  - {name}");
        }
        println!();

        // Show current trust level
        let current_source = if is_explicit { "" } else { " (default)" };
        println!("Current trust level: {current_level}{current_source}");
        println!("  {}", trust_level_description(current_level));
        println!();

        // Show new trust level
        println!("New trust level: {new_level}");
        println!("  {}", trust_level_description(new_level));
        println!();

        println!("Trusting this repository allows it to run arbitrary scripts");
        println!("during worktree operations.");

        if !force {
            print!("\nTrust this repository? [y/N] ");
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

        println!();
        println!("Repository trusted with level: {new_level}");
        println!("This applies to all worktrees in this repository.");

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

        if current_level == TrustLevel::Deny && !db.has_explicit_trust(&git_dir) {
            println!("Repository is not explicitly trusted.");
            println!("Current trust level: {current_level} (default)");
            return Ok(());
        }

        let project_root = git_dir.parent().context("Invalid git directory")?;
        println!("Repository: {}", project_root.display());
        println!("Current trust level: {current_level}");

        if !force {
            print!("\nRevoke trust for this repository? [y/N] ");
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
        if db.remove_trust(&git_dir) {
            db.save().context("Failed to save trust database")?;
            println!();
            println!("Trust revoked. Hooks will no longer run for this repository.");
        } else {
            println!("Repository was not explicitly trusted.");
        }

        Ok(())
    })();

    std::env::set_current_dir(&original_dir)?;
    result
}

/// Show trust status and available hooks.
fn cmd_status(path: &Path) -> Result<()> {
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

        // Find hooks
        let hooks = find_project_hooks(&git_dir)?;
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

        // Show commands
        match trust_level {
            TrustLevel::Deny => {
                println!("To enable hooks:");
                println!("  git daft hooks trust            # Allow hooks to run");
                println!("  git daft hooks trust --prompt   # Require confirmation");
            }
            TrustLevel::Prompt | TrustLevel::Allow => {
                println!("To revoke trust:");
                println!("  git daft hooks untrust");
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
