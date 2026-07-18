//! `daft run [<task>] [<args>...]` — execute a named task from daft.yml.
//!
//! Tasks are user-invoked job groups defined under the top-level `tasks:`
//! section (a sibling of `hooks:`). They reuse the hook job machinery
//! wholesale — parallel/piped/follow modes, `needs`, per-job `env`/`root`,
//! skip/only, background jobs — but are triggered explicitly rather than by
//! a lifecycle event.
//!
//! Word resolution: the first word runs as a task when one matches its name
//! (task names are single validated tokens, so the lookup is unambiguous);
//! otherwise the whole word list is forwarded as arguments to the reserved
//! `run` task. Everything after the first word is captured verbatim — flags
//! included — so `daft run`'s own flags come before it, and a leading `--`
//! forces all words to be forwarded without task-name matching. Forwarded
//! words are shell-escaped and appended to the task's single job command
//! (multi-job resolutions reject arguments).
//!
//! Rendering: an invocation that resolves to exactly one foreground job is a
//! **passthrough** — the job inherits the terminal (stdio and all), daft adds
//! no chrome, and the exit code propagates verbatim, exactly like running the
//! command yourself (`daft exec`'s single-target Mode A). Only a multi-job
//! resolution renders an interface: the plan-then-execute rail on a TTY (one
//! receipt row per job, logs threaded under each row), the classic hook block
//! elsewhere.
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

use crate::core::stage::{PlanCommit, Row, StageId, StepKey, StepSpec};
use crate::executor::cli_presenter::CliPresenter;
use crate::executor::presenter::JobPresenter;
use crate::git::cancel::CancelFlag;
use crate::hooks::yaml_config::{HookDef, PlatformRunCommand, RunCommand};
use crate::hooks::yaml_config_loader::{self, get_effective_jobs};
use crate::hooks::yaml_executor::{self, HookExecutionContext, JobFilter};
use crate::hooks::{TrustDatabase, TrustLevel};
use crate::output::timeline::{Timeline, TimelineMode};
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

Tasks live under a top-level `tasks:` section and reuse the hook job schema (jobs, parallel/piped/follow, needs, env, root, skip/only, tags). Bare `git daft run` executes the reserved task named `run`; a first word that names a task runs that task, and any words after it are forwarded to the task as arguments. A first word that names no task is itself forwarded — the whole word list goes to the reserved `run` task. Words after the first are passed through verbatim, flags included, so this command reads its own flags (--list, --job, --tag) only before the first word; write `--` before the first word to forward every word without task-name matching.

Forwarded words are shell-escaped and appended to the task's command, which requires the task to resolve to exactly one foreground job (narrow a multi-job task with --job). A task resolving to a single job passes the terminal straight through to the command — no wrapping interface; a multi-job task renders one live row per job with the logs threaded beneath. Tasks run until they exit or you press Ctrl+C (press it twice to force-kill) — they have no execution timeout, which makes them the home for long-running dev servers, compose stacks, and watchers.

Use `run` for tasks committed in daft.yml; use `exec` for an ad-hoc command you type on the spot.")]
pub struct Args {
    /// Task to run, then arguments forwarded to it. A first word naming no
    /// task is forwarded to the reserved `run` task along with the rest.
    #[arg(
        value_name = "TASK",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    words: Vec<String>,

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
    let args = Args::parse_from(&args_raw);
    let forced_args = has_leading_escape(&args_raw[1..], args.words.len());

    let mut output = CliOutput::new(OutputConfig::default());
    cmd_run(&args, forced_args, &mut output)
}

/// Whether the invocation wrote `--` before the first bare word (`daft run --
/// list`), forcing every word to be forwarded to the reserved task without
/// task-name matching. Clap consumes that leading delimiter, so it is
/// recovered from the raw tokens: the captured words are a suffix of the
/// token list, and force mode means the token just before that suffix was
/// the `--`.
fn has_leading_escape(tokens: &[String], words_len: usize) -> bool {
    tokens.len() > words_len && tokens[tokens.len() - words_len - 1] == "--"
}

/// How the invocation's words resolved to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// No words: the reserved `run` task, no arguments.
    Bare,
    /// The first word named a task; the rest are its arguments.
    Named,
    /// The first word named no task; all words go to the reserved task.
    Fallback,
    /// A leading `--` forced all words to the reserved task.
    Forced,
}

