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

    /// Every branch's `branch.<name>.merge` value in one call — raw
    /// `git config --get-regexp` lines (`branch.<name>.merge <ref>`), for
    /// bulk PR-tracking-ref resolution where a per-branch `config_get` would
    /// cost a read per row. No matches is not an error (exit code 1 → empty).
    pub fn branch_merge_refs(&self) -> Result<String> {
        // git_command_at (not a raw `git`) scrubs any inherited GIT_DIR so the
        // read targets the cwd's repo — not the hook-calling repo when daft runs
        // inside a git hook (e.g. post-checkout). `daft list` calls this to
        // resolve each branch's PR/MR tracking ref; an inherited GIT_DIR would
        // otherwise decorate rows from the parent repo's branch config. Mirrors
        // the `fetch_refspec` sibling scrub.
        let cwd = std::env::current_dir().context("Failed to resolve current directory")?;
        let output = crate::utils::git_command_at(&cwd)
            .args(["config", "--get-regexp", r"^branch\..*\.merge$"])
            .output()
            .context("Failed to execute git config --get-regexp command")?;

        if !output.status.success() && output.status.code() != Some(1) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git config --get-regexp failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git config output")
    }

    /// Get the tracking remote for a branch.
    pub fn get_branch_tracking_remote(&self, branch: &str) -> Result<Option<String>> {
        let key = format!("branch.{branch}.remote");
        self.config_get(&key)
    }

    /// Read one config key from an explicit working directory.
    ///
    /// Goes through [`crate::utils::git_command_at`] so `-C <cwd>` is
    /// authoritative: an inherited `GIT_DIR` (daft running inside a git hook)
    /// otherwise wins repo discovery and answers from the wrong repo, which
    /// reads as "no upstream configured" and silently changes what gets
    /// pushed where.
    fn config_get_from(&self, key: &str, cwd: &std::path::Path) -> Result<Option<String>> {
        let output = crate::utils::git_command_at(cwd)
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
            Ok(None)
        }
    }

    /// Get the tracking remote for a branch, using an explicit working directory.
    ///
    /// Required for parallel workers where `set_current_dir` would race.
    pub fn get_branch_tracking_remote_from(
        &self,
        branch: &str,
        cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        self.config_get_from(&format!("branch.{branch}.remote"), cwd)
    }

    /// Get the upstream ref a branch merges with (`branch.<name>.merge`),
    /// using an explicit working directory.
    ///
    /// The companion to [`Self::get_branch_tracking_remote_from`]: the remote
    /// alone does not say *which* ref on it the branch tracks, and the two
    /// disagreeing (local `feat` tracking `origin/main`) is what makes an
    /// implicit `<branch>:<branch>` push surprising.
    pub fn get_branch_merge_ref_from(
        &self,
        branch: &str,
        cwd: &std::path::Path,
    ) -> Result<Option<String>> {
        self.config_get_from(&format!("branch.{branch}.merge"), cwd)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::Path;
    use tempfile::tempdir;

    const GIT_ENV_VARS: &[&str] = &[
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CEILING_DIRECTORIES",
        "GIT_NAMESPACE",
    ];

    /// A `git` command scrubbed of hook-inherited env, rooted at `dir` — used to
    /// build the fixture repos without the ambient GIT_* leaking in.
    fn git_at(dir: &Path) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(dir);
        for v in GIT_ENV_VARS {
            cmd.env_remove(v);
        }
        cmd
    }

    fn init_repo_with_merge(dir: &Path, branch: &str, merge_ref: &str) {
        git_at(dir).args(["init", "-q"]).status().unwrap();
        git_at(dir)
            .args(["config", &format!("branch.{branch}.merge"), merge_ref])
            .status()
            .unwrap();
    }

    /// Regression: inside a git hook, an inherited `GIT_DIR` must not retarget
    /// `branch_merge_refs` at the hook-calling repo. Without the `git_command_at`
    /// scrub this reads the `GIT_DIR` repo's config and `daft list` decorates PR
    /// cells from the wrong repo.
    #[test]
    #[serial]
    fn branch_merge_refs_reads_cwd_repo_not_inherited_git_dir() {
        let this = tempdir().unwrap();
        let hook = tempdir().unwrap();
        let this_path = this.path().canonicalize().unwrap();
        let hook_path = hook.path().canonicalize().unwrap();

        init_repo_with_merge(&this_path, "feature", "refs/pull/7/head");
        init_repo_with_merge(&hook_path, "other", "refs/pull/999/head");

        let original_cwd = std::env::current_dir().ok();
        std::env::set_current_dir(&this_path).unwrap();
        // Simulate the hook environment: GIT_DIR points at the *other* repo.
        unsafe { std::env::set_var("GIT_DIR", hook_path.join(".git")) };

        let result = GitCommand::new(true).branch_merge_refs();

        // Restore process state before asserting so a failure can't strand
        // sibling serial tests.
        unsafe { std::env::remove_var("GIT_DIR") };
        if let Some(cwd) = original_cwd {
            let _ = std::env::set_current_dir(cwd);
        }

        let out = result.unwrap();
        assert!(
            out.contains("refs/pull/7/head"),
            "must read this dir's repo config, got: {out:?}"
        );
        assert!(
            !out.contains("refs/pull/999/head"),
            "must not read the inherited GIT_DIR repo's config, got: {out:?}"
        );
    }
}
