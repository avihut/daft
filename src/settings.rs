//! Git config-based settings for daft.
//!
//! This module provides user-configurable options via `git config`.
//! Settings are loaded from git's layered config system (local → global)
//! with built-in defaults as fallback.
//!
//! # Config Keys
//!
//! | Key | Default | Description |
//! |-----|---------|-------------|
//! | `daft.autocd` | `true` | CD into new worktrees (shell wrapper behavior) |
//! | `daft.checkout.push` | `true` | Push new branches to remote |
//! | `daft.checkout.upstream` | `true` | Set upstream tracking |
//! | `daft.remote` | `"origin"` | Default remote name |
//! | `daft.checkoutBranch.carry` | `true` | Default carry for checkout-branch |
//! | `daft.checkout.carry` | `false` | Default carry for checkout |
//! | `daft.prune.cdTarget` | `root` | Where to cd after pruning current worktree (`root` or `default-branch`) |
//! | `daft.updateCheck` | `true` | Enable/disable new version notifications |
//!
//! # Hooks Config Keys
//!
//! | Key | Default | Description |
//! |-----|---------|-------------|
//! | `daft.hooks.enabled` | `true` | Master switch for all hooks |
//! | `daft.hooks.defaultTrust` | `deny` | Default trust level for unknown repos |
//! | `daft.hooks.timeout` | `300` | Timeout for hook execution in seconds |
//! | `daft.hooks.<hookName>.enabled` | `true` | Enable/disable specific hook |
//! | `daft.hooks.<hookName>.failMode` | varies | Behavior on hook failure (abort/warn) |
//!
//! # Example
//!
//! ```bash
//! # Disable auto-cd globally
//! git config --global daft.autocd false
//!
//! # Use a different remote for this repository
//! git config daft.remote upstream
//!
//! # Disable hooks globally
//! git config --global daft.hooks.enabled false
//!
//! # Make post-create hooks abort on failure
//! git config daft.hooks.postCreate.failMode abort
//! ```

use crate::git::GitCommand;
use crate::hooks::{FailMode, HookConfig, HookType, HooksConfig, TrustLevel};
use anyhow::Result;
use std::path::PathBuf;

/// Where to cd after pruning the user's current worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneCdTarget {
    /// CD to the project root directory.
    Root,
    /// CD to the default branch worktree directory.
    DefaultBranch,
}

impl PruneCdTarget {
    /// Parse a string value into a PruneCdTarget.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "root" => Some(Self::Root),
            "default-branch" => Some(Self::DefaultBranch),
            _ => None,
        }
    }
}

/// Default values for settings.
pub mod defaults {
    use super::PruneCdTarget;

    /// Default value for autocd setting.
    pub const AUTOCD: bool = true;

    /// Default value for checkout.push setting.
    pub const CHECKOUT_PUSH: bool = true;

    /// Default value for checkout.upstream setting.
    pub const CHECKOUT_UPSTREAM: bool = true;

    /// Default value for remote setting.
    pub const REMOTE: &str = "origin";

    /// Default value for checkoutBranch.carry setting.
    pub const CHECKOUT_BRANCH_CARRY: bool = true;

    /// Default value for checkout.carry setting.
    pub const CHECKOUT_CARRY: bool = false;

    /// Default value for fetch.args setting.
    pub const FETCH_ARGS: &str = "--ff-only";

    /// Default value for multiRemote.enabled setting.
    pub const MULTI_REMOTE_ENABLED: bool = false;

    /// Default value for multiRemote.defaultRemote setting.
    pub const MULTI_REMOTE_DEFAULT_REMOTE: &str = "origin";

    /// Default value for prune.cdTarget setting.
    pub const PRUNE_CD_TARGET: PruneCdTarget = PruneCdTarget::Root;
}

/// Git config keys for daft settings.
pub mod keys {
    /// Config key for autocd setting.
    pub const AUTOCD: &str = "daft.autocd";

    /// Config key for checkout.push setting.
    pub const CHECKOUT_PUSH: &str = "daft.checkout.push";

