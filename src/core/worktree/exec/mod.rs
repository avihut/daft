//! Core logic for `daft worktree-exec`.
//!
//! Target resolution, per-worktree command pipeline, scheduler, and the
//! `ExecReport` data type that the command layer renders. No IO to stdout
//! lives here; renderers are separate.

pub mod alias_cache;
pub mod list_renderer;
pub mod progress_renderer;

pub use alias_cache::AliasCache;

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
    /// Argv form, e.g. from `-- CMD ARGS…`. First element is the program;
    /// remaining are its arguments. Routed through `$SHELL` after
    /// shell-quoting so that user-defined aliases resolve.
    Argv(Vec<String>),
    /// Shell-parsed string, e.g. from `-x 'CMD'`. Run via `$SHELL`, with
    /// alias expansion enabled.
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
    run_pipeline_streaming(target, pipeline, "", &presenter, cancel, None)
}

/// Run the full pipeline against one worktree, streaming output through the
/// presenter. Returns a `WorktreeOutcome` describing the final exit code,
/// captured output, and cancellation state.
///
/// The job name passed to the presenter is `name_prefix` when non-empty,
/// otherwise the target's branch name. Step identity within a multi-command
/// pipeline is conveyed via the `command_preview` argument on `on_job_start`
/// — not via the job name itself.
///
/// `alias_cache`, when supplied, replaces the slow `$SHELL -i -c` rc-file
/// load with an inlined alias table for the spawned shell.
pub fn run_pipeline_streaming(
    target: &ResolvedTarget,
    pipeline: &[CommandSpec],
    name_prefix: &str,
    presenter: &Arc<dyn JobPresenter>,
    cancel: &CancelFlag,
    alias_cache: Option<&AliasCache>,
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

    let job_name = base_name.to_string();
    let emit_skips = |start: usize| {
        for step in pipeline.iter().skip(start) {
            presenter.on_job_skipped(&job_name, "", Duration::ZERO, false, Some(&step.display()));
        }
    };

    for (idx, spec) in pipeline.iter().enumerate() {
        // Between commands: if cancel has been requested, stop before
        // launching the next command.
        if cancel.is_cancelled() {
            cancelled = true;
            emit_skips(idx);
            break;
        }

        last_index = idx;
        let preview = spec.display();
        let cmd_start = Instant::now();
        presenter.on_job_start(&job_name, None, Some(&preview));

        let mut cmd = build_command(spec, alias_cache);
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
            presenter.on_job_cancelled(&job_name, cmd_elapsed);
            emit_skips(idx + 1);
            break;
        }
        if exit_code == 0 {
            presenter.on_job_success(&job_name, cmd_elapsed);
        } else {
            presenter.on_job_failure(&job_name, cmd_elapsed);
            emit_skips(idx + 1);
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

/// Build the `Command` that runs a `CommandSpec` for one worktree.
///
/// User-defined aliases (e.g. `gss`, `gcvm`) resolve the same way they
/// would in an interactive shell. The fast path inlines a cached alias
/// table and runs `$SHELL -c '<aliases>; eval "$1"' -- <cmd>`, skipping
/// the rc-file load (`-i`) entirely. The slow path falls back to
/// `$SHELL -i -c <cmd>` so aliases still resolve when no cache is
/// available (unsupported shell, capture failure, first run before
/// caching). For `Argv`, parts are POSIX-quoted and joined first so the
/// inner shell sees the same argument boundaries.
pub(crate) fn build_command(spec: &CommandSpec, alias_cache: Option<&AliasCache>) -> Command {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let cmd_string = match spec {
        CommandSpec::Argv(parts) => quote_argv(parts),
        CommandSpec::Shell(s) => s.clone(),
    };

    let mut c = Command::new(&shell);
    match alias_cache {
        Some(cache) => {
            // Fast path: aliases (and functions) pre-cached. Source the
            // functions file (if any), inline the alias definitions,
            // then `eval` the user command so alias expansion (a
            // parse-time step) sees the definitions before the user
            // command is parsed.
            let mut body = String::new();
            body.push_str(cache.shell.alias_expansion_prefix());
            if let Some(p) = cache.functions_path.as_ref().and_then(|p| p.to_str()) {
                // Errors from the sourced file (e.g. a plugin helper
                // that doesn't apply in our minimal env) are best-effort
                // suppressed — the user's command itself will still
                // surface its own errors visibly.
                let quoted = shlex::try_quote(p)
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| p.to_string());
                body.push_str(&format!("source {quoted} 2>/dev/null\n"));
            }
            body.push_str(&cache.alias_lines);
            body.push_str("\neval \"$1\"");
            c.arg("-c").arg(body).arg("--").arg(cmd_string);
        }
        None => {
            // Slow path: load the user's rc files via `-i` so any aliases
            // (and shell functions) resolve. Used when no cache is
            // available — e.g. unsupported shell or capture failure.
            c.arg("-i").arg("-c").arg(cmd_string);
        }
    }
    c
}

/// Shell-quote each argv element (POSIX) and join with spaces so the
/// result can be handed to `$SHELL -c` while preserving the original
/// argument boundaries.
fn quote_argv(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            shlex::try_quote(p)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| p.clone())
        })
        .collect::<Vec<_>>()
        .join(" ")
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

    /// Captured output from `run_pipeline` may include benign stderr noise
    /// from the user's interactive shell (rc-file output, bash's
    /// "no job control in this shell" warning). Tests that look for a
    /// specific line should grep — not exact-match — the captured output.
    fn captured_lines(outcome: &WorktreeOutcome) -> Vec<String> {
        String::from_utf8_lossy(&outcome.captured_output)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    #[test]
    fn cwd_is_worktree_path() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "master");
        let spec = CommandSpec::Argv(vec!["pwd".into()]);
        let outcome = run_pipeline(&target, &[spec], &CancelFlag::new()).unwrap();
        let expected = dir.path().canonicalize().unwrap();
        let lines = captured_lines(&outcome);
        let found = lines
            .iter()
            .filter_map(|l| std::path::PathBuf::from(l).canonicalize().ok())
            .any(|p| p == expected);
        assert!(found, "expected pwd output {:?} in: {lines:?}", expected);
    }

    #[test]
    fn injects_env_vars() {
        let dir = TempDir::new().unwrap();
        let target = dummy_target(&dir, "feat/abc");
        let spec = CommandSpec::Argv(vec![
            "sh".into(),
            "-c".into(),
            "printf '%s\\n' \"$DAFT_BRANCH_NAME\"".into(),
        ]);
        let outcome = run_pipeline(&target, &[spec], &CancelFlag::new()).unwrap();
        let lines = captured_lines(&outcome);
        assert!(
            lines.iter().any(|l| l == "feat/abc"),
            "expected branch name line in: {lines:?}"
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
        let lines = captured_lines(&outcome);
        assert!(
            lines.iter().any(|l| l == "3"),
            "expected arithmetic result in: {lines:?}"
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
    fn build_command_falls_back_to_interactive_shell_when_no_cache() {
        // Slow path: when no alias cache is available (unsupported shell
        // or capture failure), fall back to `$SHELL -i -c` so aliases
        // still resolve via the user's rc files. Matches the behavior
        // shipped for `-x` on `daft go` / `daft start` in #242.
        let shell_cmd = build_command(&CommandSpec::Shell("gss".into()), None);
        let args: Vec<&std::ffi::OsStr> = shell_cmd.get_args().collect();
        assert_eq!(args, vec!["-i", "-c", "gss"]);

        // Argv parts are POSIX-quoted and joined so the inner shell sees
        // the same argument boundaries as the original argv.
        let argv_cmd = build_command(
            &CommandSpec::Argv(vec!["echo".into(), "hello world".into()]),
            None,
        );
        let args: Vec<&std::ffi::OsStr> = argv_cmd.get_args().collect();
        assert_eq!(args, vec!["-i", "-c", "echo 'hello world'"]);
    }

    #[test]
    fn build_command_uses_cached_aliases_without_interactive_shell() {
        // Fast path: a populated alias cache lets us skip the rc-file
        // load (`-i`). Cached `alias …` lines are inlined and the user
        // command is `eval`-ed so alias expansion sees them at parse
        // time.
        use super::alias_cache::{AliasCache, ShellKind};
        let cache = AliasCache {
            shell: ShellKind::Zsh,
            alias_lines: "alias gss='git status -s'".into(),
            functions_path: None,
            captured_at: std::time::SystemTime::now(),
        };

        let cmd = build_command(&CommandSpec::Shell("gss --short".into()), Some(&cache));
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        // No `-i`, and the body splits cleanly into:
        //   -c, "<aliases>; eval $1", --, <user cmd>
        assert_eq!(args[0], "-c");
        let body = args[1].to_string_lossy();
        assert!(
            body.contains("alias gss='git status -s'"),
            "missing alias inline: {body}"
        );
        assert!(body.contains("eval \"$1\""), "missing eval: {body}");
        assert_eq!(args[2], "--");
        assert_eq!(args[3], "gss --short");
        assert_eq!(args.len(), 4);
    }

    #[test]
    fn build_command_with_bash_cache_includes_expand_aliases_shopt() {
        // bash needs `shopt -s expand_aliases` before alias expansion in
        // non-interactive mode. zsh expands by default and gets no prefix.
        use super::alias_cache::{AliasCache, ShellKind};
        let cache = AliasCache {
            shell: ShellKind::Bash,
            alias_lines: "alias gss='git status -s'".into(),
            functions_path: None,
            captured_at: std::time::SystemTime::now(),
        };
        let cmd = build_command(&CommandSpec::Shell("gss".into()), Some(&cache));
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        let body = args[1].to_string_lossy();
        assert!(
            body.contains("shopt -s expand_aliases"),
            "bash body must opt into alias expansion: {body}"
        );
    }

    #[test]
    fn build_command_sources_functions_file_when_cached() {
        // When a functions snapshot exists on disk, the fast path sources
        // it before defining aliases so user shell functions resolve too.
        use super::alias_cache::{AliasCache, ShellKind};
        use std::path::PathBuf;
        let funcs_path = PathBuf::from("/tmp/daft test/functions-zsh.sh");
        let cache = AliasCache {
            shell: ShellKind::Zsh,
            alias_lines: "alias g='git'".into(),
            functions_path: Some(funcs_path),
            captured_at: std::time::SystemTime::now(),
        };
        let cmd = build_command(&CommandSpec::Shell("mygitfunc".into()), Some(&cache));
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        let body = args[1].to_string_lossy();
        // Path with whitespace must be shell-quoted to source correctly.
        assert!(
            body.contains("source '/tmp/daft test/functions-zsh.sh'"),
            "must source functions file (quoted): {body}"
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
        render_header(&mut out, report.outcomes.len(), &pipeline).unwrap();
        for o in &report.outcomes {
            render_outcome(&mut out, o, &pipeline).unwrap();
        }
        render_failed_output_dump(&mut out, &report, &pipeline).unwrap();

        let s = String::from_utf8(out).unwrap();
        assert!(
            s.contains("2 worktrees · 1 command"),
            "missing scope-summary header: {s}"
        );
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

#[cfg(test)]
mod streaming_skip_emission_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct EventRecorder {
        events: Mutex<Vec<String>>,
    }

    impl EventRecorder {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn log(&self, event: &str) {
            self.events.lock().unwrap().push(event.to_string());
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.events.lock().unwrap())
        }
    }

    impl crate::executor::presenter::JobPresenter for EventRecorder {
        fn on_phase_start(&self, _: &str) {}
        fn on_job_start(&self, name: &str, _: Option<&str>, preview: Option<&str>) {
            self.log(&format!("start:{name}:{}", preview.unwrap_or("")));
        }
        fn on_job_output(&self, _: &str, _: &str) {}
        fn on_job_success(&self, name: &str, _: std::time::Duration) {
            self.log(&format!("success:{name}"));
        }
        fn on_job_failure(&self, name: &str, _: std::time::Duration) {
            self.log(&format!("failure:{name}"));
        }
        fn on_job_cancelled(&self, name: &str, _: std::time::Duration) {
            self.log(&format!("cancelled:{name}"));
        }
        fn on_job_skipped(
            &self,
            name: &str,
            _reason: &str,
            _duration: std::time::Duration,
            _show: bool,
            preview: Option<&str>,
        ) {
            self.log(&format!("skipped:{name}:{}", preview.unwrap_or("")));
        }
        fn on_message(&self, _: &str) {}
        fn on_phase_complete(&self, _: std::time::Duration) {}
        fn take_results(&self) -> Vec<crate::executor::JobResult> {
            Vec::new()
        }
    }

    #[test]
    fn fail_fast_emits_skipped_for_unrun_steps() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: "branch-a".into(),
        };
        let pipeline = vec![
            CommandSpec::Argv(vec!["false".into()]),
            CommandSpec::Argv(vec!["echo".into(), "never".into()]),
        ];
        let recorder = EventRecorder::arc();
        // explicit cast required: Arc::clone returns Arc<EventRecorder>, not Arc<dyn Trait>
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> =
            Arc::clone(&recorder) as Arc<dyn crate::executor::presenter::JobPresenter>;

        let outcome =
            run_pipeline_streaming(&target, &pipeline, "", &presenter, &CancelFlag::new(), None)
                .unwrap();

        assert!(!outcome.succeeded(), "first step should fail");
        let events = recorder.take();
        let starts: Vec<&String> = events.iter().filter(|e| e.starts_with("start:")).collect();
        assert_eq!(starts.len(), 1, "only first step should start: {events:?}");
        assert!(
            events.iter().any(|e| e == "failure:branch-a"),
            "missing failure event: {events:?}"
        );
        assert!(
            events.iter().any(|e| e == "skipped:branch-a:echo never"),
            "missing skipped event for step 2: {events:?}"
        );
    }

    #[test]
    fn pre_cancel_emits_skipped_for_all_steps() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: "branch-b".into(),
        };
        let pipeline = vec![
            CommandSpec::Argv(vec!["echo".into(), "one".into()]),
            CommandSpec::Argv(vec!["echo".into(), "two".into()]),
        ];
        let recorder = EventRecorder::arc();
        // explicit cast required: Arc::clone returns Arc<EventRecorder>, not Arc<dyn Trait>
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> =
            Arc::clone(&recorder) as Arc<dyn crate::executor::presenter::JobPresenter>;

        let cancel = CancelFlag::new();
        cancel.escalate();

        let outcome =
            run_pipeline_streaming(&target, &pipeline, "", &presenter, &cancel, None).unwrap();

        assert!(outcome.cancelled, "expected cancelled outcome");
        let events = recorder.take();
        let starts = events.iter().filter(|e| e.starts_with("start:")).count();
        assert_eq!(
            starts, 0,
            "no steps should start when pre-cancelled: {events:?}"
        );
        assert!(
            events.iter().any(|e| e == "skipped:branch-b:echo one"),
            "missing skipped event for step 1: {events:?}"
        );
        assert!(
            events.iter().any(|e| e == "skipped:branch-b:echo two"),
            "missing skipped event for step 2: {events:?}"
        );
        assert!(
            !events.iter().any(|e| e.starts_with("cancelled:")),
            "top-of-loop cancel must not emit cancelled event: {events:?}"
        );
    }

    #[test]
    fn mid_flight_cancel_emits_cancelled_not_failure() {
        // Exercises the post-child-exit `cancel.is_cancelled()` check: start a
        // slow child, escalate cancel from a background thread, let
        // run_pipeline_streaming observe the cancel flag AFTER the child has been
        // SIGTERM'd. The in-flight step must emit on_job_cancelled (not
        // on_job_failure), and the remaining step(s) must emit on_job_skipped.
        let dir = tempfile::TempDir::new().unwrap();
        let target = ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: "branch-mid".into(),
        };
        let pipeline = vec![
            CommandSpec::Shell("sleep 5".into()),
            CommandSpec::Argv(vec!["echo".into(), "never".into()]),
        ];
        let recorder = EventRecorder::arc();
        // explicit cast required: Arc::clone returns Arc<EventRecorder>, not Arc<dyn Trait>
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> =
            Arc::clone(&recorder) as Arc<dyn crate::executor::presenter::JobPresenter>;

        let cancel = CancelFlag::new();
        let outcome = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(200));
                cancel.escalate();
            });
            run_pipeline_streaming(&target, &pipeline, "", &presenter, &cancel, None)
        })
        .unwrap();

        assert!(outcome.cancelled, "expected cancelled outcome");
        let events = recorder.take();
        assert!(
            events.iter().any(|e| e == "cancelled:branch-mid"),
            "mid-flight cancel must emit cancelled event, not failure: {events:?}"
        );
        assert!(
            !events.iter().any(|e| e.starts_with("failure:")),
            "mid-flight cancel must NOT emit failure event: {events:?}"
        );
        assert!(
            events.iter().any(|e| e == "skipped:branch-mid:echo never"),
            "mid-flight cancel must emit skipped for remaining step: {events:?}"
        );
    }
}
