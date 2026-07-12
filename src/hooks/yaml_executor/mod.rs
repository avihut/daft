//! YAML-based hook job execution engine.
//!
//! This module executes jobs defined in YAML hook configurations.
//! It supports sequential, parallel, piped, and follow execution modes.
//!
//! Job execution is delegated to the generic executor (`crate::executor::runner`)
//! via the job adapter (`crate::hooks::job_adapter`).

pub mod partition;
pub use partition::partition_foreground_background;

use super::environment::HookContext;
use super::executor::HookResult;
use super::template;
use super::yaml_config::{GroupDef, HookDef, JobDef};
use super::yaml_config_loader::get_effective_jobs;
use crate::executor::LogConfig;
use crate::executor::presenter::JobPresenter;
use crate::hooks::tracking::{TrackedAttribute, effective_tracks};
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
/// Two independent axes:
/// - The **include** side (`only_job_name`, `only_tags`) is used by
///   `hooks run` to restrict execution to a named job or tagged jobs. An empty
///   include result is an error (`bail!`).
/// - The **exclude** side (`skip`, from `--skip-hooks`) drops the matched jobs
///   *plus their downstream dependents* (see
///   [`crate::hooks::job_adapter::compute_skip_cascade`]) and reports them as
///   attributed skips. An empty exclude result is a benign no-op (warn only).
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    /// Run only the job with this name.
    pub only_job_name: Option<String>,
    /// Run only jobs that have at least one of these tags.
    pub only_tags: Vec<String>,
    /// `--skip-hooks` selectors: jobs (and their dependents) to exclude.
    pub skip: crate::hooks::job_adapter::SkipSelectors,
}

impl JobFilter {
    /// Build an exclude-only filter from raw `--skip-hooks` selector tokens.
    /// Empty input yields the default (no-op) filter, so the common no-flag
    /// path costs nothing.
    pub fn skipping(selectors: &[String]) -> Self {
        Self {
            skip: crate::hooks::job_adapter::parse_skip_selectors(selectors),
            ..Default::default()
        }
    }
}

/// Bundle of configuration values threaded into `execute_yaml_hook_with_rc`.
///
/// Replaces a long passthrough arg list (#476 Tier-3 API surface). Holds
/// non-per-hook values that previously lived as separate parameters.
pub struct HookExecutionContext<'a> {
    /// Directory under the working tree where hook YAML lives (typically
    /// `.daft`).
    pub source_dir: &'a str,
    /// Resolved working directory for the hook fire.
    pub working_dir: &'a Path,
    /// Optional shell RC file to source before every job command.
    pub rc: Option<&'a str>,
    /// Job filter from CLI args (`hooks run --job` / `--tag`).
    pub filter: &'a JobFilter,
    /// Presenter for live progress updates. Owned via `Arc` so spawned job
    /// threads can clone cheaply.
    pub presenter: &'a Arc<dyn JobPresenter>,
    /// Top-level `log:` section from the YAML config — propagated into each
    /// job's effective `LogConfig`.
    pub repo_log: Option<&'a LogConfig>,
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
    let filter = JobFilter::default();
    let cfg = HookExecutionContext {
        source_dir,
        working_dir,
        rc: None,
        filter: &filter,
        presenter: &presenter,
        repo_log: None,
    };
    execute_yaml_hook_with_rc(hook_name, hook_def, ctx, output, &cfg)
}

