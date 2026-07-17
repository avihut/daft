//! `daft run [<task>]` — execute a named task from daft.yml.
//!
//! Tasks are user-invoked job groups defined under the top-level `tasks:`
//! section (a sibling of `hooks:`). They reuse the hook job machinery
//! wholesale — parallel/piped/follow modes, `needs`, per-job `env`/`root`,
//! skip/only, the threaded log, background jobs — but are triggered explicitly
//! rather than by a lifecycle event.
//!
//! Positioning vs `daft exec`: `exec` runs an ad-hoc command you type on the
//! spot (optionally across many worktrees); `run` runs a named task committed
//! in daft.yml, in the current worktree. This mirrors npm's `exec`/`run` split.
//!
//! Unlike lifecycle hooks — which serve a running environment only if the user
//! wired it into `worktree-post-create` — a task is the *serve on demand* half
//! of the recommended split: provisioning stays finite and unattended in
//! post-create, while starting dev servers / compose stacks / watchers becomes
//! an explicit, attended `daft run`.

use crate::executor::cli_presenter::CliPresenter;
use crate::executor::presenter::JobPresenter;
use crate::git::cancel::CancelFlag;
use crate::hooks::yaml_config_loader::{self, get_effective_jobs};
use crate::hooks::yaml_executor::{self, HookExecutionContext, JobFilter};
use crate::hooks::{TrustDatabase, TrustLevel};
use crate::output::{CliOutput, Output, OutputConfig};
use crate::styles::{bold, cyan, dim};
use crate::{get_current_branch, get_current_worktree_path, get_git_common_dir, get_project_root};
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::sync::Arc;

/// The reserved task name that bare `daft run` executes.
const DEFAULT_TASK: &str = "run";

#[derive(Debug, Parser)]
#[command(name = "daft-run")]
#[command(version = crate::VERSION)]
#[command(about = "Run a named task defined in daft.yml")]
#[command(long_about = "Run a named task from the current worktree's daft.yml.

Tasks live under a top-level `tasks:` section and reuse the hook job schema (jobs, parallel/piped/follow, needs, env, root, skip/only, tags). Bare `git daft run` executes the reserved task named `run`; passing a task name runs that task instead. Tasks stream their output live and run until they exit or you press Ctrl+C (press it twice to force-kill) — they have no execution timeout, which makes them the home for long-running dev servers, compose stacks, and watchers.

Use `run` for tasks committed in daft.yml; use `exec` for an ad-hoc command you type on the spot.")]
pub struct Args {
    /// Task to run. Omit to run the reserved `run` task.
    #[arg(value_name = "TASK")]
    task: Option<String>,

    /// List the tasks defined in daft.yml and exit.
    #[arg(long)]
    list: bool,

    /// Run only the named job within the task.
    #[arg(long, value_name = "NAME")]
    job: Option<String>,

    /// Run only jobs carrying this tag (repeatable).
    #[arg(long, value_name = "TAG")]
    tag: Vec<String>,
}

pub fn run() -> Result<()> {
    // Read the `-C`-stripped argv and skip argv[0] so clap sees "run" as the
    // program name (same dispatcher shape as install/doctor/shared/layout).
    let args_raw: Vec<String> = crate::cli::argv().iter().skip(1).cloned().collect();
    let args = Args::parse_from(args_raw);

    let mut output = CliOutput::new(OutputConfig::default());
    cmd_run(&args, &mut output)
}

