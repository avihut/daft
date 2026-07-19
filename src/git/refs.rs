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

    /// Walk `<base>..<target>` along first parents, yielding each commit with
    /// its tree, its parents, and the paths it changed against its first
    /// parent.
    ///
    /// Records are NUL-delimited: a path can never contain a NUL byte, so no
    /// filename can forge a record boundary. Rename detection is off so the
    /// path lists are plain set differences, directly comparable with
    /// [`Self::diff_name_only`].
    ///
    /// `--no-show-signature` is load-bearing, not hygiene: under a user's
    /// `log.showSignature = true` git prints the gpg verification block
    /// *before* the record's NUL, so those lines parse as filenames of the
    /// preceding commit and every file-set comparison fails. Paths decode
    /// lossily — they are only compared as opaque set members here, and the
    /// tree hash is what actually proves the merge, so a non-UTF-8 filename
    /// must not turn the probe into an error.
    pub fn first_parent_commits(&self, base: &str, target: &str) -> Result<Vec<FirstParentCommit>> {
        let output = Command::new("git")
            .args([
                "log",
                "--first-parent",
                "--diff-merges=first-parent",
                "--no-renames",
                "--no-show-signature",
                "--format=%x00%H %T %P",
                "--name-only",
                &format!("{base}..{target}"),
            ])
            .output()
            .context("Failed to execute git log command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git log failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_first_parent_log(&stdout))
    }

    /// Three-way merge `branch` into `base_commit` in memory (no worktree, no
    /// index), returning the resulting tree hash.
    ///
    /// `Ok(None)` covers every non-success exit. Git reports both a conflicted
    /// merge (exit 1, conflicted tree on stdout) and a revision it refuses
    /// (also exit 1, empty stdout) the same way, and for the squash probe both
    /// mean the same thing: this candidate is not the branch's squash. The
    /// probe only ever adds merged verdicts, so collapsing them fails toward
    /// "not merged".
    pub fn merge_tree_write_tree(&self, base_commit: &str, branch: &str) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["merge-tree", "--write-tree", base_commit, branch])
            .output()
            .context("Failed to execute git merge-tree command")?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git merge-tree output")?;
        Ok(stdout.lines().next().map(|line| line.trim().to_string()))
    }

    /// Paths that differ between two revisions, with rename detection off so
    /// the result is comparable with [`Self::first_parent_commits`]' lists.
    ///
    /// Decodes lossily for the same reason as [`Self::first_parent_commits`]:
    /// both sides of the comparison go through the same substitution, so a
    /// non-UTF-8 path still matches itself.
    pub fn diff_name_only(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let output = Command::new("git")
            .args(["diff", "--name-only", "--no-renames", from, to])
            .output()
            .context("Failed to execute git diff command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git diff failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }
}

/// One commit on a first-parent walk: its hash, its tree, its parent hashes,
/// and the paths it changed against its first parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstParentCommit {
    pub commit: String,
    pub tree: String,
    pub parents: Vec<String>,
    pub files: Vec<String>,
}

