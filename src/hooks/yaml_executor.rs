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
    /// Run jobs one at a time.
    Sequential,
    /// Run all jobs concurrently (default).
    Parallel,
    /// Run sequentially, stop on first failure.
    Piped,
    /// Run sequentially, continue on failure, report all.
    Follow,
}

impl ExecutionMode {
    /// Determine execution mode from a hook definition.
    pub fn from_hook_def(hook: &HookDef) -> Self {
        if hook.piped == Some(true) {
            ExecutionMode::Piped
        } else if hook.follow == Some(true) {
            ExecutionMode::Follow
        } else if hook.parallel == Some(false) {
            ExecutionMode::Sequential
        } else {
            ExecutionMode::Parallel
        }
    }

    /// Determine execution mode from a group definition.
    pub fn from_group_def(group: &GroupDef) -> Self {
        if group.piped == Some(true) {
            ExecutionMode::Piped
        } else if group.parallel == Some(false) {
            ExecutionMode::Sequential
        } else {
            ExecutionMode::Parallel
        }
    }
}

/// Shared execution context passed through the job execution pipeline.
///
/// Groups together the many parameters that would otherwise be separate function arguments.
struct ExecContext<'a> {
    hook_ctx: &'a HookContext,
    hook_env: &'a HashMap<String, String>,
    source_dir: &'a str,
    working_dir: &'a Path,
    /// Shell RC file to source before commands (from config `rc` field).
    rc: Option<&'a str>,
}

/// Execute a YAML-defined hook.
pub fn execute_yaml_hook(
    hook_name: &str,
    hook_def: &HookDef,
    ctx: &HookContext,
    output: &mut dyn Output,
    source_dir: &str,
    working_dir: &Path,
) -> Result<HookResult> {
    execute_yaml_hook_with_rc(
        hook_name,
        hook_def,
        ctx,
        output,
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
        hook_env: &HashMap::new(),
        source_dir,
        working_dir,
        rc,
    };

    let result = execute_jobs(&jobs, mode, &exec, output)?;

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
    // If any job has `needs`, use the dependency-aware executor
    let has_deps = jobs
        .iter()
        .any(|j| j.needs.as_ref().is_some_and(|n| !n.is_empty()));

    if has_deps {
        return execute_with_dependencies(jobs, mode, exec, output);
    }

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

    // Collect job data for threads
    let mut job_data: Vec<ParallelJobData> = Vec::new();
    for job in &parallel {
        let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());

        let cmd = resolve_command(job, exec.hook_ctx, Some(&name), exec.source_dir);
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

// ─────────────────────────────────────────────────────────────────────────
// Dependency-aware (DAG) execution
// ─────────────────────────────────────────────────────────────────────────

/// Status of a job in the DAG executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Skipped,
    Failed,
    DepFailed,
}

/// Dispatch to the appropriate DAG executor based on execution mode.
fn execute_with_dependencies(
    jobs: &[JobDef],
    mode: ExecutionMode,
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
) -> Result<HookResult> {
    match mode {
        ExecutionMode::Parallel => execute_dag_parallel(jobs, exec, output),
        ExecutionMode::Piped | ExecutionMode::Sequential => {
            execute_dag_sequential(jobs, exec, output, true)
        }
        ExecutionMode::Follow => execute_dag_sequential(jobs, exec, output, false),
    }
}

/// Build the dependency graph from a list of jobs.
///
/// Returns:
/// - `name_to_idx`: map from job name to index
/// - `dependents`: `dependents[i]` = list of job indices that depend on job `i`
/// - `in_degree`: number of unsatisfied dependencies per job
fn build_dag(jobs: &[JobDef]) -> (HashMap<String, usize>, Vec<Vec<usize>>, Vec<usize>) {
    let name_to_idx: HashMap<String, usize> = jobs
        .iter()
        .enumerate()
        .filter_map(|(i, j)| j.name.as_ref().map(|n| (n.clone(), i)))
        .collect();

    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); jobs.len()];
    let mut in_degree: Vec<usize> = vec![0; jobs.len()];

    for (i, job) in jobs.iter().enumerate() {
        if let Some(ref needs) = job.needs {
            for dep_name in needs {
                if let Some(&dep_idx) = name_to_idx.get(dep_name) {
                    dependents[dep_idx].push(i);
                    in_degree[i] += 1;
                }
            }
        }
    }

    (name_to_idx, dependents, in_degree)
}

