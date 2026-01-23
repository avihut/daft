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
use daft::{get_git_common_dir, is_git_repository};
use std::io::{self, Write};
use std::path::Path;

#[derive(Parser)]
#[command(name = "hooks")]
#[command(about = "Manage hook trust for repositories")]
#[command(long_about = r#"
Manage trust settings for daft hooks.

Hooks are scripts in .daft/hooks/ that run during worktree operations.
For security, hooks only run in repositories you explicitly trust.

Trust levels:
  deny   - Never run hooks (default for unknown repos)
  prompt - Ask before each hook execution
  allow  - Run hooks without prompting

Examples:
  git daft hooks status          # Show hooks and trust status
  git daft hooks trust           # Trust current repo (allow level)
  git daft hooks trust --prompt  # Trust with confirmation prompts
  git daft hooks untrust         # Revoke trust for current repo
  git daft hooks list            # List all trusted repositories
"#)]
pub struct Args {
    #[command(subcommand)]
    command: Option<HooksCommand>,
}

#[derive(Subcommand)]
enum HooksCommand {
    /// Trust the current repository to run hooks
    #[command(long_about = r#"
Trust the current repository to run hooks.

By default, grants 'allow' level which runs hooks without prompting.
Use --prompt to require confirmation before each hook execution.

Trust is stored in ~/.config/daft/trust.json, not in the repository.
"#)]
    Trust {
        /// Trust level: require prompts before running hooks
        #[arg(long, help = "Require prompts before running hooks")]
        prompt: bool,

        /// Skip confirmation prompt
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
    },

    /// Revoke trust for the current repository
    #[command(long_about = r#"
Revoke trust for the current repository.

After running this command, hooks will no longer execute for this repository.
"#)]
    Untrust {
        /// Skip confirmation prompt
        #[arg(short = 'y', long, help = "Skip confirmation prompt")]
        yes: bool,
    },

    /// Show trust status and available hooks for current repository
    Status,

    /// List all trusted repositories
    List {
        /// Show all repositories including denied ones
        #[arg(long, help = "Show all repositories including denied ones")]
        all: bool,
    },
}

pub fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = Args::parse_from(args);

    match args.command {
        Some(HooksCommand::Trust { prompt, yes }) => cmd_trust(prompt, yes),
        Some(HooksCommand::Untrust { yes }) => cmd_untrust(yes),
        Some(HooksCommand::Status) => cmd_status(),
        Some(HooksCommand::List { all }) => cmd_list(all),
        None => cmd_status(), // Default to status if no subcommand
    }
}

/// Trust the current repository.
fn cmd_trust(prompt_level: bool, skip_confirm: bool) -> Result<()> {
    // Ensure we're in a git repository
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let git_dir = get_git_common_dir()?;
    let level = if prompt_level {
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

    println!("Repository: {}", git_dir.display());
    println!();
    println!("Hooks found in {}:", PROJECT_HOOKS_DIR);
    for hook in &hooks {
        let size = std::fs::metadata(hook)
            .map(|m| format_size(m.len()))
            .unwrap_or_else(|_| "?".to_string());
        let name = hook.file_name().unwrap_or_default().to_string_lossy();
        println!("  - {name}  ({size})");
    }
    println!();

    println!("Trusting this repository allows it to run arbitrary scripts");
    println!("during worktree operations.");
    println!();
    println!("Trust level: {level}");

    if !skip_confirm {
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

    // Load, update, and save the trust database
    let mut db = TrustDatabase::load().context("Failed to load trust database")?;
    db.set_trust_level(&git_dir, level);
    db.save().context("Failed to save trust database")?;

    println!();
    println!("Repository trusted with level: {level}");
    if level == TrustLevel::Allow {
        println!("Hooks will now run automatically.");
    } else {
        println!("You will be prompted before each hook execution.");
    }

    Ok(())
}

/// Revoke trust for the current repository.
fn cmd_untrust(skip_confirm: bool) -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let git_dir = get_git_common_dir()?;

    let db = TrustDatabase::load().context("Failed to load trust database")?;
    let current_level = db.get_trust_level(&git_dir);

    if current_level == TrustLevel::Deny && !db.has_explicit_trust(&git_dir) {
        println!("Repository is not explicitly trusted.");
        println!("Current trust level: {current_level} (default)");
        return Ok(());
    }

    println!("Repository: {}", git_dir.display());
    println!("Current trust level: {current_level}");

    if !skip_confirm {
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
}

/// Show trust status and available hooks.
fn cmd_status() -> Result<()> {
    if !is_git_repository()? {
        anyhow::bail!("Not in a git repository");
    }

    let git_dir = get_git_common_dir()?;
    let db = TrustDatabase::load().context("Failed to load trust database")?;
    let trust_level = db.get_trust_level(&git_dir);
    let is_explicit = db.has_explicit_trust(&git_dir);

    println!("Repository: {}", git_dir.display());
    println!();

    // Trust status
    let trust_source = if is_explicit { "" } else { " (default)" };
    println!("Trust level: {trust_level}{trust_source}");
    match trust_level {
        TrustLevel::Deny => {
            println!("  Hooks will NOT run for this repository.");
        }
        TrustLevel::Prompt => {
            println!("  You will be prompted before each hook execution.");
        }
        TrustLevel::Allow => {
            println!("  Hooks will run automatically without prompting.");
        }
    }
    println!();

    // Find hooks
    let hooks = find_project_hooks(&git_dir)?;
    if hooks.is_empty() {
        println!("No hooks found in {}:", PROJECT_HOOKS_DIR);
        println!("  (Create hooks by adding executable scripts to .daft/hooks/)");
    } else {
        println!("Hooks found in {}:", PROJECT_HOOKS_DIR);
        for hook in &hooks {
            let size = std::fs::metadata(hook)
                .map(|m| format_size(m.len()))
                .unwrap_or_else(|_| "?".to_string());
            let name = hook.file_name().unwrap_or_default().to_string_lossy();
            let executable = is_executable(hook);
            let status = if executable { "" } else { " (not executable)" };
            println!("  - {name}  ({size}){status}");
        }
    }

    println!();

    // Show commands
    match trust_level {
        TrustLevel::Deny => {
            println!("To enable hooks:");
            println!("  git daft hooks trust       # Allow hooks to run");
            println!("  git daft hooks trust --prompt  # Require confirmation");
        }
        TrustLevel::Prompt | TrustLevel::Allow => {
            println!("To revoke trust:");
            println!("  git daft hooks untrust");
        }
    }

    Ok(())
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

/// Format a file size in human-readable form.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
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