fn cmd_run(args: &Args, output: &mut dyn Output) -> Result<()> {
    let worktree_path = get_current_worktree_path()
        .context("Not in a git worktree. Run this command from within a worktree directory.")?;

    let config = yaml_config_loader::load_merged_config(&worktree_path)
        .context("Failed to load daft.yml")?
        .context("No daft.yml found in this worktree")?;

    if args.list {
        return list_tasks(&config, output);
    }

    let task_name = args.task.as_deref().unwrap_or(DEFAULT_TASK);

    let task_def = config
        .tasks
        .get(task_name)
        .ok_or_else(|| unknown_task_error(&config, task_name, args.task.is_none()))?;

    // Tasks are a jobs-only surface (validation enforces this too, but that
    // only runs under `daft hooks validate`; enforce at run time as well).
    if task_def.commands.is_some() {
        bail!(
            "task '{task_name}' uses the legacy 'commands:' form; tasks are jobs-only — use 'jobs:'"
        );
    }

    // Pre-validate `--job` against the task's jobs so the error reads
    // task-flavored rather than the executor's hook-flavored bail.
    if let Some(ref job) = args.job {
        let jobs = get_effective_jobs(task_def);
        if !jobs.iter().any(|j| j.name.as_deref() == Some(job.as_str())) {
            bail!("no job named '{job}' in task '{task_name}'");
        }
    }

    let git_dir = get_git_common_dir().context("Could not determine git directory")?;
    let project_root = get_project_root().context("Could not determine project root")?;
    let branch_name = get_current_branch().unwrap_or_else(|_| "HEAD".to_string());

    // Soft trust hint (like `daft hooks run`): an explicit `daft run` counts as
    // consent and executes regardless of trust, but we nudge the user to trust
    // the repo so lifecycle hooks fire automatically too.
    let trust_level = TrustDatabase::load()
        .unwrap_or_default()
        .get_trust_level(&git_dir);
    if trust_level != TrustLevel::Allow {
        output.info(&format!(
            "{} this repository is not in your trust list ({}).",
            dim("Note:"),
            trust_level
        ));
        output.info(&format!(
            "  {} run `{}` to let lifecycle hooks run automatically too.",
            dim("Tip:"),
            cyan(&crate::daft_cmd("hooks trust"))
        ));
        output.info("");
    }

    let ctx = crate::hooks::HookContext::for_task(
        task_name,
        &project_root,
        &git_dir,
        "origin",
        &worktree_path,
        &branch_name,
    );

    // Tasks stream full output live (like docker compose / foreman) and print a
    // compact per-job row on finish/cancel rather than re-dumping a dev
    // server's entire scrollback.
    let mut hooks_config = crate::core::settings::load_hooks_config()?;
    hooks_config.output.verbose = true;
    hooks_config.output.compact_finalization = true;
    let output_config = hooks_config.output.clone();

    let presenter: Arc<dyn JobPresenter> = CliPresenter::auto(&output_config);
    let filter = JobFilter {
        only_job_name: args.job.clone(),
        only_tags: args.tag.clone(),
        ..Default::default()
    };

    // Two-stage Ctrl+C: first press SIGTERMs the job tree, second SIGKILLs.
    let cancel = Arc::new(CancelFlag::new());
    arm_run_interrupt(Arc::clone(&cancel));

    let cfg = HookExecutionContext {
        source_dir: config.source_dir.as_deref().unwrap_or(".daft"),
        working_dir: &worktree_path,
        rc: config.rc.as_deref(),
        filter: &filter,
        presenter: &presenter,
        repo_log: config.log.as_ref(),
        // Tasks run until they exit or are cancelled — no execution timeout.
        default_job_timeout: None,
        cancel: Some(&cancel),
        trigger_label: Some(format!("run {task_name}")),
    };

    let result = yaml_executor::execute_yaml_hook_with_rc(task_name, task_def, &ctx, output, &cfg);

    crate::interrupt::clear_behavior();
    let result = result?;

    if result.skipped {
        if let Some(reason) = result.skip_reason {
            output.info(&dim(&format!("Skipped: {reason}")));
        }
        return Ok(());
    }
    if !result.success {
        // A cancelled task carries exit code 130 (128 + SIGINT).
        std::process::exit(result.exit_code.unwrap_or(1));
    }

    Ok(())
}

