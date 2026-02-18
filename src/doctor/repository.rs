//! Repository checks for `daft doctor`.
//!
//! Verifies that the current repository is configured correctly for daft:
//! worktree layout, worktree consistency, fetch refspec, remote HEAD.

use crate::doctor::CheckResult;
use crate::git::GitCommand;
use std::path::{Path, PathBuf};

/// Context for repository checks - gathered once and shared.
pub struct RepoContext {
    pub git_common_dir: PathBuf,
    pub project_root: PathBuf,
    pub current_worktree: PathBuf,
    pub is_bare: bool,
}

/// A parsed worktree entry from `git worktree list --porcelain`.
struct WorktreeEntry {
    path: String,
    is_bare: bool,
}

/// Parse worktree entries from porcelain output.
///
/// Each block is separated by a blank line. Lines starting with "worktree "
/// give the path, and a subsequent "bare" line marks it as the bare repo entry.
fn parse_worktree_entries(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_is_bare = false;

    for line in porcelain.lines() {
        if line.is_empty() {
            // End of block
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    is_bare: current_is_bare,
                });
            }
            current_is_bare = false;
        } else if let Some(path_str) = line.strip_prefix("worktree ") {
            current_path = Some(path_str.to_string());
        } else if line == "bare" {
            current_is_bare = true;
        }
    }

    // Handle last block (porcelain output may not end with blank line)
    if let Some(path) = current_path {
        entries.push(WorktreeEntry {
            path,
            is_bare: current_is_bare,
        });
    }

    entries
}

