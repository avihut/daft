//! `daft hooks jobs` — manage background hook jobs.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::Path;

use crate::coordinator::client::CoordinatorClient;
use crate::coordinator::log_store::{InvocationMeta, JobStatus, LogStore};
use crate::output::format::shorthand_from_seconds;
use crate::output::Output;
use crate::styles::{blue, bold, dim, dim_underline, green, red, yellow};
use tabled::{
    builder::Builder,
    settings::{object::Columns, Padding, Style},
};

/// Format a duration in seconds as `M:SS` (e.g., `0:06`, `1:32`, `12:05`).
/// For durations >= 1 hour, uses `H:MM:SS`.
fn format_duration(secs: i64) -> String {
    let secs = secs.max(0);
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h}:{m:02}:{s:02}")
    } else {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}:{s:02}")
    }
}

#[derive(serde::Serialize)]
struct JsonOutput {
    worktrees: Vec<JsonWorktree>,
}

#[derive(serde::Serialize)]
struct JsonWorktree {
    name: String,
    invocations: Vec<JsonInvocation>,
}

#[derive(serde::Serialize)]
struct JsonInvocation {
    id: String,
    short_id: String,
    trigger_command: String,
    hook_type: String,
    created_at: String,
    jobs: Vec<JsonJob>,
}

#[derive(serde::Serialize)]
struct JsonJob {
    name: String,
    background: bool,
    status: String,
    exit_code: Option<i32>,
    started_at: String,
    finished_at: Option<String>,
    duration_secs: i64,
    command: String,
}

#[derive(Parser, Debug)]
#[command(about = "Manage background hook jobs")]
pub struct JobsArgs {
    #[command(subcommand)]
    command: Option<JobsCommand>,

    /// Show jobs across all worktrees.
    #[arg(long, conflicts_with = "worktree")]
    all: bool,

    /// Output in JSON format.
    #[arg(long)]
    json: bool,

    /// Filter to a specific worktree (can be deleted).
    #[arg(long, conflicts_with = "all")]
    worktree: Option<String>,

    /// Filter to invocations containing jobs with this status.
    #[arg(long)]
    status: Option<String>,

    /// Filter to invocations of this hook type.
    #[arg(long = "hook")]
    hook_filter: Option<String>,
}

#[derive(Subcommand, Debug)]
enum JobsCommand {
    /// View output log for a background job.
    Logs {
        /// Job address: name, inv:name, worktree:name, or worktree:inv:name.
        job: String,
        /// Invocation ID prefix (overrides inline prefix).
        #[arg(long)]
        inv: Option<String>,
    },
    /// Cancel a running background job.
    Cancel {
        /// Job address (omit for --all).
        job: Option<String>,
        /// Cancel all running jobs.
        #[arg(long)]
        all: bool,
        /// Invocation ID prefix.
        #[arg(long)]
        inv: Option<String>,
    },
    /// Re-run failed jobs from an invocation.
    Retry {
        /// Target: hook name, invocation prefix, or job name.
        /// Empty = retry all failed from most recent invocation.
        target: Option<String>,
        /// Force interpretation as a hook name.
        #[arg(long, conflicts_with_all = ["inv_flag", "job_flag"])]
        hook: Option<String>,
        /// Force interpretation as an invocation prefix.
        #[arg(long = "inv", conflicts_with_all = ["hook", "job_flag"])]
        inv_flag: Option<String>,
        /// Force interpretation as a job name.
        #[arg(long = "job", conflicts_with_all = ["hook", "inv_flag"])]
        job_flag: Option<String>,
        /// Retry jobs from a specific worktree (can be deleted).
        #[arg(long)]
        worktree: Option<String>,
        /// Override working directory for all retried jobs.
        #[arg(long)]
        cwd: Option<String>,
    },
    /// Remove logs older than the retention period.
    Clean,
}

/// Parsed composite job address: `[worktree:][invocation:]job_name`.
#[derive(Debug, Clone)]
pub struct JobAddress {
    pub worktree: Option<String>,
    pub invocation_prefix: Option<String>,
    pub job_name: String,
}

impl JobAddress {
    pub fn parse(input: &str) -> Self {
        // rsplitn splits from the right, so worktree (which may contain /)
        // stays intact as a single piece.
        let parts: Vec<&str> = input.rsplitn(3, ':').collect();
        match parts.len() {
            1 => Self {
                worktree: None,
                invocation_prefix: None,
                job_name: parts[0].to_string(),
            },
            2 => {
                let left = parts[1];
                if left.contains('/') {
                    Self {
                        worktree: Some(left.to_string()),
                        invocation_prefix: None,
                        job_name: parts[0].to_string(),
                    }
                } else {
                    Self {
                        worktree: None,
                        invocation_prefix: Some(left.to_string()),
                        job_name: parts[0].to_string(),
                    }
                }
            }
            3 => Self {
                worktree: Some(parts[2].to_string()),
                invocation_prefix: Some(parts[1].to_string()),
                job_name: parts[0].to_string(),
            },
            _ => unreachable!(),
        }
    }

    pub fn with_inv_override(mut self, inv: Option<&str>) -> Self {
        if let Some(prefix) = inv {
            self.invocation_prefix = Some(prefix.to_string());
        }
        self
    }
}

#[derive(Debug)]
pub struct ResolvedAddress {
    pub invocation_id: String,
    pub job_name: String,
    pub job_dir: std::path::PathBuf,
}

