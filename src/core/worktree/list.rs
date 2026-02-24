//! Core logic for the `git-worktree-list` / `daft list` command.
//!
//! Collects enriched worktree information (ahead/behind counts, dirty status,
//! last commit age and subject) for display.

use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Enriched information about a single worktree.
pub struct WorktreeInfo {
    /// Branch name (stripped of `refs/heads/` prefix), or `"(detached)"`.
    pub name: String,
    /// Absolute path to the worktree.
    pub path: PathBuf,
    /// Whether this worktree is the one the user is currently inside.
    pub is_current: bool,
    /// Commits ahead of the base branch (None if not computable).
    pub ahead: Option<usize>,
    /// Commits behind the base branch (None if not computable).
    pub behind: Option<usize>,
    /// Whether the worktree has uncommitted or untracked changes.
    pub is_dirty: bool,
    /// Unix timestamp of the last commit (None if unavailable).
    pub last_commit_timestamp: Option<i64>,
    /// Subject line of the last commit.
    pub last_commit_subject: String,
    /// Unix timestamp of branch creation (None for detached HEAD or if unavailable).
    pub branch_creation_timestamp: Option<i64>,
    /// Short HEAD commit SHA.
    pub head_sha: String,
    /// Remote tracking branch (e.g. "origin/main"), if any.
    pub remote_branch: Option<String>,
}

/// Raw entry parsed from `git worktree list --porcelain`.
struct PorcelainEntry {
    path: PathBuf,
    head_sha: String,
    branch: Option<String>,
    is_bare: bool,
    is_detached: bool,
}

/// Parse the porcelain output of `git worktree list --porcelain`.
///
/// Each entry is separated by a blank line and has the form:
/// ```text
/// worktree /path/to/worktree
/// HEAD <sha>
/// branch refs/heads/branch-name
/// ```
/// Bare entries have `bare` instead of `branch`.
/// Detached entries have `detached` instead of `branch`.
fn parse_porcelain(output: &str) -> Vec<PorcelainEntry> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: String = String::new();
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;
    let mut is_detached = false;

    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Save previous entry if any
            if let Some(path) = current_path.take() {
                entries.push(PorcelainEntry {
                    path,
                    head_sha: std::mem::take(&mut current_head),
                    branch: current_branch.take(),
                    is_bare,
                    is_detached,
                });
            }
            current_path = Some(PathBuf::from(path_str));
            current_head = String::new();
            current_branch = None;
            is_bare = false;
            is_detached = false;
        } else if let Some(sha) = line.strip_prefix("HEAD ") {
            current_head = sha.to_string();
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            is_detached = true;
        }
    }
    // Don't forget the last entry
    if let Some(path) = current_path.take() {
        entries.push(PorcelainEntry {
            path,
            head_sha: current_head,
            branch: current_branch.take(),
            is_bare,
            is_detached,
        });
    }

    entries
}

/// Get ahead/behind counts for a branch relative to a base branch.
///
/// Runs `git rev-list --left-right --count base...branch` in the given
/// worktree directory. Returns `(ahead, behind)` or `None` if the comparison
/// is not possible (e.g. unrelated histories, missing refs).
fn get_ahead_behind(
    base_branch: &str,
    branch: &str,
    worktree_path: &Path,
) -> Option<(usize, usize)> {
    let range = format!("{base_branch}...{branch}");
    let output = Command::new("git")
        .args(["rev-list", "--left-right", "--count", &range])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let parts: Vec<&str> = stdout.trim().split('\t').collect();
    if parts.len() == 2 {
        let behind = parts[0].parse::<usize>().ok()?;
        let ahead = parts[1].parse::<usize>().ok()?;
        Some((ahead, behind))
    } else {
        None
    }
}

/// Get the last commit's Unix timestamp and subject for a worktree.
///
/// Returns `(timestamp, subject)` where timestamp is seconds since epoch.
fn get_last_commit_info(worktree_path: &Path) -> (Option<i64>, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct\x1f%s"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            if let Some((ts_str, subject)) = trimmed.split_once('\x1f') {
                let timestamp = ts_str.parse::<i64>().ok();
                (timestamp, subject.to_string())
            } else {
                (None, String::new())
            }
        }
        _ => (None, String::new()),
    }
}

/// Get the Unix timestamp of when a branch was first created.
///
/// Primary: oldest reflog entry for the branch.
/// Fallback: timestamp of the first commit on the branch.
/// Returns `None` for detached HEAD or if both methods fail.
fn get_branch_creation_timestamp(branch: &str, worktree_path: &Path) -> Option<i64> {
    // Primary: oldest reflog entry
    let reflog_output = Command::new("git")
        .args(["reflog", "show", branch, "--format=%ct"])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if reflog_output.status.success() {
        let stdout = String::from_utf8_lossy(&reflog_output.stdout);
        // Last line is the oldest reflog entry
        if let Some(last_line) = stdout.trim().lines().last() {
            if let Ok(ts) = last_line.trim().parse::<i64>() {
                return Some(ts);
            }
        }
    }

    // Fallback: first commit on the branch
    let log_output = Command::new("git")
        .args(["log", "--reverse", "--format=%ct", branch])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if log_output.status.success() {
        let stdout = String::from_utf8_lossy(&log_output.stdout);
        if let Some(first_line) = stdout.trim().lines().next() {
            if let Ok(ts) = first_line.trim().parse::<i64>() {
                return Some(ts);
            }
        }
    }

    None
}

