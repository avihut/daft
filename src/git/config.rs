use super::GitCommand;
use super::oxide;
use anyhow::{Context, Result};
use std::process::Command;

/// `git config --unset`'s exit code for "that option was not set".
///
/// Not an error for daft's purposes: unsetting an absent key leaves the
/// caller exactly where it wanted to be.
const EXIT_NOTHING_TO_UNSET: i32 = 5;

/// A `git config` command rooted at the process's working directory, with the
/// ambient `GIT_*` variables scrubbed.
///
/// Every write goes through here. An inherited `GIT_DIR` — which daft has
/// whenever it runs inside a git hook — silently outranks the working
/// directory for repo discovery, so a bare `git config` would write the
/// hook-calling repo's config instead of this one. The same hazard is already
/// documented on [`GitCommand::config_get_from`] for reads; a misdirected
/// *write* is worse, because nothing later reads it back to notice.
fn config_command() -> Result<Command> {
    let cwd = std::env::current_dir().context("Failed to resolve current directory")?;
    let mut cmd = crate::utils::git_command_at(&cwd);
    cmd.arg("config");
    Ok(cmd)
}

/// Interpret a `git config --unset` exit status.
fn unset_outcome(output: &std::process::Output, context: &str) -> Result<bool> {
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(EXIT_NOTHING_TO_UNSET) {
        return Ok(false);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!("{context}: {stderr}");
}

/// Which config file a value came from, in git's own precedence order.
///
/// The whole point of the settings screen is that "what is my configuration"
/// has no single answer — it is a stack, and the interesting question is which
/// layer won. This is that stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigScope {
    /// `$(prefix)/etc/gitconfig` and the git installation's own config.
    System,
    /// `~/.gitconfig` and `$XDG_CONFIG_HOME/git/config`.
    Global,
    /// The repository's `.git/config`.
    Local,
    /// `.git/config.worktree`, when `extensions.worktreeConfig` is on.
    Worktree,
    /// Set for this process only — `GIT_CONFIG_KEY_N`, `git -c`, or an
    /// in-process override. Outranks every file and survives no write.
    Ephemeral,
}

impl ConfigScope {
    /// The label used in ladders, origins, and status lines.
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Global => "global",
            Self::Local => "local",
            Self::Worktree => "worktree",
            Self::Ephemeral => "environment",
        }
    }

    /// Whether `daft config` can write this scope.
    ///
    /// Only the two git offers a `--global`/local flag for. Writing the
    /// system file needs privileges daft should not ask for, the worktree
    /// file needs an extension most repos have off, and the ephemeral scope
    /// is not a file at all — a write there would evaporate at exit.
    pub fn is_writable(self) -> bool {
        matches!(self, Self::Global | Self::Local)
    }

    /// Every scope, lowest precedence first — the order a ladder renders in.
    pub fn ladder() -> &'static [ConfigScope] {
        &[
            Self::System,
            Self::Global,
            Self::Local,
            Self::Worktree,
            Self::Ephemeral,
        ]
    }
}

/// One `daft.*` value as it is actually stored, with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    /// The key exactly as spelled in the file — not canonicalized, so the
    /// caller can tell `daft.checkoutBranch.carry` from a typo'd
    /// `daft.checkoutbranch.carry` that git stores as a different subsection.
    pub key: String,
    /// The stored value, unparsed.
    pub value: String,
    /// Which layer it came from.
    pub scope: ConfigScope,
    /// The file it lives in, when there is one.
    pub origin_path: Option<std::path::PathBuf>,
}

impl GitCommand {
    /// Every `daft.*` entry visible from this repository, with provenance.
    ///
    /// One pass over the merged config snapshot — cheaper and more truthful
    /// than asking `git config --get` per key per scope, which would be ~300
    /// subprocesses and still would not say which file answered.
    pub fn daft_config_entries(&self) -> Result<Vec<ConfigEntry>> {
        oxide::config_entries_prefixed(&self.gix_repo()?, "daft")
    }

    /// Set a git config value in the current repository.
    pub fn config_set(&self, key: &str, value: &str) -> Result<()> {
        let output = config_command()?
            .args([key, value])
            .output()
            .context("Failed to execute git config command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git config failed: {}", stderr);
        }

