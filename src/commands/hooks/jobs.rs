//! `daft hooks jobs` — manage background hook jobs.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::Path;

use crate::coordinator::client::CoordinatorClient;
use crate::coordinator::log_store::{InvocationMeta, JobStatus, LogStore};
use crate::output::Output;
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::output::format::{pad_to_visible_width, shorthand_from_seconds, visible_width};
use crate::output::outline::{self, Body, Node, Outline, Section};
use crate::styles::{
    BOLD, CURRENT_WORKTREE_SYMBOL, CYAN, RESET, blue, bold, dim, dim_underline, green, orange, red,
    yellow,
};
use tabled::{builder::Builder, settings::Style};

/// Format a duration as a compact human-readable string with adaptive
/// precision.
///
/// Negative durations clamp to zero.
///
/// | Range            | Format    | Example   |
/// | ---------------- | --------- | --------- |
/// | `< 1s`           | `Nms`     | `36ms`    |
/// | `< 1min`         | `Ns`      | `12s`     |
/// | `< 1h`           | `MmSs`    | `1m32s`   |
/// | `< 24h`          | `HhMm`    | `1h5m`    |
/// | `>= 24h`         | `DdHh`    | `2d3h`    |
fn format_duration(d: chrono::Duration) -> String {
    let total_ms = d.num_milliseconds().max(0);
    if total_ms < 1000 {
        return format!("{total_ms}ms");
    }
    let total_secs = total_ms / 1000;
    if total_secs < 60 {
        return format!("{total_secs}s");
    }
    if total_secs < 3600 {
        let m = total_secs / 60;
        let s = total_secs % 60;
        return format!("{m}m{s}s");
    }
    if total_secs < 86_400 {
        let h = total_secs / 3600;
        let m = (total_secs % 3600) / 60;
        return format!("{h}h{m}m");
    }
    let days = total_secs / 86_400;
    let h = (total_secs % 86_400) / 3600;
    format!("{days}d{h}h")
}

/// One-line worktree header. The marker is `CURRENT_WORKTREE_SYMBOL` (`">"`)
/// for the current worktree; non-current worktrees pass a single space so
/// the worktree-name column lines up across both.
fn worktree_header(marker: &str, name: &str) -> String {
    format!("{BOLD}{CYAN}{marker} {name}{RESET}")
}

/// Label for an invocation node hanging off the spine. The bullet (`●`) is
/// the outline renderer's responsibility; this helper produces only the
/// label text. `time_ago` is the bare relative duration (e.g. `"2h"`); the
/// helper appends `" ago"` so the rendered text reads `"2h ago"`.
fn invocation_node_label(time_ago: &str, trigger: &str, short_id: &str) -> String {
    format!(
        "{} · {trigger} {}",
        dim(&format!("{time_ago} ago")),
        dim(&format!("[{short_id}]")),
    )
}

#[derive(Parser, Debug)]
#[command(about = "Manage background hook jobs")]
pub struct JobsArgs {
    #[command(subcommand)]
    command: Option<JobsCommand>,

    /// Show jobs across all worktrees.
    #[arg(long, conflicts_with = "worktree")]
    all: bool,

    /// Filter to a specific worktree (can be deleted).
    #[arg(long, conflicts_with = "all")]
    worktree: Option<String>,

    /// Filter to invocations containing jobs with this status.
    #[arg(long)]
    status: Option<String>,

    /// Filter to invocations of this hook type.
    #[arg(long = "hook")]
    hook_filter: Option<String>,

