use crate::hooks::environment::HookContext;
use crate::hooks::executor::HookResult;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Run a shell command, capture its output, and optionally stream lines
/// through the provided channel.
pub(crate) fn run_shell_command_with_callback(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
    timeout: Duration,
    line_sender: Option<std::sync::mpsc::Sender<String>>,
) -> Result<HookResult> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);

    // Set daft environment variables
    let hook_env = crate::hooks::environment::HookEnvironment::from_context(ctx);
    command.envs(hook_env.vars());

    // Set extra environment variables
    command.envs(extra_env);

    // Non-interactive commands must not inherit stdin — a child process
    // (e.g. mise, cargo) might block waiting for input that will never come.
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to spawn: {cmd}"))?;

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let tx_stdout = line_sender.clone();
    let tx_stderr = line_sender;

    // Read stdout and stderr in separate threads so they don't block the
    // timeout.  Previously the reads were sequential on the main thread,
    // which meant `wait_with_timeout` was unreachable until the child
    // closed its pipes — effectively making the timeout dead code.
    let stdout_thread = std::thread::spawn(move || {
        let mut content = String::new();
        if let Some(stdout) = stdout_handle {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(ref tx) = tx_stdout {
                    tx.send(line.clone()).ok();
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
                    tx.send(line.clone()).ok();
                }
                content.push_str(&line);
                content.push('\n');
            }
        }
        content
    });

    // Wait with timeout — if the child exceeds the deadline it is killed,
    // which closes the pipes and unblocks the reader threads above.
    let status = wait_with_timeout(&mut child, timeout)
        .with_context(|| format!("Command execution failed: {cmd}"))?;

    let stdout_content = stdout_thread.join().unwrap_or_default();
    let stderr_content = stderr_thread.join().unwrap_or_default();

    let exit_code = status.code().unwrap_or(-1);

    if status.success() {
        Ok(HookResult {
            success: true,
            exit_code: Some(exit_code),
            stdout: stdout_content,
            stderr: stderr_content,
            skipped: false,
            skip_reason: None,
            skip_ran_command: false,
        })
    } else {
        Ok(HookResult::failed(
            exit_code,
            stdout_content,
            stderr_content,
        ))
    }
}

/// Wait for a child process with a timeout.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
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

/// Run a command with stdin/stdout inherited (for interactive jobs).
pub(crate) fn run_interactive_command(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
) -> Result<HookResult> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);

    let hook_env = crate::hooks::environment::HookEnvironment::from_context(ctx);
    command.envs(hook_env.vars());
    command.envs(extra_env);

    // Inherit stdin/stdout/stderr for interactive mode
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let status = command
        .status()
        .with_context(|| format!("Failed to run interactive command: {cmd}"))?;

    let exit_code = status.code().unwrap_or(-1);
    if status.success() {
        Ok(HookResult {
            success: true,
            exit_code: Some(exit_code),
            stdout: String::new(),
            stderr: String::new(),
            skipped: false,
            skip_reason: None,
            skip_ran_command: false,
        })
    } else {
        Ok(HookResult::failed(exit_code, String::new(), String::new()))
    }
}
