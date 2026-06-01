//! Repository query functions and URL parsing utilities.
//!
//! These functions provide common repository introspection operations used
//! across core and command layers. They are thin wrappers around `GitCommand`
//! that provide convenient, context-aware access to repository state.

use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
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
    if let Ok(Some(configured_branch)) = git.config_get_global("init.defaultBranch")
        && !configured_branch.is_empty()
    {
        return configured_branch;
    }

    // Fallback to "master"
    "master".to_string()
}

/// Where the current directory sits relative to a repository's worktree
/// structure.
///
/// `daft install` and `daft doctor` both need to act on *a worktree* — the
/// directory where a `daft.yml` actually lives and is read — not on the raw
/// cwd. The cwd can be a worktree root, a nested subdirectory of one, the bare
/// container root of a contained layout (which holds the shared `.git` but no
/// work tree of its own), or outside any repository entirely. The right target
/// differs in each case, so callers match on this instead of assuming cwd is a
/// worktree root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreePosition {
    /// `cwd` is not inside any git repository.
    NotInRepo,
    /// `cwd` is inside a work tree. `root` is that worktree's toplevel; the cwd
    /// itself may be a nested subdirectory of it.
    InWorktree { root: PathBuf },
    /// `cwd` is inside a git directory that has no work tree — the bare
    /// container root of a contained layout. `representative` is a worktree to
    /// inspect for repo-level config (the default branch's worktree when it can
    /// be resolved locally, otherwise any non-bare worktree), or `None` when no
    /// worktree exists yet.
    ContainerRoot { representative: Option<PathBuf> },
}

