use super::command::run_shell_command_with_callback;
use super::{
    check_skip_conditions, execute_jobs, execute_single_job, is_platform_skip, resolve_command,
    ExecContext, ExecutionMode, ParallelJobData,
};
use crate::hooks::executor::HookResult;
use crate::hooks::yaml_config::JobDef;
use crate::output::hook_progress::HookRenderer;
use crate::output::Output;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

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
pub(super) fn execute_with_dependencies(
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
    use std::sync::Condvar;

    let n = jobs.len();
    let (name_to_idx, dependents, in_degree) = build_dag(jobs);

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

    let renderer = Mutex::new(HookRenderer::auto(exec.output_config));

    let ctx_clone = exec.hook_ctx.clone();
    let rc = exec.rc.map(|s| s.to_string());

    let dependents_ref = &dependents;

    std::thread::scope(|scope| {
        for _ in 0..max_workers {
            let state = &state;
            let cvar = &cvar;
            let job_data = &job_data;
            let results_collector = &results_collector;
            let renderer = &renderer;
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

                    // Platform skip — completely silent, no renderer interaction
                    if is_platform_skip(job) {
                        let result: std::result::Result<HookResult, String> =
                            Ok(HookResult::platform_skipped());
                        results_collector
                            .lock()
                            .unwrap()
                            .push((job_idx, name, result));

                        // Update DAG state: mark as skipped, unlock dependents
                        {
                            let mut s = state.lock().unwrap();
                            s.status[job_idx] = JobStatus::Skipped;
                            s.active -= 1;
                            s.done += 1;
                            for &dep_idx in &dependents[job_idx] {
                                if s.status[dep_idx] == JobStatus::Pending {
                                    s.in_degree[dep_idx] -= 1;
                                    if s.in_degree[dep_idx] == 0 {
                                        s.ready.push(dep_idx);
                                        s.ready.sort_by_key(|&i| {
                                            std::cmp::Reverse(jobs[i].priority.unwrap_or(0))
                                        });
                                    }
                                }
                            }
                            cvar.notify_all();
                        }
                        continue;
                    }

                    renderer
                        .lock()
                        .unwrap()
                        .start_job_with_description(&name, job.description.as_deref());
                    let start = std::time::Instant::now();

                    // Check skip/only conditions
                    let skip_result = check_skip_conditions(job, exec.working_dir);
                    let result = if let Some(skip_info) = skip_result {
                        Ok(if skip_info.ran_command {
                            HookResult::skipped_after_command(skip_info.reason)
                        } else {
                            HookResult::skipped(skip_info.reason)
                        })
                    } else if let Some(ref data) = job_data[job_idx] {
                        let cmd = if let Some(ref rc) = rc {
                            format!("source {rc} && {}", data.cmd)
                        } else {
                            data.cmd.clone()
                        };

                        // Create channel for streaming output
                        let (tx, rx) = std::sync::mpsc::channel::<String>();
                        let reader_name = name.clone();

                        // Spawn reader thread to drain channel and update renderer
                        let reader_handle = scope.spawn(move || {
                            while let Ok(line) = rx.recv() {
                                renderer
                                    .lock()
                                    .unwrap()
                                    .update_job_output(&reader_name, &line);
                            }
                        });

                        let cmd_result = run_shell_command_with_callback(
                            &cmd,
                            &data.env,
                            &data.working_dir,
                            ctx_clone,
                            Duration::from_secs(300),
                            Some(tx),
                        )
                        .map_err(|e| e.to_string());

                        // tx is dropped, reader thread will exit
                        if reader_handle.join().is_err() {
                            eprintln!("Warning: output reader thread panicked for job '{}'", name);
                        }
                        cmd_result
                    } else {
                        // Shouldn't happen for non-interactive non-group jobs
                        Ok(HookResult::success())
                    };

                    let elapsed = start.elapsed();

                    // Finish job on renderer
                    {
                        let mut r = renderer.lock().unwrap();
                        match &result {
                            Ok(hr) if hr.skipped => {
                                let reason = hr.skip_reason.as_deref().unwrap_or("skipped");
                                r.finish_job_skipped(&name, reason, elapsed, hr.skip_ran_command);
                            }
                            Ok(hr) if hr.success => {
                                r.finish_job_success(&name, elapsed);
                            }
                            _ => {
                                r.finish_job_failure(&name, elapsed);
                            }
                        }
                    }

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

                let result = execute_single_job(&jobs[idx], exec, output, None);
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

    let mut renderer = renderer.into_inner().unwrap();

    // Report results
    let mut collected = results_collector.into_inner().unwrap();
    collected.sort_by_key(|(idx, _, _)| *idx);

    let mut any_failed = false;
    let state = state.into_inner().unwrap();

    for (idx, name, result) in &collected {
        match result {
            Ok(r) if !r.success && !r.skipped => {
                renderer.println(&format!(
                    "Job '{name}' failed (exit code: {})",
                    r.exit_code.unwrap_or(-1)
                ));
                if let Some(ref text) = jobs[*idx].fail_text {
                    renderer.println(text);
                }
                any_failed = true;
            }
            Err(e) => {
                renderer.println(&format!("Job '{name}' error: {e}"));
                any_failed = true;
            }
            _ => {}
        }
    }

    // Report dep-failed jobs
    for (i, &status) in state.status.iter().enumerate() {
        if status == JobStatus::DepFailed {
            let name = jobs[i].name.as_deref().unwrap_or("(unnamed)");
            let failed_dep = jobs[i]
                .needs
                .as_ref()
                .and_then(|needs| {
                    needs.iter().find(|dep_name| {
                        name_to_idx.get(dep_name.as_str()).is_some_and(|&dep_idx| {
                            matches!(
                                state.status[dep_idx],
                                JobStatus::Failed | JobStatus::DepFailed
                            )
                        })
                    })
                })
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            renderer.println(&format!(
                "Job '{name}' skipped: dependency '{failed_dep}' failed"
            ));
            any_failed = true;
        }
    }

    // Collect finished job results for the summary
    exec.job_results
        .lock()
        .unwrap()
        .extend(renderer.take_finished_jobs());

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
    let (name_to_idx, dependents, mut in_degree) = build_dag(jobs);
    let mut status: Vec<JobStatus> = vec![JobStatus::Pending; n];
    let mut renderer = HookRenderer::auto(exec.output_config);

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
        let job_name = jobs[idx].name.as_deref().unwrap_or("(unnamed)");

        // Platform skip — completely silent, unlock dependents
        if is_platform_skip(&jobs[idx]) {
            status[idx] = JobStatus::Skipped;
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
            continue;
        }

        if status[idx] == JobStatus::DepFailed {
            let failed_dep = jobs[idx]
                .needs
                .as_ref()
                .and_then(|needs| {
                    needs.iter().find(|dep_name| {
                        name_to_idx.get(dep_name.as_str()).is_some_and(|&dep_idx| {
                            matches!(status[dep_idx], JobStatus::Failed | JobStatus::DepFailed)
                        })
                    })
                })
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            renderer.println(&format!(
                "Job '{job_name}' skipped: dependency '{failed_dep}' failed"
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
                renderer.println(&format!(
                    "Job '{job_name}' failed (exit code: {})",
                    result.exit_code.unwrap_or(-1)
                ));
                if stop_on_failure {
                    exec.job_results
                        .lock()
                        .unwrap()
                        .extend(renderer.take_finished_jobs());
                    return Ok(result);
                }
                any_failed = true;
                last_failure = Some(result);
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

        renderer.start_job_with_description(job_name, jobs[idx].description.as_deref());
        let start = std::time::Instant::now();

        let (tx, rx) = std::sync::mpsc::channel();
        let result = execute_single_job(&jobs[idx], exec, output, Some(tx))?;
        let elapsed = start.elapsed();

        // Drain channel and feed lines to the renderer
        for line in rx.try_iter() {
            renderer.update_job_output(job_name, &line);
        }

        let job_status = if result.skipped {
            JobStatus::Skipped
        } else if result.success {
            JobStatus::Succeeded
        } else {
            JobStatus::Failed
        };
        status[idx] = job_status;

        if result.skipped {
            let reason = result.skip_reason.as_deref().unwrap_or("skipped");
            renderer.finish_job_skipped(job_name, reason, elapsed, result.skip_ran_command);
        } else if result.success {
            renderer.finish_job_success(job_name, elapsed);
        } else {
            renderer.finish_job_failure(job_name, elapsed);
            renderer.println(&format!(
                "Job '{job_name}' failed (exit code: {})",
                result.exit_code.unwrap_or(-1)
            ));
            if let Some(ref text) = jobs[idx].fail_text {
                renderer.println(text);
            }
            if stop_on_failure {
                exec.job_results
                    .lock()
                    .unwrap()
                    .extend(renderer.take_finished_jobs());
                return Ok(result);
            }
            any_failed = true;
            last_failure = Some(result);
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

    // Collect finished job results for the summary
    exec.job_results
        .lock()
        .unwrap()
        .extend(renderer.take_finished_jobs());

    if any_failed {
        Ok(last_failure.unwrap_or_else(|| HookResult::failed(1, String::new(), String::new())))
    } else {
        Ok(HookResult::success())
    }
}
