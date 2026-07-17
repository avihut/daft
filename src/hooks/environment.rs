//! Environment variable building for hook execution.
//!
//! This module provides the `HookEnvironment` struct that builds the set of
//! environment variables passed to hooks during execution.

use super::HookType;
use crate::hooks::tracking::TrackedAttribute;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Context information for hook execution.
///
/// This struct captures all the relevant context about a worktree operation
/// that hooks might need to perform their tasks.
#[derive(Debug, Clone)]
pub struct HookContext {
    /// The type of hook being executed.
    pub hook_type: HookType,

    /// The command that triggered this hook (e.g., "clone", "checkout").
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

    /// Base branch (for checkout -b commands).
    pub base_branch: Option<String>,

    /// Repository URL (for clone operations).
    pub repository_url: Option<String>,

    /// Default branch (for clone operations).
    pub default_branch: Option<String>,

    /// Reason for removal (for remove hooks).
    pub removal_reason: Option<RemovalReason>,

    /// Whether this hook is executing as part of a move operation.
    pub is_move: bool,
    /// The worktree path before the move (set in all four move phases).
    pub old_worktree_path: Option<PathBuf>,
    /// The branch name before the move (set in all four move phases).
    pub old_branch_name: Option<String>,
    /// During move hooks, the set of changed attributes for job filtering.
    pub changed_attributes: Option<HashSet<TrackedAttribute>>,

    /// Hook-specific additional env vars merged into the executed hook's
    /// environment on top of the universal `DAFT_*` set. Populated by
    /// hook-firing call sites that carry their own context (e.g. the merge
    /// command injects `DAFT_MERGE_*` here). Ordering is kept stable
    /// (`BTreeMap`) so overriding/appending is deterministic in tests.
    pub extra_env: BTreeMap<String, String>,

    /// Override for the daft state directory used when writing hook
    /// invocation/job records. `None` (the production default) routes through
    /// `daft_state_dir()` (XDG state home, modulo `DAFT_STATE_DIR` in dev
    /// builds). Tests set this to a tempdir so their LogStore writes never
    /// touch the user's real `~/.local/state/daft`.
    pub state_dir: Option<PathBuf>,

    /// Set when this context drives a `daft run <task>` invocation rather than
    /// a lifecycle hook. When present, the environment emits `DAFT_TASK=<name>`
    /// instead of `DAFT_HOOK` (tasks are not hooks), and `hook_type` is an
    /// inert placeholder read only by `working_directory` and header rendering.
    pub task_name: Option<String>,
}

/// Reason why a worktree is being removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalReason {
    /// Remote tracking branch was deleted.
    RemoteDeleted,
    /// Manual removal by user.
    Manual,
    /// Worktree being removed during flow-eject.
    Ejecting,
}