/// Execute a YAML-defined hook with optional RC file.
pub fn execute_yaml_hook_with_rc(
    hook_name: &str,
    hook_def: &HookDef,
    ctx: &HookContext,
    output: &mut dyn Output,
    cfg: &HookExecutionContext<'_>,
) -> Result<HookResult> {
    let source_dir = cfg.source_dir;
    let working_dir = cfg.working_dir;
    let rc = cfg.rc;
    let filter = cfg.filter;
    let presenter = cfg.presenter;
    let repo_log = cfg.repo_log;
    // Check hook-level skip/only conditions
    if let Some(ref skip) = hook_def.skip
        && let Some(info) = super::conditions::should_skip(skip, working_dir)
    {
        output.debug(&format!("Skipping {hook_name}: {}", info.reason));
        return Ok(if info.ran_command {
            HookResult::skipped_after_command(info.reason)
        } else {
            HookResult::skipped(info.reason)
        });
    }
    if let Some(ref only) = hook_def.only
        && let Some(info) = super::conditions::should_only_skip(only, working_dir)
    {
        output.debug(&format!("Skipping {hook_name}: {}", info.reason));
        return Ok(if info.ran_command {
            HookResult::skipped_after_command(info.reason)
        } else {
            HookResult::skipped(info.reason)
        });
    }

    // Build hook environment early so we can write an invocation record
    // unconditionally — every hook that fires (even empty / fully-filtered)
    // gets logged before any early returns below.
    let hook_env_obj = super::environment::HookEnvironment::from_context(ctx);

    // Compute repo hash and invocation ID unconditionally so every hook
    // invocation lands in the log store, even fg-only, empty, and remove hooks.
    //
    // Use the explicit `ctx.git_dir` rather than the cwd-based
    // `compute_repo_id()`. Hooks fire for a specific repo carried in the
    // context, but the cwd may be elsewhere — e.g., `daft repo remove <path>`
    // invoked from a parent directory or `/`. The cwd-based variant runs
    // `git rev-parse --git-common-dir` in cwd; outside any repo it errors,
    // the error propagates up through `try_yaml_hook`, the executor catches
    // it and falls through to legacy script discovery (which finds nothing
    // for daft.yml-only repos), and the hook silently never fires.
    let repo_hash = crate::core::repo_identity::compute_repo_id_from_common_dir(&ctx.git_dir)?;
    let invocation_id = crate::coordinator::log_store::generate_invocation_id();
    // Honor `ctx.state_dir` so unit tests route LogStore writes into a
    // tempdir; production callers leave it `None` and fall through to
    // `daft_state_dir()` (XDG state home).
    let state_base = match &ctx.state_dir {
        Some(p) => p.clone(),
        None => crate::daft_state_dir()?,
    };
    let store = std::sync::Arc::new(crate::coordinator::log_store::LogStore::for_repo_in(
        &repo_hash,
        &state_base,
    )?);

    let trigger_command = if ctx.command == "hooks-run" {
        format!("hooks run {}", hook_name)
    } else {
        hook_name.to_string()
    };

    let inv_meta = crate::coordinator::log_store::InvocationMeta {
        invocation_id: invocation_id.clone(),
        trigger_command: trigger_command.clone(),
        hook_type: hook_name.to_string(),
        worktree: ctx.branch_name.clone(),
        created_at: chrono::Utc::now(),
    };
    if let Err(e) = store.write_invocation_meta(&invocation_id, &inv_meta) {
        eprintln!("daft: failed to write invocation meta for '{hook_name}': {e}");
    }

    let mut jobs = get_effective_jobs(hook_def);

    if jobs.is_empty() {
        return Ok(HookResult::skipped("No jobs defined"));
    }

    // `--skip-hooks all` short-circuits the whole fire (the old --no-hooks
    // path). The invocation meta was already written above, so this fire is
    // still logged; we just run no jobs.
    if filter.skip.all {
        return Ok(HookResult::skipped("all hooks skipped by request"));
    }

    // A hook-type selector (`--skip-hooks worktree-post-create`) names *this*
    // fire: skip every job in the hook. Unlike `all`, this is NOT a silent
    // short-circuit — each job is routed into an attributed skip below and
    // rendered/recorded exactly as if it had been named directly, so the hook
    // still appears in the output with every job marked skipped. A hook-type
    // token naming a *different* hook (the worktree-pre-create fire of the same
    // command, or a hook this command never fires) leaves this false and is a
    // silent no-op — it never reaches `compute_skip_cascade`'s unmatched set,
    // so it raises no warning (it is a valid hook name, just not this fire).
    let skip_whole_hook = filter
        .skip
        .hook_types
        .iter()
        .any(|h| h.yaml_name() == hook_name);

    // Resolve which jobs run and which are skipped (with attribution). A
    // hook-type selector (`--skip-hooks worktree-post-create`) names *this*
    // fire: every job is skipped wholesale and ALL include/exclude/tracking
    // filters below are bypassed — the emptied job list would otherwise divert
    // into a "no job matched" bail ahead of the skip render. `requested_skips`
    // is declared before the branch so both render sites (the `specs.is_empty()`
    // early return and the post-header loop) can read it regardless of which
    // arm populated it.
    let mut requested_skips: Vec<crate::hooks::job_adapter::SkippedJob> = Vec::new();
    if skip_whole_hook {
        // Whole-hook skip: every job becomes a direct `Requested` skip (same
        // reason as a name match), then the job list is emptied so the fire
        // flows through the `specs.is_empty()` render/record path below. Config
        // `exclude_tags` is intentionally *not* applied here — a whole-hook skip
        // reports every declared job, excluded-tag jobs included.
        for job in &jobs {
            requested_skips.push(crate::hooks::job_adapter::SkippedJob {
                name: job.name.clone().unwrap_or_else(|| "(unnamed)".to_string()),
                background: crate::hooks::job_adapter::resolve_background(
                    job.background,
                    hook_def.background,
                ),
                reason: crate::hooks::job_adapter::SkipCause::Requested.reason(),
            });
        }
        jobs.clear();
    } else {
        // Filter out jobs matching config-level `exclude_tags`, before the
        // cascade so excluded-tag jobs never seed it.
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

        // Apply `--skip-hooks` exclusion: drop the matched jobs AND the
        // transitive closure of jobs that `needs:` them (downstream
        // dependents), routing each into an attributed skip. Runs while
        // `needs:` is intact (before `yaml_jobs_to_specs`) so the cascade can
        // walk the reverse-`needs` graph; this makes the adapter's
        // needs-stripping a no-op for the exclude path. Unmatched selectors
        // warn (the job runs) but never error — silent no-match is more
        // dangerous in the exclude direction.
        if !filter.skip.is_empty() {
            let cascade = crate::hooks::job_adapter::compute_skip_cascade(&jobs, &filter.skip);
            for sel in &cascade.unmatched {
                output.warning(&format!(
                    "--skip-hooks: no job or tag matched '{sel}' in hook '{hook_name}'"
                ));
            }
            if !cascade.excluded.is_empty() {
                jobs.retain(|job| {
                    let Some(name) = job.name.as_deref() else {
                        return true;
                    };
                    match cascade.excluded.get(name) {
                        None => true,
                        Some(cause) => {
                            requested_skips.push(crate::hooks::job_adapter::SkippedJob {
                                name: name.to_string(),
                                background: crate::hooks::job_adapter::resolve_background(
                                    job.background,
                                    hook_def.background,
                                ),
                                reason: cause.reason(),
                            });
                            false
                        }
                    }
                });
            }
        }

        // Apply inclusion filters (from `hooks run --job` / `--tag`).
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

        // Apply tracking filter when changed_attributes are present (move hooks).
        if let Some(ref changed) = ctx.changed_attributes {
            jobs = filter_tracked_jobs(&jobs, changed);
            if jobs.is_empty() {
                return Ok(HookResult::skipped("No jobs match changed attributes"));
            }
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

    // Build hook environment map for job specs (uses the hoisted hook_env_obj).
    let hook_env = hook_env_obj.vars().clone();

    // Convert filtered JobDefs to generic JobSpecs
    let adapter = crate::hooks::job_adapter::JobAdapterContext {
        rc,
        hook_background: hook_def.background,
        repo_log,
    };
    let (specs, mut skipped_jobs) = crate::hooks::job_adapter::yaml_jobs_to_specs(
        &jobs,
        ctx,
        &hook_env,
        source_dir,
        working_dir,
        &adapter,
    );

    // Fold `--skip-hooks` exclusions into the skipped-job set so the
    // persistence loop below records them (visible via `daft hooks jobs`)
    // exactly like the adapter's own group/platform/condition skips. The
    // separate `requested_skips` copy drives the live presenter render.
    skipped_jobs.extend(requested_skips.iter().cloned());

    // Open the per-repo SQLite store once and reuse for both the
    // skipped-job persistence loop and the repo-policy write below.
    // `for_repo_base` is the non-wiping constructor; the coordinator
    // is the only caller that should sweep legacy files.
    let job_store_for_skipped =
        match crate::coordinator::adapters::SqliteJobsStore::for_repo_base(&store.base_dir) {
            Ok(js) => Some(js),
            Err(e) => {
                eprintln!("daft: failed to open coordinator store for '{hook_name}': {e}");
                None
            }
        };

    // Write sparse records for jobs that were filtered out before execution.
    // Each skipped job gets an `output.jsonl` containing the reason string
    // plus a `JobRow` in SQLite so the user can investigate via
    // `daft hooks jobs ls` / `daft hooks jobs logs <name>`. Runs BEFORE the
    // `specs.is_empty()` early return so fully-filtered hooks still
    // produce skipped-job records.
    for sj in &skipped_jobs {
        let meta = crate::coordinator::log_store::JobMeta::skipped(
            &sj.name,
            hook_name,
            &ctx.branch_name,
            "",
            sj.background,
            vec![],
        );
        let records = vec![crate::coordinator::log_record::LogRecord::stdout(
            0,
            sj.reason.as_str(),
        )];
        if let Err(e) = store.write_job_record_jsonl(&invocation_id, &meta, &records) {
            eprintln!(
                "daft: failed to write skipped job record for '{}': {e}",
                sj.name
            );
        }
        if let Some(ref js) = job_store_for_skipped {
            use crate::coordinator::ports::JobsStorePort;
            let row = crate::store::models::JobRow {
                repo_hash: repo_hash.clone(),
                invocation_id: invocation_id.clone(),
                name: meta.name.clone(),
                hook_type: meta.hook_type.clone(),
                worktree: meta.worktree.clone(),
                command: meta.command.clone(),
                working_dir: meta.working_dir.clone(),
                env: meta.env.clone(),
                started_at: meta.started_at,
                finished_at: meta.finished_at,
                status: meta.status.as_status_str().to_string(),
                exit_code: meta.exit_code,
                pid: meta.pid,
                pgid: None,
                background: meta.background,
                needs: meta.needs.clone(),
                tags: Vec::new(),
                retention_seconds: meta.retention_seconds,
                max_log_size_bytes: meta.max_log_size_bytes,
            };
            if let Err(e) = js.upsert_job(&row) {
                eprintln!(
                    "daft: failed to persist skipped job row for '{}': {e}",
                    sj.name
                );
            }
        }
    }

    // Capture the repo-level cleanup policy so cleanup doesn't need to
    // re-parse `daft.yml` later. Written once per hook fire after spec
    // building; most-recent write wins. Note: early returns above (no
    // jobs defined, all jobs filtered by tags or changed-attribute
    // tracking) skip this write, so the previous value remains in effect
    // until the next non-fully-filtered fire. Best-effort: a failed write
    // should not break the hook fire.
    let repo_policy = crate::coordinator::clean_policy::build_repo_policy(&specs);
    if let Some(ref js) = job_store_for_skipped {
        use crate::coordinator::ports::JobsStorePort;
        if let Err(e) = js.write_repo_policy(&repo_hash, &repo_policy) {
            eprintln!("daft: failed to write repo policy for '{hook_name}': {e}");
        }
    }

    if specs.is_empty() {
        // When `--skip-hooks` emptied the survivor set (e.g. `--skip-hooks
        // install` on a graph where everything reaches install), the jobs were
        // deliberately excluded with per-job reasons — render them here so the
        // skip is attributed, not silently swallowed by this early return. This
        // is the only render site reached on the empty path (the post-header
        // loop below never runs).
        if !requested_skips.is_empty() {
            let header_target = super::executor::header_target_for_ctx(ctx);
            presenter.on_phase_start(hook_name, header_target);
            let skip_names: Vec<String> =
                requested_skips.iter().map(|sj| sj.name.clone()).collect();
            presenter.on_jobs_planned(&skip_names);
            for sj in &requested_skips {
                presenter.on_job_skipped(
                    &sj.name,
                    &sj.reason,
                    std::time::Duration::ZERO,
                    false,
                    None,
                );
            }
            presenter.on_phase_complete(std::time::Duration::ZERO);
        }
        return Ok(HookResult::skipped("All jobs skipped"));
    }

    // Partition into foreground and background phases.
    // Background jobs that are transitively depended on by foreground jobs
    // are promoted to foreground to preserve DAG validity.
    let (fg_specs, bg_specs) = partition_foreground_background(&specs);

    // Warn about promoted jobs (background flag was true but they ended up
    // in the foreground partition because a foreground job depends on them).
    for spec in &fg_specs {
        if spec.background {
            presenter.on_message(&format!(
                "⚠ Job '{}' promoted to foreground (required by a foreground job)",
                spec.name,
            ));
        }
    }

    // Clear any active spinner — the presenter writes directly to stderr.
    output.finish_spinner();

    let fg_sink: std::sync::Arc<dyn crate::executor::log_sink::LogSink> =
        std::sync::Arc::new(crate::executor::log_sink::BufferingLogSink::new(
            std::sync::Arc::clone(&store),
            repo_hash.clone(),
            invocation_id.clone(),
            hook_name.to_string(),
            ctx.branch_name.clone(),
        ));

    // Use presenter for header and execution
    let header_target = super::executor::header_target_for_ctx(ctx);
    presenter.on_phase_start(hook_name, header_target);

    // Announce every row this phase renders (survivors + attributed skips)
    // so width-aligned renderers size their name column before the first
    // receipt persists — a wider name starting in a later `needs:` wave
    // cannot re-pad rows already in scrollback.
    let planned_names: Vec<String> = specs
        .iter()
        .map(|s| s.name.clone())
        .chain(requested_skips.iter().map(|sj| sj.name.clone()))
        .collect();
    presenter.on_jobs_planned(&planned_names);

    // Render `--skip-hooks` exclusions as attributed skip lines under the hook
    // header, before the surviving jobs run. (The empty-survivor case is
    // handled at the `specs.is_empty()` early return above; these two render
    // sites are mutually exclusive.)
    for sj in &requested_skips {
        presenter.on_job_skipped(&sj.name, &sj.reason, std::time::Duration::ZERO, false, None);
    }

    let hook_start = std::time::Instant::now();

    // Execute foreground jobs via the generic runner
    let fg_results =
        crate::executor::runner::run_jobs(&fg_specs, exec_mode, presenter, Some(&fg_sink))?;

    // If there are no background jobs, print summary and return.
    if bg_specs.is_empty() {
        presenter.on_phase_complete(hook_start.elapsed());
        return job_results_to_hook_result(&fg_results);
    }

    // Detect BG jobs whose ORIGINAL (pre-partition) `needs:` referenced a
    // foreground job that did not succeed. The partitioner already stripped
    // those names from `bg_specs[*].needs`, so we look at `specs` here to
    // recover the original cross-partition deps. These names are passed to
    // the coordinator as `prefailed_jobs` (or filtered out of the inline
    // run path) so a BG job depending on a failed FG is recorded as
    // `skipped` rather than executed.
    let failed_fg_names: HashSet<&str> = fg_results
        .iter()
        .filter(|r| !matches!(r.status, crate::executor::NodeStatus::Succeeded))
        .map(|r| r.name.as_str())
        .collect();
    let prefailed_bg: Vec<String> = if failed_fg_names.is_empty() {
        Vec::new()
    } else {
        specs
            .iter()
            .filter(|s| s.background)
            .filter(|s| s.needs.iter().any(|n| failed_fg_names.contains(n.as_str())))
            .map(|s| s.name.clone())
            .collect()
    };

    // If DAFT_NO_BACKGROUND_JOBS is set, run background jobs inline as foreground.
    if std::env::var("DAFT_NO_BACKGROUND_JOBS").is_ok() {
        let bg_results =
            run_bg_inline_with_prefailed(&bg_specs, &prefailed_bg, exec_mode, presenter, &fg_sink)?;
        presenter.on_phase_complete(hook_start.elapsed());
        let mut all_results = fg_results;
        all_results.extend(bg_results);
        return job_results_to_hook_result(&all_results);
    }

    // Register background jobs in the presenter (live progress + summary)
    // BEFORE on_phase_complete so they appear in both sections.
    for spec in &bg_specs {
        presenter.on_job_background(&spec.name, spec.description.as_deref());
    }

    presenter.on_phase_complete(hook_start.elapsed());

    // Dispatch background jobs to a forked coordinator process.
    #[cfg(unix)]
    {
        let mut coord_state =
            crate::coordinator::process::CoordinatorState::new(&repo_hash, &invocation_id)
                .with_metadata(&trigger_command, hook_name, &ctx.branch_name)
                .with_prefailed(prefailed_bg);
        for spec in bg_specs {
            coord_state.add_job(spec);
        }

        // Only count jobs that the coordinator will actually run.
        // Prefailed jobs are still added to the coordinator state so they
        // produce `skipped` rows in SQLite (visible via `daft hooks jobs`),
        // but counting them in the user-facing "N background job(s) running"
        // message would overstate runtime concurrency.
        let bg_count = coord_state
            .jobs
            .len()
            .saturating_sub(coord_state.prefailed_jobs.len());
        crate::coordinator::process::spawn_coordinator(coord_state, (*store).clone())?;

        if bg_count > 0 {
            presenter.on_message(&format!(
                "⟳ {} background job{} running — daft hooks jobs to manage",
                bg_count,
                if bg_count == 1 { "" } else { "s" },
            ));
        }
    }

    // On non-Unix platforms, background coordinator is not available.
    // Fall back to running background jobs inline.
    #[cfg(not(unix))]
    {
        let bg_results =
            run_bg_inline_with_prefailed(&bg_specs, &prefailed_bg, exec_mode, presenter, &fg_sink)?;
        let mut all_results = fg_results.clone();
        all_results.extend(bg_results);
        return job_results_to_hook_result(&all_results);
    }

    // Convert foreground results to HookResult (background jobs are now
    // running in the forked coordinator and do not affect the hook outcome).
    #[cfg(unix)]
    job_results_to_hook_result(&fg_results)
}

/// Run background jobs inline (no coordinator), synthesizing `Skipped`
/// results for any job in `prefailed_bg` *and* its transitive BG→BG
/// dependents. Used by the `DAFT_NO_BACKGROUND_JOBS` debug path and by
/// the non-Unix fallback, both of which would otherwise silently run BG
/// jobs whose FG dep failed (no coordinator to consult `prefailed_jobs`).
///
/// The skip semantic matches the coordinator: prefailed jobs and their
/// dependents surface as `NodeStatus::Skipped` in the returned
/// `JobResult`s, so the hook outcome classifier sees the same failure
/// shape regardless of execution path.
///
/// Without the transitive expansion below, a BG→BG dependent of a
/// prefailed job would stay in `to_run` with a `needs:` entry pointing
/// at a name that's been moved to `skipped_specs`. `DagGraph::new`
/// inside `run_jobs` would then reject the dangling reference as
/// `MissingDependency` — the exact failure mode the coordinator-side
/// cascade was written to avoid (see
/// `coordinator::process::run_all_with_cancel`).
fn run_bg_inline_with_prefailed(
    bg_specs: &[crate::executor::JobSpec],
    prefailed_bg: &[String],
    exec_mode: crate::executor::ExecutionMode,
    presenter: &Arc<dyn JobPresenter>,
    sink: &Arc<dyn crate::executor::log_sink::LogSink>,
) -> Result<Vec<crate::executor::JobResult>> {
    if prefailed_bg.is_empty() {
        return crate::executor::runner::run_jobs(bg_specs, exec_mode, presenter, Some(sink));
    }

    let skip_set = expand_prefailed_closure(bg_specs, prefailed_bg);
    let (skipped_specs, to_run): (Vec<_>, Vec<_>) = bg_specs
        .iter()
        .cloned()
        .partition(|s| skip_set.contains(s.name.as_str()));

    let mut results = crate::executor::runner::run_jobs(&to_run, exec_mode, presenter, Some(sink))?;
    for spec in skipped_specs {
        presenter.on_job_skipped(
            &spec.name,
            "foreground dependency failed",
            std::time::Duration::ZERO,
            false,
            None,
        );
        results.push(crate::executor::JobResult {
            name: spec.name,
            status: crate::executor::NodeStatus::Skipped,
            duration: std::time::Duration::ZERO,
            exit_code: None,
            stdout: String::new(),
            stderr: "foreground dependency failed".to_string(),
        });
    }
    Ok(results)
}

/// Compute the transitive closure of `seeds` over the BG→BG `needs:`
/// graph induced by `bg_specs`. Returns the set of BG job names that
/// must be skipped (the seeds plus everything that transitively
/// depends on a seed).
///
/// `bg_specs` is the slice produced by `partition_foreground_background`,
/// so each `needs:` entry already refers to a BG-only name — no
/// cross-partition references to filter out.
fn expand_prefailed_closure<'a>(
    bg_specs: &'a [crate::executor::JobSpec],
    seeds: &'a [String],
) -> HashSet<&'a str> {
    use std::collections::HashMap;

    // Build dependents adjacency on string slices borrowed from
    // `bg_specs` to keep the closure pass allocation-light.
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::with_capacity(bg_specs.len());
    for job in bg_specs {
        for dep in &job.needs {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(job.name.as_str());
        }
    }

    let mut skip: HashSet<&str> = HashSet::with_capacity(seeds.len());
    let mut stack: Vec<&str> = seeds.iter().map(String::as_str).collect();
    while let Some(name) = stack.pop() {
        if !skip.insert(name) {
            continue;
        }
        if let Some(downs) = dependents.get(name) {
            for d in downs {
                stack.push(d);
            }
        }
    }
    skip
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
        if !tracks.is_disjoint(changed)
            && let Some(ref name) = job.name
        {
            selected_names.insert(name.clone());
        }
    }

    // 2. Pull in needs dependencies (transitive)
    let mut made_progress = true;
    while made_progress {
        made_progress = false;
        for job in jobs {
            if let Some(ref name) = job.name
                && selected_names.contains(name)
                && let Some(ref needs) = job.needs
            {
                for dep in needs {
                    if selected_names.insert(dep.clone()) {
                        made_progress = true;
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
    use crate::executor::presenter::NullPresenter;
    use crate::hooks::HookType;
    use crate::hooks::yaml_config::RunCommand;
    use crate::output::TestOutput;
    use std::collections::HashMap;
    use tempfile::TempDir;

    /// Build a `HookContext` whose `git_dir` is a real temp directory and
    /// whose `state_dir` is the same temp directory.
    ///
    /// `execute_yaml_hook_with_rc` writes a `daft-id` file into `ctx.git_dir`
    /// to compute the repo hash; pre-fix (#448) it called `compute_repo_id()`
    /// (cwd-based) and silently picked up whatever git repo the test runner
    /// was sitting in. The fix moved that to `ctx.git_dir`, so tests need a
    /// real directory there.
    ///
    /// Setting `state_dir` to the same tempdir routes the LogStore writes for
    /// the resulting invocation/job records into the tempdir as well — pre
    /// `#478` they fell through to `daft_state_dir()` and accumulated as
    /// orphan UUID directories under the user's `~/.local/state/daft/jobs/`.
    ///
    /// Tests that need the context together with the keep-alive guard call
    /// `make_ctx_with_dir`. Tests that only need the context (and don't mind
    /// leaking the dir for the test process duration) call `make_ctx`.
    fn make_ctx_with_dir() -> (HookContext, TempDir) {
        let dir = TempDir::new().unwrap();
        let git_dir = dir.path().to_path_buf();
        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            &git_dir,
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        )
        .with_state_dir(dir.path());
        (ctx, dir)
    }

    fn make_ctx() -> HookContext {
        // Leak the tempdir into the test process so callers that don't need
        // a guard can still use it. The OS reaps the dir at process exit.
        let (ctx, dir) = make_ctx_with_dir();
        std::mem::forget(dir);
        ctx
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

    /// Regression for #478: `execute_yaml_hook_with_rc` must honor
    /// `ctx.state_dir` and route LogStore writes into the supplied base, not
    /// fall through to `daft_state_dir()` and leak orphan UUID directories
    /// into the user's real `~/.local/state/daft/jobs/`.
    #[test]
    fn test_execute_yaml_hook_honors_state_dir_override() {
        let hook_def = HookDef {
            jobs: Some(vec![JobDef {
                name: Some("noop".to_string()),
                run: Some(RunCommand::Simple("true".to_string())),
                ..Default::default()
            }]),
            ..Default::default()
        };
        let (ctx, dir) = make_ctx_with_dir();
        let mut output = TestOutput::default();

        execute_yaml_hook(
            "test-hook",
            &hook_def,
            &ctx,
            &mut output,
            ".daft",
            Path::new("/tmp"),
            &HookOutputConfig::default(),
        )
        .unwrap();

        // The executor stamps a `daft-id` UUID into `ctx.git_dir` and then
        // writes invocation/job records under `<state_dir>/jobs/<uuid>/`. Read
        // the UUID it picked, then verify both halves of the contract: writes
        // landed under our tempdir, and nothing leaked into the real state
        // dir. The latter is the actual regression: pre-fix every test run
        // created a fresh orphan there.
        let repo_hash = crate::core::repo_identity::compute_repo_id_from_common_dir(dir.path())
            .expect("daft-id should have been written into ctx.git_dir");
        assert!(
            dir.path().join("jobs").join(&repo_hash).exists(),
            "LogStore writes should have landed under the tempdir state base"
        );
        let leaked = crate::daft_state_dir()
            .expect("daft_state_dir resolves")
            .join("jobs")
            .join(&repo_hash);
        assert!(
            !leaked.exists(),
            "must not leak a UUID dir into the real state dir: {}",
            leaked.display()
        );
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

    /// Regression for #556: when a foreground job fails, BG jobs that
    /// declared `needs:` on it must surface as `skipped` rather than
    /// running. This exercises the `DAFT_NO_BACKGROUND_JOBS` inline path
    /// (the coordinator path requires spawning a real child process; the
    /// equivalent unit coverage for that path is in
    /// `coordinator::process::tests::prefailed_*`).
    ///
    /// Detection uses filesystem markers rather than captured output:
    /// `TestOutput` does not see the job's shell stdout (the executor
    /// routes it through the log sink), so each BG command `touch`es a
    /// per-test marker file. If the cascade fires, the marker must not
    /// exist after the hook returns.
    ///
    /// `#[serial_test::serial(daft_no_background_jobs)]` is used because
    /// the test mutates the process-global `DAFT_NO_BACKGROUND_JOBS` env
    /// var. The same serial key is shared with
    /// `test_bg_needs_chain_cascade_skips_transitive_in_inline_path` so
    /// they never race on the env.
    #[test]
    #[serial_test::serial(daft_no_background_jobs)]
    fn test_bg_needs_failed_fg_skipped_in_inline_path() {
        let marker_dir = TempDir::new().unwrap();
        let marker = marker_dir.path().join("bg-ran.marker");
        // The path lives inside a fresh tempdir, so no special characters
        // need quoting in the shell command.
        let bg_cmd = format!("touch {}", marker.display());

        let _guard = NoBackgroundJobsEnv::set();

        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("fg-fail".to_string()),
                    run: Some(RunCommand::Simple("false".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("bg-dependent".to_string()),
                    run: Some(RunCommand::Simple(bg_cmd)),
                    background: Some(true),
                    needs: Some(vec!["fg-fail".to_string()]),
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

        assert!(!result.success, "hook should fail because fg-fail failed");
        assert!(
            !marker.exists(),
            "bg-dependent should have been skipped, but its `touch` ran (marker at {} exists)",
            marker.display()
        );
    }

    /// Regression for the review finding on #558: in the inline path, a
    /// BG job whose `needs:` chains through another BG job that's
    /// already prefailed must also be skipped. Pre-fix the chain was
    /// `fg-fail → bg-dep → bg-transitive`; the partitioner stripped
    /// `needs:[fg-fail]` from `bg-dep`, the prefailed list contained
    /// only `bg-dep`, the inline path moved `bg-dep` to skipped without
    /// expanding the closure, and `run_jobs(&[bg-transitive])` then
    /// rejected the dangling `needs: [bg-dep]` with `MissingDependency`.
    #[test]
    #[serial_test::serial(daft_no_background_jobs)]
    fn test_bg_needs_chain_cascade_skips_transitive_in_inline_path() {
        let marker_dir = TempDir::new().unwrap();
        let dep_marker = marker_dir.path().join("bg-dep-ran.marker");
        let trans_marker = marker_dir.path().join("bg-transitive-ran.marker");

        let _guard = NoBackgroundJobsEnv::set();

        let hook_def = HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("fg-fail".to_string()),
                    run: Some(RunCommand::Simple("false".to_string())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("bg-dep".to_string()),
                    run: Some(RunCommand::Simple(format!(
                        "touch {}",
                        dep_marker.display()
                    ))),
                    background: Some(true),
                    needs: Some(vec!["fg-fail".to_string()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("bg-transitive".to_string()),
                    run: Some(RunCommand::Simple(format!(
                        "touch {}",
                        trans_marker.display()
                    ))),
                    background: Some(true),
                    needs: Some(vec!["bg-dep".to_string()]),
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
        .expect(
            "hook should return Ok with a failure result rather than Err — pre-fix this \
             surfaced as a DagGraph::MissingDependency Err",
        );

        assert!(!result.success);
        assert!(
            !dep_marker.exists(),
            "bg-dep must be skipped (its FG dep failed); marker exists at {}",
            dep_marker.display()
        );
        assert!(
            !trans_marker.exists(),
            "bg-transitive must be skipped via the BG→BG cascade; marker exists at {}",
            trans_marker.display()
        );
    }

    /// RAII guard for `DAFT_NO_BACKGROUND_JOBS`. Sets the var on
    /// construction and removes it on drop so tests that need the inline
    /// path don't leak the var into other tests if they panic mid-body.
    /// Paired with `#[serial_test::serial(daft_no_background_jobs)]` on
    /// the call sites for cross-test isolation.
    struct NoBackgroundJobsEnv;

    impl NoBackgroundJobsEnv {
        fn set() -> Self {
            // SAFETY: edition-2024 marks env mutators as unsafe because
            // they race with `getenv` in other threads. The `serial`
            // attribute on the call sites guarantees only this test is
            // running, so the only reader is this test's own
            // `execute_yaml_hook` call below.
            unsafe { std::env::set_var("DAFT_NO_BACKGROUND_JOBS", "1") };
            Self
        }
    }

    impl Drop for NoBackgroundJobsEnv {
        fn drop(&mut self) {
            // SAFETY: see `NoBackgroundJobsEnv::set`.
            unsafe { std::env::remove_var("DAFT_NO_BACKGROUND_JOBS") };
        }
    }

    // ── --skip-hooks engine surfacing ───────────────────────────────────

    /// Presenter that records the events the executor emits, so tests can
    /// assert that skipped jobs surface live (with the right reason) rather
    /// than being silently dropped. `NullPresenter` discards everything and
    /// can't make these assertions.
    #[derive(Default)]
    struct RecordingPresenter {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingPresenter {
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
        fn push(&self, e: String) {
            self.events.lock().unwrap().push(e);
        }
    }

    impl crate::executor::presenter::JobPresenter for RecordingPresenter {
        fn on_phase_start(&self, phase: &str, _target: Option<&str>) {
            self.push(format!("phase_start:{phase}"));
        }
        fn on_job_start(&self, name: &str, _d: Option<&str>, _c: Option<&str>) {
            self.push(format!("start:{name}"));
        }
        fn on_job_output(&self, _name: &str, _line: &str) {}
        fn on_job_success(&self, name: &str, _dur: std::time::Duration) {
            self.push(format!("success:{name}"));
        }
        fn on_job_failure(&self, name: &str, _dur: std::time::Duration) {
            self.push(format!("failure:{name}"));
        }
        fn on_job_skipped(
            &self,
            name: &str,
            reason: &str,
            _dur: std::time::Duration,
            _show_duration: bool,
            _command_preview: Option<&str>,
        ) {
            self.push(format!("skipped:{name}:{reason}"));
        }
        fn on_job_cancelled(&self, name: &str, _dur: std::time::Duration) {
            self.push(format!("cancelled:{name}"));
        }
        fn on_job_background(&self, name: &str, _d: Option<&str>) {
            self.push(format!("background:{name}"));
        }
        fn on_message(&self, _msg: &str) {}
        fn on_phase_complete(&self, _dur: std::time::Duration) {
            self.push("phase_complete".to_string());
        }
        fn take_results(&self) -> Vec<crate::executor::JobResult> {
            Vec::new()
        }
    }

    /// The issue's worked-example hook: install / build(heavy) / test / lint.
    /// Surviving jobs run `true` so the fire is hermetic.
    fn skip_example_hook() -> HookDef {
        HookDef {
            jobs: Some(vec![
                JobDef {
                    name: Some("install".into()),
                    run: Some(RunCommand::Simple("true".into())),
                    ..Default::default()
                },
                JobDef {
                    name: Some("build".into()),
                    run: Some(RunCommand::Simple("true".into())),
                    needs: Some(vec!["install".into()]),
                    tags: Some(vec!["heavy".into()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("test".into()),
                    run: Some(RunCommand::Simple("true".into())),
                    needs: Some(vec!["build".into()]),
                    ..Default::default()
                },
                JobDef {
                    name: Some("lint".into()),
                    run: Some(RunCommand::Simple("true".into())),
                    needs: Some(vec!["install".into()]),
                    ..Default::default()
                },
            ]),
            // Sequential keeps the recorded event order deterministic.
            parallel: Some(false),
            ..Default::default()
        }
    }

    #[test]
    fn skip_all_short_circuits() {
        let hook_def = skip_example_hook();
        let (ctx, _dir) = make_ctx_with_dir();
        let mut output = TestOutput::default();
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = NullPresenter::arc();
        let filter = JobFilter {
            skip: crate::hooks::job_adapter::parse_skip_selectors(&["all".to_string()]),
            ..Default::default()
        };
        let cfg = HookExecutionContext {
            source_dir: ".daft",
            working_dir: _dir.path(),
            rc: None,
            filter: &filter,
            presenter: &presenter,
            repo_log: None,
        };
        let result =
            execute_yaml_hook_with_rc("post-create", &hook_def, &ctx, &mut output, &cfg).unwrap();
        assert!(result.skipped);
        assert_eq!(
            result.skip_reason.as_deref(),
            Some("all hooks skipped by request")
        );
    }

    #[test]
    fn skip_name_cascade_empties_and_renders_each_reason() {
        let hook_def = skip_example_hook();
        let (ctx, dir) = make_ctx_with_dir();
        let mut output = TestOutput::default();
        let recorder = Arc::new(RecordingPresenter::default());
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = recorder.clone();
        let filter = JobFilter {
            skip: crate::hooks::job_adapter::parse_skip_selectors(&["install".to_string()]),
            ..Default::default()
        };
        let cfg = HookExecutionContext {
            source_dir: ".daft",
            working_dir: dir.path(),
            rc: None,
            filter: &filter,
            presenter: &presenter,
            repo_log: None,
        };
        let result =
            execute_yaml_hook_with_rc("post-create", &hook_def, &ctx, &mut output, &cfg).unwrap();

        // Every job reaches install ⇒ all excluded ⇒ hook is skipped overall.
        assert!(result.skipped);
        let events = recorder.events();
        // Each excluded job rendered with its attributed reason.
        assert!(events.contains(&"skipped:install:requested (--skip-hooks)".to_string()));
        assert!(events.contains(&"skipped:build:depends on install (skipped)".to_string()));
        assert!(events.contains(&"skipped:test:depends on build (skipped)".to_string()));
        assert!(events.contains(&"skipped:lint:depends on install (skipped)".to_string()));
        // No job actually executed.
        assert!(!events.iter().any(|e| e.starts_with("start:")));
    }

    #[test]
    fn skip_tag_keeps_survivors_and_renders_skips() {
        let hook_def = skip_example_hook();
        let (ctx, dir) = make_ctx_with_dir();
        let mut output = TestOutput::default();
        let recorder = Arc::new(RecordingPresenter::default());
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = recorder.clone();
        let filter = JobFilter {
            skip: crate::hooks::job_adapter::parse_skip_selectors(&["tag:heavy".to_string()]),
            ..Default::default()
        };
        let cfg = HookExecutionContext {
            source_dir: ".daft",
            working_dir: dir.path(),
            rc: None,
            filter: &filter,
            presenter: &presenter,
            repo_log: None,
        };
        let result =
            execute_yaml_hook_with_rc("post-create", &hook_def, &ctx, &mut output, &cfg).unwrap();
        assert!(!result.skipped, "install+lint still run");
        let events = recorder.events();
        // build + test excluded with reasons.
        assert!(events.contains(&"skipped:build:requested (--skip-hooks)".to_string()));
        assert!(events.contains(&"skipped:test:depends on build (skipped)".to_string()));
        // install + lint executed.
        assert!(events.contains(&"start:install".to_string()));
        assert!(events.contains(&"start:lint".to_string()));
        assert!(
            !events
                .iter()
                .any(|e| e == "start:build" || e == "start:test")
        );
    }

    #[test]
    fn skip_unmatched_selector_warns_not_errors() {
        let hook_def = skip_example_hook();
        let (ctx, dir) = make_ctx_with_dir();
        let mut output = TestOutput::default();
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = NullPresenter::arc();
        let filter = JobFilter {
            skip: crate::hooks::job_adapter::parse_skip_selectors(&["ghost".to_string()]),
            ..Default::default()
        };
        let cfg = HookExecutionContext {
            source_dir: ".daft",
            working_dir: dir.path(),
            rc: None,
            filter: &filter,
            presenter: &presenter,
            repo_log: None,
        };
        // Must NOT error (contrast with the include path's bail!).
        let result =
            execute_yaml_hook_with_rc("post-create", &hook_def, &ctx, &mut output, &cfg).unwrap();
        assert!(!result.skipped, "no real exclusion ⇒ all jobs run");
        assert!(
            output.warnings().iter().any(|w| w.contains("ghost")),
            "unmatched selector should warn, got: {:?}",
            output.warnings()
        );
    }

    #[test]
    fn skip_hook_type_renders_every_job_as_skipped() {
        let hook_def = skip_example_hook();
        let (ctx, dir) = make_ctx_with_dir();
        let mut output = TestOutput::default();
        let recorder = Arc::new(RecordingPresenter::default());
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = recorder.clone();
        let filter = JobFilter {
            skip: crate::hooks::job_adapter::parse_skip_selectors(&[
                "worktree-post-create".to_string()
            ]),
            ..Default::default()
        };
        let cfg = HookExecutionContext {
            source_dir: ".daft",
            working_dir: dir.path(),
            rc: None,
            filter: &filter,
            presenter: &presenter,
            repo_log: None,
        };
        // hook_name == the selected hook type ⇒ the whole hook is skipped, but
        // it is NOT a silent drop: every job renders as skipped with the same
        // `requested` reason as a direct name skip (the user's mental model is
        // "as if I marked each job"), and no job actually runs.
        let result =
            execute_yaml_hook_with_rc("worktree-post-create", &hook_def, &ctx, &mut output, &cfg)
                .unwrap();
        assert!(result.skipped);
        let events = recorder.events();
        for job in ["install", "build", "test", "lint"] {
            assert!(
                events.contains(&format!("skipped:{job}:requested (--skip-hooks)")),
                "expected {job} rendered as skipped, got: {events:?}"
            );
        }
        assert!(
            !events.iter().any(|e| e.starts_with("start:")),
            "no job should execute on a whole-hook skip"
        );
        assert!(
            output.warnings().is_empty(),
            "a matched hook-type selector must not warn, got: {:?}",
            output.warnings()
        );
    }

    #[test]
    fn skip_hook_type_naming_a_different_fire_runs_without_warning() {
        // `--skip-hooks worktree-post-create` while the worktree-pre-create
        // fire runs: the selector names a *valid* hook, just not this one, so
        // every job runs and NO unmatched warning is emitted (the cross-fire
        // case the per-fire warning must not flag).
        let hook_def = skip_example_hook();
        let (ctx, dir) = make_ctx_with_dir();
        let mut output = TestOutput::default();
        let recorder = Arc::new(RecordingPresenter::default());
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = recorder.clone();
        let filter = JobFilter {
            skip: crate::hooks::job_adapter::parse_skip_selectors(&[
                "worktree-post-create".to_string()
            ]),
            ..Default::default()
        };
        let cfg = HookExecutionContext {
            source_dir: ".daft",
            working_dir: dir.path(),
            rc: None,
            filter: &filter,
            presenter: &presenter,
            repo_log: None,
        };
        let result =
            execute_yaml_hook_with_rc("worktree-pre-create", &hook_def, &ctx, &mut output, &cfg)
                .unwrap();
        assert!(
            !result.skipped,
            "a non-matching hook-type selector is a no-op"
        );
        assert!(recorder.events().iter().any(|e| e.starts_with("start:")));
        assert!(
            output.warnings().is_empty(),
            "a hook-type selector for a different fire must not warn, got: {:?}",
            output.warnings()
        );
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
