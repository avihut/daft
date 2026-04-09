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

/// Parsed composite job address: `[worktree:][invocation:]job_name`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct JobAddress {
    pub worktree: Option<String>,
    pub invocation_prefix: Option<String>,
    pub job_name: String,
}

#[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn with_inv_override(mut self, inv: Option<&str>) -> Self {
        if let Some(prefix) = inv {
            self.invocation_prefix = Some(prefix.to_string());
        }
        self
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct ResolvedAddress {
    pub invocation_id: String,
    pub job_name: String,
    pub job_dir: std::path::PathBuf,
}

#[allow(dead_code)]
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
                    use crate::output::format::shorthand_from_seconds;
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

#[allow(dead_code)]
fn list_job_names_in_invocation(store: &LogStore, invocation_id: &str) -> Result<Vec<String>> {
    let dirs = store.list_jobs_in_invocation(invocation_id)?;
    Ok(dirs
        .iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect())
}

#[allow(dead_code)]
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
                    background: true,
                    finished_at: None,
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