fn resolve_job_address(
    addr: &JobAddress,
    store: &LogStore,
    current_worktree: &str,
) -> Result<ResolvedAddress> {
    let worktree = addr.worktree.as_deref().unwrap_or(current_worktree);
    let invocations = store.list_invocations_for_worktree(worktree)?;

    if invocations.is_empty() {
        anyhow::bail!("No invocations found for worktree '{worktree}'.");
    }

    match &addr.invocation_prefix {
        Some(prefix) => {
            let matches: Vec<_> = invocations
                .iter()
                .filter(|inv| inv.invocation_id.starts_with(prefix.as_str()))
                .collect();

            match matches.len() {
                0 => anyhow::bail!(
                    "No invocation matching prefix '{prefix}' in worktree '{worktree}'."
                ),
                1 => {
                    let inv = matches[0];
                    let job_dir = store.base_dir.join(&inv.invocation_id).join(&addr.job_name);
                    if !job_dir.exists() {
                        let available = list_job_names_in_invocation(store, &inv.invocation_id)?;
                        anyhow::bail!(
                            "No job named '{}' found in invocation '{}'.\nAvailable jobs: {}",
                            addr.job_name,
                            &inv.invocation_id[..4.min(inv.invocation_id.len())],
                            available.join(", ")
                        );
                    }
                    Ok(ResolvedAddress {
                        invocation_id: inv.invocation_id.clone(),
                        job_name: addr.job_name.clone(),
                        job_dir,
                    })
                }
                _ => {
                    let now = chrono::Utc::now();
                    let lines: Vec<String> = matches
                        .iter()
                        .map(|inv| {
                            let ago = shorthand_from_seconds(
                                now.signed_duration_since(inv.created_at).num_seconds(),
                            );
                            format!(
                                "  {}  {} -- {} ago",
                                &inv.invocation_id[..4.min(inv.invocation_id.len())],
                                inv.trigger_command,
                                ago
                            )
                        })
                        .collect();
                    anyhow::bail!(
                        "Ambiguous invocation ID '{}' -- matches:\n{}\nUse more characters to disambiguate.",
                        prefix,
                        lines.join("\n")
                    );
                }
            }
        }
        None => {
            // No invocation prefix: find most recent invocation containing the job.
            for inv in invocations.iter().rev() {
                let job_dir = store.base_dir.join(&inv.invocation_id).join(&addr.job_name);
                if job_dir.exists() {
                    return Ok(ResolvedAddress {
                        invocation_id: inv.invocation_id.clone(),
                        job_name: addr.job_name.clone(),
                        job_dir,
                    });
                }
            }
            let all_job_names = collect_all_job_names(store, &invocations)?;
            anyhow::bail!(
                "No job named '{}' found in worktree '{}'.\nAvailable jobs: {}",
                addr.job_name,
                worktree,
                all_job_names.join(", ")
            );
        }
    }
}

fn list_job_names_in_invocation(store: &LogStore, invocation_id: &str) -> Result<Vec<String>> {
    let dirs = store.list_jobs_in_invocation(invocation_id)?;
    Ok(dirs
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect())
}

fn collect_all_job_names(
    store: &LogStore,
    invocations: &[crate::coordinator::log_store::InvocationMeta],
) -> Result<Vec<String>> {
    let mut names = std::collections::BTreeSet::new();
    for inv in invocations {
        for dir in store.list_jobs_in_invocation(&inv.invocation_id)? {
            if let Some(n) = dir.file_name() {
                names.insert(n.to_string_lossy().to_string());
            }
        }
    }
    Ok(names.into_iter().collect())
}

const KNOWN_HOOK_TYPES: &[&str] = &[
    "post-clone",
    "worktree-pre-create",
    "worktree-post-create",
    "worktree-pre-remove",
    "worktree-post-remove",
];

#[derive(Debug, PartialEq)]
enum RetryTarget {
    LatestInvocation,
    HookType(String),
    InvocationPrefix(String),
    JobName(String),
}

#[derive(Debug, Default)]
struct RetryFlags {
    hook: Option<String>,
    inv: Option<String>,
    job: Option<String>,
}

fn retry_target_from_arg(arg: Option<&str>, flags: &RetryFlags) -> RetryTarget {
    if let Some(ref h) = flags.hook {
        return RetryTarget::HookType(h.clone());
    }
    if let Some(ref i) = flags.inv {
        return RetryTarget::InvocationPrefix(i.clone());
    }
    if let Some(ref j) = flags.job {
        return RetryTarget::JobName(j.clone());
    }

    match arg {
        None => RetryTarget::LatestInvocation,
        Some(a) => {
            if KNOWN_HOOK_TYPES.contains(&a) {
                RetryTarget::HookType(a.to_string())
            } else if a.len() >= 2 && a.len() <= 8 && a.chars().all(|c| c.is_ascii_hexdigit()) {
                RetryTarget::InvocationPrefix(a.to_string())
            } else {
                RetryTarget::JobName(a.to_string())
            }
        }
    }
}

fn build_retry_set(
    metas: &[crate::coordinator::log_store::JobMeta],
) -> (Vec<crate::executor::JobSpec>, Vec<String>) {
    let retry_names: std::collections::HashSet<String> = metas
        .iter()
        .filter(|m| matches!(m.status, JobStatus::Failed | JobStatus::Cancelled))
        .map(|m| m.name.clone())
        .collect();

    let specs: Vec<crate::executor::JobSpec> = metas
        .iter()
        .filter(|m| retry_names.contains(&m.name))
        .map(|m| {
            let needs: Vec<String> = m
                .needs
                .iter()
                .filter(|n| retry_names.contains(n.as_str()))
                .cloned()
                .collect();
            crate::executor::JobSpec {
                name: m.name.clone(),
                command: m.command.clone(),
                working_dir: std::path::PathBuf::from(&m.working_dir),
                env: m.env.clone(),
                background: m.background,
                needs,
                ..Default::default()
            }
        })
        .collect();

    let names: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
    (specs, names)
}

pub fn run(args: JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    match args.command {
        None => list_jobs(&args, path, output),
        Some(JobsCommand::Logs { ref job, ref inv }) => {
            show_logs(job, inv.as_deref(), &args, path, output)
        }
        Some(JobsCommand::Cancel {
            ref job,
            all,
            ref inv,
        }) => {
            if all || job.is_none() {
                cancel_all(path, output)
            } else {
                cancel_job(job.as_ref().unwrap(), inv.as_deref(), path, output)
            }
        }
        Some(JobsCommand::Retry {
            ref target,
            ref hook,
            ref inv_flag,
            ref job_flag,
            ref worktree,
            ref cwd,
        }) => retry_command(
            target.as_deref(),
            hook,
            inv_flag,
            job_flag,
            worktree.as_deref(),
            cwd.as_deref(),
            path,
            output,
        ),
        Some(JobsCommand::Clean) => clean_logs(&args, path, output),
    }
}

/// List all repo hashes that have job directories under the state dir.
fn list_all_repo_hashes() -> Result<Vec<String>> {
    let jobs_dir = crate::daft_state_dir()?.join("jobs");
    if !jobs_dir.exists() {
        return Ok(vec![]);
    }
    let mut hashes = Vec::new();
    for entry in std::fs::read_dir(&jobs_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                if uuid::Uuid::parse_str(name).is_ok() {
                    hashes.push(name.to_string());
                }
            }
        }
    }
    Ok(hashes)
}

