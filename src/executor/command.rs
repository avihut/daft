//! Generic shell command execution.
//!
//! Provides format-agnostic functions for spawning shell commands with
//! captured or inherited I/O, timeouts, and optional line-streaming.
//! This module does **not** depend on the hooks system; callers are
//! responsible for building the full set of environment variables.

use crate::coordinator::log_record::OutputKind;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────
// Result type
// ─────────────────────────────────────────────────────────────────────────

/// Result of running a shell command.
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Whether the command exited successfully (exit code 0).
    pub success: bool,
    /// Process exit code, if available.
    pub exit_code: Option<i32>,
    /// Captured standard output (empty for interactive commands).
    pub stdout: String,
    /// Captured standard error (empty for interactive commands).
    pub stderr: String,
    /// Whether the command was terminated by a user cancellation
    /// (two-stage Ctrl+C) rather than exiting on its own. When true,
    /// `exit_code` is normalized to `Some(130)` (128 + SIGINT).
    pub cancelled: bool,
    /// The limit the command outran, when it was torn down by its timeout.
    /// When set, `exit_code` is normalized to [`TIMED_OUT_EXIT_CODE`].
    pub timed_out: Option<Duration>,
}

/// Exit code reported for a command killed by its timeout — GNU `timeout`'s
/// convention. The teardown's signal death would otherwise surface as a
/// meaningless `-1` (or a signal code that hides the cause), the same reason
/// a cancellation normalizes to 130.
pub const TIMED_OUT_EXIT_CODE: i32 = 124;

// ─────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────

/// Spawn a shell command with captured I/O, an optional line-streaming
/// channel, and a timeout.
///
/// The command is executed via `sh -c <cmd>`.  Stdout and stderr are read
/// in dedicated threads so neither blocks the timeout.  If `line_sender`
/// is provided, every line read from stdout **and** stderr is forwarded
/// through it (useful for live progress display).
///
/// If `pid_sender` is provided, the spawned child's PID is sent through
/// it once, immediately after spawn (used by the coordinator to register
/// background-job PIDs for cancellation).
///
/// The caller is responsible for building the complete set of environment
/// variables (hook env + extra env) and passing them in `env`.
///
/// If `cancel` is provided, the wait loop observes the flag: level 1 tears
/// the child's process tree down with SIGTERM+SIGCONT, level 2 escalates to
/// SIGKILL (via [`GroupCascade`]). A child killed this way returns a result
/// with `cancelled: true` and `exit_code: Some(130)`. `cancel: None` (hooks,
/// coordinator) polls nothing and is behaviorally identical to before.
#[allow(clippy::too_many_arguments)]
pub fn run_command(
    cmd: &str,
    env: &HashMap<String, String>,
    working_dir: &Path,
    timeout: Option<Duration>,
    line_sender: Option<std::sync::mpsc::Sender<(OutputKind, String)>>,
    pid_sender: Option<std::sync::mpsc::Sender<u32>>,
    cancel: Option<&crate::git::cancel::CancelFlag>,
) -> Result<CommandResult> {
    run_command_with_stdin(
        cmd,
        env,
        working_dir,
        timeout,
        line_sender,
        pid_sender,
        cancel,
        None,
    )
}

