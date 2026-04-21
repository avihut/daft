//! Core logic for `daft worktree-exec`.
//!
//! Target resolution, per-worktree command pipeline, scheduler, and the
//! `ExecReport` data type that the command layer renders. No IO to stdout
//! lives here; renderers are separate.

pub mod list_renderer;
pub mod windows_renderer;

use crate::executor::presenter::{JobPresenter, NullPresenter};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Cancellation level for in-flight exec runs.
///
/// 0 = running normally.
/// 1 = soft-cancel: children get SIGTERM; we wait for them to exit.
/// 2 = hard-cancel: children get SIGKILL.
///
/// Escalation is monotonic — the flag never goes down.
pub struct CancelFlag(AtomicUsize);

impl CancelFlag {
    pub fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    pub fn level(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.level() >= 1
    }

    pub fn escalate(&self) {
        // 0 → 1, 1 → 2, 2 → 2 (saturates). Atomic compare-and-swap so
        // concurrent escalations can't regress the level under contention.
        let _ = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |cur| {
                (cur < 2).then_some(cur + 1)
            });
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
fn terminate_child(child: &std::process::Child) {
    // Safety: `kill(2)` is signal-safe. Passing a PID of an already-reaped
    // child returns ESRCH, which we ignore — callers loop and `try_wait`
    // independently to detect exit.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate_child(_child: &std::process::Child) {
    // Windows: fall through to `child.kill()` on escalation.
}

/// A worktree that has been matched by the user's selectors and is ready
/// to receive commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub worktree_path: PathBuf,
    pub branch_name: String,
}

/// One command in a per-worktree pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    /// Direct argv exec, e.g. from `-- CMD ARGS…`. First element is the
    /// program; remaining are its arguments. Never touches a shell.
    Argv(Vec<String>),
    /// Shell-parsed string, e.g. from `-x 'CMD'`. Run via `$SHELL -c`,
    /// falling back to `sh -c` if `$SHELL` is unset.
    Shell(String),
}

impl CommandSpec {
    /// A short one-line representation of the command for UI display.
    pub fn display(&self) -> String {
        match self {
            CommandSpec::Argv(parts) => parts.join(" "),
            CommandSpec::Shell(s) => s.clone(),
        }
    }
}

/// How the scheduler fans work out across worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// All worktrees concurrently. Default.
    Parallel,
    /// One worktree at a time; stop on the first failing worktree.
    Sequential,
    /// One worktree at a time; continue through failures.
    KeepGoing,
}

/// Outcome of running the full pipeline on one worktree.
#[derive(Debug)]
pub struct WorktreeOutcome {
    pub target: ResolvedTarget,
    /// Index into the pipeline of the last command attempted (0-based). If
    /// the first command failed, this is 0. If all succeeded, this equals
    /// pipeline.len() - 1.
    pub last_command_index: usize,
    /// Exit code of the last command attempted. 0 when all succeeded.
    pub exit_code: i32,
    /// Wall-clock duration from spawn of first command to finish of last.
    pub elapsed: Duration,
    /// Captured stdout+stderr, truncated at `OUTPUT_CAP_BYTES` keeping the tail.
    pub captured_output: Vec<u8>,
    /// Whether the worktree was cancelled by SIGINT before finishing.
    pub cancelled: bool,
}

impl WorktreeOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0 && !self.cancelled
    }
}

/// The aggregate result of an exec invocation used by renderers and the
/// outer command layer.
#[derive(Debug, Default)]
pub struct ExecReport {
    pub outcomes: Vec<WorktreeOutcome>,
    pub orphan_branches_skipped: Vec<String>,
}

impl ExecReport {
    /// 0 if every worktree succeeded, 1 otherwise. Single-target
    /// pass-through never builds an `ExecReport`; it propagates the child
    /// exit code directly.
    pub fn aggregate_exit_code(&self) -> i32 {
        if self.outcomes.iter().all(|o| o.succeeded()) {
            0
        } else {
            1
        }
    }
}

/// Output-capture cap per worktree (ring buffer keeps the tail). Internal
/// constant — not user-configurable in v1.
pub const OUTPUT_CAP_BYTES: usize = 1024 * 1024; // 1 MiB

/// Byte tail-buffer: writes are appended; when total exceeds `cap`, the
/// oldest bytes are dropped so that only the last `cap` bytes remain.
///
/// Does not attempt to preserve UTF-8 boundaries. Callers that need string
/// output should `String::from_utf8_lossy(buf.tail())`.
pub struct TailBuffer {
    buf: Vec<u8>,
    cap: usize,
}