        Ok(())
    }

    /// Unset a git config value in the current repository.
    ///
    /// Returns whether anything was actually removed — a key that was not set
    /// is reported as `Ok(false)`, not an error, so callers can say "nothing
    /// to unset here" instead of surfacing git's bare exit code.
    pub fn config_unset(&self, key: &str) -> Result<bool> {
        let output = config_command()?
            .args(["--unset", key])
            .output()
            .context("Failed to execute git config --unset command")?;

        unset_outcome(&output, "Git config --unset failed")
    }

    /// Get a git config value from the current repository (respects local + global config).
    ///
    /// Always uses gitoxide for in-process config reading — no subprocess overhead.
    pub fn config_get(&self, key: &str) -> Result<Option<String>> {
        oxide::config_get(&self.gix_repo()?, key)
    }

    /// Set a git config value in global config
    pub fn config_set_global(&self, key: &str, value: &str) -> Result<()> {
        let output = config_command()?
            .args(["--global", key, value])
            .output()
            .context("Failed to execute git config --global command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git config --global failed: {}", stderr);
        }

        Ok(())
    }

    /// Unset a git config value in global config.
    ///
    /// The counterpart [`config_set_global`](Self::config_set_global) has
    /// always had, and the reason `daft config unset --global` can exist:
    /// without it, revealing an inherited value means editing `~/.gitconfig`
    /// by hand. Returns whether anything was removed.
    pub fn config_unset_global(&self, key: &str) -> Result<bool> {
        let output = config_command()?
            .args(["--global", "--unset", key])
            .output()
            .context("Failed to execute git config --global --unset command")?;

        unset_outcome(&output, "Git config --global --unset failed")
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

    /// Read a key from `dir`'s *local* config, scrubbed of ambient `GIT_*`.
    fn local_value(dir: &Path, key: &str) -> Option<String> {
        let output = git_at(dir)
            .args(["config", "--local", "--get", key])
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Regression: inside a git hook, an inherited `GIT_DIR` must not
    /// retarget a config **write** at the hook-calling repo.
    ///
    /// The read-side hazard is already documented on `config_get_from`; the
    /// write side is worse, because the value lands in a repo nobody will
    /// look at and this one keeps behaving as if it were never set.
    #[test]
    #[serial]
    fn config_writes_target_cwd_repo_not_inherited_git_dir() {
        let this = tempdir().unwrap();
        let hook = tempdir().unwrap();
        let this_path = this.path().canonicalize().unwrap();
        let hook_path = hook.path().canonicalize().unwrap();

        git_at(&this_path).args(["init", "-q"]).status().unwrap();
        git_at(&hook_path).args(["init", "-q"]).status().unwrap();

        let original_cwd = std::env::current_dir().ok();
        std::env::set_current_dir(&this_path).unwrap();
        unsafe { std::env::set_var("GIT_DIR", hook_path.join(".git")) };

        let git = GitCommand::new(true);
        let set = git.config_set("daft.autocd", "false");
        let unset_missing = git.config_unset("daft.remote");

        // Restore process state before asserting so a failure cannot strand
        // sibling serial tests.
        unsafe { std::env::remove_var("GIT_DIR") };
        if let Some(cwd) = original_cwd {
            let _ = std::env::set_current_dir(cwd);
        }

        set.unwrap();
        assert_eq!(
            local_value(&this_path, "daft.autocd").as_deref(),
            Some("false"),
            "the write must land in the cwd's repo"
        );
        assert_eq!(
            local_value(&hook_path, "daft.autocd"),
            None,
            "the write must not land in the inherited GIT_DIR's repo"
        );

        assert!(
            !unset_missing.unwrap(),
            "unsetting a key that was never set is not an error, just a no-op"
        );
    }

    /// Unsetting reports whether it removed anything, so `daft config unset`
    /// can say "nothing set here" instead of surfacing git's bare exit code.
    #[test]
    #[serial]
    fn config_unset_reports_whether_it_removed_anything() {
        let dir = tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        git_at(&path).args(["init", "-q"]).status().unwrap();

        let original_cwd = std::env::current_dir().ok();
        std::env::set_current_dir(&path).unwrap();

        let git = GitCommand::new(true);
        let outcome = git
            .config_set("daft.remote", "upstream")
            .and_then(|()| git.config_unset("daft.remote"))
            .and_then(|removed| {
                git.config_unset("daft.remote")
                    .map(|again| (removed, again))
            });

        if let Some(cwd) = original_cwd {
            let _ = std::env::set_current_dir(cwd);
        }

        let (removed, again) = outcome.unwrap();
        assert!(removed, "the key was set, so it was removed");
        assert!(!again, "the second unset had nothing to remove");
        assert_eq!(local_value(&path, "daft.remote"), None);
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