fn is_coordinator_running(repo_hash: &str) -> bool {
    crate::coordinator::coordinator_socket_path(repo_hash)
        .map(|p| p.exists())
        .unwrap_or(false)
}

fn format_status_inline(status: &JobStatus, coordinator_alive: bool) -> String {
    match status {
        JobStatus::Completed => green("\u{2713} completed"),
        JobStatus::Failed => red("\u{2717} failed"),
        JobStatus::Running => {
            if coordinator_alive {
                yellow("\u{27f3} running")
            } else {
                yellow("\u{27f3} running (stale)")
            }
        }
        JobStatus::Cancelled => dim("\u{2014} cancelled"),
        JobStatus::Skipped => dim("\u{2014} skipped"),
    }
}

fn print_json_output(
    invocations: &[InvocationMeta],
    store: &LogStore,
    coordinator_alive: bool,
    output: &mut dyn Output,
) -> Result<()> {
    let now = chrono::Utc::now();

    // Group invocations by worktree (BTreeMap for stable ordering).
    let mut groups: std::collections::BTreeMap<String, Vec<&InvocationMeta>> =
        std::collections::BTreeMap::new();
    for inv in invocations {
        groups.entry(inv.worktree.clone()).or_default().push(inv);
    }

    let mut json_worktrees: Vec<JsonWorktree> = Vec::new();

    for (worktree, inv_list) in &groups {
        let mut json_invocations: Vec<JsonInvocation> = Vec::new();

        for inv in inv_list {
            let short_id = inv.invocation_id[..4.min(inv.invocation_id.len())].to_string();

            let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
            let mut json_jobs: Vec<JsonJob> = Vec::new();

            for dir in &job_dirs {
                if let Ok(meta) = store.read_meta(dir) {
                    let status_str = match &meta.status {
                        JobStatus::Running if !coordinator_alive => "running (stale)".to_string(),
                        JobStatus::Running => "running".to_string(),
                        JobStatus::Completed => "completed".to_string(),
                        JobStatus::Failed => "failed".to_string(),
                        JobStatus::Cancelled => "cancelled".to_string(),
                        JobStatus::Skipped => "skipped".to_string(),
                    };

                    let duration_secs = match (&meta.status, meta.finished_at) {
                        (_, Some(finished)) => finished
                            .signed_duration_since(meta.started_at)
                            .num_seconds(),
                        (JobStatus::Running, None) => {
                            now.signed_duration_since(meta.started_at).num_seconds()
                        }
                        _ => 0,
                    };

                    json_jobs.push(JsonJob {
                        name: meta.name.clone(),
                        background: meta.background,
                        status: status_str,
                        exit_code: meta.exit_code,
                        started_at: meta.started_at.to_rfc3339(),
                        finished_at: meta.finished_at.map(|t| t.to_rfc3339()),
                        duration_secs,
                        command: meta.command.clone(),
                    });
                }
            }

            json_invocations.push(JsonInvocation {
                id: inv.invocation_id.clone(),
                short_id,
                trigger_command: inv.trigger_command.clone(),
                hook_type: inv.hook_type.clone(),
                created_at: inv.created_at.to_rfc3339(),
                jobs: json_jobs,
            });
        }

        json_worktrees.push(JsonWorktree {
            name: worktree.clone(),
            invocations: json_invocations,
        });
    }

    let json_output = JsonOutput {
        worktrees: json_worktrees,
    };

    let serialized =
        serde_json::to_string_pretty(&json_output).context("Failed to serialize jobs to JSON")?;
    output.info(&serialized);
    Ok(())
}

/// Default subcommand: list jobs grouped by worktree and invocation.
fn list_jobs(args: &JobsArgs, _path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let coordinator_alive = is_coordinator_running(&repo_hash);

    let store = LogStore::for_repo(&repo_hash)?;
    let invocations = if args.all {
        store.list_invocations()?
    } else if let Some(ref wt) = args.worktree {
        store.list_invocations_for_worktree(wt)?
    } else {
        store.list_invocations_for_worktree(&current_worktree)?
    };

    // Apply --hook filter.
    let invocations: Vec<_> = if let Some(ref hook) = args.hook_filter {
        invocations
            .into_iter()
            .filter(|inv| inv.hook_type == *hook)
            .collect()
    } else {
        invocations
    };

    // Apply --status filter (invocation-level: keep if any job matches).
    let invocations: Vec<_> = if let Some(ref status_str) = args.status {
        let target_status = match status_str.as_str() {
            "failed" => JobStatus::Failed,
            "completed" => JobStatus::Completed,
            "running" => JobStatus::Running,
            "cancelled" => JobStatus::Cancelled,
            "skipped" => JobStatus::Skipped,
            other => anyhow::bail!(
                "Unknown status '{}'. Valid values: failed, completed, running, cancelled, skipped.",
                other
            ),
        };
        invocations
            .into_iter()
            .filter(|inv| {
                store
                    .list_jobs_in_invocation(&inv.invocation_id)
                    .unwrap_or_default()
                    .iter()
                    .any(|dir| {
                        store
                            .read_meta(dir)
                            .map(|m| m.status == target_status)
                            .unwrap_or(false)
                    })
            })
            .collect()
    } else {
        invocations
    };

    if invocations.is_empty() {
        output.info("No background job history found.");
        return Ok(());
    }

    if args.json {
        return print_json_output(&invocations, &store, coordinator_alive, output);
    }

    // Group invocations by worktree.
    let mut groups: std::collections::BTreeMap<String, Vec<&InvocationMeta>> =
        std::collections::BTreeMap::new();
    for inv in &invocations {
        groups.entry(inv.worktree.clone()).or_default().push(inv);
    }

    let now = chrono::Utc::now();
    let mut first_group = true;

    for (worktree, inv_list) in &groups {
        if args.all || args.worktree.is_some() {
            if !first_group {
                output.info("");
            }
            output.info(&bold(worktree));
        }
        first_group = false;

        for inv in inv_list {
            let ago =
                shorthand_from_seconds(now.signed_duration_since(inv.created_at).num_seconds());
            let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];

            output.info("");
            output.info(&format!(
                "{} -- {} {}",
                dim(&ago),
                inv.trigger_command,
                dim(&format!("[{short_id}]")),
            ));

            // Collect jobs for this invocation.
            let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
            if job_dirs.is_empty() {
                output.info(&format!("  {}", dim("(no jobs declared)")));
                output.info("");
                continue;
            }

            let mut builder = Builder::new();

            // Header row.
            builder.push_record(vec![
                dim_underline("Job"),
                dim_underline("Status"),
                dim_underline("Started"),
                dim_underline("Duration"),
            ]);

            for dir in &job_dirs {
                if let Ok(meta) = store.read_meta(dir) {
                    let job_label = if meta.background {
                        format!("{} {}", blue("\u{21bb}"), meta.name)
                    } else {
                        meta.name.clone()
                    };

                    let status = format_status_inline(&meta.status, coordinator_alive);

                    let started = {
                        let local: chrono::DateTime<chrono::Local> = meta.started_at.into();
                        local.format("%H:%M:%S").to_string()
                    };

                    let duration = match (&meta.status, meta.finished_at) {
                        (_, Some(finished)) => {
                            let secs = finished
                                .signed_duration_since(meta.started_at)
                                .num_seconds();
                            format_duration(secs)
                        }
                        (JobStatus::Running, None) => {
                            let secs = now.signed_duration_since(meta.started_at).num_seconds();
                            format!("{}...", format_duration(secs))
                        }
                        _ => "\u{2014}".to_string(),
                    };

                    builder.push_record(vec![job_label, status, started, duration]);
                }
            }

            let mut table = builder.build();
            table.with(Style::blank());
            table.modify(Columns::first(), Padding::new(2, 1, 0, 0));

            output.info(&table.to_string());
        }
    }

    Ok(())
}