    #[command(flatten)]
    emit: EmitArgs,
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
        /// Only show stdout lines.
        #[arg(long, conflicts_with_all = ["stderr", "status"])]
        stdout: bool,
        /// Only show stderr lines.
        #[arg(long, conflicts_with_all = ["stdout", "status"])]
        stderr: bool,
        /// Only show lifecycle status records (started, finished, signaled).
        #[arg(long, conflicts_with_all = ["stdout", "stderr"])]
        status: bool,
        /// Prefix each line with its `seq` number.
        #[arg(long)]
        seq: bool,
        /// Skip records with `seq < N`.
        #[arg(long = "since-seq")]
        since_seq: Option<u64>,
        /// Skip records older than this duration (`5s`, `2m`, `1h`).
        #[arg(long = "since")]
        since: Option<String>,
        /// Live-tail the job: stream new records as they're produced.
        /// Requires the coordinator to still be running (otherwise the
        /// job's already done and the file read once is exhaustive).
        #[arg(long)]
        follow: bool,
    },
    /// Cancel a running background job.
    Cancel {
        /// Job address (omit for --all or a filter flag).
        job: Option<String>,
        /// Cancel all running jobs in this repo.
        #[arg(long)]
        all: bool,
        /// Invocation ID prefix.
        #[arg(long)]
        inv: Option<String>,
        /// Match jobs whose `hook_type` equals this value (e.g.
        /// `worktree-post-create`).
        #[arg(long)]
        hook: Option<String>,
        /// Match jobs in the given worktree (branch slug).
        #[arg(long)]
        worktree: Option<String>,
        /// Match jobs whose `tags:` (from YAML) contain this label.
        #[arg(long)]
        tag: Option<String>,
        /// Match jobs whose elapsed runtime is at least this duration
        /// (`30s`, `5m`, `1h`, `2d`).
        #[arg(long = "older-than")]
        older_than: Option<String>,
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
    /// Remove old job records (invocations, metadata, logs) past retention.
    Prune {
        /// Override retention for this run (e.g., `30d`, `12h`).
        #[arg(long = "older-than")]
        older_than: Option<String>,
        /// List candidates without removing anything.
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
}

// `JobAddress` + `looks_like_inv_prefix` moved to
// `crate::coordinator::types` so the IPC layer can reference them on the
// wire without a back-pointer into this CLI module.
use crate::coordinator::types::JobAddress;

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
    // Bare invocation prefix with no explicit worktree (`logs 1f2b`) — search
    // every worktree, since the matching invocation may live on a deleted
    // one. The current-worktree default only kicks in when no invocation
    // prefix is supplied (`logs db-migrate`).
    if addr.worktree.is_none() && addr.invocation_prefix.is_some() {
        return resolve_invocation_prefix_anywhere(
            addr.invocation_prefix.as_deref().unwrap(),
            &addr.job_name,
            store,
        );
    }

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
                1 => resolve_within_invocation(matches[0], &addr.job_name, store),
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

/// Resolve to a single job inside a known invocation. Handles the empty
/// `job_name` case (auto-pick for single-job invocations, otherwise error
/// with a `<wt>:<inv>:<job>` candidate list) and the explicit case
/// (existence check + helpful "available jobs" error). Shared between the
/// worktree-scoped and cross-worktree resolvers so disambiguation is
/// uniform regardless of where the invocation was found.
fn resolve_within_invocation(
    inv: &InvocationMeta,
    job_name: &str,
    store: &LogStore,
) -> Result<ResolvedAddress> {
    let short = &inv.invocation_id[..4.min(inv.invocation_id.len())];
    let job_names = list_job_names_in_invocation(store, &inv.invocation_id)?;

    if !job_name.is_empty() {
        let job_dir = store.base_dir.join(&inv.invocation_id).join(job_name);
        if !job_dir.exists() {
            anyhow::bail!(
                "No job named '{job_name}' found in invocation '{short}' (worktree '{}').\nAvailable jobs: {}",
                inv.worktree,
                job_names.join(", "),
            );
        }
        return Ok(ResolvedAddress {
            invocation_id: inv.invocation_id.clone(),
            job_name: job_name.to_string(),
            job_dir,
        });
    }

    match job_names.len() {
        0 => anyhow::bail!(
            "Invocation '{short}' (worktree '{}') has no jobs.",
            inv.worktree,
        ),
        1 => {
            let only = &job_names[0];
            let job_dir = store.base_dir.join(&inv.invocation_id).join(only);
            Ok(ResolvedAddress {
                invocation_id: inv.invocation_id.clone(),
                job_name: only.clone(),
                job_dir,
            })
        }
        _ => {
            let candidates: Vec<String> = job_names
                .iter()
                .map(|n| format!("  {}:{short}:{n}", inv.worktree))
                .collect();
            anyhow::bail!(
                "Invocation '{short}' (worktree '{}') has {} jobs. Pick one:\n{}",
                inv.worktree,
                job_names.len(),
                candidates.join("\n"),
            );
        }
    }
}

/// Resolve an invocation prefix that wasn't scoped to a worktree. Searches
/// every worktree in the log store (so invocations on removed worktrees
/// remain reachable) and delegates to `resolve_within_invocation` for the
/// single-match case.
fn resolve_invocation_prefix_anywhere(
    prefix: &str,
    job_name: &str,
    store: &LogStore,
) -> Result<ResolvedAddress> {
    let all = store.list_invocations()?;
    let matches: Vec<&InvocationMeta> = all
        .iter()
        .filter(|inv| inv.invocation_id.starts_with(prefix))
        .collect();

    match matches.len() {
        0 => anyhow::bail!("No invocation matching prefix '{prefix}'."),
        1 => resolve_within_invocation(matches[0], job_name, store),
        _ => {
            let now = chrono::Utc::now();
            let lines: Vec<String> = matches
                .iter()
                .map(|inv| {
                    let ago = shorthand_from_seconds(
                        now.signed_duration_since(inv.created_at).num_seconds(),
                    );
                    let short = &inv.invocation_id[..4.min(inv.invocation_id.len())];
                    format!(
                        "  {}:{short}  {} -- {ago} ago",
                        inv.worktree, inv.trigger_command,
                    )
                })
                .collect();
            anyhow::bail!(
                "Ambiguous invocation prefix '{prefix}' -- matches across worktrees:\n{}\nUse `<worktree>:<inv>:<job>` to disambiguate.",
                lines.join("\n"),
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
        Some(JobsCommand::Logs {
            ref job,
            ref inv,
            stdout,
            stderr,
            status,
            seq,
            since_seq,
            ref since,
            follow,
        }) => {
            let since_ms = match since {
                Some(s) => Some(
                    crate::coordinator::clean_policy::parse_duration_str(s)
                        .with_context(|| format!("Failed to parse --since duration '{s}'"))?
                        as i64
                        * 1000,
                ),
                None => None,
            };
            let filter = LogsFilter {
                stdout_only: stdout,
                stderr_only: stderr,
                status_only: status,
                show_seq: seq,
                since_seq,
                since_ms_ago: since_ms,
            };
            if follow {
                follow_logs(job, inv.as_deref(), &filter, output)
            } else {
                show_logs(job, inv.as_deref(), &filter, &args, path, output)
            }
        }
        Some(JobsCommand::Cancel {
            ref job,
            all,
            ref inv,
            ref hook,
            ref worktree,
            ref tag,
            ref older_than,
        }) => {
            if cancel_has_filter(
                hook.as_deref(),
                worktree.as_deref(),
                tag.as_deref(),
                older_than.as_deref(),
                inv.as_deref(),
            ) {
                cancel_matching(
                    hook.as_deref(),
                    worktree.as_deref(),
                    tag.as_deref(),
                    inv.as_deref(),
                    older_than.as_deref(),
                    output,
                )
            } else if all {
                cancel_all(path, output)
            } else if let Some(j) = job {
                cancel_job(j, inv.as_deref(), path, output)
            } else {
                anyhow::bail!(
                    "cancel requires a job address, --all, or at least one filter \
                     (--hook/--worktree/--tag/--older-than/--inv)"
                );
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
        Some(JobsCommand::Prune {
            ref older_than,
            dry_run,
        }) => prune_jobs(&args, path, output, older_than.as_deref(), dry_run),
    }
}

/// Whether any of the cancel-filter predicates is set. Extracted so the
/// `--inv`-respects-filter invariant has a dedicated regression test.
fn cancel_has_filter(
    hook: Option<&str>,
    worktree: Option<&str>,
    tag: Option<&str>,
    older_than: Option<&str>,
    inv: Option<&str>,
) -> bool {
    hook.is_some() || worktree.is_some() || tag.is_some() || older_than.is_some() || inv.is_some()
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
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
            && uuid::Uuid::parse_str(name).is_ok()
        {
            hashes.push(name.to_string());
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
        JobStatus::Cancelling => yellow("\u{27f3} cancelling"),
        JobStatus::Cancelled => dim("\u{2014} cancelled"),
        JobStatus::Crashed => red("\u{2717} crashed"),
        JobStatus::Skipped => dim("\u{2014} skipped"),
        JobStatus::Unknown => red("\u{2717} unknown"),
    }
}

/// Index a repo's redb `JobRow`s by `(invocation_id, job_name)` so meta
/// readers can look up Tier-1 status (including `Crashed`, which legacy
/// `meta.json` never sees) without scanning the table per directory.
///
/// Returns `None` when the redb file is unreadable — callers fall back to
/// the legacy `meta.json` reader so pre-upgrade data and pre-redb test
/// fixtures still render.
///
/// Known limitation: redb takes a process-level lock on open
/// (`coordinator::store::tests::concurrent_open_is_rejected_by_redb_lock`).
/// While a coordinator is alive holding the same `coordinator.redb`,
/// this open fails and the caller falls back to `meta.json`. For
/// terminal status (Completed/Failed/Cancelled) that's a no-op — the
/// dual-write reaches both stores. For `Crashed` (only written to redb
/// by `reconcile_active_jobs`), it means the new status is invisible
/// until the live coordinator drains.
fn load_redb_job_meta_index(
    repo_hash: &str,
    log_store_base: &Path,
) -> Option<std::collections::HashMap<(String, String), crate::coordinator::log_store::JobMeta>> {
    let store =
        crate::coordinator::store::JobStore::open_for_repo_base(repo_hash, log_store_base).ok()?;
    let rows = store.list_jobs_for_repo(repo_hash).ok()?;
    Some(
        rows.into_iter()
            .map(|r| ((r.invocation_id.clone(), r.name.clone()), r.to_job_meta()))
            .collect(),
    )
}

/// Look up the meta for a single job directory, preferring the redb row
/// when present. Falls back to the legacy `meta.json` reader so
/// pre-upgrade data still renders and the new `Crashed` reconciliation
/// status is visible to users.
fn read_job_meta_redb_first(
    redb_index: Option<
        &std::collections::HashMap<(String, String), crate::coordinator::log_store::JobMeta>,
    >,
    store: &LogStore,
    invocation_id: &str,
    job_dir: &Path,
) -> Result<crate::coordinator::log_store::JobMeta> {
    if let Some(idx) = redb_index
        && let Some(name) = job_dir.file_name().and_then(|s| s.to_str())
        && let Some(m) = idx.get(&(invocation_id.to_string(), name.to_string()))
    {
        return Ok(m.clone());
    }
    store.read_meta(job_dir)
}

/// Build a flat `Tabular` payload with one row per (invocation, job).
///
/// Each row carries its invocation context (id, short id, worktree, hook type,
/// trigger command, created_at) alongside the job fields — flat so every
/// emit format including tsv/csv/ndjson works.
fn build_jobs_payload(
    invocations: &[InvocationMeta],
    store: &LogStore,
    redb_index: Option<
        &std::collections::HashMap<(String, String), crate::coordinator::log_store::JobMeta>,
    >,
    coordinator_alive: bool,
) -> Result<EmitPayload> {
    let now = chrono::Utc::now();

    let mut table = Table::new([
        "invocation_id",
        "invocation_short",
        "worktree",
        "hook_type",
        "trigger_command",
        "invocation_created_at",
        "name",
        "status",
        "background",
        "started_at",
        "finished_at",
        "duration_secs",
        "exit_code",
        "command",
        "size_bytes",
    ]);

    for inv in invocations {
        let short_id = inv.invocation_id[..4.min(inv.invocation_id.len())].to_string();
        let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;

        for dir in &job_dirs {
            let meta = match read_job_meta_redb_first(redb_index, store, &inv.invocation_id, dir) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let status_str = match &meta.status {
                JobStatus::Running if !coordinator_alive => "running (stale)",
                JobStatus::Running => "running",
                JobStatus::Completed => "completed",
                JobStatus::Failed => "failed",
                JobStatus::Cancelling => "cancelling",
                JobStatus::Cancelled => "cancelled",
                JobStatus::Crashed => "crashed",
                JobStatus::Skipped => "skipped",
                JobStatus::Unknown => "unknown",
            };

            let duration_secs = match (&meta.status, meta.finished_at) {
                (_, Some(finished)) => Some(
                    finished
                        .signed_duration_since(meta.started_at)
                        .num_seconds(),
                ),
                (JobStatus::Running, None) => {
                    Some(now.signed_duration_since(meta.started_at).num_seconds())
                }
                _ => None,
            };

            let size = LogStore::log_path(dir).metadata().map(|m| m.len()).ok();
            let size_cell = size.map(|s| Cell::int(s as i64)).unwrap_or(Cell::Null);

            table = table.row([
                Cell::str(&inv.invocation_id),
                Cell::str(&short_id),
                Cell::str(&inv.worktree),
                Cell::str(&inv.hook_type),
                Cell::str(&inv.trigger_command),
                Cell::str(inv.created_at.to_rfc3339()),
                Cell::str(&meta.name),
                Cell::str(status_str),
                Cell::bool(meta.background),
                Cell::str(meta.started_at.to_rfc3339()),
                meta.finished_at
                    .map(|t| Cell::str(t.to_rfc3339()))
                    .unwrap_or(Cell::Null),
                duration_secs.map(Cell::int).unwrap_or(Cell::Null),
                meta.exit_code
                    .map(|c| Cell::int(c as i64))
                    .unwrap_or(Cell::Null),
                Cell::str(&meta.command),
                size_cell,
            ]);
        }
    }

    Ok(EmitPayload::Tabular(table))
}

/// Pre-rendered cells for one job row in the list-jobs table.
struct JobRow {
    job: String,
    status: String,
    started: String,
    duration: String,
    size: String,
}

/// One invocation worth of data ready for outline rendering.
struct InvocationSection<'a> {
    inv: &'a InvocationMeta,
    rows: Vec<JobRow>,
}

/// Column headers for the per-invocation jobs table.
const LIST_JOBS_HEADERS: [&str; 5] = ["Job", "Status", "Started", "Duration", "Size"];

/// Build the outline `Node` for a single invocation: bullet label + body.
///
/// Body is either a placeholder (`Body::Placeholder`) when the invocation
/// declared no jobs, or a pre-rendered tabled string (`Body::Lines`) padded
/// to the supplied per-column max widths so adjacent invocations line up.
fn build_invocation_node(
    sec: InvocationSection<'_>,
    now: chrono::DateTime<chrono::Utc>,
    max_widths: &[usize; 5],
) -> Node {
    let ago = shorthand_from_seconds(now.signed_duration_since(sec.inv.created_at).num_seconds());
    let short_id = &sec.inv.invocation_id[..4.min(sec.inv.invocation_id.len())];
    let label = invocation_node_label(&ago, &sec.inv.trigger_command, short_id);

    let body = if sec.rows.is_empty() {
        Body::Placeholder(dim("(no jobs declared)"))
    } else {
        let mut builder = Builder::new();
        builder.push_record(
            LIST_JOBS_HEADERS
                .iter()
                .enumerate()
                .map(|(c, h)| pad_to_visible_width(&dim_underline(h), max_widths[c]))
                .collect::<Vec<_>>(),
        );
        for row in &sec.rows {
            let cells = [
                &row.job,
                &row.status,
                &row.started,
                &row.duration,
                &row.size,
            ];
            builder.push_record(
                cells
                    .iter()
                    .enumerate()
                    .map(|(c, cell)| pad_to_visible_width(cell, max_widths[c]))
                    .collect::<Vec<_>>(),
            );
        }
        let mut table = builder.build();
        table.with(Style::blank());
        Body::Lines(table.to_string().lines().map(String::from).collect())
    };

    Node { label, body }
}

/// Default subcommand: list jobs grouped by worktree and invocation.
fn list_jobs(args: &JobsArgs, _path: &Path, output: &mut dyn Output) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let coordinator_alive = is_coordinator_running(&repo_hash);

    let store = LogStore::for_repo(&repo_hash)?;
    let redb_index = load_redb_job_meta_index(&repo_hash, &store.base_dir);
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
                        read_job_meta_redb_first(
                            redb_index.as_ref(),
                            &store,
                            &inv.invocation_id,
                            dir,
                        )
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

    if args.emit.is_structured() {
        let payload =
            build_jobs_payload(&invocations, &store, redb_index.as_ref(), coordinator_alive)?;
        return emit::emit_and_handle("hooks jobs", payload, &args.emit, &mut std::io::stdout())
            .map_err(|e| anyhow::anyhow!("{e}"));
    }

    // Group invocations by worktree.
    let mut groups: std::collections::BTreeMap<String, Vec<&InvocationMeta>> =
        std::collections::BTreeMap::new();
    for inv in &invocations {
        groups.entry(inv.worktree.clone()).or_default().push(inv);
    }

    let now = chrono::Utc::now();

    // ---- Pass 1: collect rows + measure global per-column widths ----
    //
    // Materialize every job row up-front so we can compute per-column max
    // visible width across all invocations in this listing. Without this,
    // adjacent invocations whose Job/Status/Duration values differ in length
    // pick different `tabled` column widths and the rendering drifts.
    let mut sections_by_worktree: Vec<(String, Vec<InvocationSection>)> = Vec::new();

    let mut max_widths: [usize; 5] = LIST_JOBS_HEADERS.map(visible_width);

    for (worktree, inv_list) in &groups {
        let mut secs: Vec<InvocationSection> = Vec::with_capacity(inv_list.len());
        for inv in inv_list {
            let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
            let mut rows: Vec<JobRow> = Vec::with_capacity(job_dirs.len());
            for dir in &job_dirs {
                let Ok(meta) =
                    read_job_meta_redb_first(redb_index.as_ref(), &store, &inv.invocation_id, dir)
                else {
                    continue;
                };
                let icon = if meta.background {
                    blue("\u{21aa}")
                } else {
                    orange("\u{2192}")
                };
                let job_label = format!("{icon} {}", meta.name);
                let status = format_status_inline(&meta.status, coordinator_alive);
                let started = {
                    let local: chrono::DateTime<chrono::Local> = meta.started_at.into();
                    local.format("%H:%M:%S").to_string()
                };
                let duration = match (&meta.status, meta.finished_at) {
                    (_, Some(finished)) => {
                        format_duration(finished.signed_duration_since(meta.started_at))
                    }
                    (JobStatus::Running, None) => format!(
                        "{}...",
                        format_duration(now.signed_duration_since(meta.started_at))
                    ),
                    _ => "\u{2014}".to_string(),
                };
                let size = LogStore::log_path(dir)
                    .metadata()
                    .map(|m| m.len())
                    .unwrap_or(0);
                let size_str = if size == 0 {
                    dim("\u{2014}").to_string()
                } else {
                    format_bytes(size)
                };

                // Update per-column max visible widths.
                for (i, cell) in [&job_label, &status, &started, &duration, &size_str]
                    .iter()
                    .enumerate()
                {
                    let v = visible_width(cell);
                    if v > max_widths[i] {
                        max_widths[i] = v;
                    }
                }

                rows.push(JobRow {
                    job: job_label,
                    status,
                    started,
                    duration,
                    size: size_str,
                });
            }
            secs.push(InvocationSection { inv, rows });
        }
        sections_by_worktree.push((worktree.clone(), secs));
    }

    // ---- Pass 2: build outline + render ----
    //
    // The spine timeline (column-0 │, ● per node, ╰─╴ terminator, gutter
    // widths) lives in `crate::output::outline`. Here we just describe the
    // structure: one section per worktree, one node per invocation, body
    // either pre-rendered table lines or a placeholder string.
    let outline = Outline {
        sections: sections_by_worktree
            .into_iter()
            .map(|(worktree, secs)| {
                let marker = if worktree == current_worktree {
                    CURRENT_WORKTREE_SYMBOL
                } else {
                    " "
                };
                Section {
                    header: worktree_header(marker, &worktree),
                    nodes: secs
                        .into_iter()
                        .map(|sec| build_invocation_node(sec, now, &max_widths))
                        .collect(),
                }
            })
            .collect(),
    };

    outline::render(&outline, |line| output.info(line));

    if args.all
        && let Ok(cache_path) = crate::daft_config_dir().map(|p| p.join("log-clean.json"))
        && let Ok(text) = std::fs::read_to_string(&cache_path)
        && let Ok(cache) = serde_json::from_str::<crate::log_clean::LogCleanCache>(&text)
        && let Some(s) = &cache.last_summary
    {
        let now = chrono::Utc::now().timestamp();
        let age = now - cache.cleaned_at;
        let ago = shorthand_from_seconds(age);
        output.info(&dim(&format!(
            "Last log cleanup {ago} ago: removed {} job log(s) ({} freed)",
            s.removed_jobs,
            format_bytes(s.freed_bytes),
        )));
        output.info("");
    }

    Ok(())
}

/// Predicates for the `daft hooks jobs logs` reader. Filters and rendering
/// flags applied to each `LogRecord` read from `output.jsonl`.
#[derive(Debug, Clone, Default)]
struct LogsFilter {
    stdout_only: bool,
    stderr_only: bool,
    status_only: bool,
    show_seq: bool,
    since_seq: Option<u64>,
    /// Lower bound on `LogRecord.ts` (unix ms). Records older than this are
    /// dropped.
    since_ms_ago: Option<i64>,
}

impl LogsFilter {
    fn accepts(&self, record: &crate::coordinator::log_record::LogRecord) -> bool {
        use crate::coordinator::log_record::LogRecordKind;
        if let Some(min_seq) = self.since_seq
            && record.seq < min_seq
        {
            return false;
        }
        if let Some(ago_ms) = self.since_ms_ago {
            let cutoff = chrono::Utc::now().timestamp_millis() - ago_ms;
            if record.ts < cutoff {
                return false;
            }
        }
        match &record.kind {
            LogRecordKind::Stdout(_) => !(self.stderr_only || self.status_only),
            LogRecordKind::Stderr(_) => !(self.stdout_only || self.status_only),
            LogRecordKind::Status(_) => !(self.stdout_only || self.stderr_only),
        }
    }

    fn format(&self, record: &crate::coordinator::log_record::LogRecord) -> String {
        use crate::coordinator::log_record::{LogRecordKind, StatusEvent};
        let body = match &record.kind {
            LogRecordKind::Stdout(s) => s.clone(),
            LogRecordKind::Stderr(s) => s.clone(),
            LogRecordKind::Status(event) => match event {
                StatusEvent::Started { pid } => format!("[status] started pid={pid}"),
                StatusEvent::Finished { exit_code } => {
                    let code = exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "?".into());
                    format!("[status] finished exit_code={code}")
                }
                StatusEvent::Signaled { signal } => format!("[status] signaled signal={signal}"),
                StatusEvent::Crashed { message } => format!("[status] crashed message={message}"),
            },
        };
        if self.show_seq {
            format!("{:>6}\t{body}", record.seq)
        } else {
            body
        }
    }
}

/// Render a job's log file into `buf` with `filter` applied.
///
/// Tries `output.jsonl` first (the new structured format). Falls back to
/// `output.log` if jsonl is absent — pre-upgrade data, treated as
/// `Stdout`-only synthetic records with `seq = line_number` and
/// `ts = mtime` (best-effort).
fn render_job_log(
    job_dir: &std::path::Path,
    filter: &LogsFilter,
    buf: &mut String,
) -> Result<bool> {
    use crate::coordinator::log_record::{LogRecord, LogRecordKind};

    let jsonl_path = LogStore::jsonl_path(job_dir);
    if jsonl_path.exists() {
        let file = std::fs::File::open(&jsonl_path)
            .with_context(|| format!("Failed to open log file: {}", jsonl_path.display()))?;
        let reader = std::io::BufReader::new(file);
        let mut wrote_any = false;
        for line in std::io::BufRead::lines(reader) {
            let line = line?;
            if line.is_empty() {
                continue;
            }
            let record: LogRecord =
                serde_json::from_str(&line).with_context(|| "Failed to parse JSONL log record")?;
            if !filter.accepts(&record) {
                continue;
            }
            buf.push_str(&filter.format(&record));
            buf.push('\n');
            wrote_any = true;
        }
        return Ok(wrote_any);
    }

    // Legacy fallback: read `output.log` as raw stdout-only records.
    // No timestamps, so `--since` cannot filter (we treat all lines as
    // "now"); `seq` is the line index.
    let legacy = LogStore::log_path(job_dir);
    if !legacy.exists() {
        return Ok(false);
    }
    let contents = std::fs::read_to_string(&legacy)
        .with_context(|| format!("Failed to read legacy log: {}", legacy.display()))?;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut wrote_any = false;
    for (idx, line) in contents.lines().enumerate() {
        let record = LogRecord {
            seq: idx as u64,
            ts: now_ms,
            kind: LogRecordKind::Stdout(line.to_string()),
        };
        if !filter.accepts(&record) {
            continue;
        }
        buf.push_str(&filter.format(&record));
        buf.push('\n');
        wrote_any = true;
    }
    Ok(wrote_any)
}

/// Live-tail a job's structured log via the coordinator's `TailLogs`
/// streaming endpoint.
///
/// Strategy (per-invocation coordinator lifecycle):
/// - If the coordinator is reachable on the per-repo socket, open a
///   `TailLogs { follow: true }` stream and write each `LogRecord` payload
///   to stdout as the server emits frames. The server-side handler stops
///   at the terminal `Status::Finished/Signaled/Crashed` record, so the
///   client iteration ends naturally.
/// - If the coordinator is *not* reachable, the job has already finished —
///   fall back to a one-shot file read so `--follow` still produces the
///   complete log instead of failing with "no coordinator".
fn follow_logs(
    job: &str,
    inv: Option<&str>,
    filter: &LogsFilter,
    output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let addr = JobAddress::parse(job).with_inv_override(inv);

    #[cfg(unix)]
    if let Some(mut client) = CoordinatorClient::connect(&repo_hash)? {
        return follow_logs_via_coordinator(&mut client, addr, filter);
    }

    // Coordinator unreachable (or non-Unix where there is no IPC) → the
    // job is final. Read once and exit.
    output.info("No coordinator running for this repository; reading log file once.");
    let store = LogStore::for_repo(&repo_hash)?;
    let current_worktree = crate::core::repo::get_current_branch().unwrap_or_default();
    let resolved = resolve_job_address(&addr, &store, &current_worktree)?;
    let mut buf = String::new();
    render_job_log(&resolved.job_dir, filter, &mut buf)?;
    print!("{buf}");
    Ok(())
}

#[cfg(unix)]
fn follow_logs_via_coordinator(
    client: &mut CoordinatorClient,
    addr: JobAddress,
    filter: &LogsFilter,
) -> Result<()> {
    use crate::coordinator::log_record::LogRecord;

    let stream = client.tail_logs(addr, true, filter.since_seq)?;
    for frame in stream {
        let resp = frame?;
        match resp {
            crate::coordinator::CoordinatorResponse::StreamFrame(value) => {
                let record: LogRecord = serde_json::from_value(value)?;
                if !filter.accepts(&record) {
                    continue;
                }
                println!("{}", filter.format(&record));
            }
            crate::coordinator::CoordinatorResponse::Error { message, .. } => {
                anyhow::bail!(message);
            }
            _ => break,
        }
    }
    Ok(())
}

/// Show the output log for a job, resolved via address.
fn show_logs(
    job: &str,
    inv: Option<&str>,
    filter: &LogsFilter,
    _args: &JobsArgs,
    _path: &Path,
    _output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;
    let store = LogStore::for_repo(&repo_hash)?;
    let redb_index = load_redb_job_meta_index(&repo_hash, &store.base_dir);
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
        render_invocation_logs(
            &store,
            redb_index.as_ref(),
            &invocation_id,
            filter,
            &mut buf,
        )?;
    } else {
        let resolved = resolve_job_address(&addr, &store, &current_worktree)?;
        render_single_job_log(&store, redb_index.as_ref(), &resolved, filter, &mut buf)?;
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
    redb_index: Option<
        &std::collections::HashMap<(String, String), crate::coordinator::log_store::JobMeta>,
    >,
    resolved: &ResolvedAddress,
    filter: &LogsFilter,
    buf: &mut String,
) -> Result<()> {
    use std::fmt::Write;

    let meta = read_job_meta_redb_first(
        redb_index,
        store,
        &resolved.invocation_id,
        &resolved.job_dir,
    )?;
    let inv_meta = store.read_invocation_meta(&resolved.invocation_id).ok();

    let now = chrono::Utc::now();
    let short_id = &resolved.invocation_id[..4.min(resolved.invocation_id.len())];

    let status_label = match meta.status {
        JobStatus::Completed => green("COMPLETED"),
        JobStatus::Failed => red("FAILED"),
        JobStatus::Running => yellow("RUNNING"),
        JobStatus::Cancelling => yellow("CANCELLING"),
        JobStatus::Cancelled => dim("CANCELLED"),
        JobStatus::Crashed => red("CRASHED"),
        JobStatus::Skipped => dim("SKIPPED"),
        JobStatus::Unknown => red("UNKNOWN"),
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
        Some(finished) => format_duration(finished.signed_duration_since(meta.started_at)),
        None => "\u{2014}".to_string(),
    };
    writeln!(buf, "duration:  {duration_str}")?;

    if !meta.command.is_empty() {
        writeln!(buf, "command:   {}", meta.command)?;
    }

    writeln!(buf)?;
    writeln!(buf, "{}", dim("--- output ---"))?;
    let wrote = render_job_log(&resolved.job_dir, filter, buf)?;
    if !wrote {
        writeln!(buf, "{}", dim("(no output)"))?;
    } else {
        let jsonl = LogStore::jsonl_path(&resolved.job_dir);
        if jsonl.exists() {
            writeln!(buf)?;
            writeln!(buf, "Full log: {}", jsonl.display())?;
        }
    }

    Ok(())
}

/// Render all job logs for a single invocation into `buf`.
fn render_invocation_logs(
    store: &LogStore,
    redb_index: Option<
        &std::collections::HashMap<(String, String), crate::coordinator::log_store::JobMeta>,
    >,
    invocation_id: &str,
    filter: &LogsFilter,
    buf: &mut String,
) -> Result<()> {
    use std::fmt::Write;

    let inv_meta = store.read_invocation_meta(invocation_id)?;
    let short_id = &invocation_id[..4.min(invocation_id.len())];
    let now = chrono::Utc::now();

    // Collect job metas sorted by started_at.
    let job_dirs = store.list_jobs_in_invocation(invocation_id)?;
    let mut jobs: Vec<(std::path::PathBuf, crate::coordinator::log_store::JobMeta)> = job_dirs
        .into_iter()
        .filter_map(|dir| {
            read_job_meta_redb_first(redb_index, store, invocation_id, &dir)
                .ok()
                .map(|m| (dir, m))
        })
        .collect();
    jobs.sort_by_key(|a| a.1.started_at);

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
            JobStatus::Cancelling => yellow("CANCELLING"),
            JobStatus::Cancelled => dim("CANCELLED"),
            JobStatus::Crashed => red("CRASHED"),
            JobStatus::Skipped => dim("SKIPPED"),
            JobStatus::Unknown => red("UNKNOWN"),
        };

        let duration_str = match meta.finished_at {
            Some(finished) => format_duration(finished.signed_duration_since(meta.started_at)),
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

        let wrote = render_job_log(dir, filter, buf)?;
        if !wrote {
            writeln!(buf, "{}", dim("(empty)"))?;
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

/// Cancel every active job matching the supplied predicates. AND-combined;
/// requires the running coordinator (the signaling side). Returns the list
/// of cancelled job names.
fn cancel_matching(
    hook: Option<&str>,
    worktree: Option<&str>,
    tag: Option<&str>,
    invocation_prefix: Option<&str>,
    older_than: Option<&str>,
    output: &mut dyn Output,
) -> Result<()> {
    let repo_hash = crate::core::repo_identity::compute_repo_id()?;

    let older_than_secs = match older_than {
        Some(s) => Some(
            crate::coordinator::clean_policy::parse_duration_str(s)
                .with_context(|| format!("Failed to parse --older-than duration '{s}'"))?,
        ),
        None => None,
    };

    match CoordinatorClient::connect(&repo_hash)? {
        Some(mut client) => {
            let names =
                client.cancel_matching(hook, worktree, tag, invocation_prefix, older_than_secs)?;
            if names.is_empty() {
                output.info("No active jobs matched the filter.");
            } else {
                output.success(&format!(
                    "Cancelled {} job(s): {}",
                    names.len(),
                    names.join(", ")
                ));
            }
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
                    if let Ok(meta) = store.read_meta(dir)
                        && meta.name == *name
                        && matches!(meta.status, JobStatus::Failed | JobStatus::Cancelled)
                    {
                        return Ok(inv.clone());
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
                if let Some(flag_wt) = worktree_flag
                    && flag_wt != wt.as_str()
                {
                    anyhow::bail!(
                        "Conflicting worktree: --worktree says '{}' but address says '{}'.",
                        flag_wt,
                        wt
                    );
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

    // NOTE: We deliberately do NOT call `write_repo_policy` here. The retry
    // path reconstructs JobSpecs from stored JobMeta via `build_retry_set`,
    // which has no access to `log_config` (it isn't persisted). Writing here
    // would build a defaults-only `RepoPolicy` and clobber the policy the
    // originating hook fire already captured. The originating hook fire's
    // sidecar is the source of truth for cleanup; retries reuse it.

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

        // `daft hooks jobs run` retries a previously-recorded invocation; the
        // hook-box title here shows the trigger command, not a worktree, so
        // no `on:` segment is needed.
        presenter.on_phase_start(&trigger_command, None);
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
            crate::coordinator::process::spawn_coordinator(coord_state, bg_store)?;
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

/// Remove old job records (invocations + metadata + logs) past retention.
///
/// Supports `--older-than <duration>` to override per-job retention for this
/// run only, and `--dry-run` to list candidates without removing anything.
fn prune_jobs(
    args: &JobsArgs,
    _path: &Path,
    output: &mut dyn Output,
    older_than: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    use crate::coordinator::clean_policy::{CleanPolicy, CleanSummary, parse_duration_str};

    let retention_override = match older_than {
        None => None,
        Some(s) => {
            let n = parse_duration_str(s)?;
            let i = i64::try_from(n)
                .map_err(|_| anyhow::anyhow!("--older-than value too large: {s}"))?;
            Some(chrono::Duration::try_seconds(i).ok_or_else(|| {
                anyhow::anyhow!("--older-than value caused duration overflow: {s}")
            })?)
        }
    };

    let process_one = |repo_hash: &str, store: &LogStore| -> Result<CleanSummary> {
        let repo_policy = match crate::coordinator::store::JobStore::open_for_repo_base(
            repo_hash,
            &store.base_dir,
        ) {
            Ok(js) => js
                .read_repo_policy(repo_hash)
                .unwrap_or_else(|_| crate::coordinator::clean_policy::RepoPolicy::defaults()),
            Err(_) => crate::coordinator::clean_policy::RepoPolicy::defaults(),
        };
        let policy = CleanPolicy {
            retention_override,
            dry_run,
            repo_policy: repo_policy.clone(),
            ..CleanPolicy::default()
        };

        // Pass 1: truncation pre-pass (skipped in dry-run since the spec
        // describes truncation as side-effecting; dry-run mode shouldn't
        // touch disk for any pass).
        let truncated = if dry_run {
            0
        } else {
            store.truncate_oversized_logs(None).unwrap_or(0)
        };

        // Pass 2: retention sweep.
        let mut summary = store.clean(&policy)?;
        summary.truncated_logs += truncated;

        // Pass 3: budget post-pass (also skipped in dry-run).
        if !dry_run {
            let bo = store.enforce_budget(&repo_policy).unwrap_or_default();
            summary.removed_invocations += bo.evicted_invocations;
            summary.removed_jobs += bo.freed_jobs;
            summary.freed_bytes += bo.freed_bytes;
        }

        Ok(summary)
    };

    if args.all {
        let hashes = list_all_repo_hashes()?;
        let mut total_jobs = 0;
        let mut total_invs = 0;
        let mut total_bytes = 0u64;
        let mut total_truncated = 0u64;
        let mut all_candidates: Vec<(String, String, String)> = Vec::new();
        for hash in &hashes {
            let store = LogStore::for_repo(hash)?;
            let s = process_one(hash, &store)?;
            total_jobs += s.removed_jobs;
            total_invs += s.removed_invocations;
            total_bytes += s.freed_bytes;
            total_truncated += s.truncated_logs as u64;
            let short_repo = &hash[..8.min(hash.len())];
            for (wt, inv, name) in s.candidates {
                all_candidates.push((format!("{short_repo}/{wt}"), inv, name));
            }
        }
        if dry_run {
            print_dry_run_summary(output, total_invs, total_jobs, total_bytes, &all_candidates);
        } else if total_jobs > 0 || total_truncated > 0 {
            let mut msg = format!(
                "Removed {total_jobs} job(s) across {total_invs} invocation(s), freed {} across all repos",
                format_bytes(total_bytes),
            );
            if total_truncated > 0 {
                msg.push_str(&format!(", truncated {total_truncated} log(s)"));
            }
            msg.push('.');
            output.success(&msg);
        } else {
            output.info("No old logs to clean.");
        }
    } else {
        let repo_hash = crate::core::repo_identity::compute_repo_id()?;
        let store = LogStore::for_repo(&repo_hash)?;
        let s = process_one(&repo_hash, &store)?;
        if dry_run {
            print_dry_run_summary(
                output,
                s.removed_invocations,
                s.removed_jobs,
                s.freed_bytes,
                &s.candidates,
            );
        } else if s.removed_jobs > 0 || s.truncated_logs > 0 {
            let mut msg = format!(
                "Removed {} job(s) ({} freed)",
                s.removed_jobs,
                format_bytes(s.freed_bytes),
            );
            if s.truncated_logs > 0 {
                msg.push_str(&format!(", truncated {} log(s)", s.truncated_logs));
            }
            msg.push('.');
            output.success(&msg);
        } else {
            output.info("No old logs to clean.");
        }
    }

    Ok(())
}

fn print_dry_run_summary(
    output: &mut dyn Output,
    invs: usize,
    jobs: usize,
    bytes: u64,
    candidates: &[(String, String, String)],
) {
    if jobs == 0 && invs == 0 {
        output.info("No candidates for removal.");
        return;
    }
    output.info(&format!(
        "Would remove {jobs} job(s) across {invs} invocation(s) ({} would be freed):",
        format_bytes(bytes),
    ));
    for (worktree, inv_id, name) in candidates {
        let short = &inv_id[..4.min(inv_id.len())];
        output.info(&format!("  {worktree}  [{short}]  {name}"));
    }
}

fn format_bytes(n: u64) -> String {
    if n >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_inv_alone_counts_as_filter() {
        // Regression: `daft hooks jobs cancel --inv abc123` previously fell
        // through to the bail! because `--inv` wasn't included in the
        // has_filter set. It should now drive cancel_matching.
        assert!(cancel_has_filter(None, None, None, None, Some("abc123")));
    }

    #[test]
    fn cancel_no_predicates_is_not_a_filter() {
        assert!(!cancel_has_filter(None, None, None, None, None));
    }

    #[test]
    fn cancel_other_predicates_still_count() {
        assert!(cancel_has_filter(
            Some("worktree-post-create"),
            None,
            None,
            None,
            None
        ));
        assert!(cancel_has_filter(None, Some("feat/x"), None, None, None));
        assert!(cancel_has_filter(None, None, Some("build"), None, None));
        assert!(cancel_has_filter(None, None, None, Some("5m"), None));
    }

    #[test]
    fn format_duration_sub_second_uses_milliseconds() {
        assert_eq!(format_duration(chrono::Duration::milliseconds(0)), "0ms");
        assert_eq!(format_duration(chrono::Duration::milliseconds(36)), "36ms");
        assert_eq!(
            format_duration(chrono::Duration::milliseconds(999)),
            "999ms"
        );
    }

    #[test]
    fn format_duration_seconds_drop_milliseconds() {
        assert_eq!(format_duration(chrono::Duration::milliseconds(1000)), "1s");
        assert_eq!(format_duration(chrono::Duration::milliseconds(1999)), "1s");
        assert_eq!(format_duration(chrono::Duration::seconds(12)), "12s");
        assert_eq!(format_duration(chrono::Duration::seconds(59)), "59s");
    }

    #[test]
    fn format_duration_minutes_include_seconds_remainder() {
        assert_eq!(format_duration(chrono::Duration::seconds(60)), "1m0s");
        assert_eq!(format_duration(chrono::Duration::seconds(92)), "1m32s");
        assert_eq!(format_duration(chrono::Duration::seconds(3599)), "59m59s");
    }

    #[test]
    fn format_duration_hours_include_minutes_remainder() {
        assert_eq!(format_duration(chrono::Duration::seconds(3600)), "1h0m");
        assert_eq!(format_duration(chrono::Duration::seconds(3900)), "1h5m");
        assert_eq!(format_duration(chrono::Duration::seconds(86_399)), "23h59m");
    }

    #[test]
    fn format_duration_days_include_hours_remainder() {
        assert_eq!(format_duration(chrono::Duration::seconds(86_400)), "1d0h");
        assert_eq!(format_duration(chrono::Duration::seconds(183_600)), "2d3h");
    }

    #[test]
    fn format_duration_negative_clamps_to_zero() {
        assert_eq!(format_duration(chrono::Duration::seconds(-5)), "0ms");
        assert_eq!(format_duration(chrono::Duration::milliseconds(-1)), "0ms");
    }

    #[test]
    fn read_job_meta_redb_first_prefers_redb_when_index_has_row() {
        use crate::coordinator::log_store::{JobMeta, JobStatus};
        use std::collections::HashMap;
        let tmp = tempfile::TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let inv = "inv1";
        let name = "build";
        let job_dir = store.create_job_dir(inv, name).unwrap();

        // Seed a stale meta.json claiming "Running".
        let stale = JobMeta::skipped(name, "worktree-post-create", "feat/test", "", true, vec![]);
        let stale = JobMeta {
            status: JobStatus::Running,
            ..stale
        };
        store.write_meta(&job_dir, &stale).unwrap();

        // The redb index reports Crashed (the post-reconciliation state).
        let mut idx = HashMap::new();
        let redb_meta = JobMeta {
            status: JobStatus::Crashed,
            ..stale.clone()
        };
        idx.insert((inv.to_string(), name.to_string()), redb_meta);

        let got =
            read_job_meta_redb_first(Some(&idx), &store, inv, &job_dir).expect("read meta ok");
        assert!(matches!(got.status, JobStatus::Crashed));
    }

    #[test]
    fn read_job_meta_redb_first_falls_back_to_meta_json_when_redb_silent() {
        use crate::coordinator::log_store::{JobMeta, JobStatus};
        use std::collections::HashMap;
        let tmp = tempfile::TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let inv = "inv1";
        let name = "build";
        let job_dir = store.create_job_dir(inv, name).unwrap();

        let meta = JobMeta {
            status: JobStatus::Completed,
            ..JobMeta::skipped(name, "worktree-post-create", "feat/x", "", true, vec![])
        };
        store.write_meta(&job_dir, &meta).unwrap();

        let empty_idx: HashMap<(String, String), JobMeta> = HashMap::new();
        let got = read_job_meta_redb_first(Some(&empty_idx), &store, inv, &job_dir).unwrap();
        assert!(matches!(got.status, JobStatus::Completed));

        // None index (redb file missing entirely) also falls back.
        let got2 = read_job_meta_redb_first(None, &store, inv, &job_dir).unwrap();
        assert!(matches!(got2.status, JobStatus::Completed));
    }

    #[test]
    fn print_dry_run_summary_emits_with_invs_only() {
        let mut output = crate::output::TestOutput::new();
        print_dry_run_summary(&mut output, 2, 0, 0, &[]);
        assert!(
            output.has_info("Would remove 0 job(s) across 2 invocation(s)"),
            "expected summary when invs > 0 even if jobs == 0; got infos: {:?}",
            output.infos(),
        );
    }

    #[test]
    fn print_dry_run_summary_silent_when_both_zero() {
        let mut output = crate::output::TestOutput::new();
        print_dry_run_summary(&mut output, 0, 0, 0, &[]);
        assert!(output.has_info("No candidates for removal."));
    }

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
    fn test_parse_two_segment_worktree_inv_when_right_is_hex() {
        // `feature:1f2b` → worktree drill-down with empty job name. The
        // resolver auto-picks single-job invocations or prints candidates.
        let addr = JobAddress::parse("feature:1f2b");
        assert_eq!(addr.worktree.as_deref(), Some("feature"));
        assert_eq!(addr.invocation_prefix.as_deref(), Some("1f2b"));
        assert_eq!(addr.job_name, "");
    }

    #[test]
    fn test_parse_two_segment_worktree_job_when_neither_side_is_hex() {
        // `feature:db-migrate` — neither side hex, no slash → worktree:job.
        // Real invocations are hex-only, so the left can only sensibly
        // be a worktree name in this case.
        let addr = JobAddress::parse("feature:db-migrate");
        assert_eq!(addr.worktree.as_deref(), Some("feature"));
        assert!(addr.invocation_prefix.is_none());
        assert_eq!(addr.job_name, "db-migrate");
    }

    #[test]
    fn test_parse_two_segment_hex_left_still_invocation_job() {
        // `c9d4:db-migrate` — hex left wins as invocation prefix even when
        // the new heuristics are enabled, preserving existing semantics.
        let addr = JobAddress::parse("c9d4:db-migrate");
        assert!(addr.worktree.is_none());
        assert_eq!(addr.invocation_prefix.as_deref(), Some("c9d4"));
        assert_eq!(addr.job_name, "db-migrate");
    }

    #[test]
    fn test_parse_bare_hex_token_is_invocation_prefix() {
        // 4-char hex (the most common short-id length) — invocation prefix.
        let addr = JobAddress::parse("1f2b");
        assert!(addr.worktree.is_none());
        assert_eq!(addr.invocation_prefix.as_deref(), Some("1f2b"));
        assert_eq!(addr.job_name, "");
    }

    #[test]
    fn test_parse_bare_2char_and_8char_hex_are_invocation_prefix() {
        // Both ends of the [2,8] hex range route to invocation prefix —
        // matches the boundary used by `retry` (`test_retry_target_8char_hex_is_invocation`).
        for token in &["ab", "abcdef12"] {
            let addr = JobAddress::parse(token);
            assert_eq!(
                addr.invocation_prefix.as_deref(),
                Some(*token),
                "expected `{token}` to parse as invocation prefix"
            );
            assert_eq!(addr.job_name, "");
        }
    }

    #[test]
    fn test_parse_bare_9char_hex_is_still_job_name() {
        // 9+ chars escapes the invocation-prefix heuristic — locks the
        // boundary against future drift (mirrors `test_retry_target_9char_hex_is_job`).
        let addr = JobAddress::parse("abcdef123");
        assert!(addr.invocation_prefix.is_none());
        assert_eq!(addr.job_name, "abcdef123");
    }

    #[test]
    fn test_parse_bare_non_hex_short_token_is_job_name() {
        // A 2-char token containing non-hex characters stays a job name.
        let addr = JobAddress::parse("go");
        assert!(addr.invocation_prefix.is_none());
        assert_eq!(addr.job_name, "go");
    }

    #[test]
    fn test_resolve_bare_invocation_prefix_finds_in_other_worktree() {
        use crate::coordinator::log_store::{InvocationMeta, JobMeta, JobStatus, LogStore};
        use std::collections::HashMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        // The invocation lives on `feature` (deleted worktree); current
        // worktree is `master`. Bare prefix `1f2b` must reach across.
        let inv_id = "1f2b000000000000";
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature".to_string(),
            created_at: now,
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        let dir = store.create_job_dir(inv_id, "warm-build").unwrap();
        let meta = JobMeta {
            name: "warm-build".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature".to_string(),
            command: "echo".to_string(),
            working_dir: "/tmp".to_string(),
            env: HashMap::new(),
            started_at: now,
            status: JobStatus::Failed,
            exit_code: Some(1),
            pid: None,
            background: true,
            finished_at: Some(now),
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();

        let addr = JobAddress::parse("1f2b");
        let resolved = resolve_job_address(&addr, &store, "master").unwrap();
        assert_eq!(resolved.invocation_id, inv_id);
        assert_eq!(resolved.job_name, "warm-build");
    }

    #[test]
    fn test_resolve_two_segment_worktree_inv_routes_to_disambiguation() {
        // `feature:1f2b` with multiple jobs in the invocation must produce
        // the same `<wt>:<inv>:<job>` candidate list as the bare `1f2b`
        // path — both flow through `resolve_within_invocation`.
        use crate::coordinator::log_store::{InvocationMeta, JobMeta, JobStatus, LogStore};
        use std::collections::HashMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        let inv_id = "1f2b000000000000";
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature".to_string(),
            created_at: now,
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        for job in &["install", "warm-build"] {
            let dir = store.create_job_dir(inv_id, job).unwrap();
            let meta = JobMeta {
                name: (*job).to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: "feature".to_string(),
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
                retention_seconds: None,
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
            };
            store.write_meta(&dir, &meta).unwrap();
        }

        let addr = JobAddress::parse("feature:1f2b");
        let err = resolve_job_address(&addr, &store, "master").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("feature:1f2b:install") && msg.contains("feature:1f2b:warm-build"),
            "expected `<wt>:<inv>:<job>` candidates in error, got: {msg}"
        );
    }

    #[test]
    fn test_resolve_bare_invocation_prefix_with_multiple_jobs_errors_with_candidates() {
        use crate::coordinator::log_store::{InvocationMeta, JobMeta, JobStatus, LogStore};
        use std::collections::HashMap;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        let inv_id = "1f2b000000000000";
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature".to_string(),
            created_at: now,
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        for job in &["warm-build", "db-seed"] {
            let dir = store.create_job_dir(inv_id, job).unwrap();
            let meta = JobMeta {
                name: (*job).to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: "feature".to_string(),
                command: "echo".to_string(),
                working_dir: "/tmp".to_string(),
                env: HashMap::new(),
                started_at: now,
                status: JobStatus::Failed,
                exit_code: Some(1),
                pid: None,
                background: true,
                finished_at: Some(now),
                needs: vec![],
                retention_seconds: None,
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
            };
            store.write_meta(&dir, &meta).unwrap();
        }

        let addr = JobAddress::parse("1f2b");
        let err = resolve_job_address(&addr, &store, "master").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("feature:1f2b:warm-build") && msg.contains("feature:1f2b:db-seed"),
            "expected `<wt>:<inv>:<job>` candidates in error, got: {msg}"
        );
        assert!(
            msg.contains("Pick one"),
            "expected `Pick one` prompt: {msg}"
        );
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
                retention_seconds: None,
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
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
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
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
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
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
    #[serial_test::serial]
    fn list_all_repo_hashes_filters_non_uuid_dirs() {
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        // DAFT_STATE_DIR is process-global; lib.rs tests also mutate it.
        // The #[serial] attribute above prevents cross-test interference.
        unsafe {
            std::env::set_var("DAFT_STATE_DIR", tmp.path());
        }
        let jobs_dir = tmp.path().join("jobs");
        std::fs::create_dir_all(&jobs_dir).unwrap();

        // One valid UUID-named dir, one legacy 16-hex-char name.
        let uuid_name = "01900000-0000-7000-8000-000000000000";
        std::fs::create_dir(jobs_dir.join(uuid_name)).unwrap();
        std::fs::create_dir(jobs_dir.join("019d12345678abcd")).unwrap();

        let hashes = list_all_repo_hashes().unwrap();
        assert_eq!(hashes, vec![uuid_name.to_string()]);

        unsafe {
            std::env::remove_var("DAFT_STATE_DIR");
        }
    }

    #[test]
    fn format_bytes_handles_all_ranges() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1500), "1.5 KB");
        assert_eq!(format_bytes(1500 * 1024), "1.5 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
    }

    #[test]
    fn clean_older_than_rejects_overflow() {
        use crate::coordinator::clean_policy::parse_duration_str;
        // u64::MAX seconds overflows i64.
        let n = parse_duration_str("18446744073709551615s");
        // The parser itself rejects overflow at the multiplier step, so this
        // would already error. The defensive layering in prune_jobs catches
        // any value that gets past the parser.
        assert!(n.is_err() || i64::try_from(n.unwrap()).is_err());
    }

    #[test]
    fn worktree_header_renders_marker_then_space_then_name() {
        assert_eq!(
            worktree_header(">", "feature/tax-calc"),
            format!("{BOLD}{CYAN}> feature/tax-calc{RESET}"),
        );
        // Non-current worktrees pass " " as the marker → two leading spaces.
        assert_eq!(
            worktree_header(" ", "main"),
            format!("{BOLD}{CYAN}  main{RESET}"),
        );
    }

    #[test]
    fn invocation_node_label_omits_bullet_and_dims_time_and_id() {
        let rendered = invocation_node_label("2h", "worktree-post-create", "c9d4");
        // The bullet is now the outline renderer's responsibility — the
        // label must not contain it.
        assert!(
            !rendered.contains('\u{25cf}'),
            "label should not contain bullet: {rendered:?}"
        );
        assert_eq!(
            rendered,
            format!("{} · worktree-post-create {}", dim("2h ago"), dim("[c9d4]"),),
        );
    }

    #[test]
    fn pad_helper_equalizes_visible_width_for_two_status_cells() {
        use crate::output::format::{pad_to_visible_width, strip_ansi};

        // Two cells that would naturally produce different column widths.
        let short = "\x1b[31m\u{2717} failed\x1b[0m"; // visible: 8
        let long = "\x1b[33m\u{27f3} running (stale)\x1b[0m"; // visible: 17

        let target = strip_ansi(short)
            .chars()
            .count()
            .max(strip_ansi(long).chars().count());

        let padded_short = pad_to_visible_width(short, target);
        let padded_long = pad_to_visible_width(long, target);

        assert_eq!(strip_ansi(&padded_short).chars().count(), target);
        assert_eq!(strip_ansi(&padded_long).chars().count(), target);
    }
}
