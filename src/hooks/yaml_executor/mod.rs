//! YAML-based hook job execution engine.
//!
//! This module executes jobs defined in YAML hook configurations.
//! It supports sequential, parallel, piped, and follow execution modes.

mod command;
mod dependency;
mod parallel;
mod sequential;

use super::environment::HookContext;
use super::executor::HookResult;
use super::template;
use super::yaml_config::{GroupDef, HookDef, JobDef};
use super::yaml_config_loader::get_effective_jobs;
use crate::output::hook_progress::JobResultEntry;
use crate::output::Output;
use crate::settings::HookOutputConfig;
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

/// Filter criteria for selecting specific jobs within a hook.
///
/// Used by `hooks run` to restrict execution to a named job or jobs with specific tags.
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    /// Run only the job with this name.
    pub only_job_name: Option<String>,
    /// Run only jobs that have at least one of these tags.
    pub only_tags: Vec<String>,
}

/// Shared execution context passed through the job execution pipeline.
///
/// Groups together the many parameters that would otherwise be separate function arguments.
pub(crate) struct ExecContext<'a> {
    pub(crate) hook_ctx: &'a HookContext,
    pub(crate) hook_env: &'a HashMap<String, String>,
    pub(crate) source_dir: &'a str,
    pub(crate) working_dir: &'a Path,
    /// Shell RC file to source before commands (from config `rc` field).
    pub(crate) rc: Option<&'a str>,
    /// Output display configuration for progress rendering.
    pub(crate) output_config: &'a HookOutputConfig,
    /// Shared collector for finished job results (used for summary).
    pub(crate) job_results: Arc<Mutex<Vec<JobResultEntry>>>,
}

/// Data needed to run a single job in a thread.
#[derive(Clone)]
pub(crate) struct ParallelJobData {
    pub(crate) name: String,
    pub(crate) cmd: String,
    pub(crate) env: HashMap<String, String>,
    pub(crate) working_dir: PathBuf,
}