/// Execute jobs respecting dependency ordering, with maximum parallelism.
///
/// Uses `std::thread::scope` + `Mutex<DagState>` + `Condvar` for a worker pool.
fn execute_dag_parallel(
    jobs: &[JobDef],
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
) -> Result<HookResult> {
    use std::sync::{Condvar, Mutex};

    let n = jobs.len();
    let (_name_to_idx, dependents, in_degree) = build_dag(jobs);

    // Pre-compute parallel job data for non-interactive, non-group jobs
    let job_data: Vec<Option<ParallelJobData>> = jobs
        .iter()
        .map(|job| {
            if job.interactive == Some(true) || job.group.is_some() {
                return None;
            }
            let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());
            let cmd = resolve_command(job, exec.hook_ctx, Some(&name), exec.source_dir);
            let mut env = exec.hook_env.clone();
            if let Some(ref job_env) = job.env {
                env.extend(job_env.clone());
            }
            let wd = job
                .root
                .as_ref()
                .map(|r| exec.working_dir.join(r))
                .unwrap_or_else(|| exec.working_dir.to_path_buf());
            Some(ParallelJobData {
                name,
                cmd,
                env,
                working_dir: wd,
            })
        })
        .collect();

    struct DagState {
        ready: Vec<usize>,
        status: Vec<JobStatus>,
        in_degree: Vec<usize>,
        active: usize,
        done: usize,
    }

    let state = Mutex::new(DagState {
        ready: (0..n).filter(|&i| in_degree[i] == 0).collect(),
        status: vec![JobStatus::Pending; n],
        in_degree,
        active: 0,
        done: 0,
    });
    let cvar = Condvar::new();

    // Sort initial ready set by priority
    {
        let mut s = state.lock().unwrap();
        s.ready
            .sort_by_key(|&i| std::cmp::Reverse(jobs[i].priority.unwrap_or(0)));
    }

    let max_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Collect results: (index, name, HookResult or error message)
    type ThreadResult = (usize, String, std::result::Result<HookResult, String>);
    let results_collector: Mutex<Vec<ThreadResult>> = Mutex::new(Vec::new());

    let ctx_clone = exec.hook_ctx.clone();
    let rc = exec.rc.map(|s| s.to_string());

    let dependents_ref = &dependents;

    std::thread::scope(|scope| {
        for _ in 0..max_workers {
            let state = &state;
            let cvar = &cvar;
            let job_data = &job_data;
            let results_collector = &results_collector;
            let ctx_clone = &ctx_clone;
            let rc = &rc;
            let dependents = dependents_ref;

            scope.spawn(move || {
                loop {
                    let job_idx;
                    {
                        let mut s = state.lock().unwrap();

                        // Try to find a non-interactive, non-group job from ready queue
                        loop {
                            let pos = s.ready.iter().rposition(|&i| {
                                jobs[i].interactive != Some(true) && jobs[i].group.is_none()
                            });

                            if let Some(pos) = pos {
                                job_idx = s.ready.remove(pos);
                                s.status[job_idx] = JobStatus::Running;
                                s.active += 1;
                                break;
                            }

                            // Nothing suitable for worker threads.
                            // Exit if: no active workers AND no non-interactive
                            // jobs in the ready queue (remaining are interactive/group
                            // which will be handled by the main thread after scope exits).
                            let has_parallel_ready = s.ready.iter().any(|&i| {
                                jobs[i].interactive != Some(true) && jobs[i].group.is_none()
                            });

                            if !has_parallel_ready && s.active == 0 {
                                return;
                            }

                            // Wait for something to change
                            s = cvar.wait(s).unwrap();
                        }
                    }

                    // Execute the job outside the lock
                    let job = &jobs[job_idx];
                    let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());

                    // Check skip/only conditions
                    let skip_result = check_skip_conditions(job, exec.working_dir);
                    let result = if let Some(reason) = skip_result {
                        Ok(HookResult::skipped(reason))
                    } else if let Some(ref data) = job_data[job_idx] {
                        let cmd = if let Some(ref rc) = rc {
                            format!("source {rc} && {}", data.cmd)
                        } else {
                            data.cmd.clone()
                        };
                        run_shell_command(
                            &cmd,
                            &data.env,
                            &data.working_dir,
                            ctx_clone,
                            Duration::from_secs(300),
                        )
                        .map_err(|e| e.to_string())
                    } else {
                        // Shouldn't happen for non-interactive non-group jobs
                        Ok(HookResult::success())
                    };

                    // Determine job status from result
                    let job_status = match &result {
                        Ok(r) if r.skipped => JobStatus::Skipped,
                        Ok(r) if r.success => JobStatus::Succeeded,
                        _ => JobStatus::Failed,
                    };
                    // Record result
                    results_collector
                        .lock()
                        .unwrap()
                        .push((job_idx, name, result));

                    // Update DAG state
                    {
                        let mut s = state.lock().unwrap();
                        s.status[job_idx] = job_status;
                        s.active -= 1;
                        s.done += 1;

                        if job_status == JobStatus::Succeeded || job_status == JobStatus::Skipped {
                            // Satisfied dependency - decrement in_degree of dependents
                            for &dep_idx in &dependents[job_idx] {
                                if s.status[dep_idx] == JobStatus::Pending {
                                    s.in_degree[dep_idx] -= 1;
                                    if s.in_degree[dep_idx] == 0 {
                                        s.ready.push(dep_idx);
                                        // Keep sorted by priority (highest priority = lowest number at end)
                                        s.ready.sort_by_key(|&i| {
                                            std::cmp::Reverse(jobs[i].priority.unwrap_or(0))
                                        });
                                    }
                                }
                            }
                        } else {
                            // Failed — cascade DepFailed to all transitive dependents
                            cascade_dep_failed(&mut s.status, dependents, job_idx);
                            s.done += count_dep_failed_cascade(&s.status, dependents, job_idx);
                        }

                        cvar.notify_all();
                    }
                }
            });
        }
    });

    // After thread scope exits, all worker threads are guaranteed to be joined
    // (std::thread::scope blocks until all spawned threads complete).
    // It's now safe to handle interactive/group jobs sequentially on the main thread.
    {
        let mut s = state.lock().unwrap();
        loop {
            let interactive_pos = s.ready.iter().position(|&i| {
                (jobs[i].interactive == Some(true) || jobs[i].group.is_some())
                    && s.status[i] == JobStatus::Pending
            });

            if let Some(pos) = interactive_pos {
                let idx = s.ready.remove(pos);
                s.status[idx] = JobStatus::Running;
                // Drop lock for execution
                drop(s);

                let result = execute_single_job(&jobs[idx], exec, output);
                let job_name = jobs[idx]
                    .name
                    .clone()
                    .unwrap_or_else(|| "(unnamed)".to_string());

                let job_status = match &result {
                    Ok(r) if r.skipped => JobStatus::Skipped,
                    Ok(r) if r.success => JobStatus::Succeeded,
                    _ => JobStatus::Failed,
                };

                results_collector.lock().unwrap().push((
                    idx,
                    job_name,
                    result.map_err(|e| e.to_string()),
                ));

                s = state.lock().unwrap();
                s.status[idx] = job_status;
                s.done += 1;

                if job_status == JobStatus::Succeeded || job_status == JobStatus::Skipped {
                    for &dep_idx in &dependents[idx] {
                        if s.status[dep_idx] == JobStatus::Pending {
                            s.in_degree[dep_idx] -= 1;
                            if s.in_degree[dep_idx] == 0 {
                                s.ready.push(dep_idx);
                            }
                        }
                    }
                } else {
                    cascade_dep_failed(&mut s.status, &dependents, idx);
                }
            } else {
                break;
            }
        }
    }

    // Report results
    let mut collected = results_collector.into_inner().unwrap();
    collected.sort_by_key(|(idx, _, _)| *idx);

    let mut any_failed = false;
    let state = state.into_inner().unwrap();

    for (idx, name, result) in &collected {
        match result {
            Ok(r) if !r.success && !r.skipped => {
                if !r.stderr.is_empty() {
                    output.warning(&format!("Job '{name}' output:\n{}", r.stderr.trim()));
                }
                output.warning(&format!(
                    "Job '{name}' failed (exit code: {})",
                    r.exit_code.unwrap_or(-1)
                ));
                if let Some(ref text) = jobs[*idx].fail_text {
                    output.error(text);
                }
                any_failed = true;
            }
            Err(e) => {
                output.error(&format!("Job '{name}' error: {e}"));
                any_failed = true;
            }
            _ => {}
        }
    }

    // Report dep-failed jobs
    for (i, &status) in state.status.iter().enumerate() {
        if status == JobStatus::DepFailed {
            let name = jobs[i].name.as_deref().unwrap_or("(unnamed)");
            // Find which dependency failed
            let failed_dep = jobs[i]
                .needs
                .as_ref()
                .and_then(|needs| {
                    needs.iter().find(|dep_name| {
                        _name_to_idx.get(dep_name.as_str()).is_some_and(|&dep_idx| {
                            matches!(
                                state.status[dep_idx],
                                JobStatus::Failed | JobStatus::DepFailed
                            )
                        })
                    })
                })
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            output.warning(&format!(
                "Job '{name}' skipped: dependency '{failed_dep}' failed"
            ));
            any_failed = true;
        }
    }

    if any_failed {
        Ok(HookResult::failed(1, String::new(), String::new()))
    } else {
        Ok(HookResult::success())
    }
}