/// Split the captured words into (task name, forwarded arguments). The first
/// word wins as a task name when one matches — a task name is a single
/// validated token, so the lookup is unambiguous — and falls through to the
/// reserved `run` task as an argument otherwise.
fn resolve_invocation<'w>(
    tasks: &std::collections::HashMap<String, HookDef>,
    words: &'w [String],
    forced: bool,
) -> (&'w str, &'w [String], Origin) {
    if forced {
        return (DEFAULT_TASK, words, Origin::Forced);
    }
    match words {
        [] => (DEFAULT_TASK, &[], Origin::Bare),
        [first, rest @ ..] if tasks.contains_key(first.as_str()) => (first, rest, Origin::Named),
        _ => (DEFAULT_TASK, words, Origin::Fallback),
    }
}

fn cmd_run(args: &Args, forced_args: bool, output: &mut dyn Output) -> Result<()> {
    let worktree_path = get_current_worktree_path()
        .context("Not in a git worktree. Run this command from within a worktree directory.")?;

    let config = yaml_config_loader::load_merged_config(&worktree_path)
        .context("Failed to load daft.yml")?
        .context("No daft.yml found in this worktree")?;

    if args.list {
        return list_tasks(&config, output);
    }

    let (task_name, task_args, origin) =
        resolve_invocation(&config.tasks, &args.words, forced_args);

    let task_def = config
        .tasks
        .get(task_name)
        .ok_or_else(|| unknown_task_error(&config, &args.words, origin))?;

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
    // the repo so lifecycle hooks fire automatically too. The hint goes to
    // stderr (`notice`, not `info`) so a single-job passthrough's stdout stays
    // verbatim — `daft run dump > out.json` must capture the job's output, not
    // three lines of trust advice prepended to it.
    let trust_level = TrustDatabase::load()
        .unwrap_or_default()
        .get_trust_level(&git_dir);
    if trust_level != TrustLevel::Allow {
        output.notice(&format!(
            "{} this repository is not in your trust list ({}).",
            dim("Note:"),
            trust_level
        ));
        output.notice(&format!(
            "  {} run `{}` to let lifecycle hooks run automatically too.",
            dim("Tip:"),
            cyan(&crate::daft_cmd("hooks trust"))
        ));
        output.notice("");
    }

    let ctx = crate::hooks::HookContext::for_task(
        task_name,
        &project_root,
        &git_dir,
        "origin",
        &worktree_path,
        &branch_name,
    );

    // Tasks stream full output live (like docker compose / foreman); the
    // knobs also shape the non-TTY hook-block fallback, which prints a
    // compact per-job row on finish/cancel rather than re-dumping a dev
    // server's entire scrollback.
    let mut hooks_config = crate::core::settings::load_hooks_config()?;
    hooks_config.output.verbose = true;
    hooks_config.output.compact_finalization = true;
    // Label the progress header "daft run" rather than the default "daft hooks".
    hooks_config.output.banner = "daft run";
    let output_config = hooks_config.output.clone();

    let filter = JobFilter {
        only_job_name: args.job.clone(),
        only_tags: args.tag.clone(),
        ..Default::default()
    };

    // An invocation resolving to a single foreground job passes the terminal
    // straight through — no banner, no rows, no summary; the job's own output
    // is the interface, exactly as if the user ran the command themselves.
    // Multi-job (or background) resolutions render the job interface: the
    // rail on a TTY, the classic hook block elsewhere. Forwarded arguments
    // are appended to the passthrough job's command (and are an error on any
    // other resolution — they'd have no single command to attach to).
    let passthrough_def = passthrough_def(task_def, task_name, &args.job, &args.tag, task_args)?;

    let task_key = StepKey::new(StageId::Task);
    let (def_to_run, presenter, mut timeline): (&HookDef, Arc<dyn JobPresenter>, Option<Timeline>) =
        match &passthrough_def {
            Some(def) => (def, CliPresenter::hidden(&output_config), None),
            None => {
                let header = format!("Running task {task_name} on {branch_name}");
                let mut tl = Timeline::new(TimelineMode::auto(false), true, header);
                tl.commit_plan(PlanCommit::new(vec![Row::Step(
                    StepSpec::new(task_key.clone()).with_label(task_name),
                )]));
                let presenter =
                    CliPresenter::embedded(&output_config, tl.handle(), task_key.clone());
                (task_def, presenter, Some(tl))
            }
        };

    // Two-stage Ctrl+C: first press SIGTERMs the job tree, second SIGKILLs.
    // Armed after the plan commits so it overrides the region's own
    // collapse-and-exit behavior (`daft exec`'s pattern) — the first ^C must
    // cancel the jobs, not tear the rail down.
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
        trigger_label: Some(if task_args.is_empty() {
            format!("run {task_name}")
        } else {
            format!("run {task_name} {}", crate::utils::quote_argv(task_args))
        }),
    };

    let result =
        yaml_executor::execute_yaml_hook_with_rc(task_name, def_to_run, &ctx, output, &cfg);

    // Close the rail with the outcome footer (no-op off the rail); the
    // region's teardown also clears the interrupt slot it owns.
    if let Some(tl) = timeline.as_mut() {
        let elapsed = tl.elapsed_display();
        match &result {
            Err(_) => tl.abort(&format!("Failed after {elapsed}")),
            Ok(r) => {
                tl.resolve_hook_step(&task_key, r.skipped, r.skip_reason.as_deref());
                match rail_outcome(cancel.is_cancelled(), r.success, r.skipped) {
                    RailOutcome::Cancelled => tl.abort(&format!("Cancelled after {elapsed}")),
                    RailOutcome::Failures => {
                        tl.finish(&format!("Finished with failures in {elapsed}"))
                    }
                    RailOutcome::Done => tl.finish(&format!("Done in {elapsed}")),
                }
            }
        }
    }

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

