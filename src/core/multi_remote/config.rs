//! Configuration helpers for multi-remote mode.

use crate::git::GitCommand;
use crate::settings::keys;
use anyhow::Result;

/// Set multi-remote configuration in the local git config.
pub fn set_multi_remote_enabled(git: &GitCommand, enabled: bool) -> Result<()> {
    let value = if enabled { "true" } else { "false" };
    git.config_set(keys::multi_remote::ENABLED, value)
}

/// Set the default remote for multi-remote mode.
pub fn set_multi_remote_default(git: &GitCommand, remote: &str) -> Result<()> {
    git.config_set(keys::multi_remote::DEFAULT_REMOTE, remote)
}

/// Clear multi-remote configuration from the local git config.
pub fn clear_multi_remote_config(git: &GitCommand) -> Result<()> {
    // Try to unset; ignore errors if key doesn't exist
    let _ = git.config_unset(keys::multi_remote::ENABLED);
    let _ = git.config_unset(keys::multi_remote::DEFAULT_REMOTE);
    Ok(())
}

/// Check if multi-remote mode should be used based on settings and context.
pub fn should_use_multi_remote(enabled: bool, remote_count: usize) -> bool {
    // Multi-remote mode only makes sense with more than one remote
    enabled && remote_count > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_use_multi_remote() {
        // Disabled mode - never use multi-remote
        assert!(!should_use_multi_remote(false, 0));
        assert!(!should_use_multi_remote(false, 1));
        assert!(!should_use_multi_remote(false, 2));

        // Enabled mode - only with multiple remotes
        assert!(!should_use_multi_remote(true, 0));
        assert!(!should_use_multi_remote(true, 1));
        assert!(should_use_multi_remote(true, 2));
        assert!(should_use_multi_remote(true, 3));
    }
}
