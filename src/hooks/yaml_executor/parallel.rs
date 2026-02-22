use super::command::run_shell_command_with_callback;
use super::{execute_single_job, is_platform_skip, resolve_command, ExecContext, ParallelJobData};
use crate::hooks::executor::HookResult;
use crate::hooks::yaml_config::JobDef;
use crate::output::hook_progress::HookRenderer;
use crate::output::Output;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Execute jobs in parallel using threads.
pub(super) fn execute_parallel(
    jobs: &[JobDef],
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
) -> Result<HookResult> {
    use std::thread;

    // For interactive jobs, run sequentially
    let (interactive, parallel): (Vec<_>, Vec<_>) =
        jobs.iter().partition(|j| j.interactive == Some(true));

    // Collect job data for threads
    let mut job_data: Vec<ParallelJobData> = Vec::new();
    for job in &parallel {
        // Platform skip — silently exclude jobs with no OS variant
        if is_platform_skip(job) {
            continue;
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
        job_data.push(ParallelJobData {
            name,
            cmd,
            env,
            working_dir: wd,
        });
    }

    let ctx_clone = exec.hook_ctx.clone();

    // Use indexed results to preserve definition order in output
    type IndexedResult = (usize, String, Result<HookResult>, Duration);
    let results: Arc<Mutex<Vec<IndexedResult>>> = Arc::new(Mutex::new(Vec::new()));

    let renderer = Arc::new(Mutex::new(HookRenderer::auto(exec.output_config)));

    // Limit concurrency to avoid thread explosion
    let max_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Process in batches if many jobs
    let mut global_offset = 0;
    for batch in job_data.chunks(max_threads) {
        // Start all jobs in this batch on the renderer
        for (batch_idx, data) in batch.iter().enumerate() {
            let job_idx = global_offset + batch_idx;
            let desc = parallel.get(job_idx).and_then(|j| j.description.as_deref());
            renderer
                .lock()
                .unwrap()
                .start_job_with_description(&data.name, desc);
        }

        let handles: Vec<_> = batch
            .iter()
            .enumerate()
            .map(|(batch_idx, data)| {
                let results = Arc::clone(&results);
                let renderer = Arc::clone(&renderer);
                let ctx_for_thread = ctx_clone.clone();
                let data = data.clone();
                let idx = global_offset + batch_idx;

                thread::spawn(move || {
                    let start = std::time::Instant::now();
                    let job_name = data.name.clone();

                    // Create channel for streaming output lines
                    let (tx, rx) = std::sync::mpsc::channel::<String>();

                    // Spawn a reader thread that drains the channel and
                    // updates the renderer in real-time
                    let reader_renderer = Arc::clone(&renderer);
                    let reader_name = job_name.clone();
                    let reader_handle = thread::spawn(move || {
                        while let Ok(line) = rx.recv() {
                            reader_renderer
                                .lock()
                                .unwrap()
                                .update_job_output(&reader_name, &line);
                        }
                    });

                    let result = run_shell_command_with_callback(
                        &data.cmd,
                        &data.env,
                        &data.working_dir,
                        &ctx_for_thread,
                        Duration::from_secs(300),
                        Some(tx),
                    );

                    // tx is dropped here, which causes the reader thread
                    // to exit its recv() loop
                    if reader_handle.join().is_err() {
                        eprintln!(
                            "Warning: output reader thread panicked for job '{}'",
                            job_name
                        );
                    }

                    let elapsed = start.elapsed();

                    // Finish the job on the renderer
                    {
                        let mut r = renderer.lock().unwrap();
                        match &result {
                            Ok(hr) if hr.success => {
                                r.finish_job_success(&job_name, elapsed);
                            }
                            _ => {
                                r.finish_job_failure(&job_name, elapsed);
                            }
                        }
                    }

                    results
                        .lock()
                        .unwrap()
                        .push((idx, data.name, result, elapsed));
                })
            })
            .collect();

        for handle in handles {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Thread panicked"))?;
        }
        global_offset += batch.len();
    }

    // Collect results in definition order
    let mut results = Arc::try_unwrap(results)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap results"))?
        .into_inner()
        .map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;
    results.sort_by_key(|(idx, _, _, _)| *idx);

    let mut renderer = Arc::try_unwrap(renderer)
        .map_err(|_| anyhow::anyhow!("Failed to unwrap renderer"))?
        .into_inner()
        .map_err(|_| anyhow::anyhow!("Mutex poisoned"))?;

    // Report failures via renderer
    let mut any_failed = false;
    for (job_idx, name, result, _) in &results {
        match result {
            Ok(r) if !r.success => {
                renderer.println(&format!(
                    "Job '{name}' failed (exit code: {})",
                    r.exit_code.unwrap_or(-1)
                ));
                if let Some(ref text) = parallel.get(*job_idx).and_then(|j| j.fail_text.clone()) {
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

    // Collect finished job results for the summary
    exec.job_results
        .lock()
        .unwrap()
        .extend(renderer.take_finished_jobs());

    // Run interactive jobs sequentially
    for job in &interactive {
        let result = execute_single_job(job, exec, output, None)?;
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