/// Show the output log for a job, resolved via address.
fn show_logs(
    job: &str,
    inv: Option<&str>,
    _args: &JobsArgs,
    _path: &Path,
    _output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();

    let addr = JobAddress::parse(job).with_inv_override(inv);

    // If the input is a single hex-only segment with no explicit --inv, check
    // whether it matches an invocation ID prefix. If so, show all jobs in that
    // invocation instead of a single job.
    let invocation_only = if addr.worktree.is_none()
        && addr.invocation_prefix.is_none()
        && inv.is_none()
        && is_hex_prefix(&addr.job_name)
    {
        let invocations = store.list_invocations_for_worktree(&current_worktree)?;
        let matches: Vec<_> = invocations
            .iter()
            .filter(|i| i.invocation_id.starts_with(&addr.job_name))
            .collect();
        if matches.len() == 1 {
            Some(matches[0].invocation_id.clone())
        } else {
            None
        }
    } else {
        None
    };

    let mut buf = String::new();

    if let Some(invocation_id) = invocation_only {
        render_invocation_logs(&store, &invocation_id, &mut buf)?;
    } else {
        let resolved = resolve_job_address(&addr, &store, &current_worktree)?;
        render_single_job_log(&store, &resolved, &mut buf)?;
    }

    crate::output::pager::display_with_pager(&buf);

    Ok(())
}