impl TailBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap.min(64 * 1024)),
            cap,
        }
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.cap {
            // New chunk alone fills (or overfills) the cap.
            let start = bytes.len() - self.cap;
            self.buf.clear();
            self.buf.extend_from_slice(&bytes[start..]);
            return;
        }
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > self.cap {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(..drop);
        }
    }

    pub fn tail(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// Minimal worktree view that `resolve_targets` needs. In production we
/// build this from `crate::core::worktree::prune::parse_worktree_list`,
/// which already gives us `(path, branch)`.
#[derive(Clone, Debug)]
pub struct WorktreeSnapshot {
    pub path: std::path::PathBuf,
    pub branch: Option<String>,
}

impl WorktreeSnapshot {
    /// True if this snapshot represents a branch that has a real checked-out
    /// worktree. Orphan branches (branch exists in refs but no worktree)
    /// are represented with `path` pointing to an unusable sentinel.
    pub fn has_worktree(&self) -> bool {
        // In production, `parse_worktree_list` only ever returns real
        // worktrees; orphan branches are collected separately and fed in
        // with the sentinel path "::orphan::" from the command-layer
        // helper. Resolve on the sentinel to distinguish the two.
        self.path.to_str() != Some("::orphan::")
    }
}

fn is_glob(tok: &str) -> bool {
    tok.contains('*') || tok.contains('?') || tok.contains('[')
}

/// Errors surfaced during target resolution. Kept as a plain enum so
/// callers render friendly messages rather than parsing anyhow strings.
#[derive(Debug)]
pub enum ResolveError {
    NoTargets,
    AllWithPositionals,
    Unmatched(Vec<String>),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NoTargets => {
                write!(
                    f,
                    "no targets: pass one or more branch/dir names, a glob, or --all"
                )
            }
            ResolveError::AllWithPositionals => {
                write!(f, "--all cannot be combined with positional targets")
            }
            ResolveError::Unmatched(toks) => {
                write!(f, "no worktree matched: {}", toks.join(", "))
            }
        }
    }
}

impl std::error::Error for ResolveError {}

