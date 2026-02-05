//! YAML-based hook job execution engine.
//!
//! This module executes jobs defined in YAML hook configurations.
//! It supports sequential, parallel, piped, and follow execution modes.

use super::environment::HookContext;
use super::executor::HookResult;
use super::template;
use super::yaml_config::{GroupDef, HookDef, JobDef};
use super::yaml_config_loader::get_effective_jobs;
use crate::output::Output;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Execution mode for a set of jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Run jobs one at a time (default).
    Sequential,
    /// Run all jobs concurrently.
    Parallel,
    /// Run sequentially, stop on first failure.
    Piped,
    /// Run sequentially, continue on failure, report all.
    Follow,
}

impl ExecutionMode {
    /// Determine execution mode from a hook definition.
    pub fn from_hook_def(hook: &HookDef) -> Self {
        if hook.parallel == Some(true) {
            ExecutionMode::Parallel
        } else if hook.piped == Some(true) {
            ExecutionMode::Piped
        } else if hook.follow == Some(true) {
            ExecutionMode::Follow
        } else {
            ExecutionMode::Sequential
        }
    }

    /// Determine execution mode from a group definition.
    pub fn from_group_def(group: &GroupDef) -> Self {
        if group.parallel == Some(true) {
            ExecutionMode::Parallel
        } else if group.piped == Some(true) {
            ExecutionMode::Piped
        } else {
            ExecutionMode::Sequential
        }
    }
}

/// Shared execution context passed through the job execution pipeline.
///
/// Groups together the many parameters that would otherwise be separate function arguments.
struct ExecContext<'a> {
    hook_ctx: &'a HookContext,
    hook_args: &'a [String],
    hook_env: &'a HashMap<String, String>,
    source_dir: &'a str,
    working_dir: &'a Path,
    /// Shell RC file to source before commands (from config `rc` field).
    rc: Option<&'a str>,
}

/// Execute a YAML-defined hook.
#[allow(clippy::too_many_arguments)]
pub fn execute_yaml_hook(
    hook_name: &str,
    hook_def: &HookDef,
    ctx: &HookContext,
    output: &mut dyn Output,
    hook_args: &[String],
    source_dir: &str,
    working_dir: &Path,
) -> Result<HookResult> {
    execute_yaml_hook_with_rc(
        hook_name,
        hook_def,
        ctx,
        output,
        hook_args,
        source_dir,
        working_dir,
        None,
    )
}

/// Execute a YAML-defined hook with optional RC file.
#[allow(clippy::too_many_arguments)]
pub fn execute_yaml_hook_with_rc(
    hook_name: &str,
    hook_def: &HookDef,
    ctx: &HookContext,
    output: &mut dyn Output,
    hook_args: &[String],
    source_dir: &str,
    working_dir: &Path,
    rc: Option<&str>,
) -> Result<HookResult> {
    // Check hook-level skip/only conditions
    if let Some(ref skip) = hook_def.skip {
        if let Some(reason) = super::conditions::should_skip(skip, working_dir) {
            output.debug(&format!("Skipping {hook_name}: {reason}"));
            return Ok(HookResult::skipped(reason));
        }
    }
    if let Some(ref only) = hook_def.only {
        if let Some(reason) = super::conditions::should_only_skip(only, working_dir) {
            output.debug(&format!("Skipping {hook_name}: {reason}"));
            return Ok(HookResult::skipped(reason));
        }
    }

    let mut jobs = get_effective_jobs(hook_def);

    if jobs.is_empty() {
        return Ok(HookResult::skipped("No jobs defined"));
    }

    // Filter out jobs matching exclude_tags
    if let Some(ref exclude_tags) = hook_def.exclude_tags {
        jobs.retain(|job| {
            if let Some(ref tags) = job.tags {
                !tags.iter().any(|t| exclude_tags.contains(t))
            } else {
                true
            }
        });
        if jobs.is_empty() {
            return Ok(HookResult::skipped("All jobs excluded by tags"));
        }
    }

    // Sort by priority if set
    jobs.sort_by_key(|j| j.priority.unwrap_or(0));

    let mode = ExecutionMode::from_hook_def(hook_def);

    let mode_str = mode_label(mode);
    output.step(&format!("Running {hook_name} hook ({mode_str})..."));

    let exec = ExecContext {
        hook_ctx: ctx,
        hook_args,
        hook_env: &HashMap::new(),
        source_dir,
        working_dir,
        rc,
    };

    let result = execute_jobs(&jobs, mode, &exec, output)?;

    // Check fail_on_changes after all jobs complete
    if hook_def.fail_on_changes == Some(true)
        && result.success
        && has_unstaged_changes(working_dir)?
    {
        output.warning("Working tree has unstaged changes after hook execution");
        return Ok(HookResult::failed(
            1,
            String::new(),
            "fail_on_changes: working tree has unstaged changes".to_string(),
        ));
    }

    Ok(result)
}

