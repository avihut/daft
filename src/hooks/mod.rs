//! Hooks system for worktree lifecycle events.
//!
//! This module provides a flexible, project-managed hooks system that allows
//! arbitrary scripts to run at worktree lifecycle events.
//!
//! # Hook Types
//!
//! | Hook | Trigger | Source |
//! |------|---------|--------|
//! | `post-clone` | After `git worktree-clone` | New default branch worktree |
//! | `post-init` | After `git worktree-init` | New initial worktree |
//! | `pre-create` | Before `git worktree add` | Source worktree |
//! | `post-create` | After worktree created | New worktree |
//! | `pre-remove` | Before `git worktree remove` | Worktree being removed |
//! | `post-remove` | After worktree removed | Current worktree |
//!
//! # Security
//!
//! Hooks are executable scripts that run with user privileges. For security,
//! hooks are not executed unless the repository is explicitly trusted by the user.
//!
//! Trust levels:
//! - `deny` (default): Hooks are never executed
//! - `prompt`: User is asked before each hook execution
//! - `allow`: Hooks run without prompting
//!
//! # Directory Structure
//!
//! Hooks are stored in the tracked codebase at `<worktree>/.daft/hooks/`:
//!
//! ```text
//! my-project/
//! ├── .daft/
//! │   └── hooks/
//! │       ├── post-clone
//! │       ├── post-create
//! │       └── pre-remove
//! └── src/
//! ```
//!
//! User-global hooks can be placed at `~/.config/daft/hooks/`.

mod environment;
mod executor;
mod trust;

pub use environment::{HookContext, HookEnvironment, RemovalReason};
pub use executor::{HookExecutor, HookResult};
pub use trust::{TrustDatabase, TrustEntry, TrustLevel};

use std::fmt;
use std::path::Path;

/// Hook types that can be executed during worktree lifecycle events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookType {
    /// Runs after `git worktree-clone` completes.
    /// Hook file is read from the new default branch worktree.
    PostClone,

    /// Runs after `git worktree-init` completes.
    /// Hook file is read from the new initial worktree.
    PostInit,

    /// Runs before `git worktree add`.
    /// Hook file is read from the source worktree (where command runs).
    PreCreate,

    /// Runs after worktree is created.
    /// Hook file is read from the new worktree.
    PostCreate,

    /// Runs before `git worktree remove`.
    /// Hook file is read from the worktree being removed.
    PreRemove,

    /// Runs after worktree is removed.
    /// Hook file is read from the current worktree (where prune runs).
    PostRemove,
}

impl HookType {
    /// Returns the filename for this hook type.
    pub fn filename(&self) -> &'static str {
        match self {
            HookType::PostClone => "post-clone",
            HookType::PostInit => "post-init",
            HookType::PreCreate => "pre-create",
            HookType::PostCreate => "post-create",
            HookType::PreRemove => "pre-remove",
            HookType::PostRemove => "post-remove",
        }
    }

    /// Returns the config key suffix for this hook type (camelCase).
    pub fn config_key(&self) -> &'static str {
        match self {
            HookType::PostClone => "postClone",
            HookType::PostInit => "postInit",
            HookType::PreCreate => "preCreate",
            HookType::PostCreate => "postCreate",
            HookType::PreRemove => "preRemove",
            HookType::PostRemove => "postRemove",
        }
    }

    /// Returns the default fail mode for this hook type.
    pub fn default_fail_mode(&self) -> FailMode {
        match self {
            // Pre-create hooks should abort by default (setup must succeed)
            HookType::PreCreate => FailMode::Abort,
            // All other hooks warn by default (don't block operations)
            _ => FailMode::Warn,
        }
    }

    /// Returns whether this is a "pre" hook (runs before the operation).
    pub fn is_pre_hook(&self) -> bool {
        matches!(self, HookType::PreCreate | HookType::PreRemove)
    }

    /// Returns all hook types.
    pub fn all() -> &'static [HookType] {
        &[
            HookType::PostClone,
            HookType::PostInit,
            HookType::PreCreate,
            HookType::PostCreate,
            HookType::PreRemove,
            HookType::PostRemove,
        ]
    }
}

impl fmt::Display for HookType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.filename())
    }
}

/// Behavior when a hook fails (non-zero exit code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailMode {
    /// Abort the operation if the hook fails.
    Abort,
    /// Warn but continue the operation if the hook fails.
    #[default]
    Warn,
}

impl FailMode {
    /// Parse a fail mode from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "abort" => Some(FailMode::Abort),
            "warn" => Some(FailMode::Warn),
            _ => None,
        }
    }
}

