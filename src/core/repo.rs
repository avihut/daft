//! Repository query functions and URL parsing utilities.
//!
//! These functions provide common repository introspection operations used
//! across core and command layers. They are thin wrappers around `GitCommand`
//! that provide convenient, context-aware access to repository state.

use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use which::which;

/// Check whether the current directory is inside a Git repository.
pub fn is_git_repository() -> Result<bool> {
    let git = GitCommand::new(true); // Use quiet mode for this check
    git.is_inside_git_repo()
}

/// Return the canonicalized path to the git common directory.
///
/// This is critical for trust database lookups — git rev-parse returns
/// relative paths in some contexts (e.g., ".git") and absolute paths in
/// others. Without canonicalization, trust set from one worktree wouldn't
/// be recognized from another worktree of the same repo.
pub fn get_git_common_dir() -> Result<PathBuf> {
    let git = GitCommand::new(false);
    let path_str = git
        .rev_parse_git_common_dir()
        .context("Failed to get git common directory")?;
    let path = PathBuf::from(path_str);

    path.canonicalize()
        .with_context(|| format!("Failed to canonicalize git directory: {}", path.display()))
}

/// Return the path to the current worktree.
pub fn get_current_worktree_path() -> Result<PathBuf> {
    let git = GitCommand::new(false);
    git.get_current_worktree_path()
}

/// Return the project root directory (parent of the git common dir).
pub fn get_project_root() -> Result<PathBuf> {
    let git_common_dir = get_git_common_dir()?;
    let project_root = git_common_dir
        .parent()
        .context("Failed to determine project root directory")?;
    Ok(project_root.to_path_buf())
}

/// Return the name of the currently checked-out branch.
pub fn get_current_branch() -> Result<String> {
    let git = GitCommand::new(false);
    let branch = git
        .symbolic_ref_short_head()
        .context("Could not determine current branch (maybe detached HEAD?)")?;

    if branch.is_empty() {
        anyhow::bail!("Empty branch name returned");
    }

    Ok(branch)
}

/// Resolve the initial branch name from explicit argument, git config, or default.
///
/// Priority:
/// 1. Explicitly provided branch name (if Some)
/// 2. Git config init.defaultBranch (global)
/// 3. Fallback to "master"
///
/// This function is used when creating new repositories or handling empty
/// repositories where no remote default branch can be queried.
pub fn resolve_initial_branch(branch: &Option<String>) -> String {
    if let Some(branch) = branch {
        return branch.clone();
    }

    // Query git config for init.defaultBranch
    let git = GitCommand::new(true); // quiet mode for config query
    if let Ok(Some(configured_branch)) = git.config_get_global("init.defaultBranch") {
        if !configured_branch.is_empty() {
            return configured_branch;
        }
    }

    // Fallback to "master"
    "master".to_string()
}

/// Extract a repository name from a URL (SSH, HTTPS, or shorthand).
///
/// The extracted name is sanitized for security (path traversal, injection, etc.).
pub fn extract_repo_name(repo_url: &str) -> Result<String> {
    let repo_name = if repo_url.contains(':') {
        let parts: Vec<&str> = repo_url.split(':').collect();
        if parts.len() >= 2 {
            Path::new(parts[1])
                .file_stem()
                .and_then(|s| s.to_str())
                .context("Failed to extract repository name from shorthand URL")?
                .to_string()
        } else {
            anyhow::bail!("Invalid repository URL format");
        }
    } else {
        Path::new(repo_url)
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Failed to extract repository name from URL")?
            .to_string()
    };

    if repo_name.is_empty() {
        anyhow::bail!("Could not extract repository name from URL: '{}'", repo_url);
    }

    // Security: Sanitize the extracted repository name
    let sanitized_name = sanitize_extracted_name(&repo_name)?;

    Ok(sanitized_name)
}

/// Sanitizes an extracted repository name for security.
///
/// Applies security measures to prevent injection attacks, path traversal,
/// and other vulnerabilities.
fn sanitize_extracted_name(name: &str) -> Result<String> {
    // Remove null bytes and control characters
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '\0')
        .collect();

    // Remove dangerous characters that could be used for injection
    let safe_chars: String = cleaned
        .chars()
        .filter(|c| match c {
            // Allow alphanumeric, hyphens, underscores, and dots
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => true,
            _ => false,
        })
        .collect();

    // Remove leading/trailing dots and ensure it's not empty
    let trimmed = safe_chars.trim_matches('.');

    if trimmed.is_empty() {
        anyhow::bail!("Repository name contains only unsafe characters");
    }

    // Prevent path traversal patterns
    if trimmed.contains("..") {
        anyhow::bail!("Repository name contains path traversal patterns");
    }

    // Length limit
    if trimmed.len() > 255 {
        anyhow::bail!("Repository name too long after sanitization");
    }

    Ok(trimmed.to_string())
}

/// Check that required external tools are installed.
pub fn check_dependencies() -> Result<()> {
    let required_tools = vec!["git", "basename", "awk"];
    let mut missing = Vec::new();

    for tool in required_tools {
        if which(tool).is_err() {
            missing.push(tool);
        }
    }

    if !missing.is_empty() {
        anyhow::bail!("Missing required dependencies: {}", missing.join(", "));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_repo_name_ssh() {
        let url = "git@github.com:user/repo.git";
        let name = extract_repo_name(url).unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn test_extract_repo_name_https() {
        let url = "https://github.com/user/repo.git";
        let name = extract_repo_name(url).unwrap();
        assert_eq!(name, "repo");
    }

    #[test]
    fn test_extract_repo_name_shorthand() {
        let url = "user:repo.git";
        let name = extract_repo_name(url).unwrap();
        assert_eq!(name, "repo");
    }
}