/// Resolve the [`WorktreePosition`] of `cwd` using only local git state — no
/// network calls.
///
/// Every probe runs through [`crate::utils::git_command_at`], which clears the
/// inherited `GIT_*` environment so an ambient `GIT_DIR` (set when daft runs
/// inside a git hook, e.g. pre-push) cannot retarget the query at a parent
/// repository — the exact failure mode the Test Hygiene rules warn about. Both
/// pipes are silenced so the negative probes never leak `fatal:` lines.
pub fn resolve_worktree_position(cwd: &Path) -> WorktreePosition {
    // Are we inside a git repository at all (work tree or bare)?
    let in_repo = crate::utils::git_command_at(cwd)
        .args(["rev-parse", "--git-dir"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !matches!(in_repo, Ok(s) if s.success()) {
        return WorktreePosition::NotInRepo;
    }

    // Inside a work tree → resolve its toplevel (the cwd may be a subdir).
    let inside_work_tree = crate::utils::git_command_at(cwd)
        .args(["rev-parse", "--is-inside-work-tree"])
        .stderr(Stdio::null())
        .output();
    let is_work_tree = matches!(
        inside_work_tree,
        Ok(ref out) if out.status.success()
            && String::from_utf8_lossy(&out.stdout).trim() == "true"
    );

    if is_work_tree {
        if let Some(root) = rev_parse_path(cwd, "--show-toplevel") {
            return WorktreePosition::InWorktree { root };
        }
        // Defensive: --is-inside-work-tree said true but --show-toplevel did
        // not resolve. Treat the cwd as the worktree root.
        return WorktreePosition::InWorktree {
            root: cwd.to_path_buf(),
        };
    }

    // No work tree → the bare container root of a contained layout.
    WorktreePosition::ContainerRoot {
        representative: find_representative_worktree(cwd),
    }
}

/// Run `git -C cwd rev-parse <arg>` and return the trimmed path, or `None` on
/// failure. Stderr is silenced.
fn rev_parse_path(cwd: &Path, arg: &str) -> Option<PathBuf> {
    let out = crate::utils::git_command_at(cwd)
        .args(["rev-parse", arg])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(PathBuf::from(s))
    }
}

/// Pick a worktree to represent a bare/container-root repo for config
/// inspection. Prefers the default branch's worktree (resolved locally from
/// `origin/HEAD`); falls back to the first non-bare worktree. Returns `None`
/// when the repo has no worktrees yet.
///
/// Local-only by design: `daft doctor`/`install` must stay fast and work
/// offline, so this never reaches for the network the way
/// [`crate::core::remote::get_default_branch_local`]'s ls-remote fallback can.
fn find_representative_worktree(cwd: &Path) -> Option<PathBuf> {
    let porcelain = crate::utils::git_command_at(cwd)
        .args(["worktree", "list", "--porcelain"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !porcelain.status.success() {
        return None;
    }
    let porcelain = String::from_utf8_lossy(&porcelain.stdout);
    let worktrees = crate::core::worktree::porcelain::parse_worktree_list_porcelain(&porcelain);

    // Prefer the default branch's worktree.
    if let Some(default) = crate::core::remote::local_default_branch(cwd, "origin")
        && let Some(entry) = worktrees
            .iter()
            .find(|w| !w.is_bare && w.branch.as_deref() == Some(default.as_str()))
    {
        return Some(entry.path.clone());
    }

    // Otherwise the first non-bare worktree.
    crate::core::worktree::porcelain::first_main_index(&worktrees)
        .map(|i| worktrees[i].path.clone())
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

    // ── WorktreePosition resolution ──────────────────────────────────────────

    use std::path::Path;

    /// Run a git command in `dir` with a fixed test identity (never touches
    /// global config — CLAUDE.md Critical Rule #1). Output captured.
    fn git(dir: &Path, args: &[&str]) {
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
            "git {args:?} failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn test_resolve_position_not_in_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_worktree_position(dir.path()),
            WorktreePosition::NotInRepo
        );
    }

    #[test]
    fn test_resolve_position_in_worktree_root_and_subdir() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        let root = dir.path().canonicalize().unwrap();

        match resolve_worktree_position(dir.path()) {
            WorktreePosition::InWorktree { root: r } => assert_eq!(r, root),
            other => panic!("expected InWorktree, got {other:?}"),
        }

        // From a nested subdir, the resolved root is still the worktree root.
        let sub = dir.path().join("nested/deep");
        std::fs::create_dir_all(&sub).unwrap();
        match resolve_worktree_position(&sub) {
            WorktreePosition::InWorktree { root: r } => assert_eq!(r, root),
            other => panic!("expected InWorktree from subdir, got {other:?}"),
        }
    }

    /// Build a contained-layout repo: `<proj>/.git` is bare, worktrees are
    /// subdirs. Returns the project (container) root.
    fn build_contained_layout(base: &Path) -> PathBuf {
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "-q", "-b", "main"]);
        std::fs::write(src.join("README.md"), "hi").unwrap();
        git(&src, &["add", "-A"]);
        git(&src, &["commit", "-q", "-m", "init"]);

        let proj = base.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        git(
            base,
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                proj.join(".git").to_str().unwrap(),
            ],
        );
        git(
            &proj,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        );
        git(&proj, &["fetch", "-q", "origin"]);
        git(&proj, &["remote", "set-head", "origin", "main"]);
        git(&proj, &["worktree", "add", "-q", "main", "main"]);
        proj
    }

    #[test]
    fn test_resolve_position_container_root_finds_representative() {
        let dir = tempfile::tempdir().unwrap();
        let proj = build_contained_layout(dir.path());

        match resolve_worktree_position(&proj) {
            WorktreePosition::ContainerRoot { representative } => {
                let rep = representative.expect("a representative worktree");
                assert_eq!(
                    rep.canonicalize().unwrap(),
                    proj.join("main").canonicalize().unwrap()
                );
            }
            other => panic!("expected ContainerRoot, got {other:?}"),
        }

        // Inside the worktree subdir it resolves to that worktree's root.
        match resolve_worktree_position(&proj.join("main")) {
            WorktreePosition::InWorktree { root } => assert_eq!(
                root.canonicalize().unwrap(),
                proj.join("main").canonicalize().unwrap()
            ),
            other => panic!("expected InWorktree, got {other:?}"),
        }
    }
}
