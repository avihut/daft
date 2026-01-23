//! Environment variable building for hook execution.
//!
//! This module provides the `HookEnvironment` struct that builds the set of
//! environment variables passed to hooks during execution.

use super::HookType;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Context information for hook execution.
///
/// This struct captures all the relevant context about a worktree operation
/// that hooks might need to perform their tasks.
#[derive(Debug, Clone)]
pub struct HookContext {
    /// The type of hook being executed.
    pub hook_type: HookType,

    /// The command that triggered this hook (e.g., "clone", "checkout-branch").
    pub command: String,

    /// Repository root (parent of .git directory).
    pub project_root: PathBuf,

    /// Path to the .git directory.
    pub git_dir: PathBuf,

    /// Remote name (usually "origin").
    pub remote: String,

    /// Worktree where the command was invoked.
    pub source_worktree: PathBuf,

    /// Target worktree (being created or removed).
    pub worktree_path: PathBuf,

    /// Branch name (for the target worktree).
    pub branch_name: String,

    /// Whether the branch is newly created.
    pub is_new_branch: bool,

    /// Base branch (for checkout-branch commands).
    pub base_branch: Option<String>,

    /// Repository URL (for clone operations).
    pub repository_url: Option<String>,

    /// Default branch (for clone operations).
    pub default_branch: Option<String>,

    /// Reason for removal (for remove hooks).
    pub removal_reason: Option<RemovalReason>,
}

/// Reason why a worktree is being removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    /// Remote tracking branch was deleted.
    RemoteDeleted,
    /// Manual removal by user.
    Manual,
}

impl RemovalReason {
    /// Returns the string representation for environment variables.
    pub fn as_str(&self) -> &'static str {
        match self {
            RemovalReason::RemoteDeleted => "remote-deleted",
            RemovalReason::Manual => "manual",
        }
    }
}

impl HookContext {
    /// Create a new hook context with minimal required fields.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        hook_type: HookType,
        command: impl Into<String>,
        project_root: impl Into<PathBuf>,
        git_dir: impl Into<PathBuf>,
        remote: impl Into<String>,
        source_worktree: impl Into<PathBuf>,
        worktree_path: impl Into<PathBuf>,
        branch_name: impl Into<String>,
    ) -> Self {
        Self {
            hook_type,
            command: command.into(),
            project_root: project_root.into(),
            git_dir: git_dir.into(),
            remote: remote.into(),
            source_worktree: source_worktree.into(),
            worktree_path: worktree_path.into(),
            branch_name: branch_name.into(),
            is_new_branch: false,
            base_branch: None,
            repository_url: None,
            default_branch: None,
            removal_reason: None,
        }
    }

    /// Set whether this is a new branch.
    pub fn with_new_branch(mut self, is_new: bool) -> Self {
        self.is_new_branch = is_new;
        self
    }

    /// Set the base branch.
    pub fn with_base_branch(mut self, base: impl Into<String>) -> Self {
        self.base_branch = Some(base.into());
        self
    }

    /// Set the repository URL (for clone operations).
    pub fn with_repository_url(mut self, url: impl Into<String>) -> Self {
        self.repository_url = Some(url.into());
        self
    }

    /// Set the default branch (for clone operations).
    pub fn with_default_branch(mut self, branch: impl Into<String>) -> Self {
        self.default_branch = Some(branch.into());
        self
    }

    /// Set the removal reason (for remove hooks).
    pub fn with_removal_reason(mut self, reason: RemovalReason) -> Self {
        self.removal_reason = Some(reason);
        self
    }
}

/// Builder for hook environment variables.
///
/// This struct builds the set of environment variables that will be passed
/// to a hook script during execution.
#[derive(Debug, Clone)]
pub struct HookEnvironment {
    vars: HashMap<String, String>,
}

impl HookEnvironment {
    /// Create a new hook environment from a context.
    pub fn from_context(ctx: &HookContext) -> Self {
        let mut env = Self {
            vars: HashMap::new(),
        };

        // Universal variables
        env.set("DAFT_HOOK", ctx.hook_type.filename());
        env.set("DAFT_COMMAND", &ctx.command);
        env.set("DAFT_PROJECT_ROOT", ctx.project_root.display());
        env.set("DAFT_GIT_DIR", ctx.git_dir.display());
        env.set("DAFT_REMOTE", &ctx.remote);
        env.set("DAFT_SOURCE_WORKTREE", ctx.source_worktree.display());

        // Worktree-specific variables
        env.set("DAFT_WORKTREE_PATH", ctx.worktree_path.display());
        env.set("DAFT_BRANCH_NAME", &ctx.branch_name);

        // Creation-specific variables
        env.set(
            "DAFT_IS_NEW_BRANCH",
            if ctx.is_new_branch { "true" } else { "false" },
        );
        if let Some(ref base) = ctx.base_branch {
            env.set("DAFT_BASE_BRANCH", base);
        }

        // Clone-specific variables
        if let Some(ref url) = ctx.repository_url {
            env.set("DAFT_REPOSITORY_URL", url);
        }
        if let Some(ref branch) = ctx.default_branch {
            env.set("DAFT_DEFAULT_BRANCH", branch);
        }

        // Removal-specific variables
        if let Some(reason) = ctx.removal_reason {
            env.set("DAFT_REMOVAL_REASON", reason.as_str());
        }

        env
    }

