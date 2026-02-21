use super::oxide;
use super::GitCommand;
use anyhow::{Context, Result};
use std::process::Command;

impl GitCommand {
    pub fn show_ref_exists(&self, ref_name: &str) -> Result<bool> {
        if self.use_gitoxide {
            return oxide::show_ref_exists(&self.gix_repo()?, ref_name);
        }
        let output = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", ref_name])
            .output()
            .context("Failed to execute git show-ref command")?;

        Ok(output.status.success())
    }

    pub fn for_each_ref(&self, format: &str, refs: &str) -> Result<String> {
        if self.use_gitoxide {
            return oxide::for_each_ref(&self.gix_repo()?, format, refs);
        }
        let output = Command::new("git")
            .args(["for-each-ref", &format!("--format={format}"), refs])
            .output()
            .context("Failed to execute git for-each-ref command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git for-each-ref failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git for-each-ref output")
    }

    /// Get the short name of the current branch
    pub fn symbolic_ref_short_head(&self) -> Result<String> {
        if self.use_gitoxide {
            return oxide::symbolic_ref_short_head(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .context("Failed to execute git symbolic-ref command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git symbolic-ref failed: {}", stderr);
        }

        String::from_utf8(output.stdout)
            .context("Failed to parse git symbolic-ref output")
            .map(|s| s.trim().to_string())
    }

    /// Resolve a ref to its SHA. Returns the full commit hash.
    pub fn rev_parse(&self, rev: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", rev])
            .output()
            .context("Failed to execute git rev-parse command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git rev-parse failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git rev-parse output")?;
        Ok(stdout.trim().to_string())
    }

    /// Check if current directory is inside a Git work tree
    pub fn rev_parse_is_inside_work_tree(&self) -> Result<bool> {
        if self.use_gitoxide {
            return oxide::rev_parse_is_inside_work_tree(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .context("Failed to execute git rev-parse command")?;

        if !output.status.success() {
            return Ok(false);
        }

        // In a bare repo root, git exits 0 but prints "false" to stdout.
        // We must check the actual output, not just the exit code.
        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git rev-parse output")?;
        Ok(stdout.trim() == "true")
    }

    /// Check if current directory is inside any Git repository (work tree or bare)
    pub fn is_inside_git_repo(&self) -> Result<bool> {
        if self.use_gitoxide {
            return oxide::is_inside_git_repo();
        }
        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .stderr(std::process::Stdio::null())
            .output()
            .context("Failed to execute git rev-parse command")?;

        Ok(output.status.success())
    }

    /// Get the Git common directory path
    pub fn rev_parse_git_common_dir(&self) -> Result<String> {
        if self.use_gitoxide {
            return oxide::rev_parse_git_common_dir(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .output()
            .context("Failed to execute git rev-parse command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git rev-parse failed: {}", stderr);
        }

        String::from_utf8(output.stdout)
            .context("Failed to parse git rev-parse output")
            .map(|s| s.trim().to_string())
    }

    /// Check if the repository is a bare repository.
    pub fn rev_parse_is_bare_repository(&self) -> Result<bool> {
        if self.use_gitoxide {
            return oxide::rev_parse_is_bare_repository(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["rev-parse", "--is-bare-repository"])
            .output()
            .context("Failed to execute git rev-parse command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git rev-parse failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git rev-parse output")?;
        Ok(stdout.trim() == "true")
    }

    pub fn get_git_dir(&self) -> Result<String> {
        if self.use_gitoxide {
            return oxide::get_git_dir(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .output()
            .context("Failed to execute git rev-parse command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git rev-parse failed: {}", stderr);
        }

        String::from_utf8(output.stdout)
            .context("Failed to parse git rev-parse output")
            .map(|s| s.trim().to_string())
    }

    /// Get the path of the current worktree
    pub fn get_current_worktree_path(&self) -> Result<std::path::PathBuf> {
        if self.use_gitoxide {
            return oxide::get_current_worktree_path(&self.gix_repo()?);
        }
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("Failed to execute git rev-parse command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git rev-parse failed: {}", stderr);
        }

        let path_str =
            String::from_utf8(output.stdout).context("Failed to parse git rev-parse output")?;
        Ok(std::path::PathBuf::from(path_str.trim()))
    }

    pub fn rev_list_count(&self, range: &str) -> Result<u32> {
        if self.use_gitoxide {
            return oxide::rev_list_count(&self.gix_repo()?, range);
        }
        let output = Command::new("git")
            .args(["rev-list", "--count", range])
            .output()
            .context("Failed to execute git rev-list command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git rev-list failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git rev-list output")?;

        stdout
            .trim()
            .parse::<u32>()
            .context("Failed to parse commit count as number")
    }

    /// Check if `commit` is an ancestor of `target` using merge-base.
    pub fn merge_base_is_ancestor(&self, commit: &str, target: &str) -> Result<bool> {
        let output = Command::new("git")
            .args(["merge-base", "--is-ancestor", commit, target])
            .output()
            .context("Failed to execute git merge-base command")?;

        Ok(output.status.success())
    }

    /// Run `git cherry <upstream> <branch>` and return output.
    /// Lines prefixed with `-` indicate patches already upstream.
    /// Lines prefixed with `+` indicate patches NOT upstream.
    pub fn cherry(&self, upstream: &str, branch: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["cherry", upstream, branch])
            .output()
            .context("Failed to execute git cherry command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git cherry failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git cherry output")
    }
}
