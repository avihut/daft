use super::GitCommand;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

impl GitCommand {
    pub fn clone_bare(&self, repo_url: &str, target_dir: &Path) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["clone", "--bare"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(repo_url).arg(target_dir);

        let output = cmd
            .output()
            .context("Failed to execute git clone command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git clone failed: {}", stderr);
        }

        Ok(())
    }

    pub fn init_bare(&self, target_dir: &Path, initial_branch: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["init", "--bare"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(format!("--initial-branch={initial_branch}"))
            .arg(target_dir);

        let output = cmd.output().context("Failed to execute git init command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git init failed: {}", stderr);
        }

        Ok(())
    }

    /// Set up the fetch refspec for a remote (required for bare repos to support upstream tracking)
    pub fn setup_fetch_refspec(&self, remote_name: &str) -> Result<()> {
        let refspec = format!("+refs/heads/*:refs/remotes/{remote_name}/*");
        self.config_set(&format!("remote.{remote_name}.fetch"), &refspec)
    }
}
