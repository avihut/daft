//! Environment variable building for hook execution.
//!
//! This module provides the `HookEnvironment` struct that builds the set of
//! environment variables passed to hooks during execution.

use super::HookType;
use crate::hooks::tracking::TrackedAttribute;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Derived per-worktree env values (#388) for a hook/task context, ready to
/// ride [`HookContext::with_derived_env`]. Returns the injection map (already
/// filtered by the never-clobber rule: names set in daft's own environment
/// are dropped) plus human-readable warnings for anything that could not be
/// derived.
///
/// Best-effort by design: a broken `values:` template must not silently kill
/// a lifecycle hook the way a config parse error would, so on a values error
/// the ports still inject and the warning names the failing template. No
/// `env:` section means no work and no warnings.
pub(crate) fn derived_injection(
    config: &crate::hooks::yaml_config::YamlConfig,
    ctx: &HookContext,
) -> (BTreeMap<String, String>, Vec<String>) {
    derived_injection_at(
        config,
        &ctx.worktree_path,
        &ctx.project_root,
        (!ctx.branch_name.is_empty()).then_some(ctx.branch_name.as_str()),
    )
}

/// [`derived_injection`] on raw coordinates, for callers without a
/// [`HookContext`] (exec targets carry only a path and branch).
pub(crate) fn derived_injection_at(
    config: &crate::hooks::yaml_config::YamlConfig,
    worktree_path: &Path,
    project_root: &Path,
    branch: Option<&str>,
) -> (BTreeMap<String, String>, Vec<String>) {
    use crate::core::env_values::{EnvSpec, ValueContext, spawn_injection};

    let Some(env_cfg) = config.env.as_ref() else {
        return (BTreeMap::new(), Vec::new());
    };
    let repo_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let spec = EnvSpec::from_config(Some(env_cfg), &repo_name);
    let slug = crate::core::slug::worktree_slug_from(worktree_path, project_root);
    let vctx = ValueContext {
        repo: &repo_name,
        worktree_path: worktree_path.to_str(),
        worktree_root: project_root.to_str(),
        branch,
    };

    let mut warnings = Vec::new();
    let resolved = match spec.resolve_all(&slug, &vctx) {
        Ok(resolved) => resolved,
        Err(e) => {
            warnings.push(format!("derived env values partially skipped: {e}"));
            let mut ports_only = spec.clone();
            ports_only.values.clear();
            match ports_only.resolve_all(&slug, &vctx) {
                Ok(resolved) => resolved,
                Err(e) => {
                    warnings.push(format!("derived env values not injected: {e}"));
                    return (BTreeMap::new(), warnings);
                }
            }
        }
    };
    (spawn_injection(&resolved), warnings)
}

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

    /// Branch name (for the target worktree). Empty for a branchless
    /// (anonymous sandbox) worktree — the contract is "empty string means no
    /// branch", and `commit` carries the identity instead.
    pub branch_name: String,

    /// The commit a branchless worktree is pinned at (full OID). Set by the
    /// sandbox creation/removal paths; `None` for ordinary branch worktrees.
    pub commit: Option<String>,

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
            commit: None,
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
    ///
    /// NOTE: this **replaces** the whole map (its callers own it outright).
    /// Derived env values must use [`Self::with_derived_env`], which extends
    /// instead — replacing here would destroy `DAFT_MERGE_*` entries a merge
    /// command threaded in earlier.
    pub fn with_extra_env(mut self, extra: BTreeMap<String, String>) -> Self {
        self.extra_env = extra;
        self
    }

    /// Extend `extra_env` with derived per-worktree values (#388) so they
    /// ride the same channel as `DAFT_MERGE_*`: applied last in
    /// [`HookEnvironment::from_context`], inherited by every job's process
    /// env, still overridable by an explicit per-job `env:`. Contrast with
    /// [`Self::with_extra_env`], which replaces the map.
    pub fn with_derived_env(mut self, derived: BTreeMap<String, String>) -> Self {
        self.extra_env.extend(derived);
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

    /// Set the pinned commit (for branchless sandbox worktrees).
    pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
        self.commit = Some(commit.into());
        self
    }

    /// The label background-job invocations are registered under — and the
    /// label removal cancels by. Branch worktrees use the branch name; a
    /// branchless sandbox (empty `branch_name`) uses the worktree's
    /// directory name, matching how `daft remove` addresses it. Registering
    /// under the empty string would orphan a sandbox's jobs: nothing ever
    /// cancels the `""` label.
    pub fn worktree_label(&self) -> &str {
        if !self.branch_name.is_empty() {
            return &self.branch_name;
        }
        self.worktree_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("")
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
        // Branchless (sandbox) worktrees: DAFT_BRANCH_NAME is "" and the
        // pinned commit carries the identity.
        if let Some(ref commit) = ctx.commit {
            env.set("DAFT_COMMIT", commit);
        }

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

    fn env_yaml_config(yaml: &str) -> crate::hooks::yaml_config::YamlConfig {
        serde_yaml::from_str(yaml).expect("test yaml parses")
    }

    /// `with_derived_env` EXTENDS: the DAFT_MERGE_* entries a merge command
    /// threaded in earlier must survive derived-value injection, and both
    /// must reach the hook environment (extra_env is applied last).
    #[test]
    fn derived_env_extends_and_merge_entries_survive() {
        let merge_extra: BTreeMap<String, String> =
            [("DAFT_MERGE_SOURCE_PATH".to_string(), "/p/feat".to_string())].into();
        let derived: BTreeMap<String, String> =
            [("WEBAPP_PORT".to_string(), "23952".to_string())].into();
        let ctx = make_test_context()
            .with_extra_env(merge_extra)
            .with_derived_env(derived);
        assert_eq!(
            ctx.extra_env
                .get("DAFT_MERGE_SOURCE_PATH")
                .map(String::as_str),
            Some("/p/feat")
        );
        let env = HookEnvironment::from_context(&ctx);
        assert_eq!(env.get("WEBAPP_PORT"), Some("23952"));
        assert_eq!(env.get("DAFT_MERGE_SOURCE_PATH"), Some("/p/feat"));
    }

    /// A derived value carrying template-looking braces reaches the hook
    /// environment verbatim — computed env is never re-substituted (the
    /// job_adapter guards this end-to-end; this pins the extra_env leg).
    #[test]
    fn derived_env_values_with_braces_stay_literal() {
        let derived: BTreeMap<String, String> =
            [("TRICKY".to_string(), "{branch}-literal".to_string())].into();
        let ctx = make_test_context().with_derived_env(derived);
        let env = HookEnvironment::from_context(&ctx);
        assert_eq!(env.get("TRICKY"), Some("{branch}-literal"));
    }

    /// End-to-end helper behavior: declared ports inject; a name already set
    /// in daft's own environment is dropped (never-clobber); a broken
    /// values: template downgrades to ports-only with a warning instead of
    /// killing the hook.
    #[test]
    fn derived_injection_filters_and_degrades() {
        let ctx = make_test_context();

        // Plain declared port injects (slug feature-new, salt pinned).
        let config = env_yaml_config("env:\n  salt: myapp\n  ports:\n    - WEBAPP_PORT\n");
        let (derived, warnings) = derived_injection(&config, &ctx);
        assert_eq!(
            derived.get("WEBAPP_PORT").map(String::as_str),
            Some("23952")
        );
        assert!(warnings.is_empty());

        // Never-clobber: a parent-set name is filtered out. set_var is
        // `unsafe fn` in edition 2024; tests may wrap it (Critical Rule 4).
        let config =
            env_yaml_config("env:\n  salt: myapp\n  ports:\n    - DAFTTEST_CLOBBERED_PORT\n");
        unsafe { std::env::set_var("DAFTTEST_CLOBBERED_PORT", "999") };
        let (derived, _) = derived_injection(&config, &ctx);
        unsafe { std::env::remove_var("DAFTTEST_CLOBBERED_PORT") };
        assert!(
            !derived.contains_key("DAFTTEST_CLOBBERED_PORT"),
            "parent env wins over derived"
        );

        // Broken values: template → warning + ports still inject.
        let config = env_yaml_config(
            "env:\n  salt: myapp\n  ports:\n    - WEBAPP_PORT\n  values:\n    BAD: \"{typo}\"\n",
        );
        let (derived, warnings) = derived_injection(&config, &ctx);
        assert_eq!(
            derived.get("WEBAPP_PORT").map(String::as_str),
            Some("23952")
        );
        assert!(!derived.contains_key("BAD"));
        assert!(
            warnings.iter().any(|w| w.contains("typo")),
            "warning names the failing template: {warnings:?}"
        );

        // No env: section → nothing, silently.
        let (derived, warnings) = derived_injection(&env_yaml_config("hooks: {}"), &ctx);
        assert!(derived.is_empty() && warnings.is_empty());
    }

    /// Branch worktrees label their background-job invocations by branch
    /// name; a branchless sandbox labels them by the worktree's directory
    /// name — the same key `daft remove` cancels by. An empty label would
    /// orphan the sandbox's jobs on removal (#53 review).
    #[test]
    fn worktree_label_falls_back_to_the_dirname_for_branchless_contexts() {
        let branch_ctx = make_test_context();
        assert_eq!(branch_ctx.worktree_label(), "feature/new");

        let sandbox_ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/origin-master",
            "",
        );
        assert_eq!(sandbox_ctx.worktree_label(), "origin-master");
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

    /// The branchless (sandbox) contract: DAFT_BRANCH_NAME is the empty
    /// string and DAFT_COMMIT carries the pinned OID. Ordinary branch
    /// contexts emit no DAFT_COMMIT at all.
    #[test]
    fn test_hook_environment_branchless_sandbox_context() {
        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/v1",
            "",
        )
        .with_commit("abc123def4567890abc123def4567890abc123de");
        let env = HookEnvironment::from_context(&ctx);

        assert_eq!(env.get("DAFT_BRANCH_NAME"), Some(""));
        assert_eq!(
            env.get("DAFT_COMMIT"),
            Some("abc123def4567890abc123def4567890abc123de")
        );

        let branch_env = HookEnvironment::from_context(&make_test_context());
        assert_eq!(branch_env.get("DAFT_COMMIT"), None);
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
            commit: None,
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
            commit: None,
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
