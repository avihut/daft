use super::oxide;
use super::GitCommand;
use anyhow::{Context, Result};
use std::process::{Command, Stdio};

impl GitCommand {
    pub fn fetch(&self, remote: &str, prune: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["fetch", remote]);

        if prune {
            cmd.arg("--prune");
        }

        let output = cmd
            .output()
            .context("Failed to execute git fetch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git fetch failed: {}", stderr);
        }

        Ok(())
    }

    pub fn fetch_refspec(&self, remote: &str, refspec: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["fetch", remote, refspec]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git fetch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git fetch with refspec failed: {}", stderr);
        }

        Ok(())
    }

    pub fn push_set_upstream(&self, remote: &str, branch: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["push", "--no-verify", "--set-upstream", remote, branch])
            .output()
            .context("Failed to execute git push command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git push failed: {}", stderr);
        }

        Ok(())
    }

    pub fn set_upstream(&self, remote: &str, branch: &str) -> Result<()> {
        let upstream = format!("{remote}/{branch}");
        let output = Command::new("git")
            .args(["branch", &format!("--set-upstream-to={upstream}")])
            .output()
            .context("Failed to execute git branch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git set upstream failed: {}", stderr);
        }

        Ok(())
    }

    /// Delete a remote branch via `git push <remote> --delete <branch>`.
    pub fn push_delete(&self, remote: &str, branch: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["push", "--no-verify", remote, "--delete", branch]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git push --delete command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git push --delete failed: {}", stderr);
        }

        Ok(())
    }

    /// Pull from remote with specified arguments
    pub fn pull(&self, args: &[&str]) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.arg("pull");

        for arg in args {
            cmd.arg(arg);
        }

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd.output().context("Failed to execute git pull command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git pull failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git pull output")
    }

    /// Pull from remote with inherited stdio, so git's progress output flows to the terminal.
    ///
    /// Unlike `pull()`, this does not capture output. It uses `Stdio::inherit()` for both
    /// stdout and stderr, making git's remote progress and ref update lines visible.
    pub fn pull_passthrough(&self, args: &[&str]) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.arg("pull");

        for arg in args {
            cmd.arg(arg);
        }

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());

        let status = cmd.status().context("Failed to execute git pull command")?;

        if !status.success() {
            anyhow::bail!("Git pull failed with exit code: {}", status);
        }

        Ok(())
    }

    /// Reset the current branch to a given target (e.g., `origin/master`).
    ///
    /// Runs `git reset --hard <target>`.
    pub fn reset_hard(&self, target: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["reset", "--hard", target]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git reset --hard command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git reset --hard failed: {}", stderr);
        }

        Ok(())
    }

    pub fn ls_remote_heads(&self, remote: &str, branch: Option<&str>) -> Result<String> {
        if self.use_gitoxide {
            if let Ok(repo) = self.gix_repo() {
                return oxide::ls_remote_heads(&repo, remote, branch);
            }
            // No local repo (e.g. during clone) — fall through to git CLI
        }
        let mut cmd = Command::new("git");
        cmd.args(["ls-remote", "--heads", remote]);

        if let Some(branch) = branch {
            cmd.arg(format!("refs/heads/{branch}"));
        }

        let output = cmd
            .output()
            .context("Failed to execute git ls-remote command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git ls-remote failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git ls-remote output")
    }

    /// Execute git ls-remote with symref to get remote HEAD
    pub fn ls_remote_symref(&self, remote_url: &str) -> Result<String> {
        if self.use_gitoxide {
            if let Ok(repo) = self.gix_repo() {
                return oxide::ls_remote_symref(&repo, remote_url);
            }
            // No local repo (e.g. during clone) — fall through to git CLI
        }
        let output = Command::new("git")
            .args(["ls-remote", "--symref", remote_url, "HEAD"])
            .output()
            .context("Failed to execute git ls-remote command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git ls-remote failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git ls-remote output")
    }

    /// Check if specific remote branch exists
    pub fn ls_remote_branch_exists(&self, remote_name: &str, branch: &str) -> Result<bool> {
        if self.use_gitoxide {
            if let Ok(repo) = self.gix_repo() {
                return oxide::ls_remote_branch_exists(&repo, remote_name, branch);
            }
            // No local repo (e.g. during clone) — fall through to git CLI
        }
        let output = Command::new("git")
            .args([
                "ls-remote",
                "--heads",
                remote_name,
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .context("Failed to execute git ls-remote command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git ls-remote failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git ls-remote output")?;
        Ok(!stdout.trim().is_empty())
    }

    pub fn remote_set_head_auto(&self, remote: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["remote", "set-head", remote, "--auto"])
            .output()
            .context("Failed to execute git remote set-head command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git remote set-head failed: {}", stderr);
        }

        Ok(())
    }

    /// List all configured remotes.
    pub fn remote_list(&self) -> Result<Vec<String>> {
        if self.use_gitoxide {
            return oxide::remote_list(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["remote"])
            .output()
            .context("Failed to execute git remote command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git remote failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git remote output")?;

        Ok(stdout
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    /// Check if a remote exists.
    pub fn remote_exists(&self, remote: &str) -> Result<bool> {
        let remotes = self.remote_list()?;
        Ok(remotes.contains(&remote.to_string()))
    }

    /// Get the URL of a remote.
    pub fn remote_get_url(&self, remote: &str) -> Result<String> {
        if self.use_gitoxide {
            return oxide::remote_get_url(&self.gix_repo()?, remote);
        }
        let output = Command::new("git")
            .args(["remote", "get-url", remote])
            .output()
            .context("Failed to execute git remote get-url command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git remote get-url failed: {}", stderr);
        }

        String::from_utf8(output.stdout)
            .context("Failed to parse git remote get-url output")
            .map(|s| s.trim().to_string())
    }
}
