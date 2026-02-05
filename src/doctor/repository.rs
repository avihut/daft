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
    pub is_bare: bool,
}

/// Try to build a RepoContext for the current directory.
/// Returns None if not in a git repository.
pub fn get_repo_context() -> Option<RepoContext> {
    if !crate::is_git_repository().ok()? {
        return None;
    }

    let git_common_dir = crate::get_git_common_dir().ok()?;
    let project_root = git_common_dir.parent()?.to_path_buf();

    let git = GitCommand::new(true);
    let is_bare = git.rev_parse_is_bare_repository().unwrap_or(false);

    Some(RepoContext {
        git_common_dir,
        project_root,
        is_bare,
    })
}

/// Check that the repository uses a daft-compatible worktree layout.
pub fn check_worktree_layout(ctx: &RepoContext) -> CheckResult {
    let git = GitCommand::new(true);

    if !ctx.is_bare {
        return CheckResult::warning(
            "Worktree layout",
            "Regular (non-bare) repository",
        )
        .with_suggestion(
            "daft works best with bare repos created via 'git worktree-clone' or 'git worktree-init'",
        );
    }

    // Count worktrees
    let worktree_count = match git.worktree_list_porcelain() {
        Ok(output) => output
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        Err(_) => 0,
    };

    CheckResult::pass(
        "Worktree layout",
        &format!("bare repo + {worktree_count} worktrees"),
    )
}

/// Check that all worktree paths exist on disk.
pub fn check_worktree_consistency(ctx: &RepoContext) -> CheckResult {
    let git = GitCommand::new(true);

    let porcelain = match git.worktree_list_porcelain() {
        Ok(output) => output,
        Err(e) => {
            return CheckResult::warning(
                "Worktree consistency",
                &format!("Could not list worktrees: {e}"),
            );
        }
    };

    let mut orphaned = Vec::new();
    let mut total = 0;

    for line in porcelain.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            total += 1;
            let path = Path::new(path_str);
            // Skip the bare repo itself (it's a valid "worktree" entry)
            if path == ctx.git_common_dir {
                continue;
            }
            if !path.exists() {
                orphaned.push(path_str.to_string());
            }
        }
    }

    if orphaned.is_empty() {
        CheckResult::pass(
            "Worktree consistency",
            &format!("{total} worktrees consistent"),
        )
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
        .with_fixable(true)
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
                .with_fixable(true)
        }
        Ok(None) => {
            // Check if origin remote exists
            match git.remote_exists("origin") {
                Ok(true) => CheckResult::warning("Fetch refspec", "not configured")
                    .with_suggestion("Run 'git config remote.origin.fetch \"+refs/heads/*:refs/remotes/origin/*\"'")
                    .with_fixable(true),
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
        Ok(true) => CheckResult::pass("Remote HEAD", "set"),
        Ok(false) => CheckResult::warning("Remote HEAD", "not set")
            .with_suggestion("Run 'git remote set-head origin --auto'")
            .with_fixable(true),
        Err(e) => CheckResult::warning("Remote HEAD", &format!("could not check: {e}")),
    }
}

/// Fix remote HEAD by running git remote set-head origin --auto.
pub fn fix_remote_head() -> Result<(), String> {
    let git = GitCommand::new(true);
    git.remote_set_head_auto("origin")
        .map_err(|e| format!("Failed to set remote HEAD: {e}"))
}