    /// Config key for checkout.upstream setting.
    pub const CHECKOUT_UPSTREAM: &str = "daft.checkout.upstream";

    /// Config key for remote setting.
    pub const REMOTE: &str = "daft.remote";

    /// Config key for checkoutBranch.carry setting.
    pub const CHECKOUT_BRANCH_CARRY: &str = "daft.checkoutBranch.carry";

    /// Config key for checkout.carry setting.
    pub const CHECKOUT_CARRY: &str = "daft.checkout.carry";

    /// Config key for fetch.args setting.
    pub const FETCH_ARGS: &str = "daft.fetch.args";

    /// Multi-remote config keys.
    pub mod multi_remote {
        /// Config key for multiRemote.enabled setting.
        pub const ENABLED: &str = "daft.multiRemote.enabled";

        /// Config key for multiRemote.defaultRemote setting.
        pub const DEFAULT_REMOTE: &str = "daft.multiRemote.defaultRemote";
    }

    /// Config key for prune.cdTarget setting.
    pub const PRUNE_CD_TARGET: &str = "daft.prune.cdTarget";

    /// Config key for updateCheck setting.
    pub const UPDATE_CHECK: &str = "daft.updateCheck";

    /// Hooks config keys.
    pub mod hooks {
        /// Config key for hooks.enabled setting.
        pub const ENABLED: &str = "daft.hooks.enabled";

        /// Config key for hooks.defaultTrust setting.
        pub const DEFAULT_TRUST: &str = "daft.hooks.defaultTrust";

        /// Config key for hooks.userDirectory setting.
        pub const USER_DIRECTORY: &str = "daft.hooks.userDirectory";

        /// Config key for hooks.timeout setting.
        pub const TIMEOUT: &str = "daft.hooks.timeout";

        /// Generate a config key for a hook-specific setting.
        pub fn hook_key(hook_name: &str, setting: &str) -> String {
            format!("daft.hooks.{hook_name}.{setting}")
        }
    }
}

/// User-configurable settings for daft commands.
///
/// Settings are loaded from git config with the following priority:
/// 1. Repository-local config (`git config daft.x`)
/// 2. Global config (`git config --global daft.x`)
/// 3. Built-in defaults
#[derive(Debug, Clone)]
pub struct DaftSettings {
    /// CD into new worktrees (shell wrapper behavior).
    pub autocd: bool,

    /// Push new branches to remote after creation.
    pub checkout_push: bool,

    /// Set upstream tracking for branches.
    pub checkout_upstream: bool,

    /// Default remote name for operations.
    pub remote: String,

    /// Default carry setting for checkout-branch command.
    pub checkout_branch_carry: bool,

    /// Default carry setting for checkout command.
    pub checkout_carry: bool,

    /// Where to cd after pruning the user's current worktree.
    pub prune_cd_target: PruneCdTarget,

    /// Default arguments for git pull in fetch command.
    pub fetch_args: String,

    /// Whether multi-remote mode is enabled.
    pub multi_remote_enabled: bool,

    /// Default remote for multi-remote mode.
    pub multi_remote_default: String,
}

impl Default for DaftSettings {
    fn default() -> Self {
        Self {
            autocd: defaults::AUTOCD,
            checkout_push: defaults::CHECKOUT_PUSH,
            checkout_upstream: defaults::CHECKOUT_UPSTREAM,
            remote: defaults::REMOTE.to_string(),
            checkout_branch_carry: defaults::CHECKOUT_BRANCH_CARRY,
            checkout_carry: defaults::CHECKOUT_CARRY,
            prune_cd_target: defaults::PRUNE_CD_TARGET,
            fetch_args: defaults::FETCH_ARGS.to_string(),
            multi_remote_enabled: defaults::MULTI_REMOTE_ENABLED,
            multi_remote_default: defaults::MULTI_REMOTE_DEFAULT_REMOTE.to_string(),
        }
    }
}

