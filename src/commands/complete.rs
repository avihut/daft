/// Dynamic completion helper for shell completions
///
/// This module provides the hidden `__complete` command that shells invoke
/// to get dynamic completion suggestions (e.g., branch names).
///
/// Performance target: < 50ms response time
use anyhow::Result;
use clap::Parser;
use std::process::Command;

#[derive(Parser)]
#[command(name = "daft-complete")]
#[command(about = "Internal helper for dynamic shell completions (not for direct use)")]
#[command(hide = true)]
struct Args {
    #[arg(help = "Command requesting completions")]
    command: String,

    #[arg(help = "Word prefix to complete")]
    word: String,

    #[arg(short, long, help = "Position of the word being completed (0-indexed)")]
    position: Option<usize>,

    #[arg(
        short,
        long,
        help = "Enable verbose error output for debugging completion issues"
    )]
    verbose: bool,
}

pub fn run() -> Result<()> {
    // When called as a subcommand, skip "daft" and "__complete" from args
    let mut args_vec: Vec<String> = std::env::args().collect();

    // If args start with [daft, __complete, ...], keep only [daft, ...]
    // to make clap parse correctly
    if args_vec.len() >= 2 && args_vec[1] == "__complete" {
        args_vec.remove(1); // Remove "__complete", keep "daft" for clap
    }

    let args = Args::parse_from(&args_vec);

    let suggestions = complete(
        &args.command,
        args.position.unwrap_or(1),
        &args.word,
        args.verbose,
    )?;

    for suggestion in suggestions {
        println!("{}", suggestion);
    }

    Ok(())
}

/// Provide context-aware completions based on command and position
fn complete(command: &str, position: usize, word: &str, verbose: bool) -> Result<Vec<String>> {
    match (command, position) {
        // git-worktree-checkout: complete existing branch names
        ("git-worktree-checkout", 1) => complete_existing_branches(word, verbose),

        // git-worktree-checkout-branch: position 1 = new branch name (no completion),
        // position 2 = base branch name (existing branches)
        ("git-worktree-checkout-branch", 1) => {
            // New branch name - suggest based on common patterns
            Ok(suggest_new_branch_names(word))
        }
        ("git-worktree-checkout-branch", 2) => complete_existing_branches(word, verbose),

        // git-worktree-clone: repository URL (no dynamic completion for now)
        ("git-worktree-clone", 1) => Ok(vec![]),

        // git-worktree-init: repository name (no dynamic completion)
        ("git-worktree-init", 1) => Ok(vec![]),

        // git-worktree-carry: complete existing branch/worktree names
        ("git-worktree-carry", _) => complete_existing_branches(word, verbose),

        // git-worktree-fetch: complete existing branch/worktree names
        ("git-worktree-fetch", _) => complete_existing_branches(word, verbose),

        // git-worktree-prune: no arguments
        ("git-worktree-prune", _) => Ok(vec![]),

        // git-worktree-flow-adopt: directory path (no dynamic completion)
        ("git-worktree-flow-adopt", _) => Ok(vec![]),

        // git-worktree-flow-eject: directory path (no dynamic completion)
        ("git-worktree-flow-eject", _) => Ok(vec![]),

        // Default: no completions
        _ => Ok(vec![]),
    }
}

/// Complete existing branch names (local and remote)
fn complete_existing_branches(prefix: &str, verbose: bool) -> Result<Vec<String>> {
    // Use git for-each-ref for fast, parseable output
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/",
            "refs/remotes/origin/",
        ])
        .output()?;

    if !output.status.success() {
        // Not in a git repository or git command failed
        if verbose {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Git command failed: {}", stderr.trim());
            eprintln!("Exit code: {}", output.status.code().unwrap_or(-1));
            if !std::path::Path::new(".git").exists() {
                eprintln!("Note: Not in a git repository (no .git directory found)");
            }
        }
        return Ok(vec![]);
    }

    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|branch| {
            // Filter out HEAD reference and apply prefix filter
            !branch.contains("HEAD") && branch.starts_with(prefix)
        })
        .map(|branch| {
            // Remove "origin/" prefix for cleaner suggestions
            branch.trim_start_matches("origin/").to_string()
        })
        .collect();

    // Deduplicate branches (local and remote might have same name)
    let mut unique_branches: Vec<String> = branches;
    unique_branches.sort();
    unique_branches.dedup();

    if verbose && unique_branches.is_empty() {
        eprintln!("No branches found matching prefix: '{}'", prefix);
    }

    Ok(unique_branches)
}

/// Suggest common branch name patterns for new branches
fn suggest_new_branch_names(prefix: &str) -> Vec<String> {
    let patterns = [
        "feature/",
        "bugfix/",
        "hotfix/",
        "release/",
        "fix/",
        "feat/",
        "chore/",
        "docs/",
        "test/",
        "refactor/",
    ];

    patterns
        .iter()
        .filter(|pattern| pattern.starts_with(prefix))
        .map(|pattern| pattern.to_string())
        .collect()
}

/// Complete local branches only (for base branch selection)
#[allow(dead_code)]
fn complete_local_branches(prefix: &str, verbose: bool) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .output()?;

    if !output.status.success() {
        if verbose {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Git command failed (local branches): {}", stderr.trim());
        }
        return Ok(vec![]);
    }

    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|branch| branch.starts_with(prefix))
        .map(String::from)
        .collect();

    Ok(branches)
}

/// Complete remote branches only
#[allow(dead_code)]
fn complete_remote_branches(prefix: &str, verbose: bool) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes/origin/",
        ])
        .output()?;

    if !output.status.success() {
        if verbose {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("Git command failed (remote branches): {}", stderr.trim());
        }
        return Ok(vec![]);
    }

    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|branch| !branch.contains("HEAD") && branch.starts_with(prefix))
        .map(|branch| branch.trim_start_matches("origin/").to_string())
        .collect();

    Ok(branches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suggest_new_branch_names() {
        let suggestions = suggest_new_branch_names("fea");
        assert!(suggestions.contains(&"feature/".to_string()));

        let suggestions = suggest_new_branch_names("hot");
        assert!(suggestions.contains(&"hotfix/".to_string()));

        let suggestions = suggest_new_branch_names("");
        assert!(suggestions.len() >= 10); // All patterns
    }

    #[test]
    fn test_suggest_new_branch_names_no_match() {
        let suggestions = suggest_new_branch_names("xyz");
        assert!(suggestions.is_empty());
    }
}
