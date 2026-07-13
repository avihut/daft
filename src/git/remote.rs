use super::GitCommand;
use super::cancel;
use super::oxide;
use crate::styles;
use crate::utils::git_command_at;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Which pipe a teed `git push` output line arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushStream {
    Stdout,
    Stderr,
}

/// Tee sink for live `git push` output lines (called from the pipe-drain
/// threads, hence `Sync`; lifetime-parametric so callers can borrow).
pub type PushOutputTee<'a> = dyn Fn(PushStream, &str) + Sync + 'a;

/// Options threaded through every push primitive into [`GitCommand::run_push`].
pub struct PushOptions<'a> {
    /// When `false`, pass `--no-verify` so git skips the repo's `pre-push`
    /// hook. Defaults to `true`: daft honors the hook (issue #599).
    pub verify: bool,
    /// Tee sink: every output line is forwarded here as it arrives, in
    /// addition to being captured in [`PushIo`]. Keeps the git layer free of
    /// presenter types — the composition layer bridges this to `JobPresenter`.
    pub on_output: Option<&'a PushOutputTee<'a>>,
}

impl Default for PushOptions<'_> {
    fn default() -> Self {
        Self {
            verify: true,
            on_output: None,
        }
    }
}

/// Captured result of a `git push` subprocess.
///
/// A push that ran and failed (hook rejection, non-fast-forward, transport)
/// is `Ok` with `success: false` so callers can inspect both streams before
/// deciding severity. `Err` means the push did not produce a verdict: the
/// subprocess could not be spawned, or — for a supervised push (sync) — it
/// was torn down mid-run, surfacing as the typed [`cancel::OperationCancelled`]
/// (cancel flag went active) or [`cancel::NeedsTerminal`] (job-control-stopped
/// on an interactive auth prompt). Callers branch on those types rather than
/// treating every `Err` as a spawn failure.
#[derive(Debug)]
pub struct PushIo {
    pub success: bool,
    /// Captured stdout: `--porcelain` ref-status lines plus any pre-push hook
    /// stdout (parse with [`crate::git::push_porcelain::parse_push_report`]).
    pub stdout: String,
    /// Captured stderr: hook stderr, transport errors, git diagnostics.
    pub stderr: String,
}

impl PushIo {
    /// Collapse into the legacy contract: bail with stderr when the push
    /// failed. For call sites that keep today's coarse error handling.
    pub fn into_result(self) -> Result<Self> {
        if self.success {
            Ok(self)
        } else {
            anyhow::bail!("Git push failed: {}", self.stderr);
        }
    }
}

/// A regular file with at least one executable bit set (git's own criterion
/// for whether a hook runs; a non-executable hook file is ignored).
fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Drain one pipe of the push subprocess line-by-line, teeing each line to
/// `on_output` (when set) and accumulating the full stream.
fn drain_push_pipe<R: Read>(
    pipe: R,
    stream: PushStream,
    on_output: Option<&PushOutputTee<'_>>,
) -> String {
    let mut captured = String::new();
    for line in BufReader::new(pipe).lines().map_while(Result::ok) {
        if let Some(tee) = on_output {
            tee(stream, &line);
        }
        captured.push_str(&line);
        captured.push('\n');
    }
    captured
}

impl GitCommand {
    pub fn fetch(&self, remote: &str, prune: bool) -> Result<()> {
        // git_command_at (not a raw `git`) scrubs any inherited GIT_DIR so
        // the fetch targets the cwd's repo — not the hook-calling repo when
        // sync runs inside a git hook — mirroring run_push's hardening.
        let cwd = std::env::current_dir().context("Failed to resolve current directory")?;
        let mut cmd = git_command_at(&cwd);
        cmd.args(["fetch", remote]);

        if prune {
            cmd.arg("--prune");
        }

        let output = cancel::output_with_cancel(&mut cmd, self.cancel_flag())
            .context("Failed to execute git fetch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git fetch failed: {}", stderr);
        }