impl DaftSettings {
    /// Load settings from git config (local + global).
    ///
    /// This method reads from the current repository's config,
    /// falling back to global config and then to defaults.
    ///
    /// Use this in commands that run inside a git repository.
    pub fn load() -> Result<Self> {
        let git = GitCommand::new(true);
        let mut settings = Self::default();

        if let Some(value) = git.config_get(keys::AUTOCD)? {
            settings.autocd = parse_bool(&value, defaults::AUTOCD);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_PUSH)? {
            settings.checkout_push = parse_bool(&value, defaults::CHECKOUT_PUSH);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_UPSTREAM)? {
            settings.checkout_upstream = parse_bool(&value, defaults::CHECKOUT_UPSTREAM);
        }

        if let Some(value) = git.config_get(keys::REMOTE)? {
            if !value.is_empty() {
                settings.remote = value;
            }
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_BRANCH_CARRY)? {
            settings.checkout_branch_carry = parse_bool(&value, defaults::CHECKOUT_BRANCH_CARRY);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_CARRY)? {
            settings.checkout_carry = parse_bool(&value, defaults::CHECKOUT_CARRY);
        }

        if let Some(value) = git.config_get(keys::PRUNE_CD_TARGET)? {
            if let Some(target) = PruneCdTarget::parse(&value) {
                settings.prune_cd_target = target;
            }
        }

        if let Some(value) = git.config_get(keys::FETCH_ARGS)? {
            if !value.is_empty() {
                settings.fetch_args = value;
            }
        }

        if let Some(value) = git.config_get(keys::multi_remote::ENABLED)? {
            settings.multi_remote_enabled = parse_bool(&value, defaults::MULTI_REMOTE_ENABLED);
        }

        if let Some(value) = git.config_get(keys::multi_remote::DEFAULT_REMOTE)? {
            if !value.is_empty() {
                settings.multi_remote_default = value;
            }
        }

        Ok(settings)
    }

    /// Load settings from global git config only.
    ///
    /// This method only reads from global config, ignoring repository-local config.
    /// Use this for commands that run before a repository exists (e.g., clone, init).
    pub fn load_global() -> Result<Self> {
        let git = GitCommand::new(true);
        let mut settings = Self::default();

        if let Some(value) = git.config_get_global(keys::AUTOCD)? {
            settings.autocd = parse_bool(&value, defaults::AUTOCD);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_PUSH)? {
            settings.checkout_push = parse_bool(&value, defaults::CHECKOUT_PUSH);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_UPSTREAM)? {
            settings.checkout_upstream = parse_bool(&value, defaults::CHECKOUT_UPSTREAM);
        }

        if let Some(value) = git.config_get_global(keys::REMOTE)? {
            if !value.is_empty() {
                settings.remote = value;
            }
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_BRANCH_CARRY)? {
            settings.checkout_branch_carry = parse_bool(&value, defaults::CHECKOUT_BRANCH_CARRY);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_CARRY)? {
            settings.checkout_carry = parse_bool(&value, defaults::CHECKOUT_CARRY);
        }

        if let Some(value) = git.config_get_global(keys::PRUNE_CD_TARGET)? {
            if let Some(target) = PruneCdTarget::parse(&value) {
                settings.prune_cd_target = target;
            }
        }

        if let Some(value) = git.config_get_global(keys::FETCH_ARGS)? {
            if !value.is_empty() {
                settings.fetch_args = value;
            }
        }

        if let Some(value) = git.config_get_global(keys::multi_remote::ENABLED)? {
            settings.multi_remote_enabled = parse_bool(&value, defaults::MULTI_REMOTE_ENABLED);
        }

        if let Some(value) = git.config_get_global(keys::multi_remote::DEFAULT_REMOTE)? {
            if !value.is_empty() {
                settings.multi_remote_default = value;
            }
        }

        Ok(settings)
    }
}