/// Parse the NUL-delimited `first_parent_commits` log. Pure, so the record
/// shape unit-tests without a repository.
///
/// Each record is `<hash> <tree> <parents...>`, a blank line, then one path
/// per line. Text before the first NUL (there is none in practice) is
/// skipped, and a record whose header lacks both a hash and a tree is dropped
/// rather than guessed at.
fn parse_first_parent_log(stdout: &str) -> Vec<FirstParentCommit> {
    stdout
        .split('\0')
        .skip(1)
        .filter_map(|record| {
            let mut lines = record.lines();
            let mut header = lines.next()?.split_whitespace();
            let commit = header.next()?.to_string();
            let tree = header.next()?.to_string();
            Some(FirstParentCommit {
                commit,
                tree,
                parents: header.map(str::to_string).collect(),
                files: lines
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_first_parent_log;
    use crate::git::GitCommand;
    use crate::test_support::CwdGuard;
    use crate::utils::git_command_at;
    use serial_test::serial;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    #[test]
    fn parses_records_with_trees_parents_and_files() {
        let log = "\0aaa111 ttt111 ppp111\n\nsrc/one.rs\nsrc/two.rs\n\0bbb222 ttt222 ppp222 ppp333\n\ndocs/x.md\n";
        let commits = parse_first_parent_log(log);

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].commit, "aaa111");
        assert_eq!(commits[0].tree, "ttt111");
        assert_eq!(commits[0].parents, vec!["ppp111"]);
        assert_eq!(commits[0].files, vec!["src/one.rs", "src/two.rs"]);
        // A merge commit carries both parents; the walk still sees one record.
        assert_eq!(commits[1].parents, vec!["ppp222", "ppp333"]);
        assert_eq!(commits[1].files, vec!["docs/x.md"]);
    }

    #[test]
    fn parses_root_commit_as_parentless() {
        let commits = parse_first_parent_log("\0aaa111 ttt111\n\nfirst.txt\n");
        assert_eq!(commits.len(), 1);
        assert!(commits[0].parents.is_empty());
    }

    #[test]
    fn parses_empty_log_and_file_less_commit() {
        assert!(parse_first_parent_log("").is_empty());
        // An empty commit has a header but no paths.
        let commits = parse_first_parent_log("\0aaa111 ttt111 ppp111\n");
        assert_eq!(commits.len(), 1);
        assert!(commits[0].files.is_empty());
    }

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

    /// Three signed commits on `main`, with `log.showSignature` on so git
    /// prints a verification block for each one.
    ///
    /// SSH signing (git >= 2.34) keeps this hermetic — no gpg agent, no
    /// keyring, all config local. Returns `None` when the toolchain cannot
    /// sign, so the test degrades to a no-op instead of failing for an
    /// environment reason.
    fn repo_with_signed_commits() -> Option<(tempfile::TempDir, PathBuf)> {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = dir.path().join("work");
        let key = dir.path().join("id");
        std::fs::create_dir_all(&work).unwrap();

        let keygen = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "test@test.com", "-f"])
            .arg(&key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !matches!(keygen, Ok(status) if status.success()) {
            return None;
        }

        let pub_key = key.with_extension("pub");
        let allowed = dir.path().join("allowed_signers");
        std::fs::write(
            &allowed,
            format!("test@test.com {}", std::fs::read_to_string(&pub_key).ok()?),
        )
        .ok()?;

        git_in(&work, &["init", "-b", "main"]);
        git_in(&work, &["config", "--local", "gpg.format", "ssh"]);
        git_in(
            &work,
            &["config", "--local", "user.signingkey", pub_key.to_str()?],
        );
        git_in(&work, &["config", "--local", "commit.gpgsign", "true"]);
        git_in(
            &work,
            &[
                "config",
                "--local",
                "gpg.ssh.allowedSignersFile",
                allowed.to_str()?,
            ],
        );
        git_in(&work, &["config", "--local", "log.showSignature", "true"]);

        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(work.join(name), name).unwrap();
            git_in(&work, &["add", "."]);
            // The first signed commit is the canary: if the toolchain refuses
            // to sign, `git_in`'s assert would fail the test for an
            // environment reason, so probe with a plain status first.
            let signed = git_command_at(&work)
                .args(["commit", "-m", name])
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .ok()?;
            if !signed.success() {
                return None;
            }
        }

        Some((dir, work))
    }

    /// #737 regression: a path that is not valid UTF-8 must not turn the
    /// squash probe into an error. Under `core.quotePath = false` git emits
    /// such a path's raw bytes, which a strict decode rejects — surfacing
    /// "could not verify merge status" where the branch would previously have
    /// been reported cleanly unmerged.
    ///
    /// Only runs where the filesystem accepts such a name: APFS/HFS+ reject
    /// non-UTF-8 filenames outright, so on macOS the scenario cannot exist.
    #[test]
    #[serial]
    #[cfg(target_os = "linux")]
    fn non_utf8_paths_do_not_error_the_walk() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let (_dir, work) = repo_with_remote();
        git_in(&work, &["config", "--local", "core.quotePath", "false"]);

        // 0xFF is not valid UTF-8 in any position.
        let raw = OsStr::from_bytes(b"bad-\xff-name.txt");
        if std::fs::write(work.join(raw), "x").is_err() {
            return;
        }
        git_in(&work, &["add", "."]);
        git_in(&work, &["commit", "-m", "add a non-utf8 path"]);

        let _guard = CwdGuard::enter(&work);
        let git = GitCommand::new(false);

        let commits = git
            .first_parent_commits("HEAD~1", "HEAD")
            .expect("the walk decodes lossily instead of erroring");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].files.len(), 1);

        // Both sides substitute identically, so the set comparison the probe
        // performs still matches the path against itself.
        let diffed = git
            .diff_name_only("HEAD~1", "HEAD")
            .expect("the diff decodes lossily too");
        assert_eq!(diffed, commits[0].files);
    }

    /// #737 regression: with `log.showSignature = true` git writes the
    /// signature verification block to *stdout*, ahead of the next record's
    /// NUL separator — so it lands in the previous commit's path list. The
    /// squash probe compares those path lists against the branch's own, so a
    /// contaminated list matches nothing and merge-tree detection silently
    /// stops finding squashes for every signed-commit user.
    #[test]
    #[serial]
    fn signature_verification_output_stays_out_of_the_path_lists() {
        let Some((_dir, work)) = repo_with_signed_commits() else {
            return;
        };
        let _guard = CwdGuard::enter(&work);

        let commits = GitCommand::new(false)
            .first_parent_commits("HEAD~2", "HEAD")
            .expect("first-parent walk succeeds");

        assert_eq!(commits.len(), 2, "two commits in HEAD~2..HEAD");
        // Newest first. Anything beyond the single path each commit touched is
        // signature text that leaked across the record boundary.
        assert_eq!(commits[0].files, vec!["c.txt"]);
        assert_eq!(commits[1].files, vec!["b.txt"]);
    }
}