/// [`run_command`] with a payload written to the child's stdin.
///
/// `stdin_text` is `Some` only for jobs that declared `use_stdin:` — daft
/// replaying the block git handed a `pre-push` or `post-rewrite` hook, which
/// the dispatcher had to drain before any job could see it.
#[allow(clippy::too_many_arguments)]
pub fn run_command_with_stdin(
    cmd: &str,
    env: &HashMap<String, String>,
    working_dir: &Path,
    timeout: Option<Duration>,
    line_sender: Option<std::sync::mpsc::Sender<(OutputKind, String)>>,
    pid_sender: Option<std::sync::mpsc::Sender<u32>>,
    cancel: Option<&crate::git::cancel::CancelFlag>,
    stdin_text: Option<&str>,
) -> Result<CommandResult> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);
    command.envs(env);
    // A hook is lifecycle automation, never the arbiter of where the user's
    // shell ends up. Inheriting `DAFT_CD_FILE` lets a hook that shells out to
    // daft — `daft go other` — write the outer invocation's cd target. Most
    // paths mask it by writing the real target afterwards, but the ones that
    // deliberately write *nothing* do not: a failed post-create hook must not
    // teleport the user into the half-set-up worktree (#765), and a bulk
    // `--fork -n N` has no single destination to move to. Scrub it (#811).
    command.env_remove(crate::CD_FILE_ENV);

    // Non-interactive commands must not inherit stdin -- a child process
    // (e.g. mise, cargo) might block waiting for input that will never come.
    // A job that asked for the stage payload gets a pipe instead, written and
    // closed below so it still sees EOF.
    command.stdin(match stdin_text {
        Some(_) => Stdio::piped(),
        None => Stdio::null(),
    });
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    // Move the child into its own process group so cancelling can signal
    // every descendant. Without this, on shells that fork+wait (e.g. dash
    // with certain command shapes) signalling the bare PID kills only the
    // wrapping `sh` and orphans the actual workload (e.g. `sleep 30`).
    //
    // `process_group(0)` calls setpgid(0, 0) post-fork pre-exec, giving
    // PID == PGID. Previously we used `pre_exec(setsid)`, which also detached
    // from the controlling TTY — but no caller of `run_command` relies on
    // that side effect (the coordinator detaches once at startup;
    // non-coordinator callers run synchronously). The PGID-equals-PID
    // invariant that `killpg` cancellation depends on is preserved.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to spawn: {cmd}"))?;

    if let Some(tx) = pid_sender {
        let _ = tx.send(child.id());
    }

    // Write the payload and drop the handle so the child sees EOF. Done on a
    // thread: a payload larger than the pipe buffer would otherwise block
    // here while the child blocked writing output nobody is reading yet.
    let stdin_writer = match (stdin_text, child.stdin.take()) {
        (Some(text), Some(mut pipe)) => {
            let text = text.to_string();
            Some(std::thread::spawn(move || {
                use std::io::Write;
                // A child that exits without reading (`head -1`) gives EPIPE;
                // that is its business, not a job failure.
                let _ = pipe.write_all(text.as_bytes());
            }))
        }
        _ => None,
    };

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    // Kept back from the reader threads: a teardown that leaves processes
    // behind says so in the job's own output.
    let tx_notice = line_sender.clone();
    let tx_stdout = line_sender.clone();
    let tx_stderr = line_sender;

    // Read stdout and stderr in separate threads so they don't block the
    // timeout.  Previously the reads were sequential on the main thread,
    // which meant `wait_with_timeout` was unreachable until the child
    // closed its pipes -- effectively making the timeout dead code.
    let stdout_thread = std::thread::spawn(move || {
        let mut content = String::new();
        if let Some(stdout) = stdout_handle {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(ref tx) = tx_stdout {
                    tx.send((OutputKind::Stdout, line.clone())).ok();
                }
                content.push_str(&line);
                content.push('\n');
            }
        }
        content
    });

    let stderr_thread = std::thread::spawn(move || {
        let mut content = String::new();
        if let Some(stderr) = stderr_handle {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(ref tx) = tx_stderr {
                    tx.send((OutputKind::Stderr, line.clone())).ok();
                }
                content.push_str(&line);
                content.push('\n');
            }
        }
        content
    });

    // Wait for the child, honoring both the optional timeout and the optional
    // cancel flag. Tearing the child's tree down (either path) closes the
    // pipes and unblocks the reader threads above; a timeout keeps escalating
    // until they have drained.
    let drains_done = || stdout_thread.is_finished() && stderr_thread.is_finished();
    let (outcome, survivors) = wait_child(&mut child, timeout, cancel, drains_done)
        .with_context(|| format!("Command execution failed: {cmd}"))?;

    let stdout_content = stdout_thread.join().unwrap_or_default();
    let mut stderr_content = stderr_thread.join().unwrap_or_default();
    if let Some(notice) = survivors_notice(&survivors) {
        if let Some(tx) = &tx_notice {
            tx.send((OutputKind::Stderr, notice.clone())).ok();
        }
        stderr_content.push_str(&notice);
        stderr_content.push('\n');
    }
    if let Some(writer) = stdin_writer {
        writer.join().ok();
    }

    match outcome {
        WaitOutcome::Exited(status) => Ok(CommandResult {
            success: status.success(),
            exit_code: Some(status.code().unwrap_or(-1)),
            stdout: stdout_content,
            stderr: stderr_content,
            cancelled: false,
            timed_out: None,
        }),
        WaitOutcome::Cancelled => Ok(CommandResult {
            success: false,
            // Normalize the signal-death status (-1) to the conventional
            // 128 + SIGINT so downstream exit-code propagation is stable.
            exit_code: Some(130),
            stdout: stdout_content,
            stderr: stderr_content,
            cancelled: true,
            timed_out: None,
        }),
        // A timeout is an outcome, not an error: the job failed, its output
        // up to the teardown is kept, and callers see why it failed.
        WaitOutcome::TimedOut(limit) => Ok(CommandResult {
            success: false,
            exit_code: Some(TIMED_OUT_EXIT_CODE),
            stdout: stdout_content,
            stderr: stderr_content,
            cancelled: false,
            timed_out: Some(limit),
        }),
    }
}

/// The line a teardown that left process groups alive adds to the job's
/// stderr — the rail tail, the background job's log, and `daft hooks jobs
/// logs` all show it. Worded like `daft sync`'s cancel report.
fn survivors_notice(pgids: &[u32]) -> Option<String> {
    if pgids.is_empty() {
        return None;
    }
    let list = pgids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "daft: processes this job started may still be running (process group(s): {list}). \
         Recover manually with: kill -KILL -<pgid>"
    ))
}