/// Which terminal footer the multi-job timeline rail closes with.
#[derive(Debug, PartialEq, Eq)]
enum RailOutcome {
    Cancelled,
    Failures,
    Done,
}

/// Pick the rail's closing footer from the run's outcome. A cancellation is
/// keyed on the interrupt flag alone — never on a 130 exit code, which a job
/// can legitimately return on its own without any Ctrl+C (that reads as a
/// failure, not an interruption).
fn rail_outcome(cancelled: bool, success: bool, skipped: bool) -> RailOutcome {
    if cancelled {
        RailOutcome::Cancelled
    } else if !success && !skipped {
        RailOutcome::Failures
    } else {
        RailOutcome::Done
    }
}

/// The forced-interactive clone for a single-job passthrough run, or `None`
/// when the invocation needs the job interface: more than one job after
/// `--job`/`--tag` narrowing, a background job (its dispatch receipt must
/// render), or filters matching nothing (the executor owns that error).
///
/// Forwarded arguments only make sense on the passthrough: they are
/// shell-escaped and appended to the single job's command. Any other
/// resolution with arguments is an error — there is no single command line
/// to attach them to.
///
/// Forcing `interactive` is what makes the passthrough real: the job
/// inherits daft's stdio and process group, so its output hits the terminal
/// unwrapped and a bare Ctrl+C reaches it directly.
fn passthrough_def(
    task_def: &HookDef,
    task_name: &str,
    only_job: &Option<String>,
    only_tags: &[String],
    task_args: &[String],
) -> Result<Option<HookDef>> {
    let jobs = get_effective_jobs(task_def);
    let selected: Vec<&crate::hooks::yaml_config::JobDef> = jobs
        .iter()
        .filter(|j| {
            only_job
                .as_deref()
                .is_none_or(|name| j.name.as_deref() == Some(name))
        })
        .filter(|j| {
            only_tags.is_empty()
                || j.tags
                    .as_ref()
                    .is_some_and(|tags| tags.iter().any(|t| only_tags.contains(t)))
        })
        .collect();
    let [job] = selected.as_slice() else {
        if selected.len() > 1 && !task_args.is_empty() {
            bail!(
                "task '{task_name}' resolves to {} jobs — forwarded arguments need exactly one foreground job\nNarrow the run with --job <name>",
                selected.len()
            );
        }
        return Ok(None);
    };
    let label = job.name.as_deref().unwrap_or(task_name);
    if crate::hooks::job_adapter::resolve_background(job.background, task_def.background) {
        if !task_args.is_empty() {
            bail!(
                "job '{label}' runs in the background — forwarded arguments need a foreground job"
            );
        }
        return Ok(None);
    }
    let appended_run = if task_args.is_empty() {
        None
    } else {
        let run = job.run.as_ref().with_context(|| {
            format!("job '{label}' has no run command to receive forwarded arguments")
        })?;
        Some(append_args(run, task_args, label)?)
    };
    let target = job.name.clone();
    let mut def = task_def.clone();
    if let Some(jobs) = def.jobs.as_mut() {
        for j in jobs.iter_mut() {
            if j.name == target {
                j.interactive = Some(true);
                if let Some(run) = &appended_run {
                    j.run = Some(run.clone());
                }
            }
        }
    }
    Ok(Some(def))
}