fn mode_label(mode: ExecutionMode) -> &'static str {
    match mode {
        ExecutionMode::Sequential => "sequential",
        ExecutionMode::Parallel => "parallel",
        ExecutionMode::Piped => "piped",
        ExecutionMode::Follow => "follow",
    }
}

/// Execute a list of jobs in the specified mode.
fn execute_jobs(
    jobs: &[JobDef],
    mode: ExecutionMode,
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
) -> Result<HookResult> {
    match mode {
        ExecutionMode::Sequential | ExecutionMode::Piped => {
            execute_sequential(jobs, exec, output, true)
        }
        ExecutionMode::Follow => execute_sequential(jobs, exec, output, false),
        ExecutionMode::Parallel => execute_parallel(jobs, exec, output),
    }
}

/// Execute jobs sequentially.
///
/// If `stop_on_failure` is true (piped mode), stops on first failure.
/// If false (follow mode), continues and reports all.
fn execute_sequential(
    jobs: &[JobDef],
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
    stop_on_failure: bool,
) -> Result<HookResult> {
    let mut any_failed = false;
    let mut last_failure: Option<HookResult> = None;

    for job in jobs {
        let job_name = job.name.as_deref().unwrap_or("(unnamed)");

        // Handle group jobs
        if let Some(ref group) = job.group {
            let group_mode = ExecutionMode::from_group_def(group);
            let group_jobs = group.jobs.as_deref().unwrap_or(&[]);
            let result = execute_jobs(group_jobs, group_mode, exec, output)?;
            if !result.success {
                if stop_on_failure {
                    return Ok(result);
                }
                any_failed = true;
                last_failure = Some(result);
            }
            continue;
        }

        let result = execute_single_job(job, exec, output)?;

        if !result.success {
            output.warning(&format!(
                "Job '{job_name}' failed (exit code: {})",
                result.exit_code.unwrap_or(-1)
            ));
            if let Some(ref text) = job.fail_text {
                output.error(text);
            }
            if stop_on_failure {
                return Ok(result);
            }
            any_failed = true;
            last_failure = Some(result);
        }
    }

    if any_failed {
        Ok(last_failure.unwrap_or_else(HookResult::success))
    } else {
        Ok(HookResult::success())
    }
}

/// Data needed to run a single job in a thread.
#[derive(Clone)]
struct ParallelJobData {
    name: String,
    cmd: String,
    env: HashMap<String, String>,
    working_dir: PathBuf,
}

