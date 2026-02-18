use crate::git_oxide;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Once, OnceLock};

static GITOXIDE_NOTICE: Once = Once::new();

pub struct GitCommand {
    quiet: bool,
    use_gitoxide: bool,
    gix_repo: OnceLock<gix::ThreadSafeRepository>,
}

impl GitCommand {
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            use_gitoxide: false,
            gix_repo: OnceLock::new(),
        }
    }

    pub fn with_gitoxide(mut self, enabled: bool) -> Self {
        self.use_gitoxide = enabled;
        if enabled {
            GITOXIDE_NOTICE.call_once(|| {
                eprintln!("[experimental] Using gitoxide backend for git operations");
            });
        }
        self
    }

    /// Lazily discover and open the git repository via gitoxide.
    /// Returns a thread-local Repository handle.
    fn gix_repo(&self) -> Result<gix::Repository> {
        if let Some(ts) = self.gix_repo.get() {
            return Ok(ts.to_thread_local());
        }
        let cwd = std::env::current_dir().context("Failed to get current working directory")?;
        let ts = gix::ThreadSafeRepository::discover(&cwd)
            .context("Failed to discover git repository via gitoxide")?;
        // If another thread raced us via set(), that's fine - use whichever won
        let _ = self.gix_repo.set(ts);
        Ok(self
            .gix_repo
            .get()
            .expect("OnceLock should be set")
            .to_thread_local())
    }

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

    /// Set up the fetch refspec for a remote (required for bare repos to support upstream tracking)
    pub fn setup_fetch_refspec(&self, remote_name: &str) -> Result<()> {
        let refspec = format!("+refs/heads/*:refs/remotes/{remote_name}/*");
        self.config_set(&format!("remote.{remote_name}.fetch"), &refspec)
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

    pub fn worktree_add(&self, path: &Path, branch: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "add"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(path).arg(branch);

        let output = cmd
            .output()
            .context("Failed to execute git worktree add command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree add failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_add_orphan(&self, path: &Path, branch_name: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "add", "--orphan"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        // --orphan creates a worktree with an unborn branch, which is needed
        // for empty repos where the bare clone's HEAD already references the
        // default branch name (causing -b to fail with "already exists").
        cmd.arg("-b").arg(branch_name).arg(path);

        let output = cmd
            .output()
            .context("Failed to execute git worktree add command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree add failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_add_new_branch(
        &self,
        path: &Path,
        new_branch: &str,
        base_branch: &str,
    ) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "add"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(path).arg("-b").arg(new_branch).arg(base_branch);

        let output = cmd
            .output()
            .context("Failed to execute git worktree add command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree add failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "remove"]);

        if force {
            cmd.arg("--force");
        }

        cmd.arg(path);

        let output = cmd
            .output()
            .context("Failed to execute git worktree remove command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree remove failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_list_porcelain(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .output()
            .context("Failed to execute git worktree list command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree list failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git worktree list output")
    }

    /// Find the worktree path for a given branch name.
    /// Returns None if no worktree is checked out on that branch.
    pub fn find_worktree_for_branch(
        &self,
        branch_name: &str,
    ) -> Result<Option<std::path::PathBuf>> {
        let porcelain_output = self.worktree_list_porcelain()?;

        let mut current_path: Option<std::path::PathBuf> = None;

        for line in porcelain_output.lines() {
            if let Some(worktree_path) = line.strip_prefix("worktree ") {
                current_path = Some(std::path::PathBuf::from(worktree_path));
            } else if let Some(branch_ref) = line.strip_prefix("branch ") {
                if let Some(branch) = branch_ref.strip_prefix("refs/heads/") {
                    if branch == branch_name {
                        return Ok(current_path.take());
                    }
                }
                current_path = None;
            } else if line.is_empty() {
                current_path = None;
            }
        }

        Ok(None)
    }

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
            return git_oxide::branch_list_verbose(&self.gix_repo()?);
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

    pub fn for_each_ref(&self, format: &str, refs: &str) -> Result<String> {
        if self.use_gitoxide {
            return git_oxide::for_each_ref(&self.gix_repo()?, format, refs);
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

    pub fn show_ref_exists(&self, ref_name: &str) -> Result<bool> {
        if self.use_gitoxide {
            return git_oxide::show_ref_exists(&self.gix_repo()?, ref_name);
        }
        let output = Command::new("git")
            .args(["show-ref", "--verify", "--quiet", ref_name])
            .output()
            .context("Failed to execute git show-ref command")?;

        Ok(output.status.success())
    }

    pub fn ls_remote_heads(&self, remote: &str, branch: Option<&str>) -> Result<String> {
        if self.use_gitoxide {
            if let Ok(repo) = self.gix_repo() {
                return git_oxide::ls_remote_heads(&repo, remote, branch);
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

    pub fn get_git_dir(&self) -> Result<String> {
        if self.use_gitoxide {
            return git_oxide::get_git_dir(&self.gix_repo()?);
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

    pub fn rev_list_count(&self, range: &str) -> Result<u32> {
        if self.use_gitoxide {
            return git_oxide::rev_list_count(&self.gix_repo()?, range);
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

    /// Check if a specific worktree path has uncommitted or untracked changes.
    pub fn has_uncommitted_changes_in(&self, worktree_path: &Path) -> Result<bool> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(worktree_path)
            .output()
            .context("Failed to execute git status command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git status failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git status output")?;
        Ok(!stdout.trim().is_empty())
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
            return git_oxide::rev_parse_is_inside_work_tree(&self.gix_repo()?);
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
            return git_oxide::is_inside_git_repo();
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
            return git_oxide::rev_parse_git_common_dir(&self.gix_repo()?);
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

    /// Get the short name of the current branch
    pub fn symbolic_ref_short_head(&self) -> Result<String> {
        if self.use_gitoxide {
            return git_oxide::symbolic_ref_short_head(&self.gix_repo()?);
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

    /// Execute git ls-remote with symref to get remote HEAD
    pub fn ls_remote_symref(&self, remote_url: &str) -> Result<String> {
        if self.use_gitoxide {
            if let Ok(repo) = self.gix_repo() {
                return git_oxide::ls_remote_symref(&repo, remote_url);
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
                return git_oxide::ls_remote_branch_exists(&repo, remote_name, branch);
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

    /// Check if working directory has uncommitted or untracked changes
    pub fn has_uncommitted_changes(&self) -> Result<bool> {
        if self.use_gitoxide {
            let repo = self.gix_repo()?;
            // Fall back to subprocess if the cached repo is bare (no workdir).
            // This happens when the repo was discovered from the project root in
            // a bare-repo worktree layout (e.g., flow-eject changes CWD to the
            // project root, then later CDs into individual worktrees).
            if repo.workdir().is_some() {
                return git_oxide::has_uncommitted_changes(&repo);
            }
        }
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .context("Failed to execute git status command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git status failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git status output")?;
        Ok(!stdout.trim().is_empty())
    }

    /// Stash all changes including untracked files
    pub fn stash_push_with_untracked(&self, message: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "push", "-u", "-m", message]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash push command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash push failed: {}", stderr);
        }

        Ok(())
    }

    /// Pop the most recent stash
    pub fn stash_pop(&self) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "pop"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash pop command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash pop failed: {}", stderr);
        }

        Ok(())
    }

    /// Apply the top stash without removing it
    pub fn stash_apply(&self) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "apply"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash apply command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash apply failed: {}", stderr);
        }

        Ok(())
    }

    /// Drop the top stash entry
    pub fn stash_drop(&self) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "drop"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash drop command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash drop failed: {}", stderr);
        }

        Ok(())
    }

    /// Get a git config value from the current repository (respects local + global config)
    pub fn config_get(&self, key: &str) -> Result<Option<String>> {
        if self.use_gitoxide {
            return git_oxide::config_get(&self.gix_repo()?, key);
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
            return git_oxide::config_get_global(key);
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

    /// Get the path of the current worktree
    pub fn get_current_worktree_path(&self) -> Result<std::path::PathBuf> {
        if self.use_gitoxide {
            return git_oxide::get_current_worktree_path(&self.gix_repo()?);
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

    /// Resolve a target (worktree name or branch name) to a worktree path.
    /// Priority: worktree name (directory name) > branch name
    pub fn resolve_worktree_path(
        &self,
        target: &str,
        project_root: &Path,
    ) -> Result<std::path::PathBuf> {
        let porcelain_output = self.worktree_list_porcelain()?;

        // Parse worktree list porcelain output
        // Format:
        // worktree /path/to/worktree
        // HEAD <sha>
        // branch refs/heads/branch-name
        // <blank line>
        let mut worktrees: Vec<(std::path::PathBuf, Option<String>)> = Vec::new();
        let mut current_path: Option<std::path::PathBuf> = None;
        let mut current_branch: Option<String> = None;

        for line in porcelain_output.lines() {
            if let Some(worktree_path) = line.strip_prefix("worktree ") {
                // Save previous worktree if any
                if let Some(path) = current_path.take() {
                    worktrees.push((path, current_branch.take()));
                }
                current_path = Some(std::path::PathBuf::from(worktree_path));
                current_branch = None;
            } else if let Some(branch_ref) = line.strip_prefix("branch ") {
                // Extract branch name from refs/heads/branch-name
                current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
            }
        }
        // Don't forget the last worktree
        if let Some(path) = current_path.take() {
            worktrees.push((path, current_branch.take()));
        }

        // First, check if target is a path relative to project root (most precise)
        let potential_path = project_root.join(target);
        for (path, _) in &worktrees {
            if path == &potential_path {
                return Ok(path.clone());
            }
        }

        // Second, check if target matches a branch name
        for (path, branch) in &worktrees {
            if let Some(branch_name) = branch {
                if branch_name == target {
                    return Ok(path.clone());
                }
            }
        }

        // Third, check if target matches a worktree directory name (convenience shorthand)
        for (path, _) in &worktrees {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == target {
                    return Ok(path.clone());
                }
            }
        }

        anyhow::bail!("No worktree found for '{}'", target)
    }

    /// List all configured remotes.
    pub fn remote_list(&self) -> Result<Vec<String>> {
        if self.use_gitoxide {
            return git_oxide::remote_list(&self.gix_repo()?);
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

    /// Get the tracking remote for a branch.
    pub fn get_branch_tracking_remote(&self, branch: &str) -> Result<Option<String>> {
        let key = format!("branch.{branch}.remote");
        self.config_get(&key)
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

    /// Check if the repository is a bare repository.
    pub fn rev_parse_is_bare_repository(&self) -> Result<bool> {
        if self.use_gitoxide {
            return git_oxide::rev_parse_is_bare_repository(&self.gix_repo()?);
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

    /// Get the URL of a remote.
    pub fn remote_get_url(&self, remote: &str) -> Result<String> {
        if self.use_gitoxide {
            return git_oxide::remote_get_url(&self.gix_repo()?, remote);
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

    /// Move a worktree to a new location.
    pub fn worktree_move(&self, from: &Path, to: &Path) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "move"]);
        cmd.arg(from).arg(to);

        let output = cmd
            .output()
            .context("Failed to execute git worktree move command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree move failed: {}", stderr);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_command_new() {
        let git = GitCommand::new(true);
        assert!(git.quiet);
        assert!(!git.use_gitoxide);

        let git = GitCommand::new(false);
        assert!(!git.quiet);
        assert!(!git.use_gitoxide);
    }

    #[test]
    fn test_git_command_with_gitoxide() {
        let git = GitCommand::new(false).with_gitoxide(true);
        assert!(!git.quiet);
        assert!(git.use_gitoxide);

        let git = GitCommand::new(true).with_gitoxide(false);
        assert!(git.quiet);
        assert!(!git.use_gitoxide);
    }
}