/// Check if the git common dir is a bare repository by reading its config.
///
/// `git rev-parse --is-bare-repository` returns false inside a worktree,
/// even when the underlying repo IS bare. Instead we check the git config
/// at the common dir level.
fn is_common_dir_bare(git_common_dir: &Path) -> bool {
    let config_path = git_common_dir.join("config");
    if let Ok(content) = std::fs::read_to_string(config_path) {
        // Look for bare = true in the [core] section
        let mut in_core = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_core = trimmed.starts_with("[core]");
            } else if in_core {
                if let Some(value) = trimmed.strip_prefix("bare") {
                    let value = value.trim().strip_prefix('=').map(|v| v.trim());
                    if value == Some("true") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Try to build a RepoContext for the current directory.
/// Returns None if not in a git repository.
pub fn get_repo_context() -> Option<RepoContext> {
    if !crate::is_git_repository().ok()? {
        return None;
    }

    let git_common_dir = crate::get_git_common_dir().ok()?;
    let project_root = git_common_dir.parent()?.to_path_buf();
    let is_bare = is_common_dir_bare(&git_common_dir);
    let current_worktree = std::env::current_dir().ok()?;

    Some(RepoContext {
        git_common_dir,
        project_root,
        current_worktree,
        is_bare,
    })
}

/// Check that the repository uses a daft-compatible worktree layout.
pub fn check_worktree_layout(ctx: &RepoContext) -> CheckResult {
    let git = GitCommand::new(true);

    if !ctx.is_bare {
        return CheckResult::pass("Worktree layout", "standard repository");
    }

    // Count actual worktrees (exclude the bare repo entry)
    let worktree_count = match git.worktree_list_porcelain() {
        Ok(output) => {
            let entries = parse_worktree_entries(&output);
            entries.iter().filter(|e| !e.is_bare).count()
        }
        Err(_) => 0,
    };

    CheckResult::pass(
        "Worktree layout",
        &format!("bare repo with {worktree_count} worktrees"),
    )
}

/// Check that all worktree paths exist on disk.
pub fn check_worktree_consistency(_ctx: &RepoContext) -> CheckResult {
    let git = GitCommand::new(true);

    let porcelain = match git.worktree_list_porcelain() {
        Ok(output) => output,
        Err(e) => {
            return CheckResult::warning(
                "Worktree consistency",
                &format!("could not list worktrees: {e}"),
            );
        }
    };

    let entries = parse_worktree_entries(&porcelain);
    // Only check non-bare entries
    let worktree_entries: Vec<&WorktreeEntry> = entries.iter().filter(|e| !e.is_bare).collect();
    let total = worktree_entries.len();

    let mut orphaned = Vec::new();
    for entry in &worktree_entries {
        let path = Path::new(&entry.path);
        if !path.exists() {
            orphaned.push(entry.path.clone());
        }
    }

    if orphaned.is_empty() {
        if total == 0 {
            CheckResult::pass("Worktree consistency", "all paths valid")
        } else {
            CheckResult::pass(
                "Worktree consistency",
                &format!("all {total} worktree paths valid"),
            )
        }
    } else {
        let details: Vec<String> = orphaned
            .iter()
            .map(|p| format!("Missing path: {p}"))
            .collect();
        CheckResult::warning(
            "Worktree consistency",
            &format!("{} orphaned worktree entries", orphaned.len()),
        )
        .with_suggestion("Run 'git worktree prune' to clean up orphaned entries")
        .with_fix(Box::new(fix_worktree_consistency))
        .with_details(details)
    }
}

/// Fix orphaned worktree entries by running git worktree prune.
pub fn fix_worktree_consistency() -> Result<(), String> {
    let output = std::process::Command::new("git")
        .args(["worktree", "prune"])
        .output()
        .map_err(|e| format!("Failed to run 'git worktree prune': {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("git worktree prune failed: {stderr}"))
    }
}

/// Check that the fetch refspec is configured correctly for bare repos.
pub fn check_fetch_refspec(ctx: &RepoContext) -> CheckResult {
    if !ctx.is_bare {
        return CheckResult::skipped("Fetch refspec", "not a bare repository");
    }

    let git = GitCommand::new(true);
    let expected = "+refs/heads/*:refs/remotes/origin/*";

    match git.config_get("remote.origin.fetch") {
        Ok(Some(refspec)) if refspec == expected => {
            CheckResult::pass("Fetch refspec", "correctly configured")
        }
        Ok(Some(refspec)) => {
            CheckResult::warning("Fetch refspec", &format!("unexpected value: {refspec}"))
                .with_suggestion(
                    "Run 'git config remote.origin.fetch \"+refs/heads/*:refs/remotes/origin/*\"'",
                )
                .with_fix(Box::new(fix_fetch_refspec))
        }
        Ok(None) => {
            // Check if origin remote exists
            match git.remote_exists("origin") {
                Ok(true) => CheckResult::warning("Fetch refspec", "not configured")
                    .with_suggestion("Run 'git config remote.origin.fetch \"+refs/heads/*:refs/remotes/origin/*\"'")
                    .with_fix(Box::new(fix_fetch_refspec)),
                _ => CheckResult::skipped("Fetch refspec", "no origin remote"),
            }
        }
        Err(e) => CheckResult::warning("Fetch refspec", &format!("could not read config: {e}")),
    }
}

/// Fix the fetch refspec by setting the correct value.
pub fn fix_fetch_refspec() -> Result<(), String> {
    let git = GitCommand::new(true);
    git.setup_fetch_refspec("origin")
        .map_err(|e| format!("Failed to set fetch refspec: {e}"))
}

/// Check that remote HEAD (refs/remotes/origin/HEAD) is set.
pub fn check_remote_head(ctx: &RepoContext) -> CheckResult {
    if !ctx.is_bare {
        return CheckResult::skipped("Remote HEAD", "not a bare repository");
    }

    let git = GitCommand::new(true);

    // Check if origin remote exists first
    match git.remote_exists("origin") {
        Ok(false) => return CheckResult::skipped("Remote HEAD", "no origin remote"),
        Err(_) => return CheckResult::skipped("Remote HEAD", "could not check remotes"),
        Ok(true) => {}
    }

    match git.show_ref_exists("refs/remotes/origin/HEAD") {
        Ok(true) => {
            // Try to get the actual target
            match git.config_get("remote.origin.head") {
                Ok(Some(head)) => {
                    let target = head.strip_prefix("refs/remotes/origin/").unwrap_or(&head);
                    CheckResult::pass("Remote HEAD", &format!("set to origin/{target}"))
                }
                _ => CheckResult::pass("Remote HEAD", "set"),
            }
        }
        Ok(false) => CheckResult::warning("Remote HEAD", "not set")
            .with_suggestion("Run 'git remote set-head origin --auto'")
            .with_fix(Box::new(fix_remote_head)),
        Err(e) => CheckResult::warning("Remote HEAD", &format!("could not check: {e}")),
    }
}

/// Fix remote HEAD by running git remote set-head origin --auto.
pub fn fix_remote_head() -> Result<(), String> {
    let git = GitCommand::new(true);
    git.remote_set_head_auto("origin")
        .map_err(|e| format!("Failed to set remote HEAD: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_entries_bare_repo() {
        let porcelain = "\
worktree /home/user/project/.git
bare

worktree /home/user/project/main
HEAD abc123
branch refs/heads/main

worktree /home/user/project/feature
HEAD def456
branch refs/heads/feature

";
        let entries = parse_worktree_entries(porcelain);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].is_bare);
        assert!(!entries[1].is_bare);
        assert!(!entries[2].is_bare);

        let non_bare: Vec<_> = entries.iter().filter(|e| !e.is_bare).collect();
        assert_eq!(non_bare.len(), 2);
    }

    #[test]
    fn test_parse_worktree_entries_no_trailing_newline() {
        let porcelain = "\
worktree /home/user/project
HEAD abc123
branch refs/heads/main";
        let entries = parse_worktree_entries(porcelain);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].is_bare);
    }

    #[test]
    fn test_is_common_dir_bare_true() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        std::fs::write(
            &config,
            "[core]\n\trepositoryformatversion = 0\n\tbare = true\n",
        )
        .unwrap();
        assert!(is_common_dir_bare(temp.path()));
    }

    #[test]
    fn test_is_common_dir_bare_false() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        std::fs::write(
            &config,
            "[core]\n\trepositoryformatversion = 0\n\tbare = false\n",
        )
        .unwrap();
        assert!(!is_common_dir_bare(temp.path()));
    }

    #[test]
    fn test_is_common_dir_bare_no_config() {
        let temp = tempfile::tempdir().unwrap();
        assert!(!is_common_dir_bare(temp.path()));
    }
}