        Ok(())
    }

    pub fn fetch_refspec(&self, remote: &str, refspec: &str) -> Result<()> {
        // git_command_at (not a raw `git`) scrubs any inherited GIT_DIR so the
        // fetch targets the cwd's repo — not the hook-calling repo when daft
        // runs inside a git hook (e.g. post-checkout) — mirroring `fetch`/
        // `run_push`. Load-bearing for forge PR checkout, whose fork fetch is
        // the whole mechanism.
        let cwd = std::env::current_dir().context("Failed to resolve current directory")?;
        let mut cmd = git_command_at(&cwd);
        cmd.args(["fetch", remote, refspec]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cancel::output_with_cancel(&mut cmd, self.cancel_flag())
            .context("Failed to execute git fetch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git fetch with refspec failed: {}", stderr);
        }

        Ok(())
    }

    /// Shared seam for every daft-initiated `git push` (issue #599).
    ///
    /// - Runs via [`git_command_at`] so `-C <cwd>` is authoritative even when
    ///   daft itself runs inside a git hook (inherited `GIT_DIR` is scrubbed),
    ///   and so the repo's `pre-push` hook fires in the right worktree.
    /// - Always passes `--porcelain`: the machine-stable ref-status report on
    ///   stdout is what callers parse (see `push_porcelain`). Never passes
    ///   `--quiet` — it suppresses those ref-status lines; quietness is a
    ///   display decision made by the capture/tee layer above.
    /// - `--no-verify` is added only when `opts.verify` is `false`. This is
    ///   the single place that literal may appear (grep-gated by test).
    fn run_push(&self, push_args: &[&str], cwd: &Path, opts: &PushOptions) -> Result<PushIo> {
        if self
            .cancel_flag()
            .is_some_and(cancel::CancelFlag::is_cancelled)
        {
            return Err(cancel::OperationCancelled.into());
        }

        let mut cmd = git_command_at(cwd);
        cmd.args(["push", "--porcelain"]);
        if !opts.verify {
            cmd.arg("--no-verify");
        }
        cmd.args(push_args);
        // Governor jobserver export (#678): the hook inherits git's env.
        if let Some(supervision) = self.push_supervision.as_ref() {
            for (key, value) in &supervision.env {
                cmd.env(key, value);
            }
        }

        // Isolated supervision: a supervised push (sync) runs in its own
        // process group so escalations tear the whole pre-push hook subtree
        // down by pgid, and a hook stage that job-control-stops freezes its
        // own group instead of daft's (#663); a background-group `/dev/tty`
        // read (interactive auth) surfaces as `NeedsTerminal`. Unsupervised
        // pushes (checkout-branch autopush, branch delete/rename) pass no
        // flag, so `supervise_command` keeps them in the caller's foreground
        // group and terminal auth prompts + Ctrl+C behave exactly as before.
        // The wait gates on the pipe drains: hook descendants inherit the
        // write-ends and can outlive git, so the loop must run until EOF or a
        // stopped/TERM-immune holder would wedge a join with nobody watching
        // the flag (the #663 wedge). All of that lives in `supervise_command`
        // — the one skeleton shared with the fetch/pull/rebase seams.
        // A fresh budget per push unit (see `PushSupervision::timeout`).
        let clock = self
            .push_supervision
            .as_ref()
            .and_then(|s| s.timeout)
            .map(|limit| std::sync::Arc::new(cancel::UnitClock::new(limit)));
        if let Some(clock) = &clock
            && let Some(on_clock) = self
                .push_supervision
                .as_ref()
                .and_then(|s| s.on_clock.as_ref())
        {
            on_clock(std::sync::Arc::clone(clock));
        }
        let supervise_opts = cancel::SuperviseOpts {
            mode: cancel::SupervisionMode::Isolated,
            on_spawn: self
                .push_supervision
                .as_ref()
                .and_then(|s| s.on_spawn.as_deref()),
            clock: clock.clone(),
        };
        let (verdict, stdout, stderr) = cancel::supervise_command(
            &mut cmd,
            self.cancel_flag(),
            supervise_opts,
            |pipe| drain_push_pipe(pipe, PushStream::Stdout, opts.on_output),
            |pipe| drain_push_pipe(pipe, PushStream::Stderr, opts.on_output),
        )
        .context("Failed to execute git push command")?;

        match verdict {
            cancel::Verdict::Completed(status) => Ok(PushIo {
                success: status.success(),
                stdout,
                stderr,
            }),
            cancel::Verdict::Cancelled => Err(cancel::OperationCancelled.into()),
            cancel::Verdict::StoppedOnTty => Err(cancel::NeedsTerminal.into()),
            cancel::Verdict::TimedOut => Err(cancel::OperationTimedOut {
                limit: clock.map(|c| c.limit()).unwrap_or_default(),
            }
            .into()),
        }
    }

    /// Absolute path of the repo's executable `pre-push` hook (as seen
    /// from `cwd`) — native or via `core.hooksPath` (lefthook, husky,
    /// pre-commit all register through one of those two mechanisms).
    /// `None` when no executable hook is installed.
    ///
    /// The resolved path is also the resource governor's profile identity
    /// (#678): its content hash keys the learned `hook_profiles` row.
    pub fn pre_push_hook_path(&self, cwd: &Path) -> Option<PathBuf> {
        let mut cmd = git_command_at(cwd);
        cmd.args(["rev-parse", "--git-path", "hooks"]);
        cmd.stdin(Stdio::null()).stderr(Stdio::null());
        let output = cmd.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let raw = String::from_utf8_lossy(&output.stdout);
        let rel = raw.trim();
        if rel.is_empty() {
            return None;
        }
        // `--git-path` prints relative to git's cwd (our `-C <cwd>`).
        let hooks_dir = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            cwd.join(rel)
        };
        let hook = hooks_dir.join("pre-push");
        is_executable_file(&hook).then_some(hook)
    }

    /// One `git push` for many branches (#678 batched strategy): the
    /// pre-push hook fires once with every ref on stdin. Callers attribute
    /// per-branch outcomes from the porcelain report on stdout.
    pub(crate) fn push_branches(
        &self,
        remote: &str,
        branches: &[String],
        force_with_lease: bool,
        cwd: &Path,
        opts: &PushOptions,
    ) -> Result<PushIo> {
        let mut args: Vec<&str> = Vec::with_capacity(branches.len() + 2);
        if force_with_lease {
            args.push("--force-with-lease");
        }
        args.push(remote);
        for branch in branches {
            args.push(branch);
        }
        self.run_push(&args, cwd, opts)
    }

    /// Whether the repo (as seen from `cwd`) has an executable `pre-push`
    /// hook installed. Used to existence-gate the synthetic `pre-push`
    /// reporting phase so a hook-less repo never renders a hollow phase
    /// header.
    pub fn pre_push_hook_exists(&self, cwd: &Path) -> bool {
        self.pre_push_hook_path(cwd).is_some()
    }

    /// Push a branch and set upstream, running from a specific directory.
    pub fn push_set_upstream_from(
        &self,
        remote: &str,
        branch: &str,
        cwd: &Path,
        opts: &PushOptions,
    ) -> Result<PushIo> {
        self.run_push(&["--set-upstream", remote, branch], cwd, opts)
    }

    pub fn set_upstream(&self, remote: &str, branch: &str) -> Result<()> {
        let upstream = format!("{remote}/{branch}");
        let output = Command::new("git")
            .args(["branch", &format!("--set-upstream-to={upstream}"), branch])
            .output()
            .context("Failed to execute git branch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git set upstream failed: {}", stderr);
        }

        Ok(())
    }

    /// Push a branch from a specific directory, optionally with --force-with-lease.
    ///
    /// Required for parallel workers where `set_current_dir` would race.
    pub fn push_from(
        &self,
        remote: &str,
        branch: &str,
        cwd: &Path,
        force_with_lease: bool,
        opts: &PushOptions,
    ) -> Result<PushIo> {
        if force_with_lease {
            self.run_push(&["--force-with-lease", remote, branch], cwd, opts)
        } else {
            self.run_push(&[remote, branch], cwd, opts)
        }
    }

    /// Delete a remote branch via `git push <remote> --delete <branch>`,
    /// running from a specific directory.
    pub fn push_delete_from(
        &self,
        remote: &str,
        branch: &str,
        cwd: &Path,
        opts: &PushOptions,
    ) -> Result<PushIo> {
        self.run_push(&[remote, "--delete", branch], cwd, opts)
    }

    /// Pull from remote with specified arguments
    pub fn pull(&self, args: &[&str]) -> Result<String> {
        self.pull_in(args, None)
    }

    /// Pull with an explicit working directory.
    ///
    /// When `dir` is `Some`, the git command runs in that directory instead
    /// of inheriting the process CWD. This is required for parallel workers
    /// where `set_current_dir` would race.
    pub fn pull_in(&self, args: &[&str], dir: Option<&Path>) -> Result<String> {
        // git_command_at scrubs inherited GIT_* so -C is authoritative even
        // inside a git hook. Parallel workers pass an explicit dir; the
        // no-dir path falls back to the process cwd.
        let mut cmd = match dir {
            Some(d) => git_command_at(d),
            None => git_command_at(
                &std::env::current_dir().context("Failed to resolve current directory")?,
            ),
        };

        // Force colored diff stats even when stdout is captured,
        // so the output renders correctly when printed to the terminal.
        if styles::colors_enabled() {
            cmd.args(["-c", "color.diff=always"]);
        }

        cmd.arg("pull");

        for arg in args {
            cmd.arg(arg);
        }

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cancel::output_with_cancel(&mut cmd, self.cancel_flag())
            .context("Failed to execute git pull command")?;

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
        if self.use_gitoxide
            && let Ok(repo) = self.gix_repo()
        {
            return oxide::ls_remote_heads(&repo, remote, branch);
        }
        // No local repo (e.g. during clone) — fall through to git CLI.
        // Routed through output_with_cancel so a supervised caller (sync's
        // gone-branch identification) can tear a stalled network ls-remote
        // down on the first Ctrl+C; unsupervised callers (clone) pass no
        // flag and get a classic blocking run.
        let mut cmd = Command::new("git");
        cmd.args(["ls-remote", "--heads", remote]);

        if let Some(branch) = branch {
            cmd.arg(format!("refs/heads/{branch}"));
        }

        let output = cancel::output_with_cancel(&mut cmd, self.cancel_flag())
            .context("Failed to execute git ls-remote command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git ls-remote failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git ls-remote output")
    }

    /// Execute git ls-remote with symref to get remote HEAD
    pub fn ls_remote_symref(&self, remote_url: &str) -> Result<String> {
        if self.use_gitoxide
            && let Ok(repo) = self.gix_repo()
        {
            return oxide::ls_remote_symref(&repo, remote_url);
        }
        // No local repo (e.g. during clone) — fall through to git CLI
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
        if self.use_gitoxide
            && let Ok(repo) = self.gix_repo()
        {
            return oxide::ls_remote_branch_exists(&repo, remote_name, branch);
        }
        // No local repo (e.g. during clone) — fall through to git CLI.
        // output_with_cancel so a supervised caller (sync gone-branch check)
        // can cancel a stalled network probe; unsupervised callers block.
        let mut cmd = Command::new("git");
        cmd.args([
            "ls-remote",
            "--heads",
            remote_name,
            &format!("refs/heads/{branch}"),
        ]);
        let output = cancel::output_with_cancel(&mut cmd, self.cancel_flag())
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
    /// Rebase the current branch onto `base`.
    ///
    /// Returns the combined stdout+stderr on success. On failure (e.g., conflicts),
    /// returns an error with the combined output.
    pub fn rebase(&self, base: &str) -> Result<String> {
        self.rebase_in(base, None, false)
    }

    /// Rebase with an explicit working directory.
    ///
    /// When `dir` is `Some`, the git command runs in that directory instead
    /// of inheriting the process CWD. Required for parallel workers.
    pub fn rebase_in(&self, base: &str, dir: Option<&Path>, autostash: bool) -> Result<String> {
        // git_command_at scrubs inherited GIT_* so -C is authoritative even
        // inside a git hook (parallel workers pass an explicit dir; the
        // no-dir path falls back to the process cwd).
        let mut cmd = match dir {
            Some(d) => git_command_at(d),
            None => git_command_at(
                &std::env::current_dir().context("Failed to resolve current directory")?,
            ),
        };
        cmd.args(["rebase", base]);
        if autostash {
            cmd.arg("--autostash");
        }

        let output = cancel::output_with_cancel(&mut cmd, self.cancel_flag())
            .context("Failed to execute git rebase command")?;

        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        if !output.status.success() {
            anyhow::bail!("{}", combined.trim());
        }

        Ok(combined)
    }

    /// Abort an in-progress rebase.
    pub fn rebase_abort(&self) -> Result<()> {
        self.rebase_abort_in(None)
    }

    /// Abort rebase with an explicit working directory.
    pub fn rebase_abort_in(&self, dir: Option<&Path>) -> Result<()> {
        let mut cmd = Command::new("git");
        if let Some(d) = dir {
            cmd.current_dir(d);
        }
        cmd.args(["rebase", "--abort"]);

        let output = cmd
            .output()
            .context("Failed to execute git rebase --abort command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git rebase --abort failed: {}", stderr);
        }

        Ok(())
    }

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

    /// Validate which branches from a list exist on the remote.
    /// Uses local refs when gitoxide is enabled (zero network), falls back to CLI.
    pub fn validate_branches_exist(
        &self,
        remote_name: &str,
        branches: &[String],
    ) -> Result<Vec<(String, bool)>> {
        if self.use_gitoxide
            && let Ok(repo) = self.gix_repo()
        {
            return branches
                .iter()
                .map(|b| {
                    oxide::validate_branch_in_remotes(&repo, remote_name, b)
                        .map(|exists| (b.clone(), exists))
                })
                .collect();
        }
        branches
            .iter()
            .map(|b| {
                self.ls_remote_branch_exists(remote_name, b)
                    .map(|exists| (b.clone(), exists))
            })
            .collect()
    }

    /// List all branches on a remote using local refs.
    /// Uses local refs when gitoxide is enabled (zero network), falls back to CLI.
    pub fn list_remote_branches(&self, remote_name: &str) -> Result<Vec<String>> {
        if self.use_gitoxide
            && let Ok(repo) = self.gix_repo()
        {
            return oxide::list_remote_branches_local(&repo, remote_name);
        }
        let output = self.ls_remote_heads(remote_name, None)?;
        Ok(output
            .lines()
            .filter_map(|line| {
                line.split('\t')
                    .nth(1)
                    .and_then(|r| r.strip_prefix("refs/heads/"))
                    .map(|s| s.to_string())
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn rs_files_under(dir: &Path, acc: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rs_files_under(&path, acc);
            } else if path.extension().is_some_and(|e| e == "rs") {
                acc.push(path);
            }
        }
    }

    /// #599 grep-gate: the no-verify push flag may appear only in
    /// `run_push`'s verify toggle. Every push must route through that seam —
    /// no primitive, call site, or raw `Command` may hardcode the bypass.
    #[test]
    fn no_verify_literal_only_in_run_push() {
        // Assembled at runtime so this test doesn't match itself. The
        // surrounding quotes keep git's unrelated no-verify-signatures
        // completion strings out of scope: only the exact quoted flag
        // literal is gated.
        let needle = format!("\"--no-{}\"", "verify");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files_under(&src, &mut files);
        assert!(
            files.len() > 100,
            "src/ walk looks broken ({} files)",
            files.len()
        );

        let this_file = Path::new(file!())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap();
        let mut offenders = Vec::new();
        let mut in_run_push_file = 0usize;
        for file in &files {
            let Ok(content) = std::fs::read_to_string(file) else {
                continue;
            };
            let count = content.matches(&needle).count();
            if count == 0 {
                continue;
            }
            if file.file_name().and_then(|n| n.to_str()) == Some(this_file)
                && file.parent().is_some_and(|p| p.ends_with("git"))
            {
                in_run_push_file = count;
            } else {
                offenders.push(file.display().to_string());
            }
        }

        assert!(
            offenders.is_empty(),
            "the no-verify push flag must only appear inside run_push (src/git/remote.rs); found in: {offenders:?}"
        );
        assert_eq!(
            in_run_push_file, 1,
            "expected exactly one no-verify occurrence in remote.rs (run_push's toggle)"
        );
    }
}
