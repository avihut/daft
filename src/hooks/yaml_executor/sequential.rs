use super::{execute_jobs, execute_single_job, ExecContext, ExecutionMode};
use crate::hooks::executor::HookResult;
use crate::hooks::yaml_config::JobDef;
use crate::output::hook_progress::HookRenderer;
use crate::output::Output;
use anyhow::Result;

/// Execute jobs sequentially.
///
/// If `stop_on_failure` is true (piped mode), stops on first failure.
/// If false (follow mode), continues and reports all.
pub(super) fn execute_sequential(
    jobs: &[JobDef],
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
    stop_on_failure: bool,
) -> Result<HookResult> {
    let mut any_failed = false;
    let mut last_failure: Option<HookResult> = None;
    let mut renderer = HookRenderer::auto(exec.output_config);

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

        renderer.start_job_with_description(job_name, job.description.as_deref());
        let start = std::time::Instant::now();

        let (tx, rx) = std::sync::mpsc::channel();
        let result = execute_single_job(job, exec, output, Some(tx))?;
        let elapsed = start.elapsed();

        // Drain channel and feed lines to the renderer
        for line in rx.try_iter() {
            renderer.update_job_output(job_name, &line);
        }

        if result.skipped {
            let reason = result.skip_reason.as_deref().unwrap_or("skipped");
            renderer.finish_job_skipped(job_name, reason, elapsed, result.skip_ran_command);
            continue;
        }

        if result.success {
            renderer.finish_job_success(job_name, elapsed);
        } else {
            renderer.finish_job_failure(job_name, elapsed);
            renderer.println(&format!(
                "Job '{}' failed (exit code: {})",
                job_name,
                result.exit_code.unwrap_or(-1)
            ));
            if let Some(ref text) = job.fail_text {
                renderer.println(text);
            }
            if stop_on_failure {
                // Collect results before returning
                exec.job_results
                    .lock()
                    .unwrap()
                    .extend(renderer.take_finished_jobs());
                return Ok(result);
            }
            any_failed = true;
            last_failure = Some(result);
        }
    }

    // Collect finished job results for the summary
    exec.job_results
        .lock()
        .unwrap()
        .extend(renderer.take_finished_jobs());

    if any_failed {
        Ok(last_failure.unwrap_or_else(HookResult::success))
    } else {
        Ok(HookResult::success())
    }
}
