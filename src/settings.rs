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
//!
//! # Example
//!
//! ```bash
//! # Disable auto-cd globally
//! git config --global daft.autocd false
//!
//! # Use a different remote for this repository
//! git config daft.remote upstream
//! ```

use crate::git::GitCommand;
use anyhow::Result;

/// Default values for settings.
pub mod defaults {
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
