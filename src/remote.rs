use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::git::GitCommand;

/// Determines the default branch of a remote Git repository
///
/// This function uses `git ls-remote --symref` to query the remote repository's
/// symbolic reference for HEAD, which points to the default branch. This is more
/// reliable than assuming "main" or "master" since repositories can have any
/// branch as their default.
///
/// # Arguments
/// * `repo_url` - The URL of the remote Git repository to query
///
/// # Returns
/// * `Ok(String)` - The name of the default branch (e.g., "main", "master", "develop")
/// * `Err` - If the remote cannot be reached or doesn't have a valid default branch
///
/// # Remote Query Strategy
/// The `ls-remote --symref` command returns output like:
/// ```text
/// ref: refs/heads/main    HEAD
/// abc123...   HEAD
/// abc123...   refs/heads/main
/// ```
/// We parse the first line to extract the branch name from the symbolic reference.
pub fn get_default_branch_remote(repo_url: &str) -> Result<String> {
    let git = GitCommand::new(false);
    let output_str = git
        .ls_remote_symref(repo_url)
        .context("Failed to query remote HEAD ref")?;

    // Parse the symbolic reference output to extract the default branch name
    for line in output_str.lines() {
        if line.starts_with("ref:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let ref_path = parts[1];
                if let Some(branch) = ref_path.strip_prefix("refs/heads/") {
                    return Ok(branch.to_string());
                }
            }
        }
    }

    anyhow::bail!("Could not parse default branch from ls-remote output")
}

/// Checks if a remote repository is empty (has no refs/commits).
///
/// Empty repositories (like a freshly created GitHub repo with no README)
/// return no refs from `git ls-remote`. This function detects that case
/// so callers can handle it appropriately.
///
/// # Arguments
/// * `repo_url` - The URL of the remote Git repository to check
///
/// # Returns
/// * `Ok(true)` - The repository is empty (no refs)
/// * `Ok(false)` - The repository has at least one ref
/// * `Err` - If the remote cannot be reached
pub fn is_remote_empty(repo_url: &str) -> Result<bool> {
    let git = GitCommand::new(false);
    let output_str = git
        .ls_remote_symref(repo_url)
        .context("Failed to query remote")?;

    // Empty repos return no refs at all (or only whitespace)
    Ok(output_str.lines().all(|line| line.trim().is_empty()))
}

pub fn get_default_branch_local(git_common_dir: &Path, remote_name: &str) -> Result<String> {
    let head_ref_file = git_common_dir
        .join("refs/remotes")
        .join(remote_name)
        .join("HEAD");

    // Try to read the local HEAD reference file first
    if head_ref_file.exists() {
        let content = fs::read_to_string(&head_ref_file)
            .with_context(|| format!("Failed to read {}", head_ref_file.display()))?;

        let content = content.trim();

        if let Some(ref_path) = content.strip_prefix("ref: ") {
            let prefix = format!("refs/remotes/{remote_name}/");
            if let Some(branch) = ref_path.strip_prefix(&prefix) {
                if !branch.is_empty() {
                    return Ok(branch.to_string());
                }
            }
        }
    }

    // Fallback: Try to determine default branch from remote
    // This happens when remote HEAD isn't set up locally
    let git = GitCommand::new(false);
    if let Ok(output_str) = git.ls_remote_symref(remote_name) {
        for line in output_str.lines() {
            if line.starts_with("ref:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let ref_path = parts[1];
                    if let Some(branch) = ref_path.strip_prefix("refs/heads/") {
                        return Ok(branch.to_string());
                    }
                }
            }
        }
    }

    anyhow::bail!(
        "Could not determine default branch for remote '{}'. \
        The local HEAD reference file was not found at '{}' and remote query failed. \
        Try: 'git remote set-head {} --auto' and 'git fetch {}'",
        remote_name,
        head_ref_file.display(),
        remote_name,
        remote_name
    );
}

pub fn get_remote_branches(remote_name: &str) -> Result<Vec<String>> {
    let git = GitCommand::new(false);
    let output_str = git
        .ls_remote_heads(remote_name, None)
        .context("Failed to get remote branches")?;

    let mut branches = Vec::new();
    for line in output_str.lines() {
        if let Some(tab_pos) = line.find('\t') {
            let ref_name = &line[tab_pos + 1..];
            if let Some(branch) = ref_name.strip_prefix("refs/heads/") {
                branches.push(branch.to_string());
            }
        }
    }

    Ok(branches)
}

pub fn remote_branch_exists(remote_name: &str, branch: &str) -> Result<bool> {
    let git = GitCommand::new(false);
    git.ls_remote_branch_exists(remote_name, branch)
        .context("Failed to check remote branch existence")
}

/// Get the default branch from the remote HEAD in an existing repository.
///
/// This function queries the remote to determine its default branch.
/// It works from within an existing git repository.
///
/// # Arguments
/// * `remote_name` - The name of the remote (e.g., "origin")
///
/// # Returns
/// * `Ok(String)` - The name of the default branch
/// * `Err` - If the remote cannot be queried
pub fn get_default_branch_from_remote_head(remote_name: &str) -> Result<String> {
    let git = GitCommand::new(false);

    // Try to query the remote for its default branch
    let output_str = git
        .ls_remote_symref(remote_name)
        .context("Failed to query remote HEAD ref")?;

    // Parse the symbolic reference output
    for line in output_str.lines() {
        if line.starts_with("ref:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let ref_path = parts[1];
                if let Some(branch) = ref_path.strip_prefix("refs/heads/") {
                    return Ok(branch.to_string());
                }
            }
        }
    }

    anyhow::bail!(
        "Could not determine default branch for remote '{}'",
        remote_name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires network access to origin remote
    fn test_remote_branch_exists() {
        let result = remote_branch_exists("origin", "nonexistent-branch");
        assert!(result.is_ok());
    }

    #[test]
    #[ignore] // Requires network access to origin remote
    fn test_get_remote_branches() {
        let result = get_remote_branches("origin");
        assert!(result.is_ok());
    }

    #[test]
    #[ignore] // Requires network access to origin remote
    fn test_is_remote_empty() {
        // This is a basic test - the function itself is tested more thoroughly
        // in integration tests with actual empty repositories
        let result = is_remote_empty("origin");
        assert!(result.is_ok());
        // Our own repo should not be empty
        assert!(!result.unwrap());
    }
}