impl RemovalReason {
    /// Returns the string representation for environment variables.
    pub fn as_str(&self) -> &'static str {
        match self {
            RemovalReason::RemoteDeleted => "remote-deleted",
            RemovalReason::Manual => "manual",
            RemovalReason::Ejecting => "ejecting",
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
            is_move: false,
            old_worktree_path: None,
            old_branch_name: None,
            changed_attributes: None,
            extra_env: BTreeMap::new(),
            state_dir: None,
            task_name: None,
        }
    }

    /// Create a context for a `daft run <task>` invocation in the current
    /// worktree.
    ///
    /// The worktree is both source and target (no create/remove is happening),
    /// mirroring `daft hooks run`. `hook_type` is set to an inert `PostCreate`
    /// placeholder: on the task execution path it is read only by
    /// `working_directory` (which returns the worktree for every non-PreCreate
    /// type) and by header rendering (which uses the branch) — both give the
    /// intended answer, and `DAFT_HOOK` is never emitted for a task.
    pub fn for_task(
        task_name: impl Into<String>,
        project_root: impl Into<PathBuf>,
        git_dir: impl Into<PathBuf>,
        remote: impl Into<String>,
        worktree_path: impl Into<PathBuf>,
        branch_name: impl Into<String>,
    ) -> Self {
        let worktree_path = worktree_path.into();
        Self {
            task_name: Some(task_name.into()),
            ..Self::new(
                HookType::PostCreate,
                "run",
                project_root,
                git_dir,
                remote,
                worktree_path.clone(),
                worktree_path,
                branch_name,
            )
        }
    }

    /// Attach hook-specific additional env vars (e.g. `DAFT_MERGE_*` for
    /// merge hooks). Merged into the hook environment after the universal
    /// vars, so later calls win over earlier ones — a no-op here since
    /// `new()` starts with an empty map.
    pub fn with_extra_env(mut self, extra: BTreeMap<String, String>) -> Self {
        self.extra_env = extra;
        self
    }

    /// Override the daft state directory used for LogStore writes. Test-only
    /// in practice: production hooks always go through `daft_state_dir()`.
    pub fn with_state_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(dir.into());
        self
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

        // Universal variables. A `daft run` task emits DAFT_TASK (and no
        // DAFT_HOOK — tasks are not lifecycle hooks); everything else emits
        // DAFT_HOOK as before.
        match &ctx.task_name {
            Some(task) => env.set("DAFT_TASK", task),
            None => env.set("DAFT_HOOK", ctx.hook_type.filename()),
        }
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

        // Move-specific variables
        if ctx.is_move {
            env.set("DAFT_IS_MOVE", "true");
            if let Some(ref old_path) = ctx.old_worktree_path {
                env.set("DAFT_OLD_WORKTREE_PATH", old_path.display());
            }
            if let Some(ref old_branch) = ctx.old_branch_name {
                env.set("DAFT_OLD_BRANCH_NAME", old_branch);
            }
        }

        // Hook-specific extra vars — applied last so callers can override
        // the universal defaults if needed (e.g. MergeHookContext stamps
        // `DAFT_MERGE_*` here). No sanitization: callers are trusted and
        // values are shell-escaped downstream in the executor.
        for (k, v) in &ctx.extra_env {
            env.set(k, v);
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
    /// so we use the source worktree — unless this is a move operation,
    /// in which case the target already exists.
    pub fn working_directory<'a>(&self, ctx: &'a HookContext) -> &'a Path {
        match ctx.hook_type {
            HookType::PreCreate if !ctx.is_move => &ctx.source_worktree,
            _ => &ctx.worktree_path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_context() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout",
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

        assert_eq!(env.get("DAFT_HOOK"), Some("worktree-post-create"));
        assert_eq!(env.get("DAFT_COMMAND"), Some("checkout"));
        assert_eq!(env.get("DAFT_PROJECT_ROOT"), Some("/project"));
        assert_eq!(env.get("DAFT_GIT_DIR"), Some("/project/.git"));
        assert_eq!(env.get("DAFT_REMOTE"), Some("origin"));
        assert_eq!(env.get("DAFT_SOURCE_WORKTREE"), Some("/project/main"));
    }

    #[test]
    fn test_for_task_emits_daft_task_not_daft_hook() {
        let ctx = HookContext::for_task(
            "dev",
            "/project",
            "/project/.git",
            "origin",
            "/project/feature/new",
            "feature/new",
        );
        let env = HookEnvironment::from_context(&ctx);

        // A task emits DAFT_TASK and DAFT_COMMAND=run, and never DAFT_HOOK.
        assert_eq!(env.get("DAFT_TASK"), Some("dev"));
        assert_eq!(env.get("DAFT_HOOK"), None);
        assert_eq!(env.get("DAFT_COMMAND"), Some("run"));
        // Source == target worktree (no create/remove is happening).
        assert_eq!(env.get("DAFT_WORKTREE_PATH"), Some("/project/feature/new"));
        assert_eq!(
            env.get("DAFT_SOURCE_WORKTREE"),
            Some("/project/feature/new")
        );
        assert_eq!(env.get("DAFT_BRANCH_NAME"), Some("feature/new"));
        // working_directory resolves to the worktree for the task path.
        assert_eq!(
            env.working_directory(&ctx),
            Path::new("/project/feature/new")
        );
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
            "checkout",
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
        assert_eq!(RemovalReason::Ejecting.as_str(), "ejecting");
    }

    #[test]
    fn test_working_directory_pre_create_move_uses_worktree_path() {
        // During a move pre-create, the target already exists — use worktree_path.
        let ctx = HookContext {
            is_move: true,
            ..HookContext::new(
                HookType::PreCreate,
                "rename",
                "/project",
                "/project/.git",
                "origin",
                "/project/source",
                "/project/new-wt",
                "feat/new",
            )
        };
        let env = HookEnvironment::from_context(&ctx);
        assert_eq!(env.working_directory(&ctx), Path::new("/project/new-wt"));
    }

    #[test]
    fn test_working_directory_pre_create_non_move_uses_source() {
        // Regular pre-create: target doesn't exist yet, use source_worktree.
        let ctx = HookContext::new(
            HookType::PreCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/source",
            "/project/new-wt",
            "feat/new",
        );
        let env = HookEnvironment::from_context(&ctx);
        assert_eq!(env.working_directory(&ctx), Path::new("/project/source"));
    }

    #[test]
    fn test_move_env_vars_set() {
        let ctx = HookContext {
            hook_type: HookType::PostCreate,
            command: "rename".to_string(),
            project_root: PathBuf::from("/project"),
            git_dir: PathBuf::from("/project/.git"),
            remote: "origin".to_string(),
            source_worktree: PathBuf::from("/project/old-wt"),
            worktree_path: PathBuf::from("/project/new-wt"),
            branch_name: "feat/new-name".to_string(),
            is_new_branch: false,
            base_branch: None,
            repository_url: None,
            default_branch: None,
            removal_reason: None,
            is_move: true,
            old_worktree_path: Some(PathBuf::from("/project/old-wt")),
            old_branch_name: Some("feat/old-name".to_string()),
            changed_attributes: None,
            extra_env: BTreeMap::new(),
            state_dir: None,
            task_name: None,
        };
        let env = HookEnvironment::from_context(&ctx);
        assert_eq!(env.vars.get("DAFT_IS_MOVE").unwrap(), "true");
        assert_eq!(
            env.vars.get("DAFT_OLD_WORKTREE_PATH").unwrap(),
            "/project/old-wt"
        );
        assert_eq!(
            env.vars.get("DAFT_OLD_BRANCH_NAME").unwrap(),
            "feat/old-name"
        );
    }

    #[test]
    fn test_non_move_has_no_move_vars() {
        let ctx = HookContext {
            hook_type: HookType::PostCreate,
            command: "checkout".to_string(),
            project_root: PathBuf::from("/project"),
            git_dir: PathBuf::from("/project/.git"),
            remote: "origin".to_string(),
            source_worktree: PathBuf::from("/project/src-wt"),
            worktree_path: PathBuf::from("/project/new-wt"),
            branch_name: "feat/new".to_string(),
            is_new_branch: true,
            base_branch: None,
            repository_url: None,
            default_branch: None,
            removal_reason: None,
            is_move: false,
            old_worktree_path: None,
            old_branch_name: None,
            changed_attributes: None,
            extra_env: BTreeMap::new(),
            state_dir: None,
            task_name: None,
        };
        let env = HookEnvironment::from_context(&ctx);
        assert!(!env.vars.contains_key("DAFT_IS_MOVE"));
        assert!(!env.vars.contains_key("DAFT_OLD_WORKTREE_PATH"));
        assert!(!env.vars.contains_key("DAFT_OLD_BRANCH_NAME"));
    }
}
