use super::oxide;
use super::GitCommand;
use anyhow::{Context, Result};
use std::process::Command;

impl GitCommand {
    pub fn branch_delete(&self, branch: &str, force: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["branch"]);

        if force {
            cmd.arg("-D");
        } else {
            cmd.arg("-d");
        }

        cmd.arg(branch);

        let output = cmd
            .output()
            .context("Failed to execute git branch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git branch delete failed: {}", stderr);
        }

        Ok(())
    }

    pub fn branch_list_verbose(&self) -> Result<String> {
        if self.use_gitoxide {
            return oxide::branch_list_verbose(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["branch", "-vv"])
            .output()
            .context("Failed to execute git branch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git branch list failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git branch output")
    }

    /// Checkout a branch in the current working directory.
    pub fn checkout(&self, branch: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["checkout"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(branch);

        let output = cmd
            .output()
            .context("Failed to execute git checkout command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git checkout failed: {}", stderr);
        }

        Ok(())
    }
}
