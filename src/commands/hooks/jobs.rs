//! `daft hooks jobs` — manage background hook jobs.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::Path;

use crate::coordinator::client::CoordinatorClient;
use crate::coordinator::log_store::{JobStatus, LogStore};
use crate::output::Output;
use crate::styles::{bold, dim, green, red, yellow};

#[derive(Parser, Debug)]
#[command(about = "Manage background hook jobs")]
pub struct JobsArgs {
    #[command(subcommand)]
    command: Option<JobsCommand>,

    /// Show jobs across all repositories.
    #[arg(long)]
    all_repos: bool,

    /// Filter to a specific worktree.
    #[arg(long)]
    worktree: Option<String>,

    /// Output in JSON format.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum JobsCommand {
    /// View output log for a background job.
    Logs {
        /// Job name.
        job: String,
    },
    /// Cancel a running background job.
    Cancel {
        /// Job name (omit for --all).
        job: Option<String>,
        /// Cancel all running jobs.
        #[arg(long)]
        all: bool,
    },
    /// Re-run a failed background job.
    Retry {
        /// Job name.
        job: String,
    },
    /// Remove logs older than the retention period.
    Clean,
}

pub fn run(args: JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    match args.command {
        None => list_jobs(&args, path, output),
        Some(JobsCommand::Logs { ref job }) => show_logs(job, &args, path, output),
        Some(JobsCommand::Cancel { ref job, all }) => {
            if all || job.is_none() {
                cancel_all(path, output)
            } else {
                cancel_job(job.as_ref().unwrap(), path, output)
            }
        }
        Some(JobsCommand::Retry { ref job }) => retry_job(job, path, output),
        Some(JobsCommand::Clean) => clean_logs(path, output),
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

fn format_elapsed(secs: Option<u64>) -> String {
    match secs {
        Some(s) if s >= 60 => format!("{}m {}s", s / 60, s % 60),
        Some(s) => format!("{}s", s),
        None => "\u{2014}".to_string(), // em dash
    }
}

fn format_status(status: &JobStatus) -> String {
    match status {
        JobStatus::Running => yellow("RUNNING"),
        JobStatus::Completed => green("COMPLETED"),
        JobStatus::Failed => red("FAILED"),
        JobStatus::Cancelled => dim("CANCELLED"),
    }
}

/// Default subcommand: list jobs (running from coordinator + history from log store).
fn list_jobs(args: &JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hashes = if args.all_repos {
        list_all_repo_hashes()?
    } else {
        vec![compute_repo_hash_from_path(path)?]
    };

    if repo_hashes.is_empty() {
        output.info("No background job history found.");
        return Ok(());
    }

    let mut any_jobs = false;

    for repo_hash in &repo_hashes {
        // Collect live running jobs from the coordinator (if one is active).
        let live_jobs = match CoordinatorClient::connect(repo_hash)? {
            Some(mut client) => client.list_jobs().unwrap_or_default(),
            None => vec![],
        };

        // Collect historical jobs from the log store.
        let store = LogStore::for_repo(repo_hash)?;
        let job_dirs = store.list_job_dirs()?;

        let mut all_metas: Vec<crate::coordinator::log_store::JobMeta> = Vec::new();
        for dir in &job_dirs {
            if let Ok(meta) = store.read_meta(dir) {
                // Apply worktree filter if specified.
                if let Some(ref wt_filter) = args.worktree {
                    if !meta.worktree.contains(wt_filter.as_str()) {
                        continue;
                    }
                }
                all_metas.push(meta);
            }
        }

        // Merge live status into stored metas: if a coordinator reports a job
        // as Running, prefer that over what's on disk (which may be stale).
        for live in &live_jobs {
            if !all_metas.iter().any(|m| m.name == live.name) {
                // Create a synthetic meta for live-only jobs.
                all_metas.push(crate::coordinator::log_store::JobMeta {
                    name: live.name.clone(),
                    hook_type: live.hook_type.clone(),
                    worktree: live.worktree.clone(),
                    command: String::new(),
                    working_dir: String::new(),
                    env: std::collections::HashMap::new(),
                    started_at: chrono::Utc::now(),
                    status: live.status.clone(),
                    exit_code: live.exit_code,
                    pid: None,
                });
            }
        }

        if all_metas.is_empty() {
            continue;
        }
        any_jobs = true;

        if args.json {
            let json = serde_json::to_string_pretty(&all_metas)?;
            output.info(&json);
            continue;
        }

        if args.all_repos {
            output.info(&bold(&format!("Repository: {repo_hash}")));
            output.info("");
        }

        // Group by status for display.
        let mut running: Vec<&crate::coordinator::log_store::JobMeta> = Vec::new();
        let mut completed: Vec<&crate::coordinator::log_store::JobMeta> = Vec::new();
        let mut failed: Vec<&crate::coordinator::log_store::JobMeta> = Vec::new();
        let mut cancelled: Vec<&crate::coordinator::log_store::JobMeta> = Vec::new();

        for meta in &all_metas {
            match meta.status {
                JobStatus::Running => running.push(meta),
                JobStatus::Completed => completed.push(meta),
                JobStatus::Failed => failed.push(meta),
                JobStatus::Cancelled => cancelled.push(meta),
            }
        }

        if !running.is_empty() {
            output.info(&yellow("RUNNING"));
            for meta in &running {
                let elapsed = live_jobs
                    .iter()
                    .find(|j| j.name == meta.name)
                    .and_then(|j| j.elapsed_secs);
                output.info(&format!(
                    "  {} ({}) [{}]",
                    bold(&meta.name),
                    meta.hook_type,
                    format_elapsed(elapsed),
                ));
                if !meta.worktree.is_empty() {
                    output.info(&format!("    worktree: {}", dim(&meta.worktree)));
                }
            }
            output.info("");
        }

        if !failed.is_empty() {
            output.info(&red("FAILED"));
            for meta in &failed {
                let exit_str = meta
                    .exit_code
                    .map(|c| format!("exit {c}"))
                    .unwrap_or_else(|| "unknown".to_string());
                output.info(&format!(
                    "  {} ({}) [{}]",
                    bold(&meta.name),
                    meta.hook_type,
                    exit_str,
                ));
                if !meta.worktree.is_empty() {
                    output.info(&format!("    worktree: {}", dim(&meta.worktree)));
                }
            }
            output.info("");
        }

        if !completed.is_empty() {
            output.info(&green("COMPLETED"));
            for meta in &completed {
                output.info(&format!("  {} ({})", bold(&meta.name), meta.hook_type,));
            }
            output.info("");
        }

        if !cancelled.is_empty() {
            output.info(&dim("CANCELLED"));
            for meta in &cancelled {
                output.info(&format!("  {} ({})", bold(&meta.name), meta.hook_type,));
            }
            output.info("");
        }
    }

    if !any_jobs {
        output.info("No background job history found.");
    }

    Ok(())
}

/// Show the output log for the most recent instance of a named job.
fn show_logs(job: &str, args: &JobsArgs, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;

    // Find the most recent job directory matching the name.
    let job_dirs = store.list_job_dirs()?;
    let mut matching: Vec<(std::path::PathBuf, chrono::DateTime<chrono::Utc>)> = Vec::new();

    for dir in &job_dirs {
        if let Ok(meta) = store.read_meta(dir) {
            if meta.name == job {
                // Apply worktree filter if specified.
                if let Some(ref wt_filter) = args.worktree {
                    if !meta.worktree.contains(wt_filter.as_str()) {
                        continue;
                    }
                }
                matching.push((dir.clone(), meta.started_at));
            }
        }
    }

    if matching.is_empty() {
        anyhow::bail!("No logs found for job '{job}'");
    }

    // Pick the most recent.
    matching.sort_by(|a, b| b.1.cmp(&a.1));
    let (job_dir, _) = &matching[0];
    let log_path = LogStore::log_path(job_dir);

    if !log_path.exists() {
        anyhow::bail!("Log file not found for job '{job}'");
    }

    let meta = store.read_meta(job_dir)?;
    output.info(&format!(
        "{} {} ({})",
        format_status(&meta.status),
        bold(&meta.name),
        meta.hook_type,
    ));
    if !meta.worktree.is_empty() {
        output.info(&format!("worktree: {}", dim(&meta.worktree)));
    }
    output.info(&format!("started:  {}", dim(&meta.started_at.to_rfc3339())));
    output.info(&dim("---"));

    let contents = std::fs::read_to_string(&log_path)
        .with_context(|| format!("Failed to read log file: {}", log_path.display()))?;
    output.info(&contents);

    Ok(())
}

/// Cancel a specific running job via the coordinator.
fn cancel_job(job: &str, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;

    match CoordinatorClient::connect(&repo_hash)? {
        Some(mut client) => {
            let msg = client.cancel_job(job)?;
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
fn retry_job(job: &str, path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;

    // Find the most recent failed instance of this job.
    let job_dirs = store.list_job_dirs()?;
    let mut matching: Vec<(std::path::PathBuf, chrono::DateTime<chrono::Utc>)> = Vec::new();

    for dir in &job_dirs {
        if let Ok(meta) = store.read_meta(dir) {
            if meta.name == job && matches!(meta.status, JobStatus::Failed) {
                matching.push((dir.clone(), meta.started_at));
            }
        }
    }

    if matching.is_empty() {
        anyhow::bail!("No failed job named '{job}' found. Only failed jobs can be retried.");
    }

    // Pick the most recent failed instance.
    matching.sort_by(|a, b| b.1.cmp(&a.1));
    let (job_dir, _) = &matching[0];
    let meta = store.read_meta(job_dir)?;

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
    let mut coord_state =
        crate::coordinator::process::CoordinatorState::new(&repo_hash, &invocation_id);
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
fn clean_logs(path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = compute_repo_hash_from_path(path)?;
    let store = LogStore::for_repo(&repo_hash)?;
    let removed = store.clean(chrono::Duration::days(7))?;

    if removed > 0 {
        output.success(&format!("Removed {removed} old job log(s)."));
    } else {
        output.info("No old logs to clean.");
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