/// Execute jobs in parallel using threads.
fn execute_parallel(
    jobs: &[JobDef],
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
) -> Result<HookResult> {
    use std::sync::{Arc, Mutex};
    use std::thread;

    // For interactive jobs, run sequentially
    let (interactive, parallel): (Vec<_>, Vec<_>) =
        jobs.iter().partition(|j| j.interactive == Some(true));

    // Collect job data for threads (file filtering done on main thread)
    let mut job_data: Vec<ParallelJobData> = Vec::new();
    for job in &parallel {
        let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());

        // File filtering
        let filtered = resolve_file_list(job, exec)?;
        if let Some(ref files) = filtered {
            if files.is_empty() {
                output.debug(&format!("Skipping job '{name}': no matching files"));
                continue;
            }
        }

        let cmd = resolve_command_with_files(
            job,
            exec.hook_ctx,
            Some(&name),
            exec.hook_args,
            exec.source_dir,
            filtered.as_deref(),
        );
        let mut env = exec.hook_env.clone();
        if let Some(ref job_env) = job.env {
            env.extend(job_env.clone());
        }
        let wd = job
            .root
            .as_ref()
            .map(|r| exec.working_dir.join(r))
            .unwrap_or_else(|| exec.working_dir.to_path_buf());
        job_data.push(ParallelJobData {
            name,
            cmd,
            env,
            working_dir: wd,
        });
    }

    let ctx_clone = exec.hook_ctx.clone();

    // Use indexed results to preserve definition order in output
    type IndexedResult = (usize, String, Result<HookResult>);
    let results: Arc<Mutex<Vec<IndexedResult>>> = Arc::new(Mutex::new(Vec::new()));

    // Limit concurrency to avoid thread explosion
    let max_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Process in batches if many jobs
    for batch in job_data.chunks(max_threads) {
        let handles: Vec<_> = batch
            .iter()
            .enumerate()
            .map(|(batch_idx, data)| {
                let results = Arc::clone(&results);
                let ctx_for_thread = ctx_clone.clone();
                let data = data.clone();
                let idx = batch_idx;

                thread::spawn(move || {
                    let result = run_shell_command(
                        &data.cmd,
                        &data.env,
                        &data.working_dir,
                        &ctx_for_thread,
                        Duration::from_secs(300),
                    );
                    results.lock().unwrap().push((idx, data.name, result));
                })
            })
            .collect();

        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Thread panicked"))?;
        }
    }

    // Collect results in definition order
    let mut results = Arc::try_unwrap(results)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap results"))?
        .into_inner()
        .map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
    results.sort_by_key(|(idx, _, _)| *idx);

    // Report results in definition order (buffered output)
    let mut any_failed = false;
    for (_, name, result) in &results {
        match result {
            Ok(r) if !r.success => {
                if !r.stderr.is_empty() {
                    output.warning(&format!("Job '{name}' output:\n{}", r.stderr.trim()));
                }
                output.warning(&format!(
                    "Job '{name}' failed (exit code: {})",
                    r.exit_code.unwrap_or(-1)
                ));
                any_failed = true;
            }
            Ok(r) if !r.stdout.is_empty() || !r.stderr.is_empty() => {
                output.debug(&format!(
                    "Job '{name}' completed (stdout: {} bytes, stderr: {} bytes)",
                    r.stdout.len(),
                    r.stderr.len()
                ));
            }
            Err(e) => {
                output.error(&format!("Job '{name}' error: {e}"));
                any_failed = true;
            }
            _ => {}
        }
    }

    // Run interactive jobs sequentially
    for job in &interactive {
        let result = execute_single_job(job, exec, output)?;
        if !result.success {
            any_failed = true;
        }
    }

    if any_failed {
        Ok(HookResult::failed(1, String::new(), String::new()))
    } else {
        Ok(HookResult::success())
    }
}