impl fmt::Display for FailMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailMode::Abort => write!(f, "abort"),
            FailMode::Warn => write!(f, "warn"),
        }
    }
}

/// Configuration for a specific hook type.
#[derive(Debug, Clone)]
pub struct HookConfig {
    /// Whether this hook is enabled.
    pub enabled: bool,
    /// Behavior when the hook fails.
    pub fail_mode: FailMode,
}

impl HookConfig {
    /// Create a new hook configuration with defaults for the given hook type.
    pub fn new(hook_type: HookType) -> Self {
        Self {
            enabled: true,
            fail_mode: hook_type.default_fail_mode(),
        }
    }
}

/// Global hooks configuration.
#[derive(Debug, Clone)]
pub struct HooksConfig {
    /// Master switch for all hooks.
    pub enabled: bool,
    /// Default trust level for unknown repositories.
    pub default_trust: TrustLevel,
    /// Path to user-global hooks directory.
    pub user_directory: std::path::PathBuf,
    /// Timeout for hook execution in seconds.
    pub timeout_seconds: u32,
    /// Per-hook configurations.
    pub post_clone: HookConfig,
    pub post_init: HookConfig,
    pub pre_create: HookConfig,
    pub post_create: HookConfig,
    pub pre_remove: HookConfig,
    pub post_remove: HookConfig,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_trust: TrustLevel::Deny,
            user_directory: default_user_hooks_dir(),
            timeout_seconds: 300,
            post_clone: HookConfig::new(HookType::PostClone),
            post_init: HookConfig::new(HookType::PostInit),
            pre_create: HookConfig::new(HookType::PreCreate),
            post_create: HookConfig::new(HookType::PostCreate),
            pre_remove: HookConfig::new(HookType::PreRemove),
            post_remove: HookConfig::new(HookType::PostRemove),
        }
    }
}

impl HooksConfig {
    /// Get the configuration for a specific hook type.
    pub fn get_hook_config(&self, hook_type: HookType) -> &HookConfig {
        match hook_type {
            HookType::PostClone => &self.post_clone,
            HookType::PostInit => &self.post_init,
            HookType::PreCreate => &self.pre_create,
            HookType::PostCreate => &self.post_create,
            HookType::PreRemove => &self.pre_remove,
            HookType::PostRemove => &self.post_remove,
        }
    }

    /// Get mutable configuration for a specific hook type.
    pub fn get_hook_config_mut(&mut self, hook_type: HookType) -> &mut HookConfig {
        match hook_type {
            HookType::PostClone => &mut self.post_clone,
            HookType::PostInit => &mut self.post_init,
            HookType::PreCreate => &mut self.pre_create,
            HookType::PostCreate => &mut self.post_create,
            HookType::PreRemove => &mut self.pre_remove,
            HookType::PostRemove => &mut self.post_remove,
        }
    }
}

/// Returns the default path for user-global hooks directory.
fn default_user_hooks_dir() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
        .join("daft")
        .join("hooks")
}

/// Path to project hooks directory within a worktree.
pub const PROJECT_HOOKS_DIR: &str = ".daft/hooks";

/// Find hook files for a given hook type.
///
/// Returns a list of paths to hook files, in execution order:
/// 1. Project hook (from worktree)
/// 2. User hook (from user config directory)
pub fn find_hooks(
    hook_type: HookType,
    worktree_path: &Path,
    config: &HooksConfig,
) -> Vec<std::path::PathBuf> {
    let mut hooks = Vec::new();

    // Project hook
    let project_hook = worktree_path
        .join(PROJECT_HOOKS_DIR)
        .join(hook_type.filename());
    if project_hook.exists() && is_executable(&project_hook) {
        hooks.push(project_hook);
    }

    // User hook
    let user_hook = config.user_directory.join(hook_type.filename());
    if user_hook.exists() && is_executable(&user_hook) {
        hooks.push(user_hook);
    }

    hooks
}

/// Check if a hook exists in the given worktree (project hooks only).
pub fn hook_exists(hook_type: HookType, worktree_path: &Path) -> bool {
    let project_hook = worktree_path
        .join(PROJECT_HOOKS_DIR)
        .join(hook_type.filename());
    project_hook.exists()
}

/// List all hooks that exist in the given worktree.
pub fn list_hooks(worktree_path: &Path) -> Vec<HookType> {
    HookType::all()
        .iter()
        .filter(|&&hook_type| hook_exists(hook_type, worktree_path))
        .copied()
        .collect()
}

