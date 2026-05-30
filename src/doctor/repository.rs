//! Repository checks for `daft doctor`.
//!
//! Verifies that the current repository is configured correctly for daft:
//! worktree layout, worktree consistency, fetch refspec, remote HEAD.

use crate::doctor::{CheckResult, FixAction};
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
            } else if in_core && let Some(value) = trimmed.strip_prefix("bare") {
                let value = value.trim().strip_prefix('=').map(|v| v.trim());
                if value == Some("true") {
                    return true;
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

    // Resolve the worktree to inspect for config/hooks repo-awarely, not as the
    // raw cwd. Running `daft doctor` from a worktree subdir, or from the bare
    // container root of a contained layout, must still find the worktree where
    // daft.yml actually lives — otherwise every config/hook check (and the
    // Repository config check) goes blind and reports "no hooks configured"
    // while a tracked/visitor daft.yml sits in a sibling worktree.
    let cwd = std::env::current_dir().ok()?;
    let current_worktree = match crate::core::repo::resolve_worktree_position(&cwd) {
        crate::core::repo::WorktreePosition::InWorktree { root } => root,
        crate::core::repo::WorktreePosition::ContainerRoot { representative } => {
            representative.unwrap_or(cwd)
        }
        crate::core::repo::WorktreePosition::NotInRepo => cwd,
    };

    Some(RepoContext {
        git_common_dir,
        project_root,
        current_worktree,
        is_bare,
    })
}

/// Report the main `daft.yml`'s presence and tracking classification.
///
/// daft.yml's tracked-vs-visitor status is a repository-configuration fact, not
/// a hooks fact, so it lives here and is reported at *every* invocation point —
/// from a worktree, a worktree subdir, or the bare container root — because
/// `ctx.current_worktree` is resolved repo-awarely (see [`get_repo_context`]).
/// Always informational (`pass`), in every state including no-config, so it
/// never flips doctor's exit code on an ordinary repo.
pub fn check_daft_config(ctx: &RepoContext) -> CheckResult {
    use crate::hooks::yaml_config_loader::{ConfigStatus, classify_main_config};

    // doctor renders a passing check as "Name (message)", so the message must
    // not carry its own parentheses or it nests them. Use em-dashes instead.
    match classify_main_config(&ctx.current_worktree) {
        ConfigStatus::Tracked => CheckResult::pass("Config", "daft.yml tracked — team baseline"),
        ConfigStatus::Visitor => {
            CheckResult::pass("Config", "daft.yml visitor — private to this clone")
        }
        ConfigStatus::Missing => CheckResult::pass("Config", "no daft.yml"),
    }
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
        .with_dry_run_fix(Box::new(dry_run_worktree_consistency))
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

/// Dry-run simulation for worktree consistency fix.
pub fn dry_run_worktree_consistency() -> Vec<FixAction> {
    let git_available = which::which("git").is_ok();
    vec![FixAction {
        description: "Run git worktree prune to clean up orphaned entries".to_string(),
        would_succeed: git_available,
        failure_reason: if git_available {
            None
        } else {
            Some("git is not available".to_string())
        },
    }]
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
                .with_dry_run_fix(Box::new(dry_run_fetch_refspec))
        }
        Ok(None) => {
            // Check if origin remote exists
            match git.remote_exists("origin") {
                Ok(true) => CheckResult::warning("Fetch refspec", "not configured")
                    .with_suggestion("Run 'git config remote.origin.fetch \"+refs/heads/*:refs/remotes/origin/*\"'")
                    .with_fix(Box::new(fix_fetch_refspec))
                    .with_dry_run_fix(Box::new(dry_run_fetch_refspec)),
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

/// Dry-run simulation for fetch refspec fix.
pub fn dry_run_fetch_refspec() -> Vec<FixAction> {
    let expected = "+refs/heads/*:refs/remotes/origin/*";
    vec![FixAction {
        description: format!("Set fetch refspec to {expected}"),
        would_succeed: true,
        failure_reason: None,
    }]
}

/// Check if remote-sync settings have been explicitly configured.
///
/// Shows a one-time informational note when none of the three remote-sync
/// keys are set, so users know the defaults have changed.
pub fn check_remote_sync_config(_ctx: &RepoContext) -> CheckResult {
    use crate::settings::keys;

    let git = GitCommand::new(true);

    let has_fetch = git
        .config_get(keys::CHECKOUT_FETCH)
        .ok()
        .flatten()
        .is_some()
        || git
            .config_get_global(keys::CHECKOUT_FETCH)
            .ok()
            .flatten()
            .is_some();
    let has_push = git.config_get(keys::CHECKOUT_PUSH).ok().flatten().is_some()
        || git
            .config_get_global(keys::CHECKOUT_PUSH)
            .ok()
            .flatten()
            .is_some();
    let has_delete = git
        .config_get(keys::BRANCH_DELETE_REMOTE)
        .ok()
        .flatten()
        .is_some()
        || git
            .config_get_global(keys::BRANCH_DELETE_REMOTE)
            .ok()
            .flatten()
            .is_some();

    if has_fetch || has_push || has_delete {
        CheckResult::pass("Remote sync", "Remote sync settings are configured")
    } else {
        CheckResult::warning(
            "Remote sync",
            "Remote sync defaults have changed \u{2014} daft no longer fetches, pushes, or deletes remote branches by default",
        )
        .with_suggestion("Run `daft config remote-sync` to configure your preference.")
    }
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
            .with_fix(Box::new(fix_remote_head))
            .with_dry_run_fix(Box::new(dry_run_remote_head)),
        Err(e) => CheckResult::warning("Remote HEAD", &format!("could not check: {e}")),
    }
}

/// Fix remote HEAD by running git remote set-head origin --auto.
pub fn fix_remote_head() -> Result<(), String> {
    let git = GitCommand::new(true);
    git.remote_set_head_auto("origin")
        .map_err(|e| format!("Failed to set remote HEAD: {e}"))
}

/// Dry-run simulation for remote HEAD fix.
pub fn dry_run_remote_head() -> Vec<FixAction> {
    vec![FixAction {
        description: "Run git remote set-head origin --auto".to_string(),
        would_succeed: true,
        failure_reason: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::CheckStatus;

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

    #[test]
    fn test_dry_run_worktree_consistency() {
        let actions = dry_run_worktree_consistency();
        assert_eq!(actions.len(), 1);
        assert!(actions[0].description.contains("git worktree prune"));
        // In test env, git should be available
        assert!(actions[0].would_succeed);
    }

    #[test]
    fn test_dry_run_fetch_refspec() {
        let actions = dry_run_fetch_refspec();
        assert_eq!(actions.len(), 1);
        assert!(actions[0].description.contains("fetch refspec"));
    }

    #[test]
    fn test_dry_run_remote_head() {
        let actions = dry_run_remote_head();
        assert_eq!(actions.len(), 1);
        assert!(actions[0].description.contains("git remote set-head"));
    }

    // ── check_daft_config (tracked / visitor / missing) ──────────────────────

    fn git(dir: &Path, args: &[&str]) {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(out.status.success(), "git {args:?} failed");
    }

    fn ctx_for(worktree: &Path) -> RepoContext {
        RepoContext {
            git_common_dir: worktree.join(".git"),
            project_root: worktree.to_path_buf(),
            current_worktree: worktree.to_path_buf(),
            is_bare: false,
        }
    }

    #[test]
    fn test_check_daft_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let result = check_daft_config(&ctx_for(dir.path()));
        assert_eq!(result.status, CheckStatus::Pass);
        assert_eq!(result.message, "no daft.yml");
    }

    #[test]
    fn test_check_daft_config_visitor() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("daft.yml"), "hooks: {}").unwrap();
        let result = check_daft_config(&ctx_for(dir.path()));
        assert_eq!(result.status, CheckStatus::Pass);
        assert!(
            result.message.contains("visitor"),
            "got: {}",
            result.message
        );
    }

    #[test]
    fn test_check_daft_config_tracked() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("daft.yml"), "hooks: {}").unwrap();
        git(dir.path(), &["add", "daft.yml"]);
        git(dir.path(), &["commit", "-q", "-m", "add"]);
        let result = check_daft_config(&ctx_for(dir.path()));
        assert_eq!(result.status, CheckStatus::Pass);
        assert!(
            result.message.contains("tracked"),
            "got: {}",
            result.message
        );
    }
}