/// Get the remote tracking branch for a local branch (e.g. "origin/main").
fn get_remote_tracking_branch(branch: &str, worktree_path: &Path) -> Option<String> {
    let refspec = format!("refs/heads/{branch}");
    let output = Command::new("git")
        .args(["for-each-ref", "--format=%(upstream:short)", &refspec])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let upstream = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if upstream.is_empty() {
        None
    } else {
        Some(upstream)
    }
}

/// Collect enriched worktree information for all worktrees in the project.
///
/// Parses the porcelain output, skips bare entries, enriches each entry with
/// ahead/behind, dirty status, and last commit info, then sorts alphabetically
/// by name (case-insensitive).
pub fn collect_worktree_info(
    git: &GitCommand,
    base_branch: &str,
    current_worktree_path: &Path,
) -> Result<Vec<WorktreeInfo>> {
    let porcelain_output = git
        .worktree_list_porcelain()
        .context("Failed to list worktrees")?;

    let entries = parse_porcelain(&porcelain_output);
    let mut infos = Vec::new();

    for entry in entries {
        // Skip bare entries (the bare repo root)
        if entry.is_bare {
            continue;
        }

        let branch_display = if entry.is_detached {
            "(detached)".to_string()
        } else {
            entry.branch.clone().unwrap_or_default()
        };

        // Canonicalize for comparison (ignore errors, fall back to raw path)
        let canonical_entry = entry
            .path
            .canonicalize()
            .unwrap_or_else(|_| entry.path.clone());
        let is_current = canonical_entry == current_worktree_path;

        // Ahead/behind relative to base branch
        let (ahead, behind) = if !entry.is_detached {
            if let Some(branch) = &entry.branch {
                match get_ahead_behind(base_branch, branch, &entry.path) {
                    Some((a, b)) => (Some(a), Some(b)),
                    None => (None, None),
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Dirty check
        let is_dirty = git.has_uncommitted_changes_in(&entry.path).unwrap_or(false);

        // Last commit info
        let (last_commit_timestamp, last_commit_subject) = get_last_commit_info(&entry.path);

        // Branch creation timestamp (only for non-detached worktrees)
        let branch_creation_timestamp = if !entry.is_detached {
            entry
                .branch
                .as_deref()
                .and_then(|b| get_branch_creation_timestamp(b, &entry.path))
        } else {
            None
        };

        // Short HEAD SHA (first 7 characters)
        let head_sha = if entry.head_sha.len() >= 7 {
            entry.head_sha[..7].to_string()
        } else {
            entry.head_sha.clone()
        };

        // Remote tracking branch
        let remote_branch = if !entry.is_detached {
            entry
                .branch
                .as_deref()
                .and_then(|b| get_remote_tracking_branch(b, &entry.path))
        } else {
            None
        };

        infos.push(WorktreeInfo {
            name: branch_display,
            path: entry.path,
            is_current,
            ahead,
            behind,
            is_dirty,
            last_commit_timestamp,
            last_commit_subject,
            branch_creation_timestamp,
            head_sha,
            remote_branch,
        });
    }

    // Sort alphabetically by name (case-insensitive)
    infos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(infos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_porcelain_basic() {
        let output = "\
worktree /home/user/project/main
HEAD abc123
branch refs/heads/main

worktree /home/user/project/feature
HEAD def456
branch refs/heads/feature-branch
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].path, PathBuf::from("/home/user/project/main"));
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(!entries[0].is_bare);
        assert!(!entries[0].is_detached);

        assert_eq!(entries[1].path, PathBuf::from("/home/user/project/feature"));
        assert_eq!(entries[1].branch.as_deref(), Some("feature-branch"));
        assert!(!entries[1].is_bare);
        assert!(!entries[1].is_detached);
    }

    #[test]
    fn test_parse_porcelain_bare_skip() {
        let output = "\
worktree /home/user/project
HEAD abc123
bare

worktree /home/user/project/main
HEAD def456
branch refs/heads/main
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 2);

        // First entry is the bare root
        assert!(entries[0].is_bare);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/project"));

        // Second entry is a normal worktree
        assert!(!entries[1].is_bare);
        assert_eq!(entries[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn test_parse_porcelain_detached_head() {
        let output = "\
worktree /home/user/project/detached-wt
HEAD abc123
detached
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 1);

        assert!(entries[0].is_detached);
        assert!(!entries[0].is_bare);
        assert!(entries[0].branch.is_none());
        assert_eq!(
            entries[0].path,
            PathBuf::from("/home/user/project/detached-wt")
        );
    }

    #[test]
    fn test_parse_porcelain_empty() {
        let entries = parse_porcelain("");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_porcelain_mixed() {
        let output = "\
worktree /home/user/project
HEAD abc123
bare

worktree /home/user/project/main
HEAD def456
branch refs/heads/main

worktree /home/user/project/hotfix
HEAD 789abc
detached

worktree /home/user/project/feature
HEAD aaa111
branch refs/heads/feature/cool
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 4);

        assert!(entries[0].is_bare);
        assert!(!entries[1].is_bare);
        assert!(!entries[1].is_detached);
        assert_eq!(entries[1].branch.as_deref(), Some("main"));
        assert!(entries[2].is_detached);
        assert!(!entries[3].is_bare);
        assert_eq!(entries[3].branch.as_deref(), Some("feature/cool"));
    }
}