/// Cascade DepFailed status to all transitive dependents of a failed job.
fn cascade_dep_failed(status: &mut [JobStatus], dependents: &[Vec<usize>], failed_idx: usize) {
    let mut stack = vec![failed_idx];
    while let Some(idx) = stack.pop() {
        for &dep_idx in &dependents[idx] {
            if status[dep_idx] == JobStatus::Pending {
                status[dep_idx] = JobStatus::DepFailed;
                stack.push(dep_idx);
            }
        }
    }
}

/// Count how many jobs were just cascaded to DepFailed from a failure.
/// (Used to update the done counter.)
fn count_dep_failed_cascade(
    status: &[JobStatus],
    dependents: &[Vec<usize>],
    failed_idx: usize,
) -> usize {
    let mut count = 0;
    let mut stack = vec![failed_idx];
    let mut visited = std::collections::HashSet::new();
    while let Some(idx) = stack.pop() {
        for &dep_idx in &dependents[idx] {
            if status[dep_idx] == JobStatus::DepFailed && visited.insert(dep_idx) {
                count += 1;
                stack.push(dep_idx);
            }
        }
    }
    count
}

/// Enqueue all transitively dep-failed dependents of a failed job into the heap for reporting.
fn enqueue_dep_failed(
    status: &[JobStatus],
    dependents: &[Vec<usize>],
    failed_idx: usize,
    jobs: &[JobDef],
    heap: &mut std::collections::BinaryHeap<std::cmp::Reverse<(i32, usize)>>,
) {
    let mut stack = vec![failed_idx];
    let mut visited = std::collections::HashSet::new();
    while let Some(idx) = stack.pop() {
        for &dep_idx in &dependents[idx] {
            if status[dep_idx] == JobStatus::DepFailed && visited.insert(dep_idx) {
                heap.push(std::cmp::Reverse((
                    jobs[dep_idx].priority.unwrap_or(0),
                    dep_idx,
                )));
                stack.push(dep_idx);
            }
        }
    }
}

