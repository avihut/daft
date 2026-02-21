use super::{find_project_hooks, styled_trust_level};
use crate::hooks::{TrustDatabase, TrustLevel};
use crate::styles::{bold, cyan, dim, green, red, yellow};
use crate::{get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// Set trust level for the repository at the given path.
pub(super) fn cmd_set_trust(path: &Path, new_level: TrustLevel, force: bool) -> Result<()> {
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
pub(super) fn cmd_deny(path: &Path, force: bool) -> Result<()> {
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

/// List all trusted repositories.
pub(super) fn cmd_list(show_all: bool) -> Result<()> {
    let db = TrustDatabase::load().context("Failed to load trust database")?;

    let repos: Vec<(&str, &crate::hooks::TrustEntry)> = if show_all {
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
pub(super) fn cmd_reset_trust(force: bool) -> Result<()> {
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
pub(super) fn cmd_reset_trust_path(path: &Path, force: bool) -> Result<()> {
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
