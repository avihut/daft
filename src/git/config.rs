use super::GitCommand;
use super::oxide;
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

    /// Get a git config value from the current repository (respects local + global config).
    ///
    /// Always uses gitoxide for in-process config reading — no subprocess overhead.
    pub fn config_get(&self, key: &str) -> Result<Option<String>> {
        oxide::config_get(&self.gix_repo()?, key)
    }

    /// Set a git config value in global config
    pub fn config_set_global(&self, key: &str, value: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["config", "--global", key, value])
            .output()
            .context("Failed to execute git config --global command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git config --global failed: {}", stderr);
        }

        Ok(())
    }

    /// Get a git config value from global config only.
    ///
    /// Always uses gitoxide for in-process config reading — no subprocess overhead.
    pub fn config_get_global(&self, key: &str) -> Result<Option<String>> {
        oxide::config_get_global(key)
    }

    /// Get the tracking remote for a branch.
    pub fn get_branch_tracking_remote(&self, branch: &str) -> Result<Option<String>> {
        let key = format!("branch.{branch}.remote");
        self.config_get(&key)
    }

    /// Get the tracking remote for a branch, using an explicit working directory.
    ///
    /// Required for parallel workers where `set_current_dir` would race.
    pub fn get_branch_tracking_remote_from(
        &self,
        branch: &str,
        cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        let key = format!("branch.{branch}.remote");
        let output = Command::new("git")
            .args(["config", "--get", &key])
            .current_dir(cwd)
            .output()
            .context("Failed to execute git config command")?;

        if output.status.success() {
            let value = String::from_utf8(output.stdout)
                .context("Failed to parse git config output")?
                .trim()
                .to_string();
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    /// Configure a branch to track an explicit remote merge ref.
    ///
    /// Used for forge PR/MR checkout: the fork head lives at a stable ref on
    /// the base repo (`refs/pull/123/head` / `refs/merge-requests/45/head`)
    /// rather than a normal `refs/heads/*` branch, so the standard
    /// `--set-upstream-to` (which needs a `refs/remotes/<remote>/<branch>`
    /// tracking ref) can't express it. Writing `branch.<name>.remote` +
    /// `branch.<name>.merge` directly makes `git pull` on the branch update
    /// from the PR/MR head.
    pub fn set_branch_tracking(&self, branch: &str, remote: &str, merge_ref: &str) -> Result<()> {
        self.config_set(&format!("branch.{branch}.remote"), remote)?;
        self.config_set(&format!("branch.{branch}.merge"), merge_ref)?;
        Ok(())
    }
}