/// Execute a single job.
fn execute_single_job(
    job: &JobDef,
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
) -> Result<HookResult> {
    let job_name = job.name.as_deref().unwrap_or("(unnamed)");

    // Job-level skip/only conditions
    if let Some(ref skip) = job.skip {
        if let Some(reason) = super::conditions::should_skip(skip, exec.working_dir) {
            output.debug(&format!("Skipping job '{job_name}': {reason}"));
            return Ok(HookResult::skipped(reason));
        }
    }
    if let Some(ref only) = job.only {
        if let Some(reason) = super::conditions::should_only_skip(only, exec.working_dir) {
            output.debug(&format!("Skipping job '{job_name}': {reason}"));
            return Ok(HookResult::skipped(reason));
        }
    }

    // File filtering: get file list, filter by glob/type/exclude, skip if empty
    let filtered_files = resolve_file_list(job, exec)?;
    if let Some(ref files) = filtered_files {
        if files.is_empty() {
            output.debug(&format!("Skipping job '{job_name}': no matching files"));
            return Ok(HookResult::skipped("No matching files"));
        }
    }

    let cmd = resolve_command_with_files(
        job,
        exec.hook_ctx,
        Some(job_name),
        exec.hook_args,
        exec.source_dir,
        filtered_files.as_deref(),
    );

    if cmd.is_empty() {
        return Ok(HookResult::skipped("Empty command"));
    }

    output.debug(&format!("Executing job '{job_name}': {cmd}"));

    // Build environment
    let mut env = exec.hook_env.clone();
    if let Some(ref job_env) = job.env {
        env.extend(job_env.clone());
    }

    // Resolve working directory
    let wd = if let Some(ref root) = job.root {
        exec.working_dir.join(root)
    } else {
        exec.working_dir.to_path_buf()
    };

    // Wrap command with RC file if configured
    let cmd = if let Some(rc) = exec.rc {
        format!("source {rc} && {cmd}")
    } else {
        cmd
    };

    let is_interactive = job.interactive == Some(true) || job.use_stdin == Some(true);

    let result = if is_interactive {
        run_interactive_command(&cmd, &env, &wd, exec.hook_ctx)
    } else {
        run_shell_command(&cmd, &env, &wd, exec.hook_ctx, Duration::from_secs(300))
    }?;

    // stage_fixed: re-stage modified files after successful run
    if job.stage_fixed == Some(true) && result.success {
        stage_fixed_files(exec.working_dir)?;
    }

    Ok(result)
}

/// Resolve the file list for a job, applying glob/file_type/exclude filters.
///
/// Returns `None` if no file filtering is configured (job runs unconditionally).
/// Returns `Some(files)` if filtering is active (job skips if empty).
fn resolve_file_list(job: &JobDef, exec: &ExecContext<'_>) -> Result<Option<Vec<String>>> {
    use super::files;

    let has_filter = job.glob.is_some() || job.file_types.is_some() || job.files.is_some();
    if !has_filter {
        return Ok(None);
    }

    // Get the base file list
    let mut file_list = if let Some(ref cmd) = job.files {
        files::custom_file_command(exec.working_dir, cmd)?
    } else {
        files::staged_files(exec.working_dir)?
    };

    // Apply glob filter
    if let Some(ref glob) = job.glob {
        file_list = files::filter_by_glob(&file_list, glob)?;
    }

    // Apply file type filter
    if let Some(ref ft) = job.file_types {
        file_list = files::filter_by_file_type(&file_list, ft);
    }

    // Apply exclude filter
    if let Some(ref exclude) = job.exclude {
        file_list = files::exclude_files(&file_list, exclude)?;
    }

    Ok(Some(file_list))
}

/// Resolve the shell command for a job, handling both `run` and `script`.
#[cfg(test)]
fn resolve_command(
    job: &JobDef,
    ctx: &HookContext,
    job_name: Option<&str>,
    hook_args: &[String],
    source_dir: &str,
) -> String {
    resolve_command_with_files(job, ctx, job_name, hook_args, source_dir, None)
}

/// Resolve the shell command with optional filtered file list.
fn resolve_command_with_files(
    job: &JobDef,
    ctx: &HookContext,
    job_name: Option<&str>,
    hook_args: &[String],
    source_dir: &str,
    filtered_files: Option<&[String]>,
) -> String {
    if let Some(ref run) = job.run {
        template::substitute_with_files(run, ctx, job_name, hook_args, filtered_files)
    } else if let Some(ref script) = job.script {
        let script_path = format!("{source_dir}/{script}");
        if let Some(ref runner) = job.runner {
            let args = job.args.as_deref().unwrap_or("");
            let cmd = if args.is_empty() {
                format!("{runner} {script_path}")
            } else {
                format!("{runner} {script_path} {args}")
            };
            template::substitute_with_files(&cmd, ctx, job_name, hook_args, filtered_files)
        } else {
            let args = job.args.as_deref().unwrap_or("");
            let cmd = if args.is_empty() {
                script_path
            } else {
                format!("{script_path} {args}")
            };
            template::substitute_with_files(&cmd, ctx, job_name, hook_args, filtered_files)
        }
    } else {
        String::new()
    }
}