pub fn resolve_targets(
    positionals: &[String],
    all: bool,
    worktrees: &[WorktreeSnapshot],
) -> Result<Vec<ResolvedTarget>, ResolveError> {
    if positionals.is_empty() && !all {
        return Err(ResolveError::NoTargets);
    }
    if !positionals.is_empty() && all {
        return Err(ResolveError::AllWithPositionals);
    }

    if all {
        let mut out: Vec<ResolvedTarget> = worktrees
            .iter()
            .filter_map(|w| {
                w.branch.as_ref().map(|b| ResolvedTarget {
                    worktree_path: w.path.clone(),
                    branch_name: b.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.branch_name.cmp(&b.branch_name));
        return Ok(out);
    }

    let mut out: Vec<ResolvedTarget> = Vec::new();
    let mut seen_paths: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut unmatched: Vec<String> = Vec::new();

    for tok in positionals {
        let mut matched = false;

        // 1. Exact branch match.
        for wt in worktrees {
            if let Some(branch) = &wt.branch {
                if branch == tok {
                    matched = true;
                    if seen_paths.insert(wt.path.clone()) {
                        out.push(ResolvedTarget {
                            worktree_path: wt.path.clone(),
                            branch_name: branch.clone(),
                        });
                    }
                    break;
                }
            }
        }
        if matched {
            continue;
        }

        // 2. Exact directory-name match.
        for wt in worktrees {
            let dir_name = wt.path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if dir_name == tok {
                if let Some(branch) = &wt.branch {
                    matched = true;
                    if seen_paths.insert(wt.path.clone()) {
                        out.push(ResolvedTarget {
                            worktree_path: wt.path.clone(),
                            branch_name: branch.clone(),
                        });
                    }
                    break;
                }
            }
        }

        if !matched {
            unmatched.push(tok.clone());
        }
    }

    if !unmatched.is_empty() {
        return Err(ResolveError::Unmatched(unmatched));
    }

    Ok(out)
}

/// Like `resolve_targets`, but supports globs and reports orphan
/// branches (branches matched by a glob that have no worktree). Orphans
/// are surfaced so the renderer can print a one-line warning; they do not
/// count as unmatched for error purposes *unless* the glob matched nothing
/// actionable at all (only orphans, or nothing).
pub fn resolve_targets_with_orphans(
    positionals: &[String],
    all: bool,
    worktrees: &[WorktreeSnapshot],
) -> Result<(Vec<ResolvedTarget>, Vec<String>), ResolveError> {
    if positionals.is_empty() && !all {
        return Err(ResolveError::NoTargets);
    }
    if !positionals.is_empty() && all {
        return Err(ResolveError::AllWithPositionals);
    }

    if all {
        let mut out: Vec<ResolvedTarget> = worktrees
            .iter()
            .filter(|w| w.has_worktree())
            .filter_map(|w| {
                w.branch.as_ref().map(|b| ResolvedTarget {
                    worktree_path: w.path.clone(),
                    branch_name: b.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.branch_name.cmp(&b.branch_name));
        return Ok((out, Vec::new()));
    }

    use globset::{Glob, GlobSetBuilder};
    let mut out: Vec<ResolvedTarget> = Vec::new();
    let mut seen_paths: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut orphans: Vec<String> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();

    for tok in positionals {
        if is_glob(tok) {
            let glob = match Glob::new(tok) {
                Ok(g) => g,
                Err(_) => {
                    unmatched.push(tok.clone());
                    continue;
                }
            };
            let mut set_builder = GlobSetBuilder::new();
            set_builder.add(glob);
            let set = match set_builder.build() {
                Ok(s) => s,
                Err(_) => {
                    unmatched.push(tok.clone());
                    continue;
                }
            };

            let mut snapshot_branches: Vec<(&WorktreeSnapshot, &String)> = worktrees
                .iter()
                .filter_map(|w| w.branch.as_ref().map(|b| (w, b)))
                .filter(|(_, b)| set.is_match(b))
                .collect();
            snapshot_branches.sort_by(|a, b| a.1.cmp(b.1));

            let mut actionable_this_glob: usize = 0;
            let mut orphans_this_glob: Vec<String> = Vec::new();

            for (wt, branch) in snapshot_branches {
                if !wt.has_worktree() {
                    orphans_this_glob.push(branch.clone());
                } else if seen_paths.insert(wt.path.clone()) {
                    out.push(ResolvedTarget {
                        worktree_path: wt.path.clone(),
                        branch_name: branch.clone(),
                    });
                    actionable_this_glob += 1;
                } else {
                    // Already pulled in by an earlier positional; still
                    // counts as "this glob produced something actionable."
                    actionable_this_glob += 1;
                }
            }

            if actionable_this_glob == 0 {
                // Either matched nothing, or matched only orphans. Both
                // are errors; don't report orphans as "skipped" because
                // the run isn't happening.
                unmatched.push(tok.clone());
            } else {
                orphans.extend(orphans_this_glob);
            }
            continue;
        }

        // Exact fallthrough: reuse the non-glob resolver's logic.
        let sub = resolve_targets(std::slice::from_ref(tok), false, worktrees);
        match sub {
            Ok(exact) => {
                for t in exact {
                    if seen_paths.insert(t.worktree_path.clone()) {
                        out.push(t);
                    }
                }
            }
            Err(ResolveError::Unmatched(ref toks)) => {
                unmatched.extend_from_slice(toks);
            }
            Err(other) => return Err(other),
        }
    }

    if !unmatched.is_empty() {
        return Err(ResolveError::Unmatched(unmatched));
    }

    Ok((out, orphans))
}

/// Build a `Vec<WorktreeSnapshot>` from the repo's current worktree list
/// plus all local branches. Branches without an associated worktree are
/// included as orphan snapshots (sentinel path "::orphan::"), which
/// `resolve_targets_with_orphans` filters during glob expansion.
pub fn collect_snapshot(git: &crate::git::GitCommand) -> anyhow::Result<Vec<WorktreeSnapshot>> {
    use crate::core::worktree::prune::parse_worktree_list;

    let wt_entries = parse_worktree_list(git)?;
    let mut snaps: Vec<WorktreeSnapshot> = Vec::new();
    let mut branches_with_worktrees: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for entry in &wt_entries {
        if let Some(branch) = &entry.branch {
            branches_with_worktrees.insert(branch.clone());
            snaps.push(WorktreeSnapshot {
                path: entry.path.clone(),
                branch: Some(branch.clone()),
            });
        }
    }

    // Orphan branches: local branches that have no worktree. These are
    // only included for glob-expansion reporting; they carry the
    // "::orphan::" sentinel so `has_worktree()` returns false.
    let branch_output = git
        .for_each_ref("%(refname:short)", "refs/heads/")
        .unwrap_or_default();
    for line in branch_output.lines() {
        let branch = line.trim();
        if branch.is_empty() {
            continue;
        }
        if !branches_with_worktrees.contains(branch) {
            snaps.push(WorktreeSnapshot {
                path: std::path::PathBuf::from("::orphan::"),
                branch: Some(branch.to_string()),
            });
        }
    }

    Ok(snaps)
}

/// Run the pipeline against a single worktree with stdout+stderr captured
/// into a tail buffer. Stops on first failing command. Does not render any
/// UI — pure function returning a `WorktreeOutcome`.
pub fn run_pipeline(
    target: &ResolvedTarget,
    pipeline: &[CommandSpec],
    cancel: &CancelFlag,
) -> anyhow::Result<WorktreeOutcome> {
    let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
    run_pipeline_streaming(target, pipeline, "", &presenter, cancel)
}

/// Run the pipeline against a single worktree, streaming every stdout/stderr
/// line to the given presenter *and* capturing it into a tail buffer. Stops
/// on first failing command. Job names presented are `{name_prefix}` for
/// single-command pipelines or `{name_prefix} [{i+1}/{n}]` for multi-command
/// pipelines; when `name_prefix` is empty, the branch name is used.
pub fn run_pipeline_streaming(
    target: &ResolvedTarget,
    pipeline: &[CommandSpec],
    name_prefix: &str,
    presenter: &Arc<dyn JobPresenter>,
    cancel: &CancelFlag,
) -> anyhow::Result<WorktreeOutcome> {
    let start = Instant::now();
    let tail = Arc::new(Mutex::new(TailBuffer::new(OUTPUT_CAP_BYTES)));
    let mut exit_code: i32 = 0;
    let mut last_index: usize = 0;
    let mut cancelled = false;
    let base_name: &str = if name_prefix.is_empty() {
        target.branch_name.as_str()
    } else {
        name_prefix
    };

    for (idx, spec) in pipeline.iter().enumerate() {
        // Between commands: if cancel has been requested, stop before
        // launching the next command.
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }

        last_index = idx;
        let job_name = if pipeline.len() > 1 {
            format!("{} [{}/{}]", base_name, idx + 1, pipeline.len())
        } else {
            base_name.to_string()
        };
        let preview = spec.display();
        let cmd_start = Instant::now();
        presenter.on_job_start(&job_name, None, Some(&preview));

        let mut cmd = build_command(spec);
        cmd.current_dir(&target.worktree_path)
            .env("DAFT_WORKTREE_PATH", &target.worktree_path)
            .env("DAFT_BRANCH_NAME", &target.branch_name)
            .env("DAFT_COMMAND", "exec")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Ok(root) = crate::get_project_root() {
            cmd.env("DAFT_PROJECT_ROOT", &root);
        }
        if let Ok(gd) = crate::get_git_common_dir() {
            cmd.env("DAFT_GIT_DIR", &gd);
        }

        let mut child = cmd.spawn()?;
        let out = child.stdout.take().expect("stdout piped");
        let err = child.stderr.take().expect("stderr piped");

        let stdout_thread = spawn_stream_reader(out, Arc::clone(&tail), presenter, &job_name);
        let stderr_thread = spawn_stream_reader(err, Arc::clone(&tail), presenter, &job_name);

        // Poll for child exit while watching for cancel escalation so we
        // can deliver SIGTERM on first SIGINT and SIGKILL on second.
        // `child.wait()` would block and ignore our flag until exit.
        let mut sent_term = false;
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            match cancel.level() {
                0 => {}
                1 => {
                    if !sent_term {
                        terminate_child(&child);
                        sent_term = true;
                    }
                }
                _ => {
                    // Hard-cancel: SIGKILL on unix, TerminateProcess on
                    // windows (both via std's `child.kill()`).
                    let _ = child.kill();
                }
            }
            thread::sleep(Duration::from_millis(50));
        };
        stdout_thread.join().ok();
        stderr_thread.join().ok();
        exit_code = status.code().unwrap_or(-1);

        let cmd_elapsed = cmd_start.elapsed();
        if cancel.is_cancelled() {
            cancelled = true;
            presenter.on_job_failure(&job_name, cmd_elapsed);
            break;
        }
        if exit_code == 0 {
            presenter.on_job_success(&job_name, cmd_elapsed);
        } else {
            presenter.on_job_failure(&job_name, cmd_elapsed);
            break;
        }
    }

    let captured = Arc::try_unwrap(tail)
        .map(|m| m.into_inner().expect("tail mutex poisoned"))
        .unwrap_or_else(|arc| {
            let guard = arc.lock().expect("tail mutex poisoned");
            TailBuffer {
                buf: guard.buf.clone(),
                cap: guard.cap,
            }
        })
        .into_inner();

    Ok(WorktreeOutcome {
        target: target.clone(),
        last_command_index: last_index,
        exit_code,
        elapsed: start.elapsed(),
        captured_output: captured,
        cancelled,
    })
}

/// Spawn a thread that reads `reader` line-by-line, forwarding each line to
/// `presenter.on_job_output(job_name, line)` and appending the bytes
/// (including the trailing newline) to `tail`.
fn spawn_stream_reader<R>(
    reader: R,
    tail: Arc<Mutex<TailBuffer>>,
    presenter: &Arc<dyn JobPresenter>,
    job_name: &str,
) -> thread::JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    let presenter = Arc::clone(presenter);
    let name = job_name.to_string();
    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(mut t) = tail.lock() {
                        t.extend(line.as_bytes());
                    }
                    let trimmed = line.strip_suffix('\n').unwrap_or(&line);
                    let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);
                    presenter.on_job_output(&name, trimmed);
                }
                Err(_) => break,
            }
        }
    })
}