/// Spawn a shell command with inherited stdin/stdout/stderr (interactive).
///
/// The command is executed via `sh -c <cmd>`.  No output is captured; the
/// child process shares the terminal with the parent.
///
/// The caller is responsible for building the complete set of environment
/// variables and passing them in `env`.
///
/// The interactive child is **not** placed in its own process group — it
/// shares the caller's foreground group so it receives the terminal's own
/// SIGINT directly (the natural Ctrl+C behavior programs like `vim` expect).
/// When `cancel` is supplied, the wait loop still escalates: level 1 is a
/// no-op (the child already got the terminal SIGINT; a redundant SIGTERM
/// would flip graceful-stop handlers into force-quit), and level 2 sends a
/// direct SIGKILL to the child pid via [`kill_pid`] — never `killpg`, which
/// would tear down daft's own group. The result is marked cancelled (exit
/// 130) only if that SIGKILL fired; a child that catches the SIGINT and exits
/// on its own propagates its real code. `cancel: None` keeps the original
/// blocking `status()` path.
pub fn run_command_interactive(
    cmd: &str,
    env: &HashMap<String, String>,
    working_dir: &Path,
    cancel: Option<&crate::git::cancel::CancelFlag>,
) -> Result<CommandResult> {
    // The child inherits stdin, so it owns the terminal for its lifetime.
    // If a rail key listener is watching (#729), stand it down and hand the
    // driver back to cooked, echoing input — otherwise the child would run
    // with `ECHO`/`ICANON` off, showing nothing as the user types, while the
    // listener raced it for the same bytes. No-op when nothing is listening.
    let _keys = crate::output::term_guard::suspend_key_input();

    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);
    command.envs(env);
    // Same reasoning as the non-interactive path above: a hook does not get
    // to choose the user's destination (#811).
    command.env_remove(crate::CD_FILE_ENV);

    // Inherit stdin/stdout/stderr for interactive mode
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    // Fast path: no cancel flag → the original blocking wait, untouched.
    let Some(cancel) = cancel else {
        let status = command
            .status()
            .with_context(|| format!("Failed to run interactive command: {cmd}"))?;
        return Ok(CommandResult {
            success: status.success(),
            exit_code: Some(status_exit_code(&status)),
            stdout: String::new(),
            stderr: String::new(),
            cancelled: false,
            timed_out: None,
        });
    };

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to run interactive command: {cmd}"))?;

    match wait_interactive_child(&mut child, cancel)? {
        WaitOutcome::Exited(status) => Ok(CommandResult {
            success: status.success(),
            exit_code: Some(status_exit_code(&status)),
            stdout: String::new(),
            stderr: String::new(),
            cancelled: false,
            timed_out: None,
        }),
        WaitOutcome::Cancelled => Ok(CommandResult {
            success: false,
            exit_code: Some(130),
            stdout: String::new(),
            stderr: String::new(),
            cancelled: true,
            timed_out: None,
        }),
        // Interactive children are never given a deadline.
        WaitOutcome::TimedOut(_) => unreachable!("interactive commands have no timeout"),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────

/// An exited child's code under the shell convention: a signal death
/// resolves `128 + N` (sh reports a SIGINT'd child as 130, SIGTERM as 143),
/// never a synthetic -1. Matters for `daft run`'s passthrough, where the
/// terminal's ^C SIGINTs the interactive child directly.
fn status_exit_code(status: &ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(-1)
}

/// Terminal outcome of waiting on a child: it exited on its own, it was torn
/// down by a user cancellation, or it outran its timeout (the limit) and was
/// torn down.
enum WaitOutcome {
    Exited(ExitStatus),
    Cancelled,
    TimedOut(Duration),
}

/// Wait for a captured-output child, polling at 100ms intervals.
///
/// Honors two independent deadlines:
/// - `cancel`: once the flag is raised, the child's process tree is torn
///   down — SIGTERM+SIGCONT at level 1, SIGKILL at level 2 — via
///   [`GroupCascade`], and the wait returns [`WaitOutcome::Cancelled`].
/// - `timeout`: when `Some(t)` and exceeded, the same tree teardown runs on a
///   clock — SIGTERM+SIGCONT at the deadline, SIGKILL once
///   [`TIMEOUT_HARD_GRACE`] has passed — and the wait returns
///   [`WaitOutcome::TimedOut`]. `None` waits forever (task jobs).
///
/// The wait returns only once the child is reaped **and** `drains_done`
/// reports its output pipes closed (the `ChildSupervisor::wait` contract): a
/// process the shell leaves behind can hold the pipes past the shell's exit,
/// and returning early would hang the caller's reader join with nothing left
/// to escalate against it. Both deadlines keep applying until then, so the
/// limit bounds the whole job, pipe holders included.
///
/// The outcome is labeled the way `ChildSupervisor::wait` labels it: a cancel
/// or a deadline counts only if it lands while the shell is still running. A
/// shell that exited on its own keeps its exit status even when a leftover
/// pipe holder is torn down afterwards — its tests passed; daft just stopped
/// waiting for the straggler.
///
/// Once a deadline teardown starts, the wait also holds until every process
/// group it signaled is gone: a descendant that ignores SIGTERM and holds no
/// pipe would otherwise outlive a job reported as stopped. A group still alive
/// [`HARD_SETTLE`] after the SIGKILL is not waited on forever: it is returned
/// beside the outcome, for the caller to report, and recorded for `daft
/// sync`'s leftover report.
///
/// The teardown reaches the groups it can find by walking the tree from the
/// shell. A process that started its own session and whose parent has already
/// exited is out of reach; the wait then lasts until it closes the pipes.
///
/// `cancel: None` polls no flag.
///
/// [`GroupCascade`]: crate::git::cancel::GroupCascade
/// [`TIMEOUT_HARD_GRACE`]: crate::git::cancel::TIMEOUT_HARD_GRACE
fn wait_child(
    child: &mut std::process::Child,
    timeout: Option<Duration>,
    cancel: Option<&crate::git::cancel::CancelFlag>,
    drains_done: impl Fn() -> bool,
) -> Result<(WaitOutcome, Vec<u32>)> {
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    let poll_interval = Duration::from_millis(100);
    let mut cancelling = false;
    let mut timed_out: Option<Duration> = None;
    let mut exited: Option<ExitStatus> = None;
    #[cfg(unix)]
    let mut teardown: Option<CancelTeardown> = None;

    loop {
        if exited.is_none() {
            exited = child.try_wait()?;
        }
        let running = exited.is_none();

        let flag_level = cancel.map_or(0, crate::git::cancel::CancelFlag::level);
        // 0 = within budget, 1 = past the deadline, 2 = past deadline + grace.
        let deadline_level = match timeout {
            Some(t) => {
                let elapsed = start.elapsed();
                if elapsed < t {
                    0
                } else if elapsed < t + crate::git::cancel::TIMEOUT_HARD_GRACE {
                    1
                } else {
                    2
                }
            }
            None => 0,
        };
        if flag_level > 0 && running {
            cancelling = true;
        }
        if deadline_level > 0 && running {
            timed_out = timeout;
        }

        if let Some(status) = exited
            && drains_done()
        {
            // A deadline teardown finishes before the wait returns.
            #[cfg(unix)]
            let settled =
                deadline_level == 0 || teardown.as_ref().is_none_or(CancelTeardown::settled);
            #[cfg(not(unix))]
            let settled = true;
            if settled {
                // Whatever outlived a settled deadline teardown is reported
                // rather than left to vanish silently.
                #[cfg(unix)]
                let survivors = match &teardown {
                    Some(teardown) if deadline_level > 0 => {
                        let survivors = teardown.survivors();
                        crate::git::cancel::record_survivors(&survivors);
                        survivors
                    }
                    _ => Vec::new(),
                };
                #[cfg(not(unix))]
                let survivors = Vec::new();
                // Priority: a user cancel outranks a timeout; a timeout
                // outranks the exit status (the teardown forged it anyway).
                let outcome = if cancelling {
                    WaitOutcome::Cancelled
                } else if let Some(limit) = timed_out {
                    WaitOutcome::TimedOut(limit)
                } else {
                    WaitOutcome::Exited(status)
                };
                return Ok((outcome, survivors));
            }
        }

        // Escalation applies whether or not the shell is still running: a
        // reaped shell's pipe holders are torn down the same way.
        let level = flag_level.max(deadline_level);
        if level > 0 {
            #[cfg(unix)]
            {
                teardown
                    .get_or_insert_with(|| CancelTeardown::new(child.id()))
                    .tick(level);
            }
            #[cfg(not(unix))]
            {
                // No process-group teardown off-unix; a direct kill of a
                // still-running child is the best available escalation.
                if running {
                    child.kill().ok();
                }
            }
        }
        thread::sleep(poll_interval);
    }
}

/// How long [`wait_child`] keeps waiting for a signaled process group to
/// disappear after SIGKILL — enough for the kernel to finish the kill and for
/// init to reap an orphan, short of hanging on a process in uninterruptible
/// sleep.
#[cfg(unix)]
const HARD_SETTLE: Duration = Duration::from_secs(1);

/// Wait for an interactive (stdio-inherited) child under cancellation.
///
/// The child shares the caller's foreground process group, so it already
/// received the terminal's SIGINT on the first Ctrl+C — level 1 is therefore
/// a deliberate no-op (a redundant SIGTERM would defeat graceful-shutdown
/// handlers). Level 2 sends a direct SIGKILL to the child pid; `killpg` is
/// off-limits here because the child is in daft's own group.
///
/// The reap reports [`WaitOutcome::Cancelled`] **only** when daft issued that
/// hard kill. A child that catches the terminal's SIGINT and exits on its own
/// — a REPL you ^C to abort a line, then quit cleanly — propagates its real
/// status: the passthrough mirrors direct invocation, so only a daft-forced
/// SIGKILL, which the child cannot turn into a graceful exit, reads as a
/// cancellation.
#[cfg(unix)]
fn wait_interactive_child(
    child: &mut std::process::Child,
    cancel: &crate::git::cancel::CancelFlag,
) -> Result<WaitOutcome> {
    use std::thread;

    let poll_interval = Duration::from_millis(100);
    let mut hard_sent = false;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(if hard_sent {
                WaitOutcome::Cancelled
            } else {
                WaitOutcome::Exited(status)
            });
        }
        // Level 1 does nothing here (the child already got the terminal's
        // SIGINT); only level 2 escalates to a direct SIGKILL of the child pid.
        if cancel.level() >= 2 && !hard_sent {
            crate::git::cancel::kill_pid(child.id(), true);
            hard_sent = true;
        }
        thread::sleep(poll_interval);
    }
}