/// Build the error for an unrecognized (or missing reserved) task name, with
/// the available tasks listed.
fn unknown_task_error(
    config: &crate::hooks::yaml_config::YamlConfig,
    task_name: &str,
    was_bare: bool,
) -> anyhow::Error {
    if config.tasks.is_empty() {
        return anyhow::anyhow!(
            "no tasks defined in daft.yml\nAdd a top-level 'tasks:' section to define tasks for `{}`",
            crate::daft_cmd("run")
        );
    }

    let mut names: Vec<&str> = config.tasks.keys().map(String::as_str).collect();
    names.sort_unstable();
    let available = names.join(", ");

    if was_bare {
        anyhow::anyhow!(
            "no '{DEFAULT_TASK}' task defined in daft.yml (bare `{}` runs the task named '{DEFAULT_TASK}')\nAvailable tasks: {available}",
            crate::daft_cmd("run")
        )
    } else {
        anyhow::anyhow!("unknown task: '{task_name}'\nAvailable tasks: {available}")
    }
}

/// Render the `--list` output: task names with job counts.
fn list_tasks(
    config: &crate::hooks::yaml_config::YamlConfig,
    output: &mut dyn Output,
) -> Result<()> {
    if config.tasks.is_empty() {
        output.info(&dim("No tasks defined in daft.yml."));
        return Ok(());
    }

    let mut names: Vec<&String> = config.tasks.keys().collect();
    names.sort();

    output.info(&bold("Available tasks:"));
    output.info("");
    for name in &names {
        let jobs = get_effective_jobs(&config.tasks[*name]);
        let word = if jobs.len() == 1 { "job" } else { "jobs" };
        output.info(&format!("  {} ({} {})", cyan(name), jobs.len(), word));
    }
    output.info("");
    output.info(&format!(
        "Run a task with: {}",
        cyan(&crate::daft_cmd("run <task>"))
    ));

    Ok(())
}

/// Arm the two-stage Ctrl+C escalation, re-arming after each fire (the
/// interrupt slot is one-shot). Each press escalates the shared cancel flag,
/// which the runner's wait loop observes to SIGTERM then SIGKILL the task's
/// process tree. Cloned from `exec::arm_exec_interrupt`.
fn arm_run_interrupt(cancel: Arc<CancelFlag>) {
    crate::interrupt::set_behavior(move || {
        cancel.escalate();
        arm_run_interrupt(Arc::clone(&cancel));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::{HookDef, YamlConfig};
    use std::collections::HashMap;

    fn config_with_tasks(names: &[&str]) -> YamlConfig {
        let mut tasks = HashMap::new();
        for n in names {
            tasks.insert((*n).to_string(), HookDef::default());
        }
        YamlConfig {
            tasks,
            ..Default::default()
        }
    }

    #[test]
    fn unknown_task_error_empty_tasks() {
        let cfg = config_with_tasks(&[]);
        let msg = unknown_task_error(&cfg, "run", true).to_string();
        assert!(msg.contains("no tasks defined"), "got: {msg}");
        assert!(msg.contains("tasks:"), "should hint the section: {msg}");
    }

    #[test]
    fn unknown_task_error_unknown_named() {
        let cfg = config_with_tasks(&["seed", "run", "build"]);
        let msg = unknown_task_error(&cfg, "nope", false).to_string();
        assert!(msg.contains("unknown task: 'nope'"), "got: {msg}");
        // Available tasks listed, sorted.
        assert!(
            msg.contains("Available tasks: build, run, seed"),
            "got: {msg}"
        );
    }

    #[test]
    fn unknown_task_error_bare_missing_reserved() {
        let cfg = config_with_tasks(&["seed", "build"]);
        let msg = unknown_task_error(&cfg, "run", true).to_string();
        assert!(msg.contains("no 'run' task defined"), "got: {msg}");
        assert!(msg.contains("Available tasks: build, seed"), "got: {msg}");
    }
}