/// Run a shell command and capture its output.
fn run_shell_command(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
    timeout: Duration,
) -> Result<HookResult> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);

    // Set daft environment variables
    let hook_env = super::environment::HookEnvironment::from_context(ctx);
    command.envs(hook_env.vars());

    // Set extra environment variables
    command.envs(extra_env);

    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to spawn: {cmd}"))?;

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let mut stdout_content = String::new();
    let mut stderr_content = String::new();

    if let Some(stdout) = stdout_handle {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            stdout_content.push_str(&line);
            stdout_content.push('\n');
        }
    }

    if let Some(stderr) = stderr_handle {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            stderr_content.push_str(&line);
            stderr_content.push('\n');
        }
    }

    let status = wait_with_timeout(&mut child, timeout)
        .with_context(|| format!("Command execution failed: {cmd}"))?;

    let exit_code = status.code().unwrap_or(-1);

    if status.success() {
        Ok(HookResult {
            success: true,
            exit_code: Some(exit_code),
            stdout: stdout_content,
            stderr: stderr_content,
            skipped: false,
            skip_reason: None,
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
fn run_interactive_command(
    cmd: &str,
    extra_env: &HashMap<String, String>,
    working_dir: &Path,
    ctx: &HookContext,
) -> Result<HookResult> {
    let mut command = Command::new("sh");
    command.args(["-c", cmd]);
    command.current_dir(working_dir);

    let hook_env = super::environment::HookEnvironment::from_context(ctx);
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
        })
    } else {
        Ok(HookResult::failed(exit_code, String::new(), String::new()))
    }
}

/// Check if the working tree has unstaged changes.
fn has_unstaged_changes(worktree: &Path) -> Result<bool> {
    let status = std::process::Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(worktree)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Failed to check for unstaged changes")?;
    Ok(!status.success())
}

