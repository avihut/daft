use super::oxide;
use super::GitCommand;
use anyhow::{Context, Result};
use std::process::Command;

impl GitCommand {
    /// Set a git config value
    pub fn config_set(&self, key: &str, value: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["config", key, value])
            .output()
            .context("Failed to execute git config command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git config failed: {}", stderr);
        }

        Ok(())
    }

    /// Unset a git config value
    pub fn config_unset(&self, key: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["config", "--unset", key])
            .output()
            .context("Failed to execute git config --unset command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git config --unset failed: {}", stderr);
        }

        Ok(())
    }

    /// Get a git config value from the current repository (respects local + global config)
    pub fn config_get(&self, key: &str) -> Result<Option<String>> {
        if self.use_gitoxide {
            return oxide::config_get(&self.gix_repo()?, key);
        }
        let output = Command::new("git")
            .args(["config", "--get", key])
            .output()
            .context("Failed to execute git config command")?;

        if output.status.success() {
            let value = String::from_utf8(output.stdout)
                .context("Failed to parse git config output")?
                .trim()
                .to_string();
            Ok(Some(value))
        } else {
            // Exit code 1 means the key was not found, which is not an error
            Ok(None)
        }
    }

    /// Get a git config value from global config only
    pub fn config_get_global(&self, key: &str) -> Result<Option<String>> {
        if self.use_gitoxide {
            return oxide::config_get_global(key);
        }
        let output = Command::new("git")
            .args(["config", "--global", "--get", key])
            .output()
            .context("Failed to execute git config command")?;

        if output.status.success() {
            let value = String::from_utf8(output.stdout)
                .context("Failed to parse git config output")?
                .trim()
                .to_string();
            Ok(Some(value))
        } else {
            // Exit code 1 means the key was not found, which is not an error
            Ok(None)
        }
    }

    /// Get the tracking remote for a branch.
    pub fn get_branch_tracking_remote(&self, branch: &str) -> Result<Option<String>> {
        let key = format!("branch.{branch}.remote");
        self.config_get(&key)
    }
}
