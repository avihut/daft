//! YAML-based hook job execution engine.
//!
//! This module executes jobs defined in YAML hook configurations.
//! It supports sequential, parallel, piped, and follow execution modes.
//!
//! Job execution is delegated to the generic executor (`crate::executor::runner`)
//! via the job adapter (`crate::hooks::job_adapter`).

use super::environment::HookContext;
use super::executor::HookResult;
use super::template;
use super::yaml_config::{GroupDef, HookDef, JobDef};
use super::yaml_config_loader::get_effective_jobs;
use crate::executor::presenter::JobPresenter;
use crate::hooks::tracking::{effective_tracks, TrackedAttribute};
use crate::output::Output;
use crate::settings::HookOutputConfig;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

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
    let presenter: Arc<dyn JobPresenter> =
        crate::executor::cli_presenter::CliPresenter::auto(output_config);
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
        &presenter,
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
    _output_config: &HookOutputConfig,
    filter: &JobFilter,
    presenter: &Arc<dyn JobPresenter>,
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

    // Apply tracking filter when changed_attributes are present (move hooks)
    if let Some(ref changed) = ctx.changed_attributes {
        jobs = filter_tracked_jobs(&jobs, changed);
        if jobs.is_empty() {
            return Ok(HookResult::skipped("No jobs match changed attributes"));
        }
    }

    // Sort by priority if set
    jobs.sort_by_key(|j| j.priority.unwrap_or(0));

    // Map YAML execution mode to generic executor mode
    let yaml_mode = ExecutionMode::from_hook_def(hook_def);
    let exec_mode = match yaml_mode {
        ExecutionMode::Sequential | ExecutionMode::Follow => {
            crate::executor::ExecutionMode::Sequential
        }
        ExecutionMode::Piped => crate::executor::ExecutionMode::Piped,
        ExecutionMode::Parallel => crate::executor::ExecutionMode::Parallel,
    };

    // Build hook environment
    let hook_env_obj = super::environment::HookEnvironment::from_context(ctx);
    let hook_env = hook_env_obj.vars().clone();

    // Convert filtered JobDefs to generic JobSpecs
    let specs = crate::hooks::job_adapter::yaml_jobs_to_specs(
        &jobs,
        ctx,
        &hook_env,
        source_dir,
        working_dir,
        rc,
    );

    if specs.is_empty() {
        return Ok(HookResult::skipped("All jobs skipped"));
    }

    // Clear any active spinner — the presenter writes directly to stderr.
    output.finish_spinner();

    // Use presenter for header and execution
    presenter.on_phase_start(hook_name);
    let hook_start = std::time::Instant::now();

    // Execute via the generic runner
    let results = crate::executor::runner::run_jobs(&specs, exec_mode, presenter)?;

    presenter.on_phase_complete(hook_start.elapsed());

    // Convert Vec<JobResult> to HookResult
    job_results_to_hook_result(&results)
}

/// Convert generic executor results into a `HookResult`.
fn job_results_to_hook_result(results: &[crate::executor::JobResult]) -> Result<HookResult> {
    if results.is_empty() {
        return Ok(HookResult::success());
    }

    // Find the first failure
    let first_failure = results
        .iter()
        .find(|r| r.status == crate::executor::NodeStatus::Failed);

    match first_failure {
        Some(failed) => Ok(HookResult::failed(
            failed.exit_code.unwrap_or(-1),
            failed.stdout.clone(),
            failed.stderr.clone(),
        )),
        None => Ok(HookResult::success()),
    }
}

