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
    #[arg(long)]
    all: bool,

    /// Output in JSON format.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum JobsCommand {
    /// View output log for a background job.
    Logs {
        /// Job address: name, inv:name, or worktree:inv:name.
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
    /// Re-run a failed background job.
    Retry {
        /// Job address.
        job: String,
        /// Invocation ID prefix.
        #[arg(long)]
        inv: Option<String>,
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
            2 => Self {
                worktree: None,
                invocation_prefix: Some(parts[1].to_string()),
                job_name: parts[0].to_string(),
            },
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
        Some(JobsCommand::Retry { ref job, ref inv }) => {
            retry_job(job, inv.as_deref(), path, output)
        }
        Some(JobsCommand::Clean) => clean_logs(&args, path, output),
    }
}

/// Compute the repo hash from a working directory path.
///
/// This must produce the same hash as `compute_repo_hash()` in the YAML
/// executor so that the CLI can find the coordinator socket and log store
/// for the current repository.
fn compute_repo_hash_from_path(path: &Path) -> Result<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let project_root = crate::get_project_root()
        .context("Could not determine project root. Are you inside a git repository?")?;
    let _ = path; // path arg reserved for future use; we resolve from cwd
    let mut hasher = DefaultHasher::new();
    project_root.display().to_string().hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
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
                hashes.push(name.to_string());
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
fn list_jobs(args: &JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let coordinator_alive = is_coordinator_running(&repo_hash);

    let store = LogStore::for_repo(&repo_hash)?;
    let invocations = if args.all {
        store.list_invocations()?
    } else {
        store.list_invocations_for_worktree(&current_worktree)?
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
        if args.all {
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
    path: &Path,
    output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();

    let addr = JobAddress::parse(job).with_inv_override(inv);
    let resolved = resolve_job_address(&addr, &store, &current_worktree)?;

    let meta = store.read_meta(&resolved.job_dir)?;
    let log_path = LogStore::log_path(&resolved.job_dir);

    // Read invocation meta for worktree/trigger info.
    let inv_meta = store.read_invocation_meta(&resolved.invocation_id).ok();

    let now = chrono::Utc::now();
    let short_id = &resolved.invocation_id[..4.min(resolved.invocation_id.len())];

    // Status header.
    let status_label = match meta.status {
        JobStatus::Completed => green("COMPLETED"),
        JobStatus::Failed => red("FAILED"),
        JobStatus::Running => yellow("RUNNING"),
        JobStatus::Cancelled => dim("CANCELLED"),
    };
    output.info(&format!(
        "{}  {}  {}",
        status_label,
        bold(&meta.name),
        dim(&format!("[{short_id}]")),
    ));

    // Worktree.
    let worktree_display = inv_meta
        .as_ref()
        .map(|m| m.worktree.as_str())
        .unwrap_or(&meta.worktree);
    if !worktree_display.is_empty() {
        output.info(&format!("worktree:  {}", worktree_display));
    }

    // Trigger.
    if let Some(ref im) = inv_meta {
        output.info(&format!("trigger:   {}", im.trigger_command));
    }

    // Started.
    let ago = shorthand_from_seconds(now.signed_duration_since(meta.started_at).num_seconds());
    let local_started: chrono::DateTime<chrono::Local> = meta.started_at.into();
    output.info(&format!(
        "started:   {} ago ({})",
        ago,
        local_started.format("%Y-%m-%d %H:%M:%S"),
    ));

    // Duration.
    let duration_str = match meta.finished_at {
        Some(finished) => {
            let secs = finished
                .signed_duration_since(meta.started_at)
                .num_seconds();
            format_duration(secs)
        }
        None => "\u{2014}".to_string(),
    };
    output.info(&format!("duration:  {duration_str}"));

    // Command.
    if !meta.command.is_empty() {
        output.info(&format!("command:   {}", meta.command));
    }

    // Log output.
    if log_path.exists() {
        output.info("");
        output.info(&dim("--- output ---"));
        let contents = std::fs::read_to_string(&log_path)
            .with_context(|| format!("Failed to read log file: {}", log_path.display()))?;
        output.info(&contents);
        output.info("");
        output.info(&format!("Full log: {}", log_path.display()));
    } else {
        output.info("");
        output.info(&dim("(no output log)"));
    }

    Ok(())
}

/// Cancel a specific running job via the coordinator.
fn cancel_job(job: &str, inv: Option<&str>, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
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
fn cancel_all(path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;

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

/// Retry a failed job by reconstructing a JobSpec from stored metadata.
fn retry_job(job: &str, inv: Option<&str>, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();

    let addr = JobAddress::parse(job).with_inv_override(inv);
    let resolved = resolve_job_address(&addr, &store, &current_worktree)?;
    let meta = store.read_meta(&resolved.job_dir)?;

    if !matches!(meta.status, JobStatus::Failed) {
        anyhow::bail!(
            "Job '{}' is not in a failed state (current: {:?}). Only failed jobs can be retried.",
            resolved.job_name,
            meta.status
        );
    }

    if meta.command.is_empty() {
        anyhow::bail!(
            "Cannot retry job '{job}': no command recorded in metadata. \
             This job may have been created by an older version of daft."
        );
    }

    let working_dir = std::path::PathBuf::from(&meta.working_dir);
    if !working_dir.exists() {
        anyhow::bail!(
            "Cannot retry job '{job}': working directory '{}' no longer exists.",
            meta.working_dir
        );
    }

    output.info(&format!("Retrying job: {}", bold(&meta.name)));
    output.info(&format!("  command:  {}", dim(&meta.command)));
    output.info(&format!("  workdir:  {}", dim(&meta.working_dir)));

    // Reconstruct a JobSpec and spawn a coordinator.
    let job_spec = crate::executor::JobSpec {
        name: meta.name.clone(),
        command: meta.command,
        working_dir,
        env: meta.env,
        background: true,
        ..Default::default()
    };

    let invocation_id = generate_invocation_id();
    let retry_store = LogStore::for_repo(&repo_hash)?;
    let trigger_command = format!("hooks jobs retry {}", meta.name);
    let mut coord_state =
        crate::coordinator::process::CoordinatorState::new(&repo_hash, &invocation_id)
            .with_metadata(&trigger_command, &meta.hook_type, &meta.worktree);
    coord_state.add_job(job_spec);

    #[cfg(unix)]
    {
        crate::coordinator::process::fork_coordinator(coord_state, retry_store)?;
        output.success(&format!("Job '{}' re-dispatched to background.", meta.name));
    }

    #[cfg(not(unix))]
    {
        let _ = (coord_state, retry_store);
        anyhow::bail!("Background job retry is only supported on Unix systems.");
    }

    Ok(())
}

/// Remove logs older than the default retention period (7 days).
fn clean_logs(args: &JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
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
        let repo_hash = compute_repo_hash_from_path(path)?;
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

/// Generate a unique invocation ID (same logic as the yaml executor).
fn generate_invocation_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("{ts:016x}")
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
        };
        store.write_meta(&dir, &meta).unwrap();

        let addr = JobAddress::parse("c9d4:db-migrate");
        let result = resolve_job_address(&addr, &store, "feature/x").unwrap();
        assert_eq!(result.invocation_id, inv_id);
    }
}