/// Re-stage files that were previously staged and modified by a job.
fn stage_fixed_files(worktree: &Path) -> Result<()> {
    // Get the list of staged files, then add any that have been modified
    let staged = super::files::staged_files(worktree)?;
    if staged.is_empty() {
        return Ok(());
    }

    // Check which staged files have been modified (unstaged changes)
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(worktree)
        .output()
        .context("Failed to check modified files")?;

    let modified: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();

    // Re-stage files that were both staged and modified
    let to_restage: Vec<&String> = staged.iter().filter(|f| modified.contains(f)).collect();
    if to_restage.is_empty() {
        return Ok(());
    }

    let args: Vec<&str> = std::iter::once("add")
        .chain(to_restage.iter().map(|s| s.as_str()))
        .collect();

    std::process::Command::new("git")
        .args(&args)
        .current_dir(worktree)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Failed to re-stage fixed files")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookType;
    use crate::output::TestOutput;

    fn make_ctx() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout-branch",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        )
    }

    #[test]
    fn test_execution_mode_from_hook_def() {
        let def = HookDef {
            parallel: Some(true),
            ..Default::default()
        };
        assert_eq!(ExecutionMode::from_hook_def(&def), ExecutionMode::Parallel);

        let def = HookDef {
            piped: Some(true),
            ..Default::default()
        };
        assert_eq!(ExecutionMode::from_hook_def(&def), ExecutionMode::Piped);

        let def = HookDef {
            follow: Some(true),
            ..Default::default()
        };
        assert_eq!(ExecutionMode::from_hook_def(&def), ExecutionMode::Follow);

        let def = HookDef::default();
        assert_eq!(
            ExecutionMode::from_hook_def(&def),
            ExecutionMode::Sequential
        );
    }

    #[test]
    fn test_resolve_command_run() {
        let job = JobDef {
            run: Some("echo {branch}".to_string()),
            ..Default::default()
        };
        let ctx = make_ctx();
        let cmd = resolve_command(&job, &ctx, Some("test"), &[], ".daft");
        assert_eq!(cmd, "echo feature/new");
    }

    #[test]
    fn test_resolve_command_script() {
        let job = JobDef {
            script: Some("hooks/setup.sh".to_string()),
            runner: Some("bash".to_string()),
            ..Default::default()
        };
        let ctx = make_ctx();
        let cmd = resolve_command(&job, &ctx, Some("test"), &[], ".daft");
        assert_eq!(cmd, "bash .daft/hooks/setup.sh");
    }

    #[test]
    fn test_resolve_command_script_with_args() {
        let job = JobDef {
            script: Some("hooks/setup.sh".to_string()),
            runner: Some("bash".to_string()),
            args: Some("--verbose".to_string()),
            ..Default::default()
        };
        let ctx = make_ctx();
        let cmd = resolve_command(&job, &ctx, Some("test"), &[], ".daft");
        assert_eq!(cmd, "bash .daft/hooks/setup.sh --verbose");
    }

    #[test]
    fn test_execute_yaml_hook_empty_jobs() {
        let hook_def = HookDef::default();
        let ctx = make_ctx();
        let mut output = TestOutput::default();

        let result = execute_yaml_hook(
            "test-hook",
            &hook_def,
            &ctx,
            &mut output,
            &[],
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.skipped);
    }

    #[test]
    fn test_execute_yaml_hook_simple_job() {
        let hook_def = HookDef {
            jobs: Some(vec![JobDef {
                name: Some("test".to_string()),
                run: Some("true".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let ctx = make_ctx();
        let mut output = TestOutput::default();

        let result = execute_yaml_hook(
            "test-hook",
            &hook_def,
            &ctx,
            &mut output,
            &[],
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_execute_yaml_hook_failing_job() {
        let hook_def = HookDef {
            jobs: Some(vec![JobDef {
                name: Some("fail".to_string()),
                run: Some("false".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let ctx = make_ctx();
        let mut output = TestOutput::default();

        let result = execute_yaml_hook(
            "test-hook",
            &hook_def,
            &ctx,
            &mut output,
            &[],
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_execute_yaml_hook_piped_stops_on_failure() {
        let hook_def = HookDef {
            piped: Some(true),
            jobs: Some(vec![
                JobDef {
                    name: Some("fail".to_string()),
                    run: Some("false".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("should-not-run".to_string()),
                    run: Some("echo should-not-run".to_string()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let ctx = make_ctx();
        let mut output = TestOutput::default();

        let result = execute_yaml_hook(
            "test-hook",
            &hook_def,
            &ctx,
            &mut output,
            &[],
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_execute_yaml_hook_follow_continues_on_failure() {
        let hook_def = HookDef {
            follow: Some(true),
            jobs: Some(vec![
                JobDef {
                    name: Some("fail".to_string()),
                    run: Some("false".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("still-runs".to_string()),
                    run: Some("true".to_string()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let ctx = make_ctx();
        let mut output = TestOutput::default();

        let result = execute_yaml_hook(
            "test-hook",
            &hook_def,
            &ctx,
            &mut output,
            &[],
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_execute_yaml_hook_with_env() {
        let hook_def = HookDef {
            jobs: Some(vec![JobDef {
                name: Some("env-test".to_string()),
                run: Some("test \"$MY_VAR\" = hello".to_string()),
                env: Some({
                    let mut m = HashMap::new();
                    m.insert("MY_VAR".to_string(), "hello".to_string());
                    m
                }),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let ctx = make_ctx();
        let mut output = TestOutput::default();

        let result = execute_yaml_hook(
            "test-hook",
            &hook_def,
            &ctx,
            &mut output,
            &[],
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }
}