/// Check skip/only conditions for a job without executing it.
fn check_skip_conditions(job: &JobDef, working_dir: &Path) -> Option<String> {
    if let Some(ref skip) = job.skip {
        if let Some(reason) = super::conditions::should_skip(skip, working_dir) {
            return Some(reason);
        }
    }
    if let Some(ref only) = job.only {
        if let Some(reason) = super::conditions::should_only_skip(only, working_dir) {
            return Some(reason);
        }
    }
    None
}

/// Execute jobs in topological order sequentially (for piped/follow modes with `needs`).
///
/// Uses Kahn's algorithm to produce a topological ordering with priority as tiebreaker.
fn execute_dag_sequential(
    jobs: &[JobDef],
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
    stop_on_failure: bool,
) -> Result<HookResult> {
    use std::collections::BinaryHeap;

    let n = jobs.len();
    let (_name_to_idx, dependents, mut in_degree) = build_dag(jobs);
    let mut status: Vec<JobStatus> = vec![JobStatus::Pending; n];

    // Kahn's algorithm with priority-based BinaryHeap
    // BinaryHeap is a max-heap; we want lowest priority first, so use Reverse
    let mut heap: BinaryHeap<std::cmp::Reverse<(i32, usize)>> = BinaryHeap::new();
    for i in 0..n {
        if in_degree[i] == 0 {
            heap.push(std::cmp::Reverse((jobs[i].priority.unwrap_or(0), i)));
        }
    }

    let mut any_failed = false;
    let mut last_failure: Option<HookResult> = None;

    while let Some(std::cmp::Reverse((_, idx))) = heap.pop() {
        if status[idx] == JobStatus::DepFailed {
            let name = jobs[idx].name.as_deref().unwrap_or("(unnamed)");
            let failed_dep = jobs[idx]
                .needs
                .as_ref()
                .and_then(|needs| {
                    needs.iter().find(|dep_name| {
                        _name_to_idx.get(dep_name.as_str()).is_some_and(|&dep_idx| {
                            matches!(status[dep_idx], JobStatus::Failed | JobStatus::DepFailed)
                        })
                    })
                })
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            output.warning(&format!(
                "Job '{name}' skipped: dependency '{failed_dep}' failed"
            ));
            any_failed = true;

            // Cascade to dependents
            for &dep_idx in &dependents[idx] {
                if status[dep_idx] == JobStatus::Pending {
                    in_degree[dep_idx] -= 1;
                    status[dep_idx] = JobStatus::DepFailed;
                    if in_degree[dep_idx] == 0 {
                        heap.push(std::cmp::Reverse((
                            jobs[dep_idx].priority.unwrap_or(0),
                            dep_idx,
                        )));
                    }
                }
            }
            continue;
        }

        // Handle group jobs
        if let Some(ref group) = jobs[idx].group {
            let group_mode = ExecutionMode::from_group_def(group);
            let group_jobs = group.jobs.as_deref().unwrap_or(&[]);
            let result = execute_jobs(group_jobs, group_mode, exec, output)?;
            let job_status = if result.skipped {
                JobStatus::Skipped
            } else if result.success {
                JobStatus::Succeeded
            } else {
                JobStatus::Failed
            };
            status[idx] = job_status;

            if !result.success && !result.skipped {
                let job_name = jobs[idx].name.as_deref().unwrap_or("(unnamed)");
                output.warning(&format!(
                    "Job '{job_name}' failed (exit code: {})",
                    result.exit_code.unwrap_or(-1)
                ));
                if stop_on_failure {
                    return Ok(result);
                }
                any_failed = true;
                last_failure = Some(result);
                // Cascade DepFailed and enqueue for reporting
                cascade_dep_failed(&mut status, &dependents, idx);
                enqueue_dep_failed(&status, &dependents, idx, jobs, &mut heap);
            }

            // Unlock dependents
            if job_status == JobStatus::Succeeded || job_status == JobStatus::Skipped {
                for &dep_idx in &dependents[idx] {
                    if status[dep_idx] == JobStatus::Pending {
                        in_degree[dep_idx] -= 1;
                        if in_degree[dep_idx] == 0 {
                            heap.push(std::cmp::Reverse((
                                jobs[dep_idx].priority.unwrap_or(0),
                                dep_idx,
                            )));
                        }
                    }
                }
            }
            continue;
        }

        let result = execute_single_job(&jobs[idx], exec, output)?;
        let job_status = if result.skipped {
            JobStatus::Skipped
        } else if result.success {
            JobStatus::Succeeded
        } else {
            JobStatus::Failed
        };
        status[idx] = job_status;

        if !result.success && !result.skipped {
            let job_name = jobs[idx].name.as_deref().unwrap_or("(unnamed)");
            output.warning(&format!(
                "Job '{job_name}' failed (exit code: {})",
                result.exit_code.unwrap_or(-1)
            ));
            if let Some(ref text) = jobs[idx].fail_text {
                output.error(text);
            }
            if stop_on_failure {
                return Ok(result);
            }
            any_failed = true;
            last_failure = Some(result);
            // Cascade DepFailed and enqueue all dep-failed jobs for reporting
            cascade_dep_failed(&mut status, &dependents, idx);
            enqueue_dep_failed(&status, &dependents, idx, jobs, &mut heap);
        }

        // Unlock dependents (only if succeeded/skipped)
        if job_status == JobStatus::Succeeded || job_status == JobStatus::Skipped {
            for &dep_idx in &dependents[idx] {
                if status[dep_idx] == JobStatus::Pending {
                    in_degree[dep_idx] -= 1;
                    if in_degree[dep_idx] == 0 {
                        heap.push(std::cmp::Reverse((
                            jobs[dep_idx].priority.unwrap_or(0),
                            dep_idx,
                        )));
                    }
                }
            }
        }
    }

    if any_failed {
        Ok(last_failure.unwrap_or_else(|| HookResult::failed(1, String::new(), String::new())))
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

    let cmd = resolve_command(job, exec.hook_ctx, Some(job_name), exec.source_dir);

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

    let is_interactive = job.interactive == Some(true);

    let result = if is_interactive {
        run_interactive_command(&cmd, &env, &wd, exec.hook_ctx)
    } else {
        run_shell_command(&cmd, &env, &wd, exec.hook_ctx, Duration::from_secs(300))
    }?;

    Ok(result)
}

/// Resolve the shell command for a job, handling both `run` and `script`.
fn resolve_command(
    job: &JobDef,
    ctx: &HookContext,
    job_name: Option<&str>,
    source_dir: &str,
) -> String {
    if let Some(ref run) = job.run {
        template::substitute(run, ctx, job_name)
    } else if let Some(ref script) = job.script {
        let script_path = format!("{source_dir}/{script}");
        if let Some(ref runner) = job.runner {
            let args = job.args.as_deref().unwrap_or("");
            let cmd = if args.is_empty() {
                format!("{runner} {script_path}")
            } else {
                format!("{runner} {script_path} {args}")
            };
            template::substitute(&cmd, ctx, job_name)
        } else {
            let args = job.args.as_deref().unwrap_or("");
            let cmd = if args.is_empty() {
                script_path
            } else {
                format!("{script_path} {args}")
            };
            template::substitute(&cmd, ctx, job_name)
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
    run_shell_command_with_callback(cmd, extra_env, working_dir, ctx, timeout, None)
}

/// Run a shell command, capture its output, and optionally stream lines
/// through the provided channel.
fn run_shell_command_with_callback(
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
    let hook_env = super::environment::HookEnvironment::from_context(ctx);
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
        assert_eq!(ExecutionMode::from_hook_def(&def), ExecutionMode::Parallel);

        let def = HookDef {
            parallel: Some(false),
            ..Default::default()
        };
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
        let cmd = resolve_command(&job, &ctx, Some("test"), ".daft");
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
        let cmd = resolve_command(&job, &ctx, Some("test"), ".daft");
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
        let cmd = resolve_command(&job, &ctx, Some("test"), ".daft");
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Dependency (needs) tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_needs_simple_chain() {
        // A → B → C: each must run only after its dependency
        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("true".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["b".to_string()]),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_diamond() {
        // A → B, A → C, B+C → D
        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("true".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("d".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["b".to_string(), "c".to_string()]),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_failure_propagation() {
        // A fails, B needs A → B dep-failed
        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("false".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(!result.success);
        // Check that dependency failure was reported
        assert!(output.has_warning("dependency"));
    }

    #[test]
    fn test_needs_failure_cascade() {
        // A fails, B needs A, C needs B → both B and C dep-failed
        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("false".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["b".to_string()]),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(!result.success);
        // Both b and c should have dependency failure warnings
        let warnings = output.warnings();
        let dep_warning_count = warnings.iter().filter(|w| w.contains("dependency")).count();
        assert!(dep_warning_count >= 2);
    }

    #[test]
    fn test_needs_skip_satisfied() {
        // A skipped (via skip: true), B needs A → B still runs
        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("true".to_string()),
                    skip: Some(crate::hooks::yaml_config::SkipCondition::Bool(true)),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_no_deps_unchanged() {
        // No `needs` anywhere → identical to current behavior
        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("true".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_priority_tiebreaker() {
        // Two independent jobs with different priorities;
        // lower priority number should run first in sequential mode
        let hook_def = HookDef {
            piped: Some(true),
            jobs: Some(vec![
                JobDef {
                    name: Some("low-prio".to_string()),
                    run: Some("true".to_string()),
                    priority: Some(10),
                    needs: Some(vec!["root".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("high-prio".to_string()),
                    run: Some("true".to_string()),
                    priority: Some(1),
                    needs: Some(vec!["root".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("root".to_string()),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_topological_sort_sequential() {
        // In piped mode with deps: A, B depends on A, C depends on B
        // Should execute A → B → C in order
        let hook_def = HookDef {
            piped: Some(true),
            jobs: Some(vec![
                JobDef {
                    name: Some("c".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["b".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("true".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_sequential_failure_propagation() {
        // In follow mode: A fails, B needs A → B dep-failed, C (independent) still runs
        let hook_def = HookDef {
            follow: Some(true),
            jobs: Some(vec![
                JobDef {
                    name: Some("a".to_string()),
                    run: Some("false".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some("true".to_string()),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
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
            ".daft",
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(!result.success);
        // B should be reported as dep-failed
        assert!(output.has_warning("dependency"));
    }

    #[test]
    fn test_run_shell_command_streams_output() {
        let ctx = make_ctx();
        let (tx, rx) = std::sync::mpsc::channel::<String>();

        let result = run_shell_command_with_callback(
            "echo hello && echo world",
            &HashMap::new(),
            Path::new("/tmp"),
            &ctx,
            Duration::from_secs(10),
            Some(tx),
        )
        .unwrap();

        assert!(result.success);
        assert!(result.stdout.contains("hello"));

        let lines: Vec<String> = rx.try_iter().collect();
        assert!(lines.iter().any(|l| l.contains("hello")));
        assert!(lines.iter().any(|l| l.contains("world")));
    }
}
