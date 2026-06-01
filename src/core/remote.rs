use anyhow::{Context, Result};
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
/// * `use_gitoxide` - Whether to use gitoxide backend for git operations
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
pub fn get_default_branch_remote(repo_url: &str, use_gitoxide: bool) -> Result<String> {
    let git = GitCommand::new(false).with_gitoxide(use_gitoxide);
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
/// * `use_gitoxide` - Whether to use gitoxide backend for git operations
///
/// # Returns
/// * `Ok(true)` - The repository is empty (no refs)
/// * `Ok(false)` - The repository has at least one ref
/// * `Err` - If the remote cannot be reached
pub fn is_remote_empty(repo_url: &str, use_gitoxide: bool) -> Result<bool> {
    let git = GitCommand::new(false).with_gitoxide(use_gitoxide);
    let output_str = git
        .ls_remote_symref(repo_url)
        .context("Failed to query remote")?;

    // Empty repos return no refs at all (or only whitespace)
    Ok(output_str.lines().all(|line| line.trim().is_empty()))
}

/// Extract the default branch name from the output of
/// `git symbolic-ref --short refs/remotes/<remote>/HEAD` (e.g. `"origin/main"`
/// with remote `"origin"` → `Some("main")`; `"origin/feature/x"` →
/// `Some("feature/x")`). Returns `None` when the short ref is empty, has no
/// branch component, or belongs to a different remote. Pure — no I/O.
pub fn default_branch_from_short_symref(short: &str, remote_name: &str) -> Option<String> {
    let prefix = format!("{remote_name}/");
    short
        .strip_prefix(&prefix)
        .filter(|branch| !branch.is_empty())
        .map(String::from)
}

/// Resolve the repository's default branch from the LOCAL `origin/HEAD` symref
/// (`refs/remotes/<remote>/HEAD`) with no network round-trip. Returns `None`
/// when the symref is unset or not symbolic.
///
/// Runs through [`crate::utils::git_command_at`] so an inherited `GIT_DIR`
/// can't retarget the query (the helper clears `GIT_*`), with stderr nulled.
/// `dir` may be a worktree root, a contained-layout container root, or the bare
/// common dir — all resolve the same symref.
pub fn local_default_branch(dir: &Path, remote_name: &str) -> Option<String> {
    let out = crate::utils::git_command_at(dir)
        .args([
            "symbolic-ref",
            "--short",
            &format!("refs/remotes/{remote_name}/HEAD"),
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let short = String::from_utf8_lossy(&out.stdout);
    default_branch_from_short_symref(short.trim(), remote_name)
}

/// Determine a repository's default branch from local state, falling back to a
/// remote query. Prefers the local `origin/HEAD` symref (fast, offline); only
/// reaches for `ls-remote` when the symref isn't set up locally.
///
/// `dir` may be a worktree, container root, or bare common dir.
pub fn get_default_branch_local(
    dir: &Path,
    remote_name: &str,
    use_gitoxide: bool,
) -> Result<String> {
    // Prefer the local origin/HEAD symref — no network round-trip.
    if let Some(branch) = local_default_branch(dir, remote_name) {
        return Ok(branch);
    }

    // Fallback: query the remote when the local HEAD symref isn't set up.
    let git = GitCommand::new(false).with_gitoxide(use_gitoxide);
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
        The local HEAD symref was not set and the remote query failed. \
        Try: 'git remote set-head {} --auto' and 'git fetch {}'",
        remote_name,
        remote_name,
        remote_name
    );
}

pub fn get_remote_branches(remote_name: &str, use_gitoxide: bool) -> Result<Vec<String>> {
    let git = GitCommand::new(false).with_gitoxide(use_gitoxide);
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

pub fn remote_branch_exists(remote_name: &str, branch: &str, use_gitoxide: bool) -> Result<bool> {
    let git = GitCommand::new(false).with_gitoxide(use_gitoxide);
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
/// * `use_gitoxide` - Whether to use gitoxide backend for git operations
///
/// # Returns
/// * `Ok(String)` - The name of the default branch
/// * `Err` - If the remote cannot be queried
pub fn get_default_branch_from_remote_head(
    remote_name: &str,
    use_gitoxide: bool,
) -> Result<String> {
    let git = GitCommand::new(false).with_gitoxide(use_gitoxide);

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
        let result = remote_branch_exists("origin", "nonexistent-branch", false);
        assert!(result.is_ok());
    }

    #[test]
    #[ignore] // Requires network access to origin remote
    fn test_get_remote_branches() {
        let result = get_remote_branches("origin", false);
        assert!(result.is_ok());
    }

    #[test]
    #[ignore] // Requires network access to origin remote
    fn test_is_remote_empty() {
        // This is a basic test - the function itself is tested more thoroughly
        // in integration tests with actual empty repositories
        let result = is_remote_empty("origin", false);
        assert!(result.is_ok());
        // Our own repo should not be empty
        assert!(!result.unwrap());
    }

    // ── default_branch_from_short_symref (pure parser) ────────────────────────

    #[test]
    fn short_symref_extracts_branch_under_remote() {
        assert_eq!(
            default_branch_from_short_symref("origin/main", "origin"),
            Some("main".to_string())
        );
    }

    #[test]
    fn short_symref_preserves_slashed_branch() {
        assert_eq!(
            default_branch_from_short_symref("origin/feature/x", "origin"),
            Some("feature/x".to_string())
        );
    }

    #[test]
    fn short_symref_empty_branch_is_none() {
        assert_eq!(default_branch_from_short_symref("origin/", "origin"), None);
    }

    #[test]
    fn short_symref_blank_is_none() {
        assert_eq!(default_branch_from_short_symref("", "origin"), None);
    }

    #[test]
    fn short_symref_other_remote_is_none() {
        // A symref under a different remote must not be misread as origin's default.
        assert_eq!(
            default_branch_from_short_symref("upstream/main", "origin"),
            None
        );
    }

    // ── local_default_branch (real git, local-only — no network) ──────────────

    /// Run git in an isolated temp repo with a fixed identity — never global
    /// config (CLAUDE.md Rule #1), never this project's repo (Rule #2).
    fn git_at(dir: &std::path::Path, args: &[&str]) {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn local_default_branch_reads_origin_head_from_worktree() {
        let dir = tempfile::tempdir().unwrap();
        git_at(dir.path(), &["init", "-q", "-b", "main"]);
        // Hand-write the remote-tracking origin/HEAD symref (no network round-trip).
        git_at(
            dir.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        assert_eq!(
            local_default_branch(dir.path(), "origin"),
            Some("main".to_string())
        );
    }

    #[test]
    fn local_default_branch_reads_origin_head_from_bare_common_dir() {
        // get_default_branch_local passes the bare common dir; the symref read
        // must work there too.
        let dir = tempfile::tempdir().unwrap();
        git_at(dir.path(), &["init", "-q", "--bare", "-b", "main"]);
        git_at(
            dir.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/develop",
            ],
        );
        assert_eq!(
            local_default_branch(dir.path(), "origin"),
            Some("develop".to_string())
        );
    }

    #[test]
    fn local_default_branch_none_when_head_absent() {
        let dir = tempfile::tempdir().unwrap();
        git_at(dir.path(), &["init", "-q", "-b", "main"]);
        assert_eq!(local_default_branch(dir.path(), "origin"), None);
    }
}
