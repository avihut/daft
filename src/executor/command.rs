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
}

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
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);
    command.envs(env);

    // Non-interactive commands must not inherit stdin -- a child process
    // (e.g. mise, cargo) might block waiting for input that will never come.
    command.stdin(Stdio::null());
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

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

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
    // cancel flag. Killing the child (either path) closes the pipes and
    // unblocks the reader threads above.
    let outcome = wait_child(&mut child, timeout, cancel)
        .with_context(|| format!("Command execution failed: {cmd}"))?;

    let stdout_content = stdout_thread.join().unwrap_or_default();
    let stderr_content = stderr_thread.join().unwrap_or_default();

    match outcome {
        WaitOutcome::Exited(status) => Ok(CommandResult {
            success: status.success(),
            exit_code: Some(status.code().unwrap_or(-1)),
            stdout: stdout_content,
            stderr: stderr_content,
            cancelled: false,
        }),
        WaitOutcome::Cancelled => Ok(CommandResult {
            success: false,
            // Normalize the signal-death status (-1) to the conventional
            // 128 + SIGINT so downstream exit-code propagation is stable.
            exit_code: Some(130),
            stdout: stdout_content,
            stderr: stderr_content,
            cancelled: true,
        }),
    }
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
/// would tear down daft's own group. `cancel: None` keeps the original
/// blocking `status()` path.
pub fn run_command_interactive(
    cmd: &str,
    env: &HashMap<String, String>,
    working_dir: &Path,
    cancel: Option<&crate::git::cancel::CancelFlag>,
) -> Result<CommandResult> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);
    command.envs(env);

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
            exit_code: Some(status.code().unwrap_or(-1)),
            stdout: String::new(),
            stderr: String::new(),
            cancelled: false,
        });
    };

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to run interactive command: {cmd}"))?;

    match wait_interactive_child(&mut child, cancel)? {
        WaitOutcome::Exited(status) => Ok(CommandResult {
            success: status.success(),
            exit_code: Some(status.code().unwrap_or(-1)),
            stdout: String::new(),
            stderr: String::new(),
            cancelled: false,
        }),
        WaitOutcome::Cancelled => Ok(CommandResult {
            success: false,
            exit_code: Some(130),
            stdout: String::new(),
            stderr: String::new(),
            cancelled: true,
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────

/// Terminal outcome of waiting on a child: it exited on its own, or it was
/// torn down by a user cancellation.
enum WaitOutcome {
    Exited(ExitStatus),
    Cancelled,
}

/// Wait for a captured-output child, polling at 100ms intervals.
///
/// Honors two independent deadlines:
/// - `cancel` (checked first): once the flag is raised, the child's process
///   tree is torn down — SIGTERM+SIGCONT at level 1, SIGKILL at level 2 — via
///   [`GroupCascade`], and the eventual reap returns [`WaitOutcome::Cancelled`].
/// - `timeout`: when `Some(t)` and exceeded, the child is killed and an error
///   is returned (the pre-existing hook timeout semantics). `None` waits
///   forever (task jobs).
///
/// `cancel: None` polls no flag and is behaviorally identical to the previous
/// `wait_with_timeout`.
fn wait_child(
    child: &mut std::process::Child,
    timeout: Option<Duration>,
    cancel: Option<&crate::git::cancel::CancelFlag>,
) -> Result<WaitOutcome> {
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    let poll_interval = Duration::from_millis(100);
    let mut cancelling = false;
    #[cfg(unix)]
    let mut teardown: Option<CancelTeardown> = None;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(if cancelling {
                WaitOutcome::Cancelled
            } else {
                WaitOutcome::Exited(status)
            });
        }

        // Cancellation takes precedence over the timeout deadline.
        if let Some(flag) = cancel
            && flag.is_cancelled()
        {
            cancelling = true;
            #[cfg(unix)]
            {
                teardown
                    .get_or_insert_with(|| CancelTeardown::new(child.id()))
                    .tick(flag.level());
            }
            #[cfg(not(unix))]
            {
                // No process-group teardown off-unix; direct kill is the
                // best available escalation.
                child.kill().ok();
            }
            thread::sleep(poll_interval);
            continue;
        }

        if let Some(t) = timeout
            && start.elapsed() >= t
        {
            child.kill().ok();
            anyhow::bail!("Command timed out after {t:?}");
        }
        thread::sleep(poll_interval);
    }
}

/// Wait for an interactive (stdio-inherited) child under cancellation.
///
/// The child shares the caller's foreground process group, so it already
/// received the terminal's SIGINT on the first Ctrl+C — level 1 is therefore
/// a deliberate no-op (a redundant SIGTERM would defeat graceful-shutdown
/// handlers). Level 2 sends a direct SIGKILL to the child pid; `killpg` is
/// off-limits here because the child is in daft's own group.
#[cfg(unix)]
fn wait_interactive_child(
    child: &mut std::process::Child,
    cancel: &crate::git::cancel::CancelFlag,
) -> Result<WaitOutcome> {
    use std::thread;

    let poll_interval = Duration::from_millis(100);
    let mut cancelling = false;
    let mut hard_sent = false;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(if cancelling {
                WaitOutcome::Cancelled
            } else {
                WaitOutcome::Exited(status)
            });
        }
        let level = cancel.level();
        if level >= 1 {
            cancelling = true;
            if level >= 2 && !hard_sent {
                crate::git::cancel::kill_pid(child.id(), true);
                hard_sent = true;
            }
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
    let mut cancelling = false;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(if cancelling {
                WaitOutcome::Cancelled
            } else {
                WaitOutcome::Exited(status)
            });
        }
        if cancel.is_cancelled() {
            cancelling = true;
            if cancel.level() >= 2 {
                child.kill().ok();
            }
        }
        thread::sleep(poll_interval);
    }
}

/// Escalating process-tree teardown state for a captured-output child under
/// cancellation. Wraps a [`GroupCascade`] with the tick cadence: the first
/// soft tick fires immediately, then every ~500ms while at level 1; a single
/// hard tick fires on the transition to level 2.
#[cfg(unix)]
struct CancelTeardown {
    cascade: crate::git::cancel::GroupCascade,
    last_soft: Option<std::time::Instant>,
    hard_sent: bool,
}

#[cfg(unix)]
impl CancelTeardown {
    fn new(root_pid: u32) -> Self {
        Self {
            cascade: crate::git::cancel::GroupCascade::new(root_pid),
            last_soft: None,
            hard_sent: false,
        }
    }

    fn tick(&mut self, level: usize) {
        if level >= 2 {
            if !self.hard_sent {
                self.cascade.hard_tick();
                self.hard_sent = true;
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
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("timed out"),
            "expected timeout error, got: {msg}"
        );
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
}