/// Append the forwarded words — each shell-escaped — to the job's run
/// command. Only a single-line, single-command run can receive arguments:
/// appending to a multi-line script or a command list would silently attach
/// them to whatever happens to execute last.
fn append_args(run: &RunCommand, args: &[String], label: &str) -> Result<RunCommand> {
    let base = match run {
        RunCommand::Simple(s) => s.clone(),
        RunCommand::Platform(map) => {
            let Some(os) = RunCommand::current_target_os() else {
                // Unsupported platform: the adapter reports "no command for
                // this OS" downstream; nothing runs, so nothing to append to.
                return Ok(run.clone());
            };
            match map.get(&os) {
                Some(PlatformRunCommand::Simple(s)) => s.clone(),
                Some(PlatformRunCommand::List(_)) => bail!(
                    "job '{label}' runs a list of commands — forwarded arguments can only be appended to a single command"
                ),
                None => return Ok(run.clone()),
            }
        }
    };
    // A YAML block scalar (`run: |`) carries a trailing newline; appending
    // after it would run the words as their own command line.
    let base = base.trim_end();
    if base.is_empty() {
        bail!("job '{label}' has an empty run command — nothing to forward arguments to");
    }
    if base.contains('\n') {
        bail!(
            "job '{label}' runs a multi-line script — forwarded arguments can only be appended to a single-line command"
        );
    }
    Ok(RunCommand::Simple(format!(
        "{base} {}",
        crate::utils::quote_argv(args)
    )))
}