fn build_command(spec: &CommandSpec) -> Command {
    match spec {
        CommandSpec::Argv(parts) => {
            let mut c = Command::new(&parts[0]);
            if parts.len() > 1 {
                c.args(&parts[1..]);
            }
            c
        }
        CommandSpec::Shell(s) => {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
            let mut c = Command::new(shell);
            c.arg("-c").arg(s);
            c
        }
    }
}

/// Run the pipeline across all targets in the requested mode. Returns the
/// aggregated report. Rendering is the caller's responsibility.
pub fn run_scheduler(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    mode: ExecMode,
    cancel: &CancelFlag,
) -> anyhow::Result<ExecReport> {
    match mode {
        ExecMode::Parallel => run_parallel(targets, pipeline, cancel),
        ExecMode::Sequential => run_sequential(targets, pipeline, false, cancel),
        ExecMode::KeepGoing => run_sequential(targets, pipeline, true, cancel),
    }
}

fn run_parallel(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    cancel: &CancelFlag,
) -> anyhow::Result<ExecReport> {
    // `thread::scope` lets workers borrow `cancel` and `pipeline` without
    // requiring `'static`, so SIGINT escalations on the caller's flag
    // propagate live into every worker.
    let outcomes = thread::scope(|scope| -> anyhow::Result<Vec<WorktreeOutcome>> {
        let handles: Vec<_> = targets
            .iter()
            .map(|t| scope.spawn(move || run_pipeline(t, pipeline, cancel)))
            .collect();

        let mut out: Vec<WorktreeOutcome> = Vec::new();
        for h in handles {
            match h.join() {
                Ok(Ok(o)) => out.push(o),
                Ok(Err(e)) => return Err(e),
                Err(panic) => return Err(anyhow::anyhow!("worker thread panicked: {:?}", panic)),
            }
        }
        Ok(out)
    })?;

    // Preserve targets' resolved order for stable UI rendering.
    let mut outcomes = outcomes;
    outcomes.sort_by_key(|o| {
        targets
            .iter()
            .position(|t| t.worktree_path == o.target.worktree_path)
            .unwrap_or(usize::MAX)
    });

    Ok(ExecReport {
        outcomes,
        orphan_branches_skipped: Vec::new(),
    })
}