/// Filter jobs to those whose effective tracking set intersects with the
/// changed attributes, plus any jobs they depend on via `needs`.
pub fn filter_tracked_jobs(jobs: &[JobDef], changed: &HashSet<TrackedAttribute>) -> Vec<JobDef> {
    // 1. Find directly tracked jobs
    let mut selected_names: HashSet<String> = HashSet::new();
    for job in jobs {
        let tracks = effective_tracks(job);
        if !tracks.is_disjoint(changed) {
            if let Some(ref name) = job.name {
                selected_names.insert(name.clone());
            }
        }
    }

    // 2. Pull in needs dependencies (transitive)
    let mut made_progress = true;
    while made_progress {
        made_progress = false;
        for job in jobs {
            if let Some(ref name) = job.name {
                if selected_names.contains(name) {
                    if let Some(ref needs) = job.needs {
                        for dep in needs {
                            if selected_names.insert(dep.clone()) {
                                made_progress = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Return selected jobs in original order
    jobs.iter()
        .filter(|job| {
            job.name
                .as_ref()
                .map(|n| selected_names.contains(n))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

/// Check if a job should be silently skipped due to platform mismatch.
///
/// Returns `true` when the job uses an OS-keyed `run` map and the current OS
/// has no entry in that map. These jobs are completely invisible in output.
pub(crate) fn is_platform_skip(job: &JobDef) -> bool {
    match &job.run {
        Some(super::yaml_config::RunCommand::Platform(map)) => {
            super::yaml_config::RunCommand::current_target_os()
                .map(|os| !map.contains_key(&os))
                .unwrap_or(true)
        }
        _ => false,
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
        match run.resolve_for_current_os() {
            Some(cmd) => template::substitute(&cmd, ctx, job_name),
            None => String::new(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::RunCommand;
    use crate::hooks::HookType;
    use crate::output::TestOutput;
    use std::collections::HashMap;

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
            run: Some(RunCommand::Simple("echo {branch}".to_string())),
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
                run: Some(RunCommand::Simple("true".to_string())),
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
                run: Some(RunCommand::Simple("false".to_string())),
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
                    run: Some(RunCommand::Simple("false".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("should-not-run".to_string()),
                    run: Some(RunCommand::Simple("echo should-not-run".to_string())),
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
                    run: Some(RunCommand::Simple("false".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("still-runs".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                run: Some(RunCommand::Simple("test \"$MY_VAR\" = hello".to_string())),
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
                    run: Some(RunCommand::Simple("true".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("true".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("d".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("false".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("false".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("true".to_string())),
                    skip: Some(crate::hooks::yaml_config::SkipCondition::Bool(true)),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("true".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("true".to_string())),
                    priority: Some(10),
                    needs: Some(vec!["root".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("high-prio".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    priority: Some(1),
                    needs: Some(vec!["root".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("root".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("true".to_string())),
                    needs: Some(vec!["b".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("a".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("false".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("b".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
                    needs: Some(vec!["a".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("c".to_string()),
                    run: Some(RunCommand::Simple("true".to_string())),
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
                    run: Some(RunCommand::Simple("echo 'output-a'".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("job-b".to_string()),
                    run: Some(RunCommand::Simple("echo 'output-b'".to_string())),
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
}

#[cfg(test)]
mod tracking_filter_tests {
    use super::*;
    use crate::hooks::tracking::TrackedAttribute;
    use crate::hooks::yaml_config::{JobDef, RunCommand};
    use std::collections::HashSet;

    #[test]
    fn test_filter_jobs_by_changed_path() {
        let jobs = vec![
            JobDef {
                name: Some("path-job".to_string()),
                run: Some(RunCommand::Simple("mise trust".to_string())),
                tracks: Some(vec![TrackedAttribute::Path]),
                ..Default::default()
            },
            JobDef {
                name: Some("branch-job".to_string()),
                run: Some(RunCommand::Simple("docker up".to_string())),
                tracks: Some(vec![TrackedAttribute::Branch]),
                ..Default::default()
            },
            JobDef {
                name: Some("untracked".to_string()),
                run: Some(RunCommand::Simple("bun install".to_string())),
                ..Default::default()
            },
        ];
        let changed = HashSet::from([TrackedAttribute::Path]);
        let filtered = filter_tracked_jobs(&jobs, &changed);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name.as_deref(), Some("path-job"));
    }

    #[test]
    fn test_filter_includes_implicit_tracking() {
        let jobs = vec![JobDef {
            name: Some("implicit-path".to_string()),
            run: Some(RunCommand::Simple(
                "direnv allow {worktree_path}".to_string(),
            )),
            ..Default::default()
        }];
        let changed = HashSet::from([TrackedAttribute::Path]);
        let filtered = filter_tracked_jobs(&jobs, &changed);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_filter_pulls_in_needs_dependencies() {
        let jobs = vec![
            JobDef {
                name: Some("dep".to_string()),
                run: Some(RunCommand::Simple("mise install".to_string())),
                ..Default::default()
            },
            JobDef {
                name: Some("tracked".to_string()),
                run: Some(RunCommand::Simple("mise trust".to_string())),
                tracks: Some(vec![TrackedAttribute::Path]),
                needs: Some(vec!["dep".to_string()]),
                ..Default::default()
            },
        ];
        let changed = HashSet::from([TrackedAttribute::Path]);
        let filtered = filter_tracked_jobs(&jobs, &changed);
        assert_eq!(filtered.len(), 2);
        let names: Vec<_> = filtered
            .iter()
            .map(|j| j.name.as_deref().unwrap())
            .collect();
        assert!(names.contains(&"dep"));
        assert!(names.contains(&"tracked"));
    }
}