    /// Set an environment variable.
    fn set(&mut self, key: &str, value: impl ToString) {
        self.vars.insert(key.to_string(), value.to_string());
    }

    /// Get an environment variable.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// Get all environment variables as a reference to the internal HashMap.
    pub fn vars(&self) -> &HashMap<String, String> {
        &self.vars
    }

    /// Convert to a vector of (key, value) pairs for Command::envs().
    pub fn to_vec(&self) -> Vec<(String, String)> {
        self.vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Get the working directory for hook execution.
    ///
    /// For most hooks, this is the target worktree path.
    /// For pre-create hooks, the target worktree doesn't exist yet,
    /// so we use the source worktree.
    pub fn working_directory<'a>(&self, ctx: &'a HookContext) -> &'a Path {
        if ctx.hook_type == HookType::PreCreate {
            &ctx.source_worktree
        } else {
            &ctx.worktree_path
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_context() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout-branch",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        )
    }

    #[test]
    fn test_hook_environment_universal_vars() {
        let ctx = make_test_context();
        let env = HookEnvironment::from_context(&ctx);

        assert_eq!(env.get("DAFT_HOOK"), Some("post-create"));
        assert_eq!(env.get("DAFT_COMMAND"), Some("checkout-branch"));
        assert_eq!(env.get("DAFT_PROJECT_ROOT"), Some("/project"));
        assert_eq!(env.get("DAFT_GIT_DIR"), Some("/project/.git"));
        assert_eq!(env.get("DAFT_REMOTE"), Some("origin"));
        assert_eq!(env.get("DAFT_SOURCE_WORKTREE"), Some("/project/main"));
    }

    #[test]
    fn test_hook_environment_worktree_vars() {
        let ctx = make_test_context();
        let env = HookEnvironment::from_context(&ctx);

        assert_eq!(env.get("DAFT_WORKTREE_PATH"), Some("/project/feature/new"));
        assert_eq!(env.get("DAFT_BRANCH_NAME"), Some("feature/new"));
        assert_eq!(env.get("DAFT_IS_NEW_BRANCH"), Some("false"));
    }

    #[test]
    fn test_hook_environment_with_new_branch() {
        let ctx = make_test_context().with_new_branch(true);
        let env = HookEnvironment::from_context(&ctx);

        assert_eq!(env.get("DAFT_IS_NEW_BRANCH"), Some("true"));
    }

    #[test]
    fn test_hook_environment_with_base_branch() {
        let ctx = make_test_context().with_base_branch("main");
        let env = HookEnvironment::from_context(&ctx);

        assert_eq!(env.get("DAFT_BASE_BRANCH"), Some("main"));
    }

    #[test]
    fn test_hook_environment_clone_vars() {
        let ctx = HookContext::new(
            HookType::PostClone,
            "clone",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/main",
            "main",
        )
        .with_repository_url("git@github.com:user/repo.git")
        .with_default_branch("main");

        let env = HookEnvironment::from_context(&ctx);

        assert_eq!(
            env.get("DAFT_REPOSITORY_URL"),
            Some("git@github.com:user/repo.git")
        );
        assert_eq!(env.get("DAFT_DEFAULT_BRANCH"), Some("main"));
    }

    #[test]
    fn test_hook_environment_removal_vars() {
        let ctx = HookContext::new(
            HookType::PreRemove,
            "prune",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/old",
            "feature/old",
        )
        .with_removal_reason(RemovalReason::RemoteDeleted);

        let env = HookEnvironment::from_context(&ctx);

        assert_eq!(env.get("DAFT_REMOVAL_REASON"), Some("remote-deleted"));
    }

    #[test]
    fn test_working_directory_pre_create() {
        let ctx = HookContext::new(
            HookType::PreCreate,
            "checkout-branch",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        );
        let env = HookEnvironment::from_context(&ctx);

        // Pre-create should use source worktree since target doesn't exist yet
        assert_eq!(env.working_directory(&ctx), Path::new("/project/main"));
    }

    #[test]
    fn test_working_directory_post_create() {
        let ctx = make_test_context();
        let env = HookEnvironment::from_context(&ctx);

        // Post-create should use target worktree
        assert_eq!(
            env.working_directory(&ctx),
            Path::new("/project/feature/new")
        );
    }

    #[test]
    fn test_removal_reason_as_str() {
        assert_eq!(RemovalReason::RemoteDeleted.as_str(), "remote-deleted");
        assert_eq!(RemovalReason::Manual.as_str(), "manual");
    }
}
