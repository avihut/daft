use super::GitCommand;
use super::oxide;
use crate::utils::git_command_at;
use anyhow::{Context, Result};
use std::path::Path;
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

    /// Count commits reachable from `rev` but absent from every tracking ref
    /// of `remote` (`git rev-list --count <rev> --not --remotes=<remote>`).
    ///
    /// Zero means a push of `rev` to `remote` is ref-only: every commit it
    /// would publish is already reachable from that remote's refs as of the
    /// last fetch. Runs at an explicit `cwd` via [`git_command_at`] so the
    /// answer comes from that worktree's repo even when daft itself runs
    /// inside a git hook. Subprocess-only by design — the gitoxide backend
    /// has no `--not --remotes` walk (same precedent as `merge_base` and
    /// `commit_tree` below).
    pub fn count_commits_not_on_remote(&self, rev: &str, remote: &str, cwd: &Path) -> Result<u64> {
        let output = git_command_at(cwd)
            .args([
                "rev-list",
                "--count",
                rev,
                "--not",
                &format!("--remotes={remote}"),
            ])
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
            .parse::<u64>()
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

    /// Resolve the merge base of two revisions.
    ///
    /// Returns `Ok(None)` when the revisions share no common ancestor
    /// (unrelated histories — `git merge-base` exits 1 with no output).
    /// Any other failure (bad revision, not a repository, ...) is an error.
    pub fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["merge-base", a, b])
            .output()
            .context("Failed to execute git merge-base command")?;

        if !output.status.success() {
            if output.status.code() == Some(1) {
                return Ok(None);
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git merge-base failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git merge-base output")?;
        Ok(Some(stdout.trim().to_string()))
    }

    /// Create an unreferenced commit wrapping `tree` with a single `parent`,
    /// returning the new commit hash.
    ///
    /// Used by squash-merge detection to synthesize a one-commit equivalent
    /// of a branch's cumulative diff. Identity and dates are injected via
    /// environment variables — identity so the probe works even where
    /// user.name/user.email are not configured, fixed epoch dates so
    /// repeated probes of the same tree+parent produce the same SHA instead
    /// of accumulating a new dangling object per run. The object is
    /// deliberately left unreferenced, and `git gc` sweeps it with other
    /// unreachable objects.
    pub fn commit_tree(&self, tree: &str, parent: &str, message: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["commit-tree", tree, "-p", parent, "-m", message])
            .env("GIT_AUTHOR_NAME", "daft")
            .env("GIT_AUTHOR_EMAIL", "daft@localhost")
            .env("GIT_AUTHOR_DATE", "1970-01-01T00:00:00+0000")
            .env("GIT_COMMITTER_NAME", "daft")
            .env("GIT_COMMITTER_EMAIL", "daft@localhost")
            .env("GIT_COMMITTER_DATE", "1970-01-01T00:00:00+0000")
            .output()
            .context("Failed to execute git commit-tree command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git commit-tree failed: {}", stderr);
        }

        String::from_utf8(output.stdout)
            .context("Failed to parse git commit-tree output")
            .map(|s| s.trim().to_string())
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

#[cfg(test)]
mod tests {
    use crate::git::GitCommand;
    use crate::utils::git_command_at;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    fn git_in(dir: &Path, args: &[&str]) {
        let status = git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("failed to spawn git");
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    /// Local bare remote + a one-commit clone on `main` with `origin` wired
    /// up (mirrors the fixture private to `core::worktree::push::tests`).
    fn repo_with_remote() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let remote = dir.path().join("remote.git");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&remote).unwrap();
        git_in(&remote, &["init", "--bare"]);
        std::fs::create_dir_all(&work).unwrap();
        git_in(&work, &["init", "-b", "main"]);
        git_in(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(work.join("a.txt"), "a").unwrap();
        git_in(&work, &["add", "."]);
        git_in(&work, &["commit", "-m", "init"]);
        (dir, work)
    }

    #[test]
    fn counts_zero_for_a_new_branch_at_a_pushed_tip() {
        let (_dir, work) = repo_with_remote();
        git_in(&work, &["push", "-u", "origin", "main"]);
        git_in(&work, &["branch", "feat"]);

        let count = GitCommand::new(false)
            .count_commits_not_on_remote("refs/heads/feat", "origin", &work)
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn counts_local_commits_missing_from_the_remote() {
        let (_dir, work) = repo_with_remote();
        git_in(&work, &["push", "-u", "origin", "main"]);
        std::fs::write(work.join("b.txt"), "b").unwrap();
        git_in(&work, &["add", "."]);
        git_in(&work, &["commit", "-m", "local work"]);

        let count = GitCommand::new(false)
            .count_commits_not_on_remote("refs/heads/main", "origin", &work)
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn counts_all_commits_when_the_remote_has_no_tracking_refs() {
        let (_dir, work) = repo_with_remote();

        let count = GitCommand::new(false)
            .count_commits_not_on_remote("refs/heads/main", "origin", &work)
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn scopes_the_count_to_the_target_remote() {
        let (_dir, work) = repo_with_remote();
        git_in(&work, &["push", "-u", "origin", "main"]);
        let second = work.parent().unwrap().join("second.git");
        std::fs::create_dir_all(&second).unwrap();
        git_in(&second, &["init", "--bare"]);
        git_in(
            &work,
            &["remote", "add", "second", second.to_str().unwrap()],
        );

        let count = GitCommand::new(false)
            .count_commits_not_on_remote("refs/heads/main", "second", &work)
            .unwrap();
        assert_eq!(count, 1, "commits on origin are still new to `second`");
    }
}