/// Parse a git config boolean value.
///
/// Git accepts various boolean representations:
/// - true: `true`, `yes`, `on`, `1`
/// - false: `false`, `no`, `off`, `0`
///
/// Returns the default value if parsing fails.
fn parse_bool(value: &str, default: bool) -> bool {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => true,
        "false" | "no" | "off" | "0" => false,
        _ => default,
    }
}

/// Load hooks configuration from git config.
///
/// This loads hooks settings from the current repository's config,
/// falling back to global config and then to defaults.
pub fn load_hooks_config() -> Result<HooksConfig> {
    let git = GitCommand::new(true);
    let mut config = HooksConfig::default();

    // Load global hooks settings
    if let Some(value) = git.config_get(keys::hooks::ENABLED)? {
        config.enabled = parse_bool(&value, true);
    }

    if let Some(value) = git.config_get(keys::hooks::DEFAULT_TRUST)? {
        if let Some(level) = TrustLevel::parse(&value) {
            config.default_trust = level;
        }
    }

    if let Some(value) = git.config_get(keys::hooks::USER_DIRECTORY)? {
        if !value.is_empty() {
            // Expand ~ to home directory
            let expanded = if let Some(stripped) = value.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(stripped)
                } else {
                    PathBuf::from(&value)
                }
            } else {
                PathBuf::from(&value)
            };
            config.user_directory = expanded;
        }
    }

    if let Some(value) = git.config_get(keys::hooks::TIMEOUT)? {
        if let Ok(timeout) = value.parse::<u32>() {
            config.timeout_seconds = timeout;
        }
    }

    // Load per-hook settings
    for hook_type in HookType::all() {
        let hook_config = config.get_hook_config_mut(*hook_type);
        load_hook_type_config(&git, *hook_type, hook_config)?;
    }

    Ok(config)
}

/// Load hooks configuration from global git config only.
///
/// Use this for commands that run before a repository exists (e.g., clone).
pub fn load_hooks_config_global() -> Result<HooksConfig> {
    let git = GitCommand::new(true);
    let mut config = HooksConfig::default();

    // Load global hooks settings
    if let Some(value) = git.config_get_global(keys::hooks::ENABLED)? {
        config.enabled = parse_bool(&value, true);
    }

    if let Some(value) = git.config_get_global(keys::hooks::DEFAULT_TRUST)? {
        if let Some(level) = TrustLevel::parse(&value) {
            config.default_trust = level;
        }
    }

    if let Some(value) = git.config_get_global(keys::hooks::USER_DIRECTORY)? {
        if !value.is_empty() {
            let expanded = if let Some(stripped) = value.strip_prefix("~/") {
                if let Some(home) = dirs::home_dir() {
                    home.join(stripped)
                } else {
                    PathBuf::from(&value)
                }
            } else {
                PathBuf::from(&value)
            };
            config.user_directory = expanded;
        }
    }

    if let Some(value) = git.config_get_global(keys::hooks::TIMEOUT)? {
        if let Ok(timeout) = value.parse::<u32>() {
            config.timeout_seconds = timeout;
        }
    }

    // Load per-hook settings from global config
    for hook_type in HookType::all() {
        let hook_config = config.get_hook_config_mut(*hook_type);
        load_hook_type_config_global(&git, *hook_type, hook_config)?;
    }

    Ok(config)
}