/// True when `s` is a non-empty ASCII hex string (0-9, a-f, A-F).
fn is_hex_prefix(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Render a single job's metadata and log into `buf`.
fn render_single_job_log(
    store: &LogStore,
    resolved: &ResolvedAddress,
    buf: &mut String,
) -> Result<()> {
    use std::fmt::Write;

    let meta = store.read_meta(&resolved.job_dir)?;
    let log_path = LogStore::log_path(&resolved.job_dir);
    let inv_meta = store.read_invocation_meta(&resolved.invocation_id).ok();

    let now = chrono::Utc::now();
    let short_id = &resolved.invocation_id[..4.min(resolved.invocation_id.len())];

    let status_label = match meta.status {
        JobStatus::Completed => green("COMPLETED"),
        JobStatus::Failed => red("FAILED"),
        JobStatus::Running => yellow("RUNNING"),
        JobStatus::Cancelled => dim("CANCELLED"),
        JobStatus::Skipped => dim("SKIPPED"),
    };
    writeln!(
        buf,
        "{}  {}  {}",
        status_label,
        bold(&meta.name),
        dim(&format!("[{short_id}]")),
    )?;

    let worktree_display = inv_meta
        .as_ref()
        .map(|m| m.worktree.as_str())
        .unwrap_or(&meta.worktree);
    if !worktree_display.is_empty() {
        writeln!(buf, "worktree:  {}", worktree_display)?;
    }

    if let Some(ref im) = inv_meta {
        writeln!(buf, "trigger:   {}", im.trigger_command)?;
    }

    let ago = shorthand_from_seconds(now.signed_duration_since(meta.started_at).num_seconds());
    let local_started: chrono::DateTime<chrono::Local> = meta.started_at.into();
    writeln!(
        buf,
        "started:   {} ago ({})",
        ago,
        local_started.format("%Y-%m-%d %H:%M:%S"),
    )?;

    let duration_str = match meta.finished_at {
        Some(finished) => format_duration(
            finished
                .signed_duration_since(meta.started_at)
                .num_seconds(),
        ),
        None => "\u{2014}".to_string(),
    };
    writeln!(buf, "duration:  {duration_str}")?;

    if !meta.command.is_empty() {
        writeln!(buf, "command:   {}", meta.command)?;
    }

    if log_path.exists() {
        writeln!(buf)?;
        writeln!(buf, "{}", dim("--- output ---"))?;
        let contents = std::fs::read_to_string(&log_path)
            .with_context(|| format!("Failed to read log file: {}", log_path.display()))?;
        buf.push_str(&contents);
        if !contents.ends_with('\n') {
            buf.push('\n');
        }
        writeln!(buf)?;
        writeln!(buf, "Full log: {}", log_path.display())?;
    } else {
        writeln!(buf)?;
        writeln!(buf, "{}", dim("(no output log)"))?;
    }

    Ok(())
}

/// Render all job logs for a single invocation into `buf`.
fn render_invocation_logs(store: &LogStore, invocation_id: &str, buf: &mut String) -> Result<()> {
    use std::fmt::Write;

    let inv_meta = store.read_invocation_meta(invocation_id)?;
    let short_id = &invocation_id[..4.min(invocation_id.len())];
    let now = chrono::Utc::now();

    // Collect job metas sorted by started_at.
    let job_dirs = store.list_jobs_in_invocation(invocation_id)?;
    let mut jobs: Vec<(std::path::PathBuf, crate::coordinator::log_store::JobMeta)> = job_dirs
        .into_iter()
        .filter_map(|dir| store.read_meta(&dir).ok().map(|m| (dir, m)))
        .collect();
    jobs.sort_by(|a, b| a.1.started_at.cmp(&b.1.started_at));

    // Invocation header.
    writeln!(
        buf,
        "{}  {}",
        bold(&inv_meta.trigger_command),
        dim(&format!("[{short_id}]")),
    )?;
    if !inv_meta.worktree.is_empty() {
        writeln!(buf, "worktree:  {}", inv_meta.worktree)?;
    }
    let ago = shorthand_from_seconds(now.signed_duration_since(inv_meta.created_at).num_seconds());
    let local_started: chrono::DateTime<chrono::Local> = inv_meta.created_at.into();
    writeln!(
        buf,
        "started:   {} ago ({})",
        ago,
        local_started.format("%Y-%m-%d %H:%M:%S"),
    )?;
    writeln!(buf, "jobs:      {}", jobs.len())?;
    writeln!(buf)?;

    // Per-job sections.
    for (dir, meta) in &jobs {
        let status_label = match meta.status {
            JobStatus::Completed => green("COMPLETED"),
            JobStatus::Failed => red("FAILED"),
            JobStatus::Running => yellow("RUNNING"),
            JobStatus::Cancelled => dim("CANCELLED"),
            JobStatus::Skipped => dim("SKIPPED"),
        };

        let duration_str = match meta.finished_at {
            Some(finished) => format_duration(
                finished
                    .signed_duration_since(meta.started_at)
                    .num_seconds(),
            ),
            None => "\u{2014}".to_string(),
        };

        writeln!(
            buf,
            "{} {}  {}  {}",
            dim("══"),
            status_label,
            bold(&meta.name),
            dim(&format!("({duration_str})")),
        )?;

        let log_path = LogStore::log_path(dir);
        if log_path.exists() {
            let contents = std::fs::read_to_string(&log_path)
                .with_context(|| format!("Failed to read log file: {}", log_path.display()))?;
            if contents.is_empty() {
                writeln!(buf, "{}", dim("(empty)"))?;
            } else {
                buf.push_str(&contents);
                if !contents.ends_with('\n') {
                    buf.push('\n');
                }
            }
        } else {
            writeln!(buf, "{}", dim("(no output log)"))?;
        }
        writeln!(buf)?;
    }

    Ok(())
}

/// Cancel a specific running job via the coordinator.
fn cancel_job(job: &str, inv: Option<&str>, _path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();

    let addr = JobAddress::parse(job).with_inv_override(inv);
    let resolved = resolve_job_address(&addr, &store, &current_worktree)?;

    match CoordinatorClient::connect(&repo_hash)? {
        Some(mut client) => {
            let msg = client.cancel_job(&resolved.job_name)?;
            output.success(&msg);
        }
        None => {
            anyhow::bail!("No coordinator running for this repository. Is the job still active?");
        }
    }

    Ok(())
}

/// Cancel all running jobs via the coordinator.
fn cancel_all(_path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;

    match CoordinatorClient::connect(&repo_hash)? {
        Some(mut client) => {
            let msg = client.cancel_all()?;
            output.success(&msg);
        }
        None => {
            output.info("No coordinator running for this repository.");
        }
    }

    Ok(())
}

/// Resolve the retry target to a specific invocation.
fn resolve_retry_invocation(
    target: &RetryTarget,
    store: &LogStore,
    current_worktree: &str,
) -> Result<InvocationMeta> {
    match target {
        RetryTarget::LatestInvocation => {
            let invocations = store.list_invocations_for_worktree(current_worktree)?;
            invocations.into_iter().last().ok_or_else(|| {
                anyhow::anyhow!(
                    "No invocations found in worktree '{current_worktree}'. Run a hook first."
                )
            })
        }
        RetryTarget::HookType(hook) => {
            let invocations = store.list_invocations_for_worktree(current_worktree)?;
            invocations
                .into_iter()
                .rfind(|inv| inv.hook_type == *hook)
                .ok_or_else(|| {
                    anyhow::anyhow!("No invocations of '{hook}' in worktree '{current_worktree}'.")
                })
        }
        RetryTarget::InvocationPrefix(prefix) => {
            let matches = store.find_invocations_by_prefix(current_worktree, prefix)?;
            match matches.len() {
                0 => anyhow::bail!(
                    "No invocation matching prefix '{prefix}' in worktree '{current_worktree}'."
                ),
                1 => Ok(matches.into_iter().next().unwrap()),
                _ => {
                    let now = chrono::Utc::now();
                    let lines: Vec<String> = matches
                        .iter()
                        .map(|inv| {
                            let ago = shorthand_from_seconds(
                                now.signed_duration_since(inv.created_at).num_seconds(),
                            );
                            let short = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                            format!("  {short}  {} -- {ago} ago", inv.trigger_command)
                        })
                        .collect();
                    anyhow::bail!(
                        "Ambiguous invocation prefix '{prefix}' -- matches:\n{}\n\
                         Use more characters to disambiguate.",
                        lines.join("\n")
                    );
                }
            }
        }
        RetryTarget::JobName(name) => {
            let invocations = store.list_invocations_for_worktree(current_worktree)?;
            for inv in invocations.iter().rev() {
                let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
                for dir in &job_dirs {
                    if let Ok(meta) = store.read_meta(dir) {
                        if meta.name == *name
                            && matches!(meta.status, JobStatus::Failed | JobStatus::Cancelled)
                        {
                            return Ok(inv.clone());
                        }
                    }
                }
            }
            anyhow::bail!("No failed job named '{name}' in worktree '{current_worktree}'.")
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn retry_command(
    target: Option<&str>,
    hook_flag: &Option<String>,
    inv_flag: &Option<String>,
    job_flag: &Option<String>,
    worktree_flag: Option<&str>,
    cwd_flag: Option<&str>,
    _path: &Path,
    output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();

    let mut effective_worktree = worktree_flag
        .map(|s| s.to_string())
        .unwrap_or_else(|| current_worktree.clone());

    let flags = RetryFlags {
        hook: hook_flag.clone(),
        inv: inv_flag.clone(),
        job: job_flag.clone(),
    };
    let parsed = retry_target_from_arg(target, &flags);

    // Handle composite address for job-name targets (supports cross-worktree).
    let parsed = if let RetryTarget::JobName(ref name) = parsed {
        if name.contains(':') {
            let addr = JobAddress::parse(name);
            if let Some(ref wt) = addr.worktree {
                // Check for conflict between --worktree flag and composite address
                if let Some(flag_wt) = worktree_flag {
                    if flag_wt != wt.as_str() {
                        anyhow::bail!(
                            "Conflicting worktree: --worktree says '{}' but address says '{}'.",
                            flag_wt,
                            wt
                        );
                    }
                }
                effective_worktree = wt.clone();
                RetryTarget::JobName(addr.job_name)
            } else {
                parsed
            }
        } else {
            parsed
        }
    } else {
        parsed
    };

    // Resolve to a source invocation.
    let inv = resolve_retry_invocation(&parsed, &store, &effective_worktree)?;

    // Load all job metas from the source invocation.
    let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
    let mut metas: Vec<crate::coordinator::log_store::JobMeta> = Vec::new();
    for dir in &job_dirs {
        if let Ok(meta) = store.read_meta(dir) {
            metas.push(meta);
        }
    }

    // Check for running jobs — bail if any.
    let running: Vec<&str> = metas
        .iter()
        .filter(|m| matches!(m.status, JobStatus::Running))
        .map(|m| m.name.as_str())
        .collect();
    if !running.is_empty() {
        anyhow::bail!(
            "Cannot retry: {} job(s) still running ({}). Cancel them first.",
            running.len(),
            running.join(", ")
        );
    }

    // For single-job form, filter metas to just that job and validate.
    if let RetryTarget::JobName(ref name) = parsed {
        // Extract the bare job name (strip worktree/invocation prefix).
        let bare_name = if name.contains(':') {
            let addr = JobAddress::parse(name);
            addr.job_name.clone()
        } else {
            name.clone()
        };

        let job_meta = metas.iter().find(|m| m.name == bare_name);
        match job_meta {
            None => {
                anyhow::bail!(
                    "No job named '{}' in invocation '{}'.",
                    bare_name,
                    &inv.invocation_id[..4.min(inv.invocation_id.len())]
                );
            }
            Some(m) => {
                if !matches!(m.status, JobStatus::Failed | JobStatus::Cancelled) {
                    anyhow::bail!(
                        "Job '{}' is not in a failed or cancelled state (current: {:?}).",
                        bare_name,
                        m.status
                    );
                }
            }
        }

        // Filter metas to just this one job.
        metas.retain(|m| m.name == bare_name);
    }

    // Compute the retry set.
    let (specs, _retry_names) = build_retry_set(&metas);

    if specs.is_empty() {
        output.info("Nothing to retry — all jobs succeeded.");
        return Ok(());
    }

    // Validate --cwd path if provided.
    if let Some(cwd) = cwd_flag {
        let cwd_path = std::path::Path::new(cwd);
        if !cwd_path.exists() || !cwd_path.is_dir() {
            anyhow::bail!("--cwd path '{}' does not exist or is not a directory.", cwd);
        }
    }

    // Validate working dirs and commands.
    for spec in &specs {
        if spec.command.is_empty() {
            anyhow::bail!(
                "Cannot retry job '{}': no command recorded in metadata. \
                 This job may have been created by an older version of daft.",
                spec.name
            );
        }
        if !spec.working_dir.exists() && cwd_flag.is_none() {
            anyhow::bail!(
                "Cannot retry job '{}': working directory '{}' no longer exists. \
                 Use --cwd to specify an alternative.",
                spec.name,
                spec.working_dir.display()
            );
        }
    }

    // Split into foreground and background sets.
    let (mut fg_specs, mut bg_specs): (Vec<_>, Vec<_>) =
        specs.into_iter().partition(|s| !s.background);

    if let Some(cwd) = cwd_flag {
        let cwd_path = std::path::PathBuf::from(cwd);
        for spec in &mut fg_specs {
            spec.working_dir = cwd_path.clone();
        }
        for spec in &mut bg_specs {
            spec.working_dir = cwd_path.clone();
        }
    }

    let total = fg_specs.len() + bg_specs.len();
    let new_invocation_id = crate::coordinator::log_store::generate_invocation_id();
    let short_id = &new_invocation_id[..4.min(new_invocation_id.len())];

    // Build trigger command for the new invocation.
    let trigger_command = match &parsed {
        RetryTarget::LatestInvocation => "hooks jobs retry".to_string(),
        RetryTarget::HookType(h) => format!("hooks jobs retry {h}"),
        RetryTarget::InvocationPrefix(p) => format!("hooks jobs retry {p}"),
        RetryTarget::JobName(n) => format!("hooks jobs retry {n}"),
    };

    // Write invocation meta for the new retry invocation.
    let inv_meta = InvocationMeta {
        invocation_id: new_invocation_id.clone(),
        trigger_command: trigger_command.clone(),
        hook_type: inv.hook_type.clone(),
        worktree: effective_worktree.clone(),
        created_at: chrono::Utc::now(),
    };
    store.write_invocation_meta(&new_invocation_id, &inv_meta)?;

    // ── Foreground phase ────────────────────────────────────────────────
    let fg_count = fg_specs.len();
    if !fg_specs.is_empty() {
        let config = crate::settings::HookOutputConfig::default();
        let presenter: std::sync::Arc<dyn crate::executor::presenter::JobPresenter> =
            crate::executor::cli_presenter::CliPresenter::auto(&config);

        let fg_sink: std::sync::Arc<dyn crate::executor::log_sink::LogSink> =
            std::sync::Arc::new(crate::executor::BufferingLogSink::new(
                std::sync::Arc::new(store.clone()),
                new_invocation_id.clone(),
                inv.hook_type.clone(),
                effective_worktree.clone(),
            ));

        presenter.on_phase_start(&trigger_command);
        let fg_start = std::time::Instant::now();
        let _fg_results = crate::executor::runner::run_jobs(
            &fg_specs,
            crate::executor::ExecutionMode::Parallel,
            &presenter,
            Some(&fg_sink),
        )?;
        presenter.on_phase_complete(fg_start.elapsed());
    }

    // ── Background phase ────────────────────────────────────────────────
    let bg_count = bg_specs.len();
    if !bg_specs.is_empty() {
        #[cfg(unix)]
        {
            let bg_store = LogStore::for_repo(&repo_hash)?;
            let mut coord_state =
                crate::coordinator::process::CoordinatorState::new(&repo_hash, &new_invocation_id)
                    .with_metadata(&trigger_command, &inv.hook_type, &effective_worktree);
            for spec in bg_specs {
                coord_state.add_job(spec);
            }
            crate::coordinator::process::fork_coordinator(coord_state, bg_store)?;
        }

        #[cfg(not(unix))]
        {
            let _ = bg_specs;
            anyhow::bail!("Background job retry is only supported on Unix systems.");
        }
    }

    // ── Summary ─────────────────────────────────────────────────────────
    let fg_label = if fg_count > 0 {
        format!("{fg_count} foreground done")
    } else {
        String::new()
    };
    let bg_label = if bg_count > 0 {
        format!("{bg_count} background running")
    } else {
        String::new()
    };
    let parts: Vec<&str> = [fg_label.as_str(), bg_label.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();

    output.success(&format!(
        "Retried {} job(s) in invocation {} ({}). Check status: daft hooks jobs",
        total,
        short_id,
        parts.join(", ")
    ));

    Ok(())
}

/// Remove logs older than the default retention period (7 days).
fn clean_logs(args: &JobsArgs, _path: &Path, output: &mut dyn Output) -> Result<()> {
    if args.all {
        let hashes = list_all_repo_hashes()?;
        let mut total = 0;
        for hash in &hashes {
            let store = LogStore::for_repo(hash)?;
            total += store.clean(chrono::Duration::days(7))?;
        }
        if total > 0 {
            output.success(&format!("Removed {total} old job log(s) across all repos."));
        } else {
            output.info("No old logs to clean.");
        }
    } else {
        let repo_hash = crate::core::repo_identity::compute_repo_id()?;
        let store = LogStore::for_repo(&repo_hash)?;
        let removed = store.clean(chrono::Duration::days(7))?;

        if removed > 0 {
            output.success(&format!("Removed {removed} old job log(s)."));
        } else {
            output.info("No old logs to clean.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_job_address_name_only() {
        let addr = JobAddress::parse("db-migrate");
        assert_eq!(addr.job_name, "db-migrate");
        assert!(addr.invocation_prefix.is_none());
        assert!(addr.worktree.is_none());
    }

    #[test]
    fn test_parse_job_address_invocation_and_name() {
        let addr = JobAddress::parse("c9d4:db-migrate");
        assert_eq!(addr.job_name, "db-migrate");
        assert_eq!(addr.invocation_prefix.as_deref(), Some("c9d4"));
        assert!(addr.worktree.is_none());
    }

    #[test]
    fn test_parse_job_address_full() {
        let addr = JobAddress::parse("feat/tax-calc:c9d4:db-migrate");
        assert_eq!(addr.job_name, "db-migrate");
        assert_eq!(addr.invocation_prefix.as_deref(), Some("c9d4"));
        assert_eq!(addr.worktree.as_deref(), Some("feat/tax-calc"));
    }

    #[test]
    fn test_parse_job_address_worktree_with_slash() {
        let addr = JobAddress::parse("feature/auth/v2:a3f2:warm-build");
        assert_eq!(addr.worktree.as_deref(), Some("feature/auth/v2"));
        assert_eq!(addr.invocation_prefix.as_deref(), Some("a3f2"));
        assert_eq!(addr.job_name, "warm-build");
    }

    #[test]
    fn test_parse_job_address_worktree_job_two_segment() {
        let addr = JobAddress::parse("feature/auth:db-migrate");
        assert_eq!(addr.worktree.as_deref(), Some("feature/auth"));
        assert!(addr.invocation_prefix.is_none());
        assert_eq!(addr.job_name, "db-migrate");
    }

    #[test]
    fn test_parse_job_address_two_segment_inv_job_no_slash() {
        let addr = JobAddress::parse("c9d4:db-migrate");
        assert!(addr.worktree.is_none());
        assert_eq!(addr.invocation_prefix.as_deref(), Some("c9d4"));
        assert_eq!(addr.job_name, "db-migrate");
    }

    #[test]
    fn test_resolve_job_address_name_only_finds_most_recent() {
        use crate::coordinator::log_store::{InvocationMeta, JobMeta, JobStatus, LogStore};
        use std::collections::HashMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        let now = chrono::Utc::now();
        for (inv_id, offset) in &[("0001000000000000", 100i64), ("0002000000000000", 50)] {
            let inv_meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: "feature/x".to_string(),
                created_at: now - chrono::Duration::seconds(*offset),
            };
            store.write_invocation_meta(inv_id, &inv_meta).unwrap();

            let dir = store.create_job_dir(inv_id, "db-migrate").unwrap();
            let meta = JobMeta {
                name: "db-migrate".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: "feature/x".to_string(),
                command: "echo".to_string(),
                working_dir: "/tmp".to_string(),
                env: HashMap::new(),
                started_at: now - chrono::Duration::seconds(*offset),
                status: JobStatus::Completed,
                exit_code: Some(0),
                pid: None,
                background: true,
                finished_at: Some(now - chrono::Duration::seconds(offset - 3)),
                needs: vec![],
            };
            store.write_meta(&dir, &meta).unwrap();
        }

        let addr = JobAddress::parse("db-migrate");
        let result = resolve_job_address(&addr, &store, "feature/x").unwrap();
        // Should resolve to the most recent invocation (0002... has later created_at)
        assert!(result.invocation_id.starts_with("0002"));
        assert_eq!(result.job_name, "db-migrate");
    }

    #[test]
    fn test_resolve_job_address_with_prefix() {
        use crate::coordinator::log_store::{InvocationMeta, JobMeta, JobStatus, LogStore};
        use std::collections::HashMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        let inv_id = "c9d4e7f2a3b10000";
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            created_at: now,
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        let dir = store.create_job_dir(inv_id, "db-migrate").unwrap();
        let meta = JobMeta {
            name: "db-migrate".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "echo".to_string(),
            working_dir: "/tmp".to_string(),
            env: HashMap::new(),
            started_at: now,
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: true,
            finished_at: Some(now),
            needs: vec![],
        };
        store.write_meta(&dir, &meta).unwrap();

        let addr = JobAddress::parse("c9d4:db-migrate");
        let result = resolve_job_address(&addr, &store, "feature/x").unwrap();
        assert_eq!(result.invocation_id, inv_id);
    }

    #[test]
    fn format_status_inline_renders_skipped() {
        let rendered = format_status_inline(&JobStatus::Skipped, true);
        // The dim helper wraps in ANSI codes; check that the literal "skipped"
        // is present inside.
        assert!(rendered.contains("skipped"));
    }

    #[test]
    fn test_retry_target_empty_is_latest() {
        let target = retry_target_from_arg(None, &RetryFlags::default());
        assert!(matches!(target, RetryTarget::LatestInvocation));
    }

    #[test]
    fn test_retry_target_known_hook_type() {
        let target = retry_target_from_arg(Some("worktree-post-create"), &RetryFlags::default());
        assert!(matches!(target, RetryTarget::HookType(ref h) if h == "worktree-post-create"));
    }

    #[test]
    fn test_retry_target_hex_prefix() {
        let target = retry_target_from_arg(Some("a3f2"), &RetryFlags::default());
        assert!(matches!(target, RetryTarget::InvocationPrefix(ref p) if p == "a3f2"));
    }

    #[test]
    fn test_retry_target_job_name() {
        let target = retry_target_from_arg(Some("db-migrate"), &RetryFlags::default());
        assert!(matches!(target, RetryTarget::JobName(ref n) if n == "db-migrate"));
    }

    #[test]
    fn test_retry_target_flag_overrides_shape() {
        let flags = RetryFlags {
            hook: Some("worktree-post-create".into()),
            ..Default::default()
        };
        let target = retry_target_from_arg(None, &flags);
        assert!(matches!(target, RetryTarget::HookType(ref h) if h == "worktree-post-create"));

        let flags = RetryFlags {
            inv: Some("a3f2".into()),
            ..Default::default()
        };
        let target = retry_target_from_arg(None, &flags);
        assert!(matches!(target, RetryTarget::InvocationPrefix(ref p) if p == "a3f2"));

        let flags = RetryFlags {
            job: Some("db-migrate".into()),
            ..Default::default()
        };
        let target = retry_target_from_arg(None, &flags);
        assert!(matches!(target, RetryTarget::JobName(ref n) if n == "db-migrate"));
    }

    #[test]
    fn test_retry_target_post_clone_is_hook_not_job() {
        let target = retry_target_from_arg(Some("post-clone"), &RetryFlags::default());
        assert!(matches!(target, RetryTarget::HookType(ref h) if h == "post-clone"));
    }

    #[test]
    fn test_retry_target_8char_hex_is_invocation() {
        let target = retry_target_from_arg(Some("deadbeef"), &RetryFlags::default());
        assert!(matches!(target, RetryTarget::InvocationPrefix(_)));
    }

    #[test]
    fn test_retry_target_9char_hex_is_job() {
        let target = retry_target_from_arg(Some("deadbeef0"), &RetryFlags::default());
        assert!(matches!(target, RetryTarget::JobName(_)));
    }

    fn make_test_job_meta(
        name: &str,
        status: crate::coordinator::log_store::JobStatus,
        needs: Vec<String>,
    ) -> crate::coordinator::log_store::JobMeta {
        crate::coordinator::log_store::JobMeta {
            name: name.to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: format!("echo {name}"),
            working_dir: "/tmp".to_string(),
            env: std::collections::HashMap::new(),
            started_at: chrono::Utc::now(),
            status,
            exit_code: None,
            pid: None,
            background: false,
            finished_at: None,
            needs,
        }
    }

    #[test]
    fn test_build_retry_set_picks_failed_and_cancelled() {
        let metas = vec![
            make_test_job_meta("a", JobStatus::Completed, vec![]),
            make_test_job_meta("b", JobStatus::Failed, vec!["a".into()]),
            make_test_job_meta("c", JobStatus::Cancelled, vec!["b".into()]),
            make_test_job_meta("d", JobStatus::Skipped, vec![]),
        ];
        let (specs, _) = build_retry_set(&metas);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
        // b's needs should be pruned (a is not in retry set)
        let b_spec = specs.iter().find(|s| s.name == "b").unwrap();
        assert!(b_spec.needs.is_empty());
        // c's needs should point to b (b IS in retry set)
        let c_spec = specs.iter().find(|s| s.name == "c").unwrap();
        assert_eq!(c_spec.needs, vec!["b".to_string()]);
    }

    #[test]
    fn test_build_retry_set_all_green_returns_empty() {
        let metas = vec![
            make_test_job_meta("a", JobStatus::Completed, vec![]),
            make_test_job_meta("b", JobStatus::Completed, vec!["a".into()]),
        ];
        let (specs, _) = build_retry_set(&metas);
        assert!(specs.is_empty());
    }

    #[test]
    fn test_build_retry_set_single_failed() {
        let metas = vec![make_test_job_meta("only", JobStatus::Failed, vec![])];
        let (specs, _) = build_retry_set(&metas);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "only");
    }

    #[test]
    fn test_resolve_retry_invocation_with_worktree_override() {
        use crate::coordinator::log_store::{InvocationMeta, LogStore};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        // Create an invocation in feature/other (not "current")
        std::fs::create_dir_all(tmp.path().join("inv1")).unwrap();
        let inv_meta = InvocationMeta {
            invocation_id: "inv1".to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/other".to_string(),
            created_at: now,
        };
        store.write_invocation_meta("inv1", &inv_meta).unwrap();

        // Resolving with "feature/other" as worktree should find it
        let result =
            resolve_retry_invocation(&RetryTarget::LatestInvocation, &store, "feature/other");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().worktree, "feature/other");

        // Resolving with "feature/current" should find nothing
        let result =
            resolve_retry_invocation(&RetryTarget::LatestInvocation, &store, "feature/current");
        assert!(result.is_err());
    }

    #[test]
    fn list_all_repo_hashes_filters_non_uuid_dirs() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        // SAFETY: this test mutates a process-global env var. If any
        // other test in this file sets DAFT_STATE_DIR, add #[serial] here.
        std::env::set_var("DAFT_STATE_DIR", tmp.path());
        let jobs_dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&jobs_dir).unwrap();

        // One valid UUID-named dir, one legacy 16-hex-char name.
        let uuid_name = "01900000-0000-7000-8000-000000000000";
        std::fs::create_dir(jobs_dir.join(uuid_name)).unwrap();
        std::fs::create_dir(jobs_dir.join("019d12345678abcd")).unwrap();

        let hashes = list_all_repo_hashes().unwrap();
        assert_eq!(hashes, vec![uuid_name.to_string()]);

        std::env::remove_var("DAFT_STATE_DIR");
    }
}