#[cfg(not(unix))]
fn wait_interactive_child(
    child: &mut std::process::Child,
    cancel: &crate::git::cancel::CancelFlag,
) -> Result<WaitOutcome> {
    use std::thread;

    let poll_interval = Duration::from_millis(100);
    let mut hard_killed = false;

    loop {
        if let Some(status) = child.try_wait()? {
            // Cancelled only when daft forced the kill; a child that exits on
            // its own propagates its real status (mirrors direct invocation).
            return Ok(if hard_killed {
                WaitOutcome::Cancelled
            } else {
                WaitOutcome::Exited(status)
            });
        }
        if cancel.level() >= 2 && !hard_killed {
            child.kill().ok();
            hard_killed = true;
        }
        thread::sleep(poll_interval);
    }
}

/// Escalating process-tree teardown state for a captured-output child under
/// cancellation or past its deadline. Wraps a [`GroupCascade`] with the tick
/// cadence: the first soft tick fires immediately, then every ~500ms while at
/// level 1; a single hard tick fires on the transition to level 2.
#[cfg(unix)]
struct CancelTeardown {
    cascade: crate::git::cancel::GroupCascade,
    last_soft: Option<std::time::Instant>,
    hard_sent: Option<std::time::Instant>,
}

#[cfg(unix)]
impl CancelTeardown {
    fn new(root_pid: u32) -> Self {
        Self {
            cascade: crate::git::cancel::GroupCascade::new(root_pid),
            last_soft: None,
            hard_sent: None,
        }
    }