/// Check if a file is executable.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    // On non-Unix systems, assume files are executable
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_hook_type_filename() {
        assert_eq!(HookType::PostClone.filename(), "post-clone");
        assert_eq!(HookType::PostInit.filename(), "post-init");
        assert_eq!(HookType::PreCreate.filename(), "pre-create");
        assert_eq!(HookType::PostCreate.filename(), "post-create");
        assert_eq!(HookType::PreRemove.filename(), "pre-remove");
        assert_eq!(HookType::PostRemove.filename(), "post-remove");
    }

    #[test]
    fn test_hook_type_config_key() {
        assert_eq!(HookType::PostClone.config_key(), "postClone");
        assert_eq!(HookType::PreCreate.config_key(), "preCreate");
    }

    #[test]
    fn test_hook_type_default_fail_mode() {
        assert_eq!(HookType::PreCreate.default_fail_mode(), FailMode::Abort);
        assert_eq!(HookType::PostCreate.default_fail_mode(), FailMode::Warn);
        assert_eq!(HookType::PreRemove.default_fail_mode(), FailMode::Warn);
    }

    #[test]
    fn test_hook_type_is_pre_hook() {
        assert!(HookType::PreCreate.is_pre_hook());
        assert!(HookType::PreRemove.is_pre_hook());
        assert!(!HookType::PostCreate.is_pre_hook());
        assert!(!HookType::PostClone.is_pre_hook());
    }

    #[test]
    fn test_fail_mode_parse() {
        assert_eq!(FailMode::parse("abort"), Some(FailMode::Abort));
        assert_eq!(FailMode::parse("ABORT"), Some(FailMode::Abort));
        assert_eq!(FailMode::parse("warn"), Some(FailMode::Warn));
        assert_eq!(FailMode::parse("WARN"), Some(FailMode::Warn));
        assert_eq!(FailMode::parse("invalid"), None);
    }

    #[test]
    fn test_hooks_config_default() {
        let config = HooksConfig::default();
        assert!(config.enabled);
        assert_eq!(config.default_trust, TrustLevel::Deny);
        assert_eq!(config.timeout_seconds, 300);
        assert!(config.pre_create.enabled);
        assert_eq!(config.pre_create.fail_mode, FailMode::Abort);
        assert!(config.post_create.enabled);
        assert_eq!(config.post_create.fail_mode, FailMode::Warn);
    }

    #[test]
    fn test_hook_type_all() {
        let all = HookType::all();
        assert_eq!(all.len(), 6);
        assert!(all.contains(&HookType::PostClone));
        assert!(all.contains(&HookType::PostInit));
        assert!(all.contains(&HookType::PreCreate));
        assert!(all.contains(&HookType::PostCreate));
        assert!(all.contains(&HookType::PreRemove));
        assert!(all.contains(&HookType::PostRemove));
    }

    #[test]
    fn test_hook_type_display() {
        assert_eq!(format!("{}", HookType::PostClone), "post-clone");
        assert_eq!(format!("{}", HookType::PreCreate), "pre-create");
    }

    #[test]
    fn test_fail_mode_display() {
        assert_eq!(format!("{}", FailMode::Abort), "abort");
        assert_eq!(format!("{}", FailMode::Warn), "warn");
    }

    #[test]
    fn test_hook_config_new() {
        let config = HookConfig::new(HookType::PreCreate);
        assert!(config.enabled);
        assert_eq!(config.fail_mode, FailMode::Abort);

        let config = HookConfig::new(HookType::PostCreate);
        assert!(config.enabled);
        assert_eq!(config.fail_mode, FailMode::Warn);
    }

    #[test]
    fn test_hooks_config_get_hook_config() {
        let config = HooksConfig::default();
        assert_eq!(
            config.get_hook_config(HookType::PreCreate).fail_mode,
            FailMode::Abort
        );
        assert_eq!(
            config.get_hook_config(HookType::PostCreate).fail_mode,
            FailMode::Warn
        );
    }

    #[test]
    fn test_hooks_config_get_hook_config_mut() {
        let mut config = HooksConfig::default();
        config.get_hook_config_mut(HookType::PostCreate).enabled = false;
        assert!(!config.post_create.enabled);
    }

    // Helper to create an executable hook file
    #[cfg(unix)]
    fn create_executable_hook(dir: &std::path::Path, hook_name: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let hooks_dir = dir.join(PROJECT_HOOKS_DIR);
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join(hook_name);
        fs::write(&hook_path, "#!/bin/bash\necho test").unwrap();

        let mut perms = fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).unwrap();

        hook_path
    }

    #[cfg(not(unix))]
    fn create_executable_hook(dir: &std::path::Path, hook_name: &str) -> std::path::PathBuf {
        let hooks_dir = dir.join(PROJECT_HOOKS_DIR);
        fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join(hook_name);
        fs::write(&hook_path, "@echo off\necho test").unwrap();
        hook_path
    }

    #[test]
    fn test_hook_exists_no_hooks_dir() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        // No hooks directory - should return false
        assert!(!hook_exists(HookType::PostCreate, &worktree));
    }

    #[test]
    fn test_hook_exists_with_hook() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_executable_hook(&worktree, "post-create");

        assert!(hook_exists(HookType::PostCreate, &worktree));
        assert!(!hook_exists(HookType::PreCreate, &worktree));
    }

    #[test]
    fn test_list_hooks_empty() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        let hooks = list_hooks(&worktree);
        assert!(hooks.is_empty());
    }

    #[test]
    fn test_list_hooks_multiple() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        create_executable_hook(&worktree, "post-create");
        create_executable_hook(&worktree, "pre-remove");
        create_executable_hook(&worktree, "post-clone");

        let hooks = list_hooks(&worktree);
        assert_eq!(hooks.len(), 3);
        assert!(hooks.contains(&HookType::PostCreate));
        assert!(hooks.contains(&HookType::PreRemove));
        assert!(hooks.contains(&HookType::PostClone));
    }

    #[test]
    fn test_find_hooks_no_hooks() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        let config = HooksConfig::default();
        let hooks = find_hooks(HookType::PostCreate, &worktree, &config);
        assert!(hooks.is_empty());
    }

    #[test]
    fn test_find_hooks_project_hook_only() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        let hook_path = create_executable_hook(&worktree, "post-create");

        let config = HooksConfig::default();
        let hooks = find_hooks(HookType::PostCreate, &worktree, &config);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0], hook_path);
    }

    #[test]
    fn test_find_hooks_user_hook_only() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        // Create user hook directory
        let user_hooks_dir = temp_dir.path().join("user_hooks");
        fs::create_dir_all(&user_hooks_dir).unwrap();

        // Create user hook
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let user_hook = user_hooks_dir.join("post-create");
            fs::write(&user_hook, "#!/bin/bash\necho user").unwrap();
            let mut perms = fs::metadata(&user_hook).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&user_hook, perms).unwrap();
        }
        #[cfg(not(unix))]
        {
            let user_hook = user_hooks_dir.join("post-create");
            fs::write(&user_hook, "@echo off\necho user").unwrap();
        }

        let mut config = HooksConfig::default();
        config.user_directory = user_hooks_dir.clone();

        let hooks = find_hooks(HookType::PostCreate, &worktree, &config);
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0], user_hooks_dir.join("post-create"));
    }

    #[test]
    fn test_find_hooks_both_project_and_user() {
        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        fs::create_dir_all(&worktree).unwrap();

        // Create project hook
        let project_hook = create_executable_hook(&worktree, "post-create");

        // Create user hooks directory and hook
        let user_hooks_dir = temp_dir.path().join("user_hooks");
        fs::create_dir_all(&user_hooks_dir).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let user_hook = user_hooks_dir.join("post-create");
            fs::write(&user_hook, "#!/bin/bash\necho user").unwrap();
            let mut perms = fs::metadata(&user_hook).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&user_hook, perms).unwrap();
        }
        #[cfg(not(unix))]
        {
            let user_hook = user_hooks_dir.join("post-create");
            fs::write(&user_hook, "@echo off\necho user").unwrap();
        }

        let mut config = HooksConfig::default();
        config.user_directory = user_hooks_dir.clone();

        let hooks = find_hooks(HookType::PostCreate, &worktree, &config);
        // Should find both hooks, project first then user
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0], project_hook);
        assert_eq!(hooks[1], user_hooks_dir.join("post-create"));
    }

    #[test]
    #[cfg(unix)]
    fn test_find_hooks_non_executable_ignored() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempdir().unwrap();
        let worktree = temp_dir.path().join("main");
        let hooks_dir = worktree.join(PROJECT_HOOKS_DIR);
        fs::create_dir_all(&hooks_dir).unwrap();

        // Create a non-executable hook file
        let hook_path = hooks_dir.join("post-create");
        fs::write(&hook_path, "#!/bin/bash\necho test").unwrap();

        // Explicitly set non-executable
        let mut perms = fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&hook_path, perms).unwrap();

        let config = HooksConfig::default();
        let hooks = find_hooks(HookType::PostCreate, &worktree, &config);
        // Non-executable hook should be ignored
        assert!(hooks.is_empty());
    }
}