/// Execute a YAML-defined hook.
#[allow(clippy::too_many_arguments)]
pub fn execute_yaml_hook(
    hook_name: &str,
    hook_def: &HookDef,
    ctx: &HookContext,
    output: &mut dyn Output,
    source_dir: &str,
    working_dir: &Path,
    output_config: &HookOutputConfig,
) -> Result<HookResult> {
    execute_yaml_hook_with_rc(
        hook_name,
        hook_def,
        ctx,
        output,
        source_dir,
        working_dir,
        None,
        output_config,
        &JobFilter::default(),
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
    output_config: &HookOutputConfig,
    filter: &JobFilter,
) -> Result<HookResult> {
    // Check hook-level skip/only conditions
    if let Some(ref skip) = hook_def.skip {
        if let Some(info) = super::conditions::should_skip(skip, working_dir) {
            output.debug(&format!("Skipping {hook_name}: {}", info.reason));
            return Ok(if info.ran_command {
                HookResult::skipped_after_command(info.reason)
            } else {
                HookResult::skipped(info.reason)
            });
        }
    }
    if let Some(ref only) = hook_def.only {
        if let Some(info) = super::conditions::should_only_skip(only, working_dir) {
            output.debug(&format!("Skipping {hook_name}: {}", info.reason));
            return Ok(if info.ran_command {
                HookResult::skipped_after_command(info.reason)
            } else {
                HookResult::skipped(info.reason)
            });
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

    // Apply inclusion filters (from `hooks run --job` / `--tag`)
    if let Some(ref name) = filter.only_job_name {
        jobs.retain(|j| j.name.as_deref() == Some(name.as_str()));
        if jobs.is_empty() {
            anyhow::bail!("No job named '{name}' found in hook '{hook_name}'");
        }
    }
    if !filter.only_tags.is_empty() {
        jobs.retain(|job| {
            job.tags
                .as_ref()
                .is_some_and(|tags| tags.iter().any(|t| filter.only_tags.contains(t)))
        });
        if jobs.is_empty() {
            anyhow::bail!(
                "No jobs matching tags {:?} in hook '{hook_name}'",
                filter.only_tags
            );
        }
    }

    // Sort by priority if set
    jobs.sort_by_key(|j| j.priority.unwrap_or(0));

    let mode = ExecutionMode::from_hook_def(hook_def);

    let job_results: Arc<Mutex<Vec<JobResultEntry>>> = Arc::new(Mutex::new(Vec::new()));

    let exec = ExecContext {
        hook_ctx: ctx,
        hook_env: &HashMap::new(),
        source_dir,
        working_dir,
        rc,
        output_config,
        job_results: Arc::clone(&job_results),
    };

    // Print header and track total time
    crate::output::hook_progress::print_hook_header(hook_name);
    let hook_start = std::time::Instant::now();

    let result = execute_jobs(&jobs, mode, &exec, output)?;

    // Print summary with collected job results
    let collected = job_results.lock().unwrap();
    crate::output::hook_progress::print_hook_summary(&collected, hook_start.elapsed());

    Ok(result)
}

/// Execute a list of jobs in the specified mode.
pub(crate) fn execute_jobs(
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
        return dependency::execute_with_dependencies(jobs, mode, exec, output);
    }

    match mode {
        ExecutionMode::Sequential | ExecutionMode::Piped => {
            sequential::execute_sequential(jobs, exec, output, true)
        }
        ExecutionMode::Follow => sequential::execute_sequential(jobs, exec, output, false),
        ExecutionMode::Parallel => parallel::execute_parallel(jobs, exec, output),
    }
}

/// Resolve the shell command for a job, handling both `run` and `script`.
pub(crate) fn resolve_command(
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

/// Check skip/only conditions for a job without executing it.
pub(crate) fn check_skip_conditions(
    job: &JobDef,
    working_dir: &Path,
) -> Option<super::conditions::SkipInfo> {
    if let Some(reason) = super::conditions::check_platform_constraints(job) {
        return Some(super::conditions::SkipInfo {
            reason,
            ran_command: false,
        });
    }
    if let Some(ref skip) = job.skip {
        if let Some(info) = super::conditions::should_skip(skip, working_dir) {
            return Some(info);
        }
    }
    if let Some(ref only) = job.only {
        if let Some(info) = super::conditions::should_only_skip(only, working_dir) {
            return Some(info);
        }
    }
    None
}

/// Execute a single job.
///
/// If `line_sender` is provided, output lines from non-interactive commands
/// are streamed through the channel for progress rendering.
pub(crate) fn execute_single_job(
    job: &JobDef,
    exec: &ExecContext<'_>,
    output: &mut dyn Output,
    line_sender: Option<std::sync::mpsc::Sender<String>>,
) -> Result<HookResult> {
    let job_name = job.name.as_deref().unwrap_or("(unnamed)");

    // Platform constraints (os/arch)
    if let Some(reason) = super::conditions::check_platform_constraints(job) {
        output.debug(&format!("Skipping job '{job_name}': {reason}"));
        return Ok(HookResult::skipped(reason));
    }

    // Job-level skip/only conditions
    if let Some(ref skip) = job.skip {
        if let Some(info) = super::conditions::should_skip(skip, exec.working_dir) {
            output.debug(&format!("Skipping job '{job_name}': {}", info.reason));
            return Ok(if info.ran_command {
                HookResult::skipped_after_command(info.reason)
            } else {
                HookResult::skipped(info.reason)
            });
        }
    }
    if let Some(ref only) = job.only {
        if let Some(info) = super::conditions::should_only_skip(only, exec.working_dir) {
            output.debug(&format!("Skipping job '{job_name}': {}", info.reason));
            return Ok(if info.ran_command {
                HookResult::skipped_after_command(info.reason)
            } else {
                HookResult::skipped(info.reason)
            });
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
        command::run_interactive_command(&cmd, &env, &wd, exec.hook_ctx)
    } else {
        command::run_shell_command_with_callback(
            &cmd,
            &env,
            &wd,
            exec.hook_ctx,
            std::time::Duration::from_secs(300),
            line_sender,
        )
    }?;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookType;
    use crate::output::TestOutput;

    fn make_ctx() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout",
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
            &HookOutputConfig::default(),
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
            &HookOutputConfig::default(),
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
            &HookOutputConfig::default(),
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
            &HookOutputConfig::default(),
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
            &HookOutputConfig::default(),
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Dependency (needs) tests
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_needs_simple_chain() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_diamond() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_failure_propagation() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_needs_failure_cascade() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_needs_skip_satisfied() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_no_deps_unchanged() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_priority_tiebreaker() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_topological_sort_sequential() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_needs_sequential_failure_propagation() {
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(!result.success);
    }

    #[test]
    fn test_parallel_hook_captures_output() {
        let hook_def = HookDef {
            parallel: Some(true),
            jobs: Some(vec![
                JobDef {
                    name: Some("job-a".to_string()),
                    run: Some("echo 'output-a'".to_string()),
                    ..Default::default()
                },
                JobDef {
                    name: Some("job-b".to_string()),
                    run: Some("echo 'output-b'".to_string()),
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
            &HookOutputConfig::default(),
        )
        .unwrap();

        assert!(result.success);
    }

    #[test]
    fn test_run_shell_command_streams_output() {
        let ctx = make_ctx();
        let (tx, rx) = std::sync::mpsc::channel::<String>();

        let result = command::run_shell_command_with_callback(
            "echo hello && echo world",
            &HashMap::new(),
            Path::new("/tmp"),
            &ctx,
            std::time::Duration::from_secs(10),
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