/// Build the error for a task lookup miss, listing the available tasks. The
/// wording tracks how the words resolved — a bare, forced, or fallback
/// invocation misses only when the reserved `run` task is undefined.
fn unknown_task_error(
    config: &crate::hooks::yaml_config::YamlConfig,
    words: &[String],
    origin: Origin,
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

    match origin {
        Origin::Bare => anyhow::anyhow!(
            "no '{DEFAULT_TASK}' task defined in daft.yml (bare `{}` runs the task named '{DEFAULT_TASK}')\nAvailable tasks: {available}",
            crate::daft_cmd("run")
        ),
        Origin::Forced => anyhow::anyhow!(
            "no '{DEFAULT_TASK}' task defined in daft.yml (`--` forwards every word to the task named '{DEFAULT_TASK}')\nAvailable tasks: {available}"
        ),
        Origin::Fallback => anyhow::anyhow!(
            "unknown task: '{}' (no '{DEFAULT_TASK}' task defined to receive it as an argument)\nAvailable tasks: {available}",
            words[0]
        ),
        // Named origins matched an existing task; a miss cannot carry one.
        Origin::Named => anyhow::anyhow!("unknown task\nAvailable tasks: {available}"),
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
    use crate::hooks::yaml_config::{JobDef, YamlConfig};
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

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // ── rail footer outcome ───────────────────────────────────────────────

    #[test]
    fn rail_outcome_keys_cancellation_on_the_interrupt_flag_not_exit_130() {
        // A job that exits 130 on its own (no Ctrl+C) is a failure, not a
        // cancellation — the footer must not read the exit code.
        assert_eq!(
            rail_outcome(false, false, false),
            RailOutcome::Failures,
            "a 130 exit without an interrupt is a failure"
        );
        // The interrupt flag wins regardless of the aggregate success flag.
        assert_eq!(rail_outcome(true, false, false), RailOutcome::Cancelled);
        assert_eq!(rail_outcome(true, true, false), RailOutcome::Cancelled);
        // Clean success, and a skip, both close as done.
        assert_eq!(rail_outcome(false, true, false), RailOutcome::Done);
        assert_eq!(rail_outcome(false, true, true), RailOutcome::Done);
    }

    // ── clap surface: verbatim trailing capture ───────────────────────────

    fn parse(tokens: &[&str]) -> Args {
        Args::parse_from(std::iter::once("run").chain(tokens.iter().copied()))
    }

    #[test]
    fn flags_after_the_first_word_are_captured_verbatim() {
        let args = parse(&["build", "--job", "x"]);
        assert_eq!(args.words, words(&["build", "--job", "x"]));
        assert_eq!(args.job, None, "the flag belongs to the task, not to run");
    }

    #[test]
    fn flags_before_the_first_word_belong_to_run() {
        let args = parse(&["--job", "web", "stack", "--verbose"]);
        assert_eq!(args.job.as_deref(), Some("web"));
        assert_eq!(args.words, words(&["stack", "--verbose"]));
    }

    #[test]
    fn leading_double_dash_is_consumed_by_clap() {
        // The delimiter itself is eaten; has_leading_escape recovers it from
        // the raw tokens.
        let args = parse(&["--", "list"]);
        assert_eq!(args.words, words(&["list"]));
        assert!(!args.list);
    }

    #[test]
    fn double_dash_after_capture_starts_is_a_word() {
        let args = parse(&["build", "--", "x"]);
        assert_eq!(args.words, words(&["build", "--", "x"]));
    }

    #[test]
    fn detects_the_leading_escape() {
        assert!(has_leading_escape(&words(&["--", "list"]), 1));
        assert!(has_leading_escape(
            &words(&["--job", "w", "--", "serve"]),
            1
        ));
        assert!(has_leading_escape(&words(&["--"]), 0), "bare `run --`");
        assert!(!has_leading_escape(&words(&["list"]), 1));
        assert!(!has_leading_escape(&words(&[]), 0));
        // A `--` between words is captured, not a leading escape.
        assert!(!has_leading_escape(&words(&["build", "--", "x"]), 3));
    }

    // ── word resolution ───────────────────────────────────────────────────

    #[test]
    fn bare_invocation_resolves_the_reserved_task() {
        let cfg = config_with_tasks(&["run"]);
        let w = words(&[]);
        let (name, args, origin) = resolve_invocation(&cfg.tasks, &w, false);
        assert_eq!((name, args, origin), ("run", &w[..], Origin::Bare));
    }

    #[test]
    fn first_word_naming_a_task_wins_and_the_rest_forward() {
        let cfg = config_with_tasks(&["run", "greet"]);
        let w = words(&["greet", "hello", "world"]);
        let (name, args, origin) = resolve_invocation(&cfg.tasks, &w, false);
        assert_eq!(name, "greet");
        assert_eq!(args, &w[1..]);
        assert_eq!(origin, Origin::Named);
    }

    #[test]
    fn unmatched_first_word_falls_through_to_the_reserved_task() {
        let cfg = config_with_tasks(&["run", "greet"]);
        let w = words(&["hello", "world"]);
        let (name, args, origin) = resolve_invocation(&cfg.tasks, &w, false);
        assert_eq!(name, "run");
        assert_eq!(args, &w[..], "every word forwards");
        assert_eq!(origin, Origin::Fallback);
    }

    #[test]
    fn forced_escape_skips_task_name_matching() {
        // `daft run -- greet`: greet is a task, but the escape forces it to
        // be an argument to the reserved task.
        let cfg = config_with_tasks(&["run", "greet"]);
        let w = words(&["greet"]);
        let (name, args, origin) = resolve_invocation(&cfg.tasks, &w, true);
        assert_eq!(name, "run");
        assert_eq!(args, &w[..]);
        assert_eq!(origin, Origin::Forced);
    }

    // ── task lookup errors ────────────────────────────────────────────────

    #[test]
    fn unknown_task_error_empty_tasks() {
        let cfg = config_with_tasks(&[]);
        let msg = unknown_task_error(&cfg, &words(&[]), Origin::Bare).to_string();
        assert!(msg.contains("no tasks defined"), "got: {msg}");
        assert!(msg.contains("tasks:"), "should hint the section: {msg}");
    }

    #[test]
    fn unknown_task_error_fallback_without_reserved_task() {
        let cfg = config_with_tasks(&["seed", "build"]);
        let msg = unknown_task_error(&cfg, &words(&["nope"]), Origin::Fallback).to_string();
        assert!(msg.contains("unknown task: 'nope'"), "got: {msg}");
        assert!(
            msg.contains("no 'run' task defined to receive it as an argument"),
            "should explain the fallback: {msg}"
        );
        assert!(msg.contains("Available tasks: build, seed"), "got: {msg}");
    }

    #[test]
    fn unknown_task_error_bare_missing_reserved() {
        let cfg = config_with_tasks(&["seed", "build"]);
        let msg = unknown_task_error(&cfg, &words(&[]), Origin::Bare).to_string();
        assert!(msg.contains("no 'run' task defined"), "got: {msg}");
        assert!(msg.contains("Available tasks: build, seed"), "got: {msg}");
    }

    #[test]
    fn unknown_task_error_forced_missing_reserved() {
        let cfg = config_with_tasks(&["seed"]);
        let msg = unknown_task_error(&cfg, &words(&["x"]), Origin::Forced).to_string();
        assert!(msg.contains("no 'run' task defined"), "got: {msg}");
        assert!(msg.contains("`--` forwards"), "got: {msg}");
    }

    // ── passthrough gating ────────────────────────────────────────────────

    fn job(name: &str) -> JobDef {
        JobDef {
            name: Some(name.to_string()),
            run: Some(RunCommand::Simple(format!("echo {name}"))),
            ..Default::default()
        }
    }

    fn task(jobs: Vec<JobDef>) -> HookDef {
        HookDef {
            jobs: Some(jobs),
            ..Default::default()
        }
    }

    fn gate(def: &HookDef, only_job: &Option<String>, only_tags: &[String]) -> Option<HookDef> {
        passthrough_def(def, "task", only_job, only_tags, &[]).unwrap()
    }

    #[test]
    fn single_foreground_job_passes_through_forced_interactive() {
        let def =
            gate(&task(vec![job("serve")]), &None, &[]).expect("single fg job is a passthrough");
        let forced = &def.jobs.as_ref().unwrap()[0];
        assert_eq!(forced.interactive, Some(true));
    }

    #[test]
    fn multi_job_task_renders_the_interface() {
        let def = task(vec![job("api"), job("web")]);
        assert!(gate(&def, &None, &[]).is_none());
    }

    #[test]
    fn job_flag_narrowing_to_one_passes_through() {
        let def = task(vec![job("api"), job("web")]);
        let forced =
            gate(&def, &Some("web".into()), &[]).expect("--job narrows to a single passthrough");
        let jobs = forced.jobs.unwrap();
        assert_eq!(jobs[0].interactive, None, "api stays untouched");
        assert_eq!(jobs[1].interactive, Some(true), "web forced interactive");
    }

    #[test]
    fn tag_narrowing_to_one_passes_through() {
        let mut api = job("api");
        api.tags = Some(vec!["backend".into()]);
        let def = task(vec![api, job("web")]);
        assert!(gate(&def, &None, &["backend".into()]).is_some());
    }

    #[test]
    fn tag_matching_nothing_defers_to_the_executor() {
        let def = task(vec![job("api"), job("web")]);
        assert!(gate(&def, &None, &["nope".into()]).is_none());
    }

    #[test]
    fn background_job_renders_its_dispatch_receipt() {
        let mut bg = job("indexer");
        bg.background = Some(true);
        assert!(gate(&task(vec![bg]), &None, &[]).is_none());
    }

    #[test]
    fn hook_level_background_default_blocks_passthrough() {
        let mut def = task(vec![job("serve")]);
        def.background = Some(true);
        assert!(gate(&def, &None, &[]).is_none());
    }

    #[test]
    fn already_interactive_job_stays_a_passthrough() {
        let mut j = job("shell");
        j.interactive = Some(true);
        let def = gate(&task(vec![j]), &None, &[]).expect("passthrough");
        assert_eq!(def.jobs.unwrap()[0].interactive, Some(true));
    }

    // ── forwarded arguments ───────────────────────────────────────────────

    fn simple(def: &HookDef, idx: usize) -> String {
        match def.jobs.as_ref().unwrap()[idx].run.as_ref().unwrap() {
            RunCommand::Simple(s) => s.clone(),
            other => panic!("expected a simple run command, got {other:?}"),
        }
    }

    #[test]
    fn args_append_to_the_single_job_preserving_boundaries() {
        let def = passthrough_def(
            &task(vec![job("serve")]),
            "task",
            &None,
            &[],
            &words(&["a b", "c"]),
        )
        .unwrap()
        .expect("passthrough");
        let appended = simple(&def, 0);
        assert_eq!(
            shlex::split(&appended).unwrap(),
            vec!["echo", "serve", "a b", "c"],
            "quoting must preserve argument boundaries: {appended}"
        );
        assert_eq!(def.jobs.as_ref().unwrap()[0].interactive, Some(true));
    }

    #[test]
    fn args_on_a_multi_job_task_error_with_the_narrowing_hint() {
        let def = task(vec![job("api"), job("web")]);
        let err = passthrough_def(&def, "stack", &None, &[], &words(&["x"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("resolves to 2 jobs"), "got: {err}");
        assert!(err.contains("--job"), "should hint narrowing: {err}");
    }

    #[test]
    fn args_with_job_narrowing_rewrite_only_that_job() {
        let def = task(vec![job("api"), job("web")]);
        let out = passthrough_def(&def, "stack", &Some("web".into()), &[], &words(&["x"]))
            .unwrap()
            .expect("narrowed passthrough");
        assert_eq!(simple(&out, 0), "echo api", "api untouched");
        assert_eq!(simple(&out, 1), "echo web x");
    }

    #[test]
    fn args_on_a_background_job_error() {
        let mut bg = job("indexer");
        bg.background = Some(true);
        let err = passthrough_def(&task(vec![bg]), "task", &None, &[], &words(&["x"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("background"), "got: {err}");
    }

    #[test]
    fn args_with_no_filter_match_defer_to_the_executor() {
        let def = task(vec![job("api"), job("web")]);
        let out = passthrough_def(&def, "task", &None, &["nope".into()], &words(&["x"])).unwrap();
        assert!(out.is_none(), "the executor owns the no-match error");
    }

    #[test]
    fn append_trims_a_block_scalar_trailing_newline() {
        // `run: |` blocks end with '\n'; appending after it would run the
        // words as their own command line.
        let out =
            append_args(&RunCommand::Simple("echo hi\n".into()), &words(&["x"]), "j").unwrap();
        assert_eq!(out, RunCommand::Simple("echo hi x".into()));
    }

    #[test]
    fn append_rejects_a_multi_line_script() {
        let err = append_args(
            &RunCommand::Simple("echo a\necho b".into()),
            &words(&["x"]),
            "j",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("multi-line"), "got: {err}");
    }

    #[test]
    fn append_rejects_an_empty_run_command() {
        let err = append_args(&RunCommand::Simple("  \n".into()), &words(&["x"]), "j")
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty run command"), "got: {err}");
    }

    #[test]
    fn append_reaches_the_current_platform_command() {
        let os = RunCommand::current_target_os().expect("test host has a target OS");
        let mut map = HashMap::new();
        map.insert(os, PlatformRunCommand::Simple("echo plat".into()));
        let out = append_args(&RunCommand::Platform(map), &words(&["x"]), "j").unwrap();
        assert_eq!(out, RunCommand::Simple("echo plat x".into()));
    }

    #[test]
    fn append_rejects_a_platform_command_list() {
        let os = RunCommand::current_target_os().expect("test host has a target OS");
        let mut map = HashMap::new();
        map.insert(
            os,
            PlatformRunCommand::List(vec!["echo a".into(), "echo b".into()]),
        );
        let err = append_args(&RunCommand::Platform(map), &words(&["x"]), "j")
            .unwrap_err()
            .to_string();
        assert!(err.contains("list of commands"), "got: {err}");
    }

    #[test]
    fn append_leaves_a_platform_miss_untouched() {
        // No command for this OS: nothing runs downstream, nothing to
        // append to.
        let run = RunCommand::Platform(HashMap::new());
        let out = append_args(&run, &words(&["x"]), "j").unwrap();
        assert_eq!(out, run);
    }

    #[test]
    fn args_on_a_job_without_a_run_command_error() {
        let bare = JobDef {
            name: Some("ghost".into()),
            ..Default::default()
        };
        let err = passthrough_def(&task(vec![bare]), "task", &None, &[], &words(&["x"]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("no run command"), "got: {err}");
    }
}
