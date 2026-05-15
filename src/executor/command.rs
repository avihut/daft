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
pub fn run_command(
    cmd: &str,
    env: &HashMap<String, String>,
    working_dir: &Path,
    timeout: Duration,
    line_sender: Option<std::sync::mpsc::Sender<(OutputKind, String)>>,
    pid_sender: Option<std::sync::mpsc::Sender<u32>>,
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

    // Wait with timeout -- if the child exceeds the deadline it is killed,
    // which closes the pipes and unblocks the reader threads above.
    let status = wait_with_timeout(&mut child, timeout)
        .with_context(|| format!("Command execution failed: {cmd}"))?;

    let stdout_content = stdout_thread.join().unwrap_or_default();
    let stderr_content = stderr_thread.join().unwrap_or_default();

    let exit_code = status.code().unwrap_or(-1);

    Ok(CommandResult {
        success: status.success(),
        exit_code: Some(exit_code),
        stdout: stdout_content,
        stderr: stderr_content,
    })
}

/// Spawn a shell command with inherited stdin/stdout/stderr (interactive).
///
/// The command is executed via `sh -c <cmd>`.  No output is captured; the
/// child process shares the terminal with the parent.
///
/// The caller is responsible for building the complete set of environment
/// variables and passing them in `env`.
pub fn run_command_interactive(
    cmd: &str,
    env: &HashMap<String, String>,
    working_dir: &Path,
) -> Result<CommandResult> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);
    command.envs(env);

    // Inherit stdin/stdout/stderr for interactive mode
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let status = command
        .status()
        .with_context(|| format!("Failed to run interactive command: {cmd}"))?;

    let exit_code = status.code().unwrap_or(-1);

    Ok(CommandResult {
        success: status.success(),
        exit_code: Some(exit_code),
        stdout: String::new(),
        stderr: String::new(),
    })
}

// ─────────────────────────────────────────────────────────────────────────
// Private helpers
// ─────────────────────────────────────────────────────────────────────────

/// Wait for a child process, polling at 100ms intervals up to `timeout`.
///
/// If the timeout is reached the child is killed and an error is returned.
fn wait_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Result<ExitStatus> {
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    let poll_interval = Duration::from_millis(100);

    loop {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None => {
                if start.elapsed() >= timeout {
                    child.kill().ok();
                    anyhow::bail!("Command timed out after {timeout:?}");
                }
                thread::sleep(poll_interval);
            }
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
        let result =
            run_command("echo hello", &env, &dir, Duration::from_secs(5), None, None).unwrap();
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
            Duration::from_secs(5),
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
        let result =
            run_command("exit 42", &env, &dir, Duration::from_secs(5), None, None).unwrap();
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
            Duration::from_secs(5),
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
        let result = run_command("pwd", &env, &dir, Duration::from_secs(5), None, None).unwrap();
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
            Duration::from_secs(5),
            Some(tx),
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
            Duration::from_secs(5),
            Some(tx),
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
            Duration::from_millis(200),
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
            Duration::from_secs(5),
            None,
            Some(pid_tx),
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
            Duration::from_secs(5),
            Some(line_tx),
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
        let result = run_command_interactive("true", &env, &dir).unwrap();
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
        let result = run_command_interactive("exit 7", &env, &dir).unwrap();
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(7));
    }

    #[test]
    fn run_command_interactive_env_vars() {
        let mut env = HashMap::new();
        env.insert("INTERACTIVE_VAR".into(), "present".into());
        let dir = std::env::temp_dir();
        // Use test -n to verify the var is set (non-empty string).
        let result = run_command_interactive("test -n \"$INTERACTIVE_VAR\"", &env, &dir).unwrap();
        assert!(result.success);
    }
}