/// Load configuration for a specific hook type.
///
/// Falls back to deprecated config keys if the new key is not found.
fn load_hook_type_config(
    git: &GitCommand,
    hook_type: HookType,
    hook_config: &mut HookConfig,
) -> Result<()> {
    let enabled_key = keys::hooks::hook_key(hook_type.config_key(), "enabled");
    let enabled_value = match (
        git.config_get(&enabled_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get(&keys::hooks::hook_key(dep, "enabled"))?,
        (None, None) => None,
    };
    if let Some(value) = enabled_value {
        hook_config.enabled = parse_bool(&value, true);
    }

    let fail_mode_key = keys::hooks::hook_key(hook_type.config_key(), "failMode");
    let fail_mode_value = match (
        git.config_get(&fail_mode_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get(&keys::hooks::hook_key(dep, "failMode"))?,
        (None, None) => None,
    };
    if let Some(value) = fail_mode_value {
        if let Some(mode) = FailMode::parse(&value) {
            hook_config.fail_mode = mode;
        }
    }

    Ok(())
}

/// Load configuration for a specific hook type from global config only.
///
/// Falls back to deprecated config keys if the new key is not found.
fn load_hook_type_config_global(
    git: &GitCommand,
    hook_type: HookType,
    hook_config: &mut HookConfig,
) -> Result<()> {
    let enabled_key = keys::hooks::hook_key(hook_type.config_key(), "enabled");
    let enabled_value = match (
        git.config_get_global(&enabled_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get_global(&keys::hooks::hook_key(dep, "enabled"))?,
        (None, None) => None,
    };
    if let Some(value) = enabled_value {
        hook_config.enabled = parse_bool(&value, true);
    }

    let fail_mode_key = keys::hooks::hook_key(hook_type.config_key(), "failMode");
    let fail_mode_value = match (
        git.config_get_global(&fail_mode_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get_global(&keys::hooks::hook_key(dep, "failMode"))?,
        (None, None) => None,
    };
    if let Some(value) = fail_mode_value {
        if let Some(mode) = FailMode::parse(&value) {
            hook_config.fail_mode = mode;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = DaftSettings::default();
        assert!(settings.autocd);
        assert!(settings.checkout_push);
        assert!(settings.checkout_upstream);
        assert_eq!(settings.remote, "origin");
        assert!(settings.checkout_branch_carry);
        assert!(!settings.checkout_carry);
        assert_eq!(settings.prune_cd_target, PruneCdTarget::Root);
        assert_eq!(settings.fetch_args, "--ff-only");
        assert!(!settings.multi_remote_enabled);
        assert_eq!(settings.multi_remote_default, "origin");
    }

    #[test]
    fn test_prune_cd_target_parse() {
        assert_eq!(PruneCdTarget::parse("root"), Some(PruneCdTarget::Root));
        assert_eq!(PruneCdTarget::parse("Root"), Some(PruneCdTarget::Root));
        assert_eq!(PruneCdTarget::parse("ROOT"), Some(PruneCdTarget::Root));
        assert_eq!(
            PruneCdTarget::parse("default-branch"),
            Some(PruneCdTarget::DefaultBranch)
        );
        assert_eq!(
            PruneCdTarget::parse("Default-Branch"),
            Some(PruneCdTarget::DefaultBranch)
        );
        assert_eq!(PruneCdTarget::parse("invalid"), None);
        assert_eq!(PruneCdTarget::parse(""), None);
    }

    #[test]
    fn test_parse_bool_true_variants() {
        assert!(parse_bool("true", false));
        assert!(parse_bool("True", false));
        assert!(parse_bool("TRUE", false));
        assert!(parse_bool("yes", false));
        assert!(parse_bool("Yes", false));
        assert!(parse_bool("YES", false));
        assert!(parse_bool("on", false));
        assert!(parse_bool("On", false));
        assert!(parse_bool("ON", false));
        assert!(parse_bool("1", false));
    }

    #[test]
    fn test_parse_bool_false_variants() {
        assert!(!parse_bool("false", true));
        assert!(!parse_bool("False", true));
        assert!(!parse_bool("FALSE", true));
        assert!(!parse_bool("no", true));
        assert!(!parse_bool("No", true));
        assert!(!parse_bool("NO", true));
        assert!(!parse_bool("off", true));
        assert!(!parse_bool("Off", true));
        assert!(!parse_bool("OFF", true));
        assert!(!parse_bool("0", true));
    }

    #[test]
    fn test_parse_bool_invalid_returns_default() {
        assert!(parse_bool("invalid", true));
        assert!(!parse_bool("invalid", false));
        assert!(parse_bool("", true));
        assert!(!parse_bool("", false));
        assert!(parse_bool("maybe", true));
        assert!(!parse_bool("maybe", false));
    }
}