fn run_sequential(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    keep_going: bool,
    cancel: &CancelFlag,
) -> anyhow::Result<ExecReport> {
    let mut outcomes = Vec::with_capacity(targets.len());
    for t in targets {
        if cancel.is_cancelled() {
            break;
        }
        let outcome = run_pipeline(t, pipeline, cancel)?;
        let succeeded = outcome.succeeded();
        outcomes.push(outcome);
        if !succeeded && !keep_going {
            break;
        }
    }
    Ok(ExecReport {
        outcomes,
        orphan_branches_skipped: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_everything_under_cap() {
        let mut r = TailBuffer::new(16);
        r.extend(b"hello world");
        assert_eq!(r.tail(), b"hello world");
    }

    #[test]
    fn ring_keeps_only_tail_over_cap() {
        let mut r = TailBuffer::new(4);
        r.extend(b"abcdefghij");
        assert_eq!(r.tail(), b"ghij");
    }

    #[test]
    fn ring_exact_cap_boundary() {
        let mut r = TailBuffer::new(4);
        r.extend(b"abcd");
        assert_eq!(r.tail(), b"abcd");
        r.extend(b"e");
        assert_eq!(r.tail(), b"bcde");
    }

    #[test]
    fn ring_multi_extend_accumulates_tail() {
        let mut r = TailBuffer::new(5);
        r.extend(b"aaa");
        r.extend(b"bbb");
        r.extend(b"ccc");
        assert_eq!(r.tail(), b"bbccc");
    }

    use std::path::PathBuf;

    /// A lean snapshot of a worktree for resolver tests. Produced from
    /// `parse_worktree_list` in production; built by hand in tests.
    fn snap(path: &str, branch: &str) -> WorktreeSnapshot {
        WorktreeSnapshot {
            path: PathBuf::from(path),
            branch: Some(branch.to_string()),
        }
    }

    fn all_worktrees() -> Vec<WorktreeSnapshot> {
        vec![
            snap("/r/master", "master"),
            snap("/r/feat-a", "feat/a"),
            snap("/r/feat-b", "feat/b"),
            snap("/r/fix-crash", "fix/crash"),
        ]
    }

    #[test]
    fn exact_branch_name_resolves() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/a".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch_name, "feat/a");
        assert_eq!(got[0].worktree_path, PathBuf::from("/r/feat-a"));
    }

    #[test]
    fn exact_dir_name_resolves_when_branch_miss() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat-b".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch_name, "feat/b");
    }

    #[test]
    fn branch_takes_precedence_over_dir_name_collision() {
        // A worktree whose dir name collides with another worktree's branch.
        let wts = vec![
            snap("/r/x", "branch-x"), // dir name "x", branch "branch-x"
            snap("/r/y", "x"),        // branch "x", dir name "y"
        ];
        let got = resolve_targets(&["x".into()], false, &wts).unwrap();
        assert_eq!(got[0].branch_name, "x");
        assert_eq!(got[0].worktree_path, PathBuf::from("/r/y"));
    }

    #[test]
    fn dedupe_by_path() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/a".into(), "feat-a".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn preserves_user_order() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/b".into(), "feat/a".into()], false, &wts).unwrap();
        assert_eq!(got[0].branch_name, "feat/b");
        assert_eq!(got[1].branch_name, "feat/a");
    }

    fn snap_no_wt(branch: &str) -> WorktreeSnapshot {
        // A branch that exists in the repo but has no worktree checked out.
        // For test purposes we represent this as a snapshot whose path is
        // a sentinel — `has_worktree()` returns false for this path.
        WorktreeSnapshot {
            path: PathBuf::from("::orphan::"),
            branch: Some(branch.to_string()),
        }
    }

    fn all_with_orphans() -> Vec<WorktreeSnapshot> {
        let mut v = all_worktrees();
        v.push(snap_no_wt("feat/orphan-1"));
        v.push(snap_no_wt("feat/orphan-2"));
        v
    }

    fn resolve_report(
        positionals: &[String],
        all: bool,
        worktrees: &[WorktreeSnapshot],
    ) -> (Vec<ResolvedTarget>, Vec<String>) {
        let (tgts, orphans) = resolve_targets_with_orphans(positionals, all, worktrees).unwrap();
        (tgts, orphans)
    }

    #[test]
    fn glob_matches_branches_alphabetically() {
        let wts = all_worktrees();
        let (got, orphans) = resolve_report(&["feat/*".into()], false, &wts);
        assert_eq!(
            got.iter()
                .map(|t| t.branch_name.as_str())
                .collect::<Vec<_>>(),
            vec!["feat/a", "feat/b"]
        );
        assert!(orphans.is_empty());
    }

    #[test]
    fn glob_skips_orphan_branches_but_reports_them() {
        let wts = all_with_orphans();
        let (got, orphans) = resolve_report(&["feat/*".into()], false, &wts);
        let branches: Vec<_> = got.iter().map(|t| t.branch_name.as_str()).collect();
        assert_eq!(branches, vec!["feat/a", "feat/b"]);
        assert_eq!(orphans, vec!["feat/orphan-1", "feat/orphan-2"]);
    }

    #[test]
    fn glob_that_matches_nothing_is_error() {
        let wts = all_worktrees();
        let err = resolve_targets_with_orphans(&["zzz*".into()], false, &wts).unwrap_err();
        assert!(matches!(err, ResolveError::Unmatched(_)));
    }

    #[test]
    fn glob_that_only_matches_orphans_is_error_not_silent_skip() {
        // "feat/orphan-*" only matches branches with no worktree; nothing
        // actionable, so we error rather than run zero commands silently.
        let wts = all_with_orphans();
        let err = resolve_targets_with_orphans(&["feat/orphan-*".into()], false, &wts).unwrap_err();
        assert!(matches!(err, ResolveError::Unmatched(_)));
    }

    use tempfile::TempDir;

    fn dummy_target(dir: &TempDir, branch: &str) -> ResolvedTarget {
        ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: branch.to_string(),
        }
    }

    #[test]
    fn runs_single_argv_command_captures_output() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let spec = CommandSpec::Argv(vec!["echo".into(), "hi".into()]);
        let outcome = run_pipeline(&target, &[spec], &CancelFlag::new()).unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert!(
            String::from_utf8_lossy(&outcome.captured_output).contains("hi"),
            "captured: {:?}",
            outcome.captured_output
        );
    }

    #[test]
    fn stops_pipeline_on_first_failure() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let pipeline = vec![
            CommandSpec::Argv(vec!["false".into()]),
            CommandSpec::Argv(vec!["echo".into(), "should-not-run".into()]),
        ];
        let outcome = run_pipeline(&target, &pipeline, &CancelFlag::new()).unwrap();
        assert_ne!(outcome.exit_code, 0);
        assert_eq!(outcome.last_command_index, 0);
        assert!(
            !String::from_utf8_lossy(&outcome.captured_output).contains("should-not-run"),
            "second command must not run after the first failed"
        );
    }

    #[test]
    fn cwd_is_worktree_path() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let spec = CommandSpec::Argv(vec!["pwd".into()]);
        let outcome = run_pipeline(&target, &[spec], &CancelFlag::new()).unwrap();
        let out = String::from_utf8_lossy(&outcome.captured_output);
        let expected = dir.path().canonicalize().unwrap();
        let got = std::path::PathBuf::from(out.trim()).canonicalize().unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn injects_env_vars() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "feat/abc");
        let spec = CommandSpec::Argv(vec![
            "sh".into(),
            "-c".into(),
            "printf %s \"$DAFT_BRANCH_NAME\"".into(),
        ]);
        let outcome = run_pipeline(&target, &[spec], &CancelFlag::new()).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&outcome.captured_output).trim(),
            "feat/abc"
        );
    }

    #[test]
    fn shell_form_runs_via_sh() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        // Pipeline + env expansion only works in the shell form.
        let spec = CommandSpec::Shell("echo $((1+2))".into());
        let outcome = run_pipeline(&target, &[spec], &CancelFlag::new()).unwrap();
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&outcome.captured_output).trim(),
            "3"
        );
    }

    #[test]
    fn parallel_runs_all_targets_and_aggregates() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget {
                worktree_path: dir1.path().into(),
                branch_name: "a".into(),
            },
            ResolvedTarget {
                worktree_path: dir2.path().into(),
                branch_name: "b".into(),
            },
        ];
        let pipeline = vec![CommandSpec::Argv(vec!["echo".into(), "ok".into()])];
        let report =
            run_scheduler(&targets, &pipeline, ExecMode::Parallel, &CancelFlag::new()).unwrap();
        assert_eq!(report.outcomes.len(), 2);
        assert!(report.outcomes.iter().all(|o| o.succeeded()));
        assert_eq!(report.aggregate_exit_code(), 0);
    }

    #[test]
    fn parallel_reports_failures_via_aggregate() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget {
                worktree_path: dir1.path().into(),
                branch_name: "a".into(),
            },
            ResolvedTarget {
                worktree_path: dir2.path().into(),
                branch_name: "b".into(),
            },
        ];
        let pipeline = vec![CommandSpec::Argv(vec!["false".into()])];
        let report =
            run_scheduler(&targets, &pipeline, ExecMode::Parallel, &CancelFlag::new()).unwrap();
        assert_eq!(report.aggregate_exit_code(), 1);
        assert!(report.outcomes.iter().all(|o| !o.succeeded()));
    }

    #[test]
    fn shell_form_does_not_use_interactive_flag() {
        // Regression guard: the legacy src/exec.rs passes -i which loads
        // rcfiles. `daft exec -x` must NOT do that — the test passes an
        // env var with a value that an rcfile would likely clobber, and
        // asserts we see the outer env.
        std::env::set_var("DAFT_EXEC_TEST_MARKER", "outer-value");
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let spec = CommandSpec::Shell("echo $DAFT_EXEC_TEST_MARKER".into());
        let outcome = run_pipeline(&target, &[spec], &CancelFlag::new()).unwrap();
        std::env::remove_var("DAFT_EXEC_TEST_MARKER");
        assert_eq!(
            String::from_utf8_lossy(&outcome.captured_output).trim(),
            "outer-value"
        );
    }

    #[test]
    fn sequential_stops_after_first_failure() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let dir3 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget {
                worktree_path: dir1.path().into(),
                branch_name: "a".into(),
            },
            ResolvedTarget {
                worktree_path: dir2.path().into(),
                branch_name: "b".into(),
            },
            ResolvedTarget {
                worktree_path: dir3.path().into(),
                branch_name: "c".into(),
            },
        ];

        // A pipeline whose first command uses the branch name to decide exit code.
        let pipeline = vec![CommandSpec::Shell(
            r#"case "$DAFT_BRANCH_NAME" in b) exit 1;; *) exit 0;; esac"#.into(),
        )];
        let report = run_scheduler(
            &targets,
            &pipeline,
            ExecMode::Sequential,
            &CancelFlag::new(),
        )
        .unwrap();
        assert_eq!(report.outcomes.len(), 2, "third target must not have run");
        assert_eq!(report.outcomes[0].target.branch_name, "a");
        assert_eq!(report.outcomes[1].target.branch_name, "b");
        assert_eq!(report.aggregate_exit_code(), 1);
    }

    #[test]
    fn keep_going_runs_all_despite_failures() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        let dir3 = TempDir::new().unwrap();
        let targets = vec![
            ResolvedTarget {
                worktree_path: dir1.path().into(),
                branch_name: "a".into(),
            },
            ResolvedTarget {
                worktree_path: dir2.path().into(),
                branch_name: "b".into(),
            },
            ResolvedTarget {
                worktree_path: dir3.path().into(),
                branch_name: "c".into(),
            },
        ];
        let pipeline = vec![CommandSpec::Shell(
            r#"case "$DAFT_BRANCH_NAME" in b) exit 1;; *) exit 0;; esac"#.into(),
        )];
        let report =
            run_scheduler(&targets, &pipeline, ExecMode::KeepGoing, &CancelFlag::new()).unwrap();
        assert_eq!(report.outcomes.len(), 3);
        assert_eq!(report.aggregate_exit_code(), 1);
    }

    #[test]
    fn list_renderer_header_and_rows_and_failed_dump() {
        use super::list_renderer::{render_failed_output_dump, render_header, render_outcome};

        let pipeline = vec![CommandSpec::Argv(vec!["cargo".into(), "test".into()])];
        let outcomes = vec![
            WorktreeOutcome {
                target: ResolvedTarget {
                    worktree_path: "/r/master".into(),
                    branch_name: "master".into(),
                },
                last_command_index: 0,
                exit_code: 0,
                elapsed: std::time::Duration::from_millis(800),
                captured_output: b"ok\n".to_vec(),
                cancelled: false,
            },
            WorktreeOutcome {
                target: ResolvedTarget {
                    worktree_path: "/r/feat-dirty".into(),
                    branch_name: "feat/dirty".into(),
                },
                last_command_index: 0,
                exit_code: 101,
                elapsed: std::time::Duration::from_millis(1200),
                captured_output: b"panicked!\n".to_vec(),
                cancelled: false,
            },
        ];
        let report = ExecReport {
            outcomes,
            orphan_branches_skipped: vec![],
        };

        let mut out: Vec<u8> = Vec::new();
        render_header(&mut out, &pipeline).unwrap();
        for o in &report.outcomes {
            render_outcome(&mut out, o, &pipeline).unwrap();
        }
        render_failed_output_dump(&mut out, &report, &pipeline).unwrap();

        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Commands"), "missing Commands header");
        assert!(s.contains("1. cargo test"), "missing pipeline row");
        assert!(
            s.contains("✓") && s.contains("master"),
            "missing success row"
        );
        assert!(
            s.contains("✗") && s.contains("feat/dirty"),
            "missing fail row"
        );
        assert!(s.contains("exit 101"), "missing exit code");
        assert!(s.contains("panicked!"), "missing failed output dump");
    }

    #[test]
    fn cancel_flag_monotonic_escalation() {
        let f = CancelFlag::new();
        assert_eq!(f.level(), 0);
        assert!(!f.is_cancelled());
        f.escalate();
        assert_eq!(f.level(), 1);
        assert!(f.is_cancelled());
        f.escalate();
        assert_eq!(f.level(), 2);
        f.escalate(); // saturates
        assert_eq!(f.level(), 2);
    }
}