    fn tick(&mut self, level: usize) {
        if level >= 2 {
            if self.hard_sent.is_none() {
                self.cascade.hard_tick();
                self.hard_sent = Some(std::time::Instant::now());
            }
            return;
        }
        let due = self
            .last_soft
            .is_none_or(|t| t.elapsed() >= Duration::from_millis(500));
        if due {
            self.cascade.soft_tick();
            self.last_soft = Some(std::time::Instant::now());
        }
    }

    /// Signaled process groups that still have live members.
    fn survivors(&self) -> Vec<u32> {
        self.cascade.survivors()
    }

    /// Whether the teardown is over: every signaled group is gone, or the
    /// SIGKILL went out at least [`HARD_SETTLE`] ago and waiting longer would
    /// not help.
    fn settled(&self) -> bool {
        self.hard_sent.is_some_and(|at| at.elapsed() >= HARD_SETTLE) || self.survivors().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    // ── CommandResult ──────────────────────────────────────────────────

    #[test]
    fn command_result_success_fields() {
        let result = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: "hello\n".into(),
            stderr: String::new(),
            cancelled: false,
            timed_out: None,
        };
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "hello\n");
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn command_result_failure_fields() {
        let result = CommandResult {
            success: false,
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "error\n".into(),
            cancelled: false,
            timed_out: None,
        };
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(1));
        assert_eq!(result.stderr, "error\n");
    }

    #[test]
    fn command_result_clone() {
        let result = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: "ok".into(),
            stderr: String::new(),
            cancelled: false,
            timed_out: None,
        };
        let cloned = result.clone();
        assert_eq!(cloned.success, result.success);
        assert_eq!(cloned.exit_code, result.exit_code);
        assert_eq!(cloned.stdout, result.stdout);
    }

    #[test]
    fn command_result_debug() {
        let result = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            cancelled: false,
            timed_out: None,
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("CommandResult"));
        assert!(debug.contains("success: true"));
    }

    // ── run_command ────────────────────────────────────────────────────

    #[test]
    fn run_command_echo() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command(
            "echo hello",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.trim(), "hello");
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn run_command_captures_stderr() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command(
            "echo err >&2",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.success);
        assert_eq!(result.stderr.trim(), "err");
    }

    #[test]
    fn run_command_nonzero_exit() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command(
            "exit 42",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            None,
            None,
            None,
        )
        .unwrap();
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(42));
    }

    #[test]
    fn run_command_env_vars() {
        let mut env = HashMap::new();
        env.insert("MY_TEST_VAR".into(), "test_value_123".into());
        let dir = std::env::temp_dir();
        let result = run_command(
            "echo $MY_TEST_VAR",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.success);
        assert_eq!(result.stdout.trim(), "test_value_123");
    }

    #[test]
    fn run_command_working_dir() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command(
            "pwd",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            None,
            None,
            None,
        )
        .unwrap();
        assert!(result.success);
        // On macOS /tmp is a symlink to /private/tmp, so canonicalize both.
        let expected = dir.canonicalize().unwrap();
        let actual = std::path::PathBuf::from(result.stdout.trim())
            .canonicalize()
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn run_command_line_sender() {
        let (tx, rx) = mpsc::channel::<(OutputKind, String)>();
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command(
            "echo line1; echo line2",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            Some(tx),
            None,
            None,
        )
        .unwrap();
        assert!(result.success);

        let received: Vec<(OutputKind, String)> = rx.try_iter().collect();
        let stdout_lines: Vec<&str> = received
            .iter()
            .filter(|(k, _)| *k == OutputKind::Stdout)
            .map(|(_, l)| l.as_str())
            .collect();
        assert!(stdout_lines.contains(&"line1"));
        assert!(stdout_lines.contains(&"line2"));
    }

    #[test]
    fn run_command_stderr_lines_are_tagged_stderr() {
        let (tx, rx) = mpsc::channel::<(OutputKind, String)>();
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command(
            "echo on-stderr 1>&2; echo on-stdout",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            Some(tx),
            None,
            None,
        )
        .unwrap();
        assert!(result.success);

        let received: Vec<(OutputKind, String)> = rx.try_iter().collect();
        let by_kind: HashMap<&str, Vec<&str>> =
            received.iter().fold(HashMap::new(), |mut acc, (k, line)| {
                let tag = match k {
                    OutputKind::Stdout => "stdout",
                    OutputKind::Stderr => "stderr",
                };
                acc.entry(tag).or_default().push(line.as_str());
                acc
            });
        assert!(
            by_kind
                .get("stdout")
                .is_some_and(|v| v.contains(&"on-stdout")),
            "stdout missing: {by_kind:?}"
        );
        assert!(
            by_kind
                .get("stderr")
                .is_some_and(|v| v.contains(&"on-stderr")),
            "stderr missing: {by_kind:?}"
        );
    }

    #[test]
    fn run_command_timeout() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command(
            "sleep 60",
            &env,
            &dir,
            Some(Duration::from_millis(200)),
            None,
            None,
            None,
        );
        // A timeout is an outcome, not an error: the result says which limit
        // was hit and normalizes the exit code.
        let result = result.expect("a timed-out command still returns a result");
        assert!(!result.success);
        assert_eq!(result.timed_out, Some(Duration::from_millis(200)));
        assert_eq!(result.exit_code, Some(TIMED_OUT_EXIT_CODE));
        assert!(!result.cancelled);
    }

    /// Regression (#1009): the timeout used to SIGKILL only the `sh` pid. A
    /// compound command runs its workload as a child of `sh` (the trailing
    /// `echo` keeps every shell from exec'ing the `sleep` in place), and that
    /// child survived the kill, kept the output pipes open, and kept running.
    /// The deadline must tear the whole tree down, promptly, and keep the
    /// output captured before the teardown.
    #[cfg(unix)]
    #[test]
    fn run_command_timeout_tears_down_forked_workload() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let (pid_tx, pid_rx) = mpsc::channel::<u32>();
        let started = std::time::Instant::now();
        let result = run_command(
            "echo started; sleep 31; echo after",
            &env,
            &dir,
            Some(Duration::from_millis(300)),
            None,
            Some(pid_tx),
            None,
        )
        .expect("a timed-out command still returns a result");
        let elapsed = started.elapsed();

        assert_eq!(result.timed_out, Some(Duration::from_millis(300)));
        assert_eq!(result.exit_code, Some(TIMED_OUT_EXIT_CODE));
        assert!(
            result.stdout.contains("started"),
            "output before the teardown is kept: {:?}",
            result.stdout
        );
        assert!(!result.stdout.contains("after"));
        // The soft cascade (SIGTERM) ends sleep at once; well inside the hard
        // grace, and nowhere near the 31s the orphan would have run.
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout returned after {elapsed:?}"
        );
        // The job is its own process-group leader (pid == pgid), and the
        // forked `sleep` lives in that group. Probing the group — not a
        // machine-wide command-line match — keeps a concurrent run of this
        // suite from failing the test.
        let pgid = pid_rx.recv().expect("the child pid is reported");
        let survivors = Command::new("pgrep")
            .args(["-g", &pgid.to_string()])
            .output()
            .expect("pgrep runs");
        assert!(
            survivors.stdout.is_empty(),
            "the forked workload survived the timeout: {}",
            String::from_utf8_lossy(&survivors.stdout)
        );
    }

    /// A group that outlives even SIGKILL (a process in uninterruptible
    /// sleep, a recycled pgid) can't be produced on demand, so the notice
    /// that reports one is tested as a formatter.
    #[test]
    fn survivors_notice_names_the_groups_and_the_recovery() {
        assert_eq!(survivors_notice(&[]), None);
        let notice = survivors_notice(&[4242, 4343]).expect("survivors are reported");
        assert!(
            notice.contains("may still be running (process group(s): 4242, 4343)"),
            "{notice}"
        );
        assert!(notice.contains("kill -KILL -<pgid>"), "{notice}");
    }

    /// Live members of process group `pgid`, killed on the way out so a
    /// failing assertion leaves nothing behind.
    #[cfg(unix)]
    fn group_survivors(pgid: u32) -> String {
        let out = Command::new("pgrep")
            .args(["-g", &pgid.to_string()])
            .output()
            .expect("pgrep runs");
        let found = String::from_utf8_lossy(&out.stdout).into_owned();
        if !found.is_empty() {
            let _ = Command::new("kill")
                .args(["-KILL", &format!("-{pgid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        found
    }

    /// Regression (#1009 review): a descendant that ignores SIGTERM and holds
    /// no output pipe outlived the timeout. The soft cascade killed the shell,
    /// the pipes closed, and the wait returned — reporting the job stopped
    /// while its workload kept running, with no SIGKILL ever sent. The
    /// teardown must reach the hard kill before the wait returns.
    #[cfg(unix)]
    #[test]
    fn run_command_timeout_kills_a_term_immune_descendant_without_pipes() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let (pid_tx, pid_rx) = mpsc::channel::<u32>();
        let started = std::time::Instant::now();
        let result = run_command(
            "sh -c 'trap \"\" TERM; exec sleep 33' >/dev/null 2>&1 & wait",
            &env,
            &dir,
            Some(Duration::from_millis(300)),
            None,
            Some(pid_tx),
            None,
        )
        .expect("a timed-out command still returns a result");
        let elapsed = started.elapsed();
        let pgid = pid_rx.recv().expect("the child pid is reported");
        let survivors = group_survivors(pgid);

        assert_eq!(result.timed_out, Some(Duration::from_millis(300)));
        assert!(
            survivors.is_empty(),
            "the TERM-immune descendant survived the timeout: {survivors}"
        );
        // SIGKILL goes out TIMEOUT_HARD_GRACE after the limit; the bound only
        // guards against waiting on the 33s sleep itself.
        assert!(
            elapsed < Duration::from_secs(25),
            "timeout returned after {elapsed:?}"
        );
    }

    /// Regression (#1009 review): a shell that exits before its limit while a
    /// process it backgrounded still holds the output pipes left the deadline
    /// unsupervised — the wait returned on the exit, and the reader join then
    /// blocked until the straggler finished, however long that took. The
    /// limit must still tear the straggler down. The shell's own exit status
    /// stands: it finished in time.
    #[cfg(unix)]
    #[test]
    fn run_command_timeout_tears_down_a_pipe_holder_left_by_an_exited_shell() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let (pid_tx, pid_rx) = mpsc::channel::<u32>();
        let started = std::time::Instant::now();
        let result = run_command(
            "sleep 34 & echo exited",
            &env,
            &dir,
            Some(Duration::from_millis(300)),
            None,
            Some(pid_tx),
            None,
        )
        .expect("the command returns a result");
        let elapsed = started.elapsed();
        let pgid = pid_rx.recv().expect("the child pid is reported");
        let survivors = group_survivors(pgid);

        assert!(
            elapsed < Duration::from_secs(5),
            "the wait outlived the limit by {elapsed:?}"
        );
        assert!(
            survivors.is_empty(),
            "the pipe holder survived the limit: {survivors}"
        );
        assert!(result.success, "the shell exited 0 before the limit");
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.timed_out, None);
        assert_eq!(result.stdout.trim(), "exited");
    }

    #[test]
    fn run_command_sends_child_pid_on_pid_sender() {
        let (pid_tx, pid_rx) = mpsc::channel::<u32>();
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        run_command(
            "true",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            None,
            Some(pid_tx),
            None,
        )
        .unwrap();
        let pid = pid_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("pid not sent");
        assert!(pid > 0, "pid should be a positive integer");
    }

    /// Regression test for #412: replacing `pre_exec(setsid)` with
    /// `Command::process_group(0)` must preserve the cancel-by-PGID
    /// invariant — the spawned shell must be a process-group leader
    /// (PID == PGID).
    #[test]
    #[cfg(unix)]
    fn run_command_child_is_process_group_leader() {
        let (line_tx, line_rx) = mpsc::channel::<(OutputKind, String)>();
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        // Both BSD and GNU `ps` accept this form; print the shell's own pid
        // and its pgid on a single line.
        run_command(
            "ps -o pid=,pgid= -p $$ | tr -s ' '",
            &env,
            &dir,
            Some(Duration::from_secs(5)),
            Some(line_tx),
            None,
            None,
        )
        .unwrap();
        let (_kind, line) = line_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("ps output");
        let mut parts = line.split_whitespace();
        let pid: i32 = parts.next().unwrap().parse().unwrap();
        let pgid: i32 = parts.next().unwrap().parse().unwrap();
        assert_eq!(
            pid, pgid,
            "child must be process-group leader for cancel-by-PGID (got pid={pid}, pgid={pgid})"
        );
    }

    // ── run_command_interactive ─────────────────────────────────────────

    #[test]
    fn run_command_interactive_success() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command_interactive("true", &env, &dir, None).unwrap();
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        // Interactive commands don't capture output.
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn run_command_interactive_failure() {
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command_interactive("exit 7", &env, &dir, None).unwrap();
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(7));
    }

    #[test]
    #[cfg(unix)]
    fn run_command_interactive_signal_death_resolves_shell_convention() {
        // A child killed by a signal carries no exit code; the shell
        // convention is 128 + N — a ^C'd passthrough job must surface 130,
        // not a synthetic -1 (255 once truncated to a u8).
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result = run_command_interactive("kill -INT $$", &env, &dir, None).unwrap();
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(130));
    }

    #[test]
    fn run_command_interactive_env_vars() {
        let mut env = HashMap::new();
        env.insert("INTERACTIVE_VAR".into(), "present".into());
        let dir = std::env::temp_dir();
        // Use test -n to verify the var is set (non-empty string).
        let result =
            run_command_interactive("test -n \"$INTERACTIVE_VAR\"", &env, &dir, None).unwrap();
        assert!(result.success);
    }

    // ── cancellation ────────────────────────────────────────────────────

    #[test]
    fn run_command_no_timeout_waits_for_completion() {
        // `timeout: None` must not fire — a short sleep completes normally.
        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let result =
            run_command("sleep 0.2; echo done", &env, &dir, None, None, None, None).unwrap();
        assert!(result.success);
        assert!(!result.cancelled);
        assert_eq!(result.stdout.trim(), "done");
    }

    #[test]
    #[cfg(unix)]
    fn run_command_soft_cancel_tears_down_child() {
        use crate::git::cancel::CancelFlag;
        use std::sync::Arc;
        use std::time::Instant;

        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let cancel = Arc::new(CancelFlag::new());

        // Raise the soft-cancel level from another thread shortly after the
        // child (a 30s sleep) starts; the cascade should tear it down well
        // before the sleep would finish.
        let flag = Arc::clone(&cancel);
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.escalate(); // 0 -> 1 (soft)
        });

        let start = Instant::now();
        let result = run_command("sleep 30", &env, &dir, None, None, None, Some(&cancel)).unwrap();
        raiser.join().ok();

        assert!(
            start.elapsed() < Duration::from_secs(10),
            "cancel should terminate the child promptly"
        );
        assert!(result.cancelled, "result must be marked cancelled");
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(130));
    }

    #[test]
    #[cfg(unix)]
    fn run_command_hard_cancel_kills_sigterm_trapping_child() {
        use crate::git::cancel::CancelFlag;
        use std::sync::Arc;
        use std::time::Instant;

        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let cancel = Arc::new(CancelFlag::new());

        // A child that traps SIGTERM and keeps running; only SIGKILL (level 2)
        // stops it.
        let flag = Arc::clone(&cancel);
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.escalate(); // -> 1 (soft, trapped)
            std::thread::sleep(Duration::from_millis(600));
            flag.escalate(); // -> 2 (hard)
        });

        let start = Instant::now();
        let result = run_command(
            "trap '' TERM; sleep 30",
            &env,
            &dir,
            None,
            None,
            None,
            Some(&cancel),
        )
        .unwrap();
        raiser.join().ok();

        assert!(
            start.elapsed() < Duration::from_secs(10),
            "hard cancel should SIGKILL the trapping child"
        );
        assert!(result.cancelled);
        assert_eq!(result.exit_code, Some(130));
    }

    #[test]
    #[cfg(unix)]
    fn interactive_soft_cancel_propagates_a_clean_self_exit() {
        // Regression: a passthrough child that fields the terminal's SIGINT and
        // then exits 0 on its own (a REPL you ^C to abort a line, then quit
        // cleanly) must propagate exit 0 — not be laundered into a 130
        // cancellation just because the interrupt flag was raised. Level 1
        // never signals the child on this path, so a self-exit is authoritative.
        use crate::git::cancel::CancelFlag;
        use std::sync::Arc;

        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let cancel = Arc::new(CancelFlag::new());

        // Raise the soft level while the child still runs; it must not flip the
        // outcome once the child exits 0 on its own.
        let flag = Arc::clone(&cancel);
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.escalate(); // 0 -> 1 (soft only, never hard)
        });

        let result =
            run_command_interactive("sleep 0.4; exit 0", &env, &dir, Some(&cancel)).unwrap();
        raiser.join().ok();

        assert!(
            !result.cancelled,
            "a soft-cancelled child that exits 0 on its own is not a cancellation"
        );
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    #[cfg(unix)]
    fn interactive_hard_cancel_marks_cancelled() {
        // The second Ctrl+C (level 2) SIGKILLs an interactive child that traps
        // the softer signals; the reap reports the run cancelled with exit 130.
        use crate::git::cancel::CancelFlag;
        use std::sync::Arc;
        use std::time::Instant;

        let env = HashMap::new();
        let dir = std::env::temp_dir();
        let cancel = Arc::new(CancelFlag::new());

        let flag = Arc::clone(&cancel);
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            flag.escalate(); // -> 1 (soft, trapped)
            flag.escalate(); // -> 2 (hard)
        });

        let start = Instant::now();
        let result =
            run_command_interactive("trap '' INT TERM; sleep 30", &env, &dir, Some(&cancel))
                .unwrap();
        raiser.join().ok();

        assert!(
            start.elapsed() < Duration::from_secs(10),
            "hard cancel must SIGKILL the trapping interactive child promptly"
        );
        assert!(result.cancelled);
        assert_eq!(result.exit_code, Some(130));
    }
}
