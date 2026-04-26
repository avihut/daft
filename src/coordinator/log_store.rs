use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMeta {
    pub name: String,
    pub hook_type: String,
    pub worktree: String,
    pub command: String,
    pub working_dir: String,
    pub env: HashMap<String, String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
    pub background: bool,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub needs: Vec<String>,
    /// Retention captured at hook-fire time. None = use repo default.
    #[serde(default)]
    pub retention_seconds: Option<i64>,
    /// Per-log size cap captured at hook-fire time. None = use repo default.
    #[serde(default)]
    pub max_log_size_bytes: Option<u64>,
    /// True if `output.log` has been truncated by a cleanup pass.
    #[serde(default)]
    pub log_truncated: bool,
    /// Original size in bytes before truncation, if `log_truncated == true`.
    #[serde(default)]
    pub original_size_bytes: Option<u64>,
}

impl JobMeta {
    pub fn skipped(
        name: &str,
        hook_type: &str,
        worktree: &str,
        command: &str,
        background: bool,
        needs: Vec<String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            hook_type: hook_type.to_string(),
            worktree: worktree.to_string(),
            command: command.to_string(),
            working_dir: String::new(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Skipped,
            exit_code: None,
            pid: None,
            background,
            finished_at: None,
            needs,
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationMeta {
    pub invocation_id: String,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Hash `(nanos, pid)` into a 16-hex-char invocation ID.
///
/// Earlier versions formatted the raw millisecond timestamp as `{ts:012x}` and
/// used the first 4 chars as a short ID. The top 16 bits of a ms timestamp
/// flip only every ~3 days, so every invocation inside that window collided on
/// the prefix. Hashing spreads entropy uniformly across the ID, so the first
/// 4 hex chars (and any other prefix) reliably discriminate between distinct
/// inputs.
pub fn invocation_id_from_seed(nanos: u128, pid: u32) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;
    let mut h = DefaultHasher::new();
    h.write_u128(nanos);
    h.write_u32(pid);
    format!("{:016x}", h.finish())
}

/// Generate a unique invocation ID for the current process.
///
/// Seeds from nanosecond-resolution timestamp and PID, then hashes to a
/// 16-hex-char string. Leading prefixes are collision-resistant for
/// display-shortening purposes.
pub fn generate_invocation_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    invocation_id_from_seed(nanos, std::process::id())
}

/// Manages background job log storage on disk.
///
/// Directory structure:
/// ```text
/// <base_dir>/
///   <invocation-id>/
///     <job-name>/
///       meta.json
///       output.log
/// ```
#[derive(Clone)]
pub struct LogStore {
    pub base_dir: PathBuf,
}

impl LogStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Returns the log store for a specific repository.
    pub fn for_repo(repo_hash: &str) -> Result<Self> {
        let base = crate::daft_state_dir()?.join("jobs").join(repo_hash);
        Ok(Self::new(base))
    }

    pub fn create_job_dir(&self, invocation_id: &str, job_name: &str) -> Result<PathBuf> {
        let dir = self.base_dir.join(invocation_id).join(job_name);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create job log dir: {}", dir.display()))?;
        Ok(dir)
    }

    pub fn write_meta(&self, job_dir: &Path, meta: &JobMeta) -> Result<()> {
        let path = job_dir.join("meta.json");
        let content = serde_json::to_string_pretty(meta)?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write job meta: {}", path.display()))?;
        Ok(())
    }

    pub fn read_meta(&self, job_dir: &Path) -> Result<JobMeta> {
        let path = job_dir.join("meta.json");
        let content = fs::read_to_string(&path)?;
        let meta: JobMeta = serde_json::from_str(&content)?;
        Ok(meta)
    }

    pub fn log_path(job_dir: &Path) -> PathBuf {
        job_dir.join("output.log")
    }

    /// Write `meta.json` and `output.log` for a completed job atomically.
    ///
    /// Creates the job directory if needed. Used by `BufferingLogSink` (for
    /// foreground jobs) and by `yaml_executor` (for skipped job records).
    pub fn write_job_record(
        &self,
        invocation_id: &str,
        meta: &JobMeta,
        log_bytes: &[u8],
    ) -> Result<PathBuf> {
        let job_dir = self.create_job_dir(invocation_id, &meta.name)?;
        self.write_meta(&job_dir, meta)?;
        fs::write(Self::log_path(&job_dir), log_bytes)
            .with_context(|| format!("Failed to write log file for job: {}", meta.name))?;
        Ok(job_dir)
    }

    pub fn list_job_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = Vec::new();
        if !self.base_dir.exists() {
            return Ok(dirs);
        }
        for inv_entry in fs::read_dir(&self.base_dir)? {
            let inv_entry = inv_entry?;
            if inv_entry.file_type()?.is_dir() {
                for job_entry in fs::read_dir(inv_entry.path())? {
                    let job_entry = job_entry?;
                    if job_entry.file_type()?.is_dir() {
                        dirs.push(job_entry.path());
                    }
                }
            }
        }
        Ok(dirs)
    }

    /// Clean job dirs according to policy. Returns a summary of what was done.
    pub fn clean(
        &self,
        policy: &crate::coordinator::clean_policy::CleanPolicy,
    ) -> Result<crate::coordinator::clean_policy::CleanSummary> {
        use crate::coordinator::clean_policy::CleanSummary;

        let mut summary = CleanSummary {
            reason: "retention".into(),
            ..CleanSummary::default()
        };

        let now = chrono::Utc::now();
        // try_seconds avoids the i64::MIN panic in Duration::seconds. T1's parser
        // rejects negatives, but a corrupted on-disk value could still reach here.
        let stale_threshold =
            chrono::Duration::try_seconds(policy.repo_policy.stale_running_after_resolved())
                .unwrap_or_else(|| chrono::Duration::seconds(86_400));

        // Build the candidate set: group by worktree for sanity-floor evaluation.
        // Each entry holds (inv_id, job_dir, meta).
        let mut by_worktree: std::collections::BTreeMap<String, Vec<(String, PathBuf, JobMeta)>> =
            Default::default();
        // Track total job count per invocation across all worktrees so we know
        // when an entire invocation has been removed.
        let mut jobs_per_inv: std::collections::BTreeMap<String, usize> = Default::default();

        for job_dir in self.list_job_dirs()? {
            let meta = match self.read_meta(&job_dir) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let inv_id = job_dir
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            *jobs_per_inv.entry(inv_id.clone()).or_default() += 1;

            // Stale-Running: if Running for >threshold and no live socket, treat as terminal.
            let effective_status = if matches!(meta.status, JobStatus::Running) {
                let age = now.signed_duration_since(meta.started_at);
                let socket =
                    crate::coordinator::coordinator_socket_path(&self.repo_id_or_empty()).ok();
                let socket_alive = socket.as_ref().map(|p| p.exists()).unwrap_or(false);
                if age > stale_threshold && !socket_alive {
                    summary.stale_running_marked += 1;
                    JobStatus::Cancelled
                } else {
                    JobStatus::Running
                }
            } else {
                meta.status.clone()
            };
            if matches!(effective_status, JobStatus::Running) {
                continue; // never delete running jobs
            }

            by_worktree
                .entry(meta.worktree.clone())
                .or_default()
                .push((inv_id, job_dir, meta));
        }

        // Determine which jobs are eligible for retention-based removal.
        let keep_last = policy.repo_policy.keep_last_resolved();

        // (job_dir, log_size, inv_id) for each candidate.
        let mut candidates: Vec<(PathBuf, u64, String)> = Vec::new();

        for (_worktree, entries) in by_worktree {
            // Group by invocation. Sanity floor counts invocations, not jobs.
            let mut by_inv: std::collections::BTreeMap<String, Vec<(PathBuf, JobMeta)>> =
                Default::default();
            for (inv_id, dir, meta) in entries {
                by_inv.entry(inv_id).or_default().push((dir, meta));
            }
            // Re-sort invocations by recency (newest first).
            let mut invs: Vec<(String, Vec<(PathBuf, JobMeta)>)> = by_inv.into_iter().collect();
            invs.sort_by_key(|(_, jobs)| {
                std::cmp::Reverse(
                    jobs.iter()
                        .map(|(_, m)| m.started_at)
                        .max()
                        .unwrap_or_else(chrono::Utc::now),
                )
            });

            for (idx, (inv_id, jobs)) in invs.into_iter().enumerate() {
                if idx < keep_last {
                    continue; // sanity floor — keep most recent N
                }
                for (dir, meta) in jobs {
                    let retention = policy.retention_override.unwrap_or_else(|| {
                        meta.retention_seconds
                            .and_then(chrono::Duration::try_seconds)
                            .unwrap_or(policy.default_retention)
                    });
                    if now.signed_duration_since(meta.started_at) > retention {
                        let size = log_file_size(&dir);
                        candidates.push((dir.clone(), size, inv_id.clone()));
                        summary.candidates.push((
                            meta.worktree.clone(),
                            inv_id.clone(),
                            meta.name.clone(),
                        ));
                    }
                }
            }
        }

        // Tally candidates per invocation so we can drop the entire invocation
        // dir (including invocation.json) when every job in it was a candidate.
        // Hoisted above the dry-run early-return so dry-run can report the same
        // would-be-removed invocation count as the live path.
        let mut candidates_per_inv: std::collections::BTreeMap<String, usize> = Default::default();
        for (_, _, inv_id) in &candidates {
            *candidates_per_inv.entry(inv_id.clone()).or_default() += 1;
        }

        if policy.dry_run {
            summary.freed_bytes = candidates.iter().map(|(_, s, _)| s).sum();
            summary.removed_jobs = candidates.len();
            // Mirror the live path's invocation tally: an invocation is
            // counted as removed when every job in it was a candidate.
            for (inv_id, count) in &candidates_per_inv {
                let total = jobs_per_inv.get(inv_id).copied().unwrap_or(0);
                if *count >= total && total > 0 {
                    summary.removed_invocations += 1;
                }
            }
            return Ok(summary);
        }

        // Atomic remove: rename to .deleting-, then remove.
        let mut touched_invs: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for (dir, size, inv_id) in candidates {
            if let Some(parent) = dir.parent() {
                let trash = parent.join(format!(
                    ".deleting-{}",
                    dir.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown"),
                ));
                if fs::rename(&dir, &trash).is_ok() {
                    let _ = fs::remove_dir_all(&trash);
                    summary.removed_jobs += 1;
                    summary.freed_bytes += size;
                    touched_invs.insert(inv_id.clone());
                }
            }
        }

        // For invocations where every job was removed, also drop the parent
        // directory (including the invocation.json sidecar). For invocations
        // with partial removal, try `remove_dir` (succeeds only if empty —
        // back-compat with stores that have no sidecar).
        for inv_id in touched_invs {
            let inv_dir = self.base_dir.join(&inv_id);
            let total = jobs_per_inv.get(&inv_id).copied().unwrap_or(0);
            let removed = candidates_per_inv.get(&inv_id).copied().unwrap_or(0);
            if removed >= total && total > 0 {
                if fs::remove_dir_all(&inv_dir).is_ok() {
                    summary.removed_invocations += 1;
                }
            } else {
                let _ = fs::remove_dir(&inv_dir); // succeeds only if empty
                if !inv_dir.exists() {
                    summary.removed_invocations += 1;
                }
            }
        }

        Ok(summary)
    }

    /// Truncate any terminal-status log file that exceeds its
    /// `max_log_size_bytes`. Append a footer recording the original size.
    /// Skips Running jobs (truncating a live writer invites corruption).
    ///
    /// `default_cap` is used when JobMeta.max_log_size_bytes is None.
    /// Pass None to use the built-in 10 MB default.
    pub fn truncate_oversized_logs(&self, default_cap: Option<u64>) -> Result<usize> {
        const BUILTIN_DEFAULT_CAP: u64 = 10 * 1024 * 1024;
        const MIN_CAP: u64 = 1024; // Floor: cap below this is treated as 1KB.

        let mut truncated = 0;
        for job_dir in self.list_job_dirs()? {
            let mut meta = match self.read_meta(&job_dir) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if matches!(meta.status, JobStatus::Running) {
                continue;
            }
            if meta.log_truncated {
                continue; // already handled
            }

            let cap = meta
                .max_log_size_bytes
                .or(default_cap)
                .unwrap_or(BUILTIN_DEFAULT_CAP)
                .max(MIN_CAP);

            let log_path = LogStore::log_path(&job_dir);
            let log_size = match log_path.metadata() {
                Ok(m) => m.len(),
                Err(_) => continue,
            };
            if log_size <= cap {
                continue;
            }

            // Build footer
            let footer = format!("\n[output truncated at {log_size} bytes]\n");
            let footer_bytes = footer.as_bytes();
            let head_len = cap.saturating_sub(footer_bytes.len() as u64);

            // Read [0..head_len), write [head][footer] atomically via tmpfile-and-rename.
            let mut head = vec![0u8; head_len as usize];
            {
                use std::io::Read;
                let mut f = fs::File::open(&log_path)?;
                f.read_exact(&mut head)?;
            }

            // Atomic replacement: write head + footer to a sibling tmpfile, then rename.
            // File is renamed before meta is updated; if meta.write fails, the on-disk
            // file is correctly truncated but `log_truncated` stays false. The subsequent
            // `log_size <= cap` short-circuit prevents re-truncation, at the cost of
            // permanently losing `original_size_bytes`. Single-flight protection is
            // added in T6 (currently best-effort under concurrent calls).
            let tmp_path = log_path.with_extension("log.truncating");
            let result = (|| -> Result<()> {
                use std::io::Write;
                let mut tmp = fs::File::create(&tmp_path)?;
                tmp.write_all(&head)?;
                tmp.write_all(footer_bytes)?;
                drop(tmp);
                fs::rename(&tmp_path, &log_path)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&tmp_path); // best-effort cleanup
            }
            result?;

            meta.log_truncated = true;
            meta.original_size_bytes = Some(log_size);
            self.write_meta(&job_dir, &meta)?;
            truncated += 1;
        }
        Ok(truncated)
    }

    /// Total bytes consumed under base_dir (recursive).
    pub fn total_size_bytes(&self) -> Result<u64> {
        if !self.base_dir.exists() {
            return Ok(0);
        }
        let total: u64 = walkdir::WalkDir::new(&self.base_dir)
            .into_iter()
            .filter_entry(|e| {
                // Skip orphaned trash directories from crashed cleanup runs.
                // They're never reaped here; T6 handles the reaping.
                e.file_name()
                    .to_str()
                    .map(|n| !n.starts_with(".deleting-"))
                    .unwrap_or(true)
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        Ok(total)
    }

    /// Evict invocations oldest-first until total size is under budget.
    /// Honors `keep_last` per-worktree: an invocation is never evicted if
    /// doing so would drop its worktree below the sanity floor.
    pub fn enforce_budget(
        &self,
        policy: &crate::coordinator::clean_policy::RepoPolicy,
    ) -> Result<crate::coordinator::clean_policy::BudgetOutcome> {
        use crate::coordinator::clean_policy::BudgetOutcome;
        let budget = policy.max_total_size_resolved();
        let keep_last = policy.keep_last_resolved();

        let mut total = self.total_size_bytes()?;
        if total <= budget {
            return Ok(BudgetOutcome::default());
        }

        // List invocations with (worktree, inv_id, created_at, total_size, job_count).
        let mut invs: Vec<(String, String, chrono::DateTime<chrono::Utc>, u64, usize)> = Vec::new();
        for inv in self.list_invocations()? {
            let inv_dir = self.base_dir.join(&inv.invocation_id);
            let mut size: u64 = 0;
            let mut job_count: usize = 0;
            for entry in walkdir::WalkDir::new(&inv_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() {
                    size += entry.metadata().map(|m| m.len()).unwrap_or(0);
                } else if entry.file_type().is_dir() && entry.depth() == 1 {
                    job_count += 1;
                }
            }
            invs.push((
                inv.worktree.clone(),
                inv.invocation_id.clone(),
                inv.created_at,
                size,
                job_count,
            ));
        }

        // Group by worktree, count for sanity floor.
        let mut per_wt_count: std::collections::BTreeMap<String, usize> = Default::default();
        for (wt, _, _, _, _) in &invs {
            *per_wt_count.entry(wt.clone()).or_default() += 1;
        }

        // Sort all invocations by created_at ascending (oldest first).
        invs.sort_by_key(|(_, _, ts, _, _)| *ts);

        let mut outcome = BudgetOutcome::default();
        for (wt, inv_id, _, size, jobs) in invs {
            if total <= budget {
                break;
            }
            // Sanity floor: never evict if it would drop this worktree below keep_last.
            if let Some(count) = per_wt_count.get_mut(&wt) {
                if *count <= keep_last {
                    continue;
                }
                *count -= 1;
            }

            let inv_dir = self.base_dir.join(&inv_id);
            let trash = self.base_dir.join(format!(".deleting-{inv_id}"));
            if fs::rename(&inv_dir, &trash).is_ok() {
                let _ = fs::remove_dir_all(&trash);
                total = total.saturating_sub(size);
                outcome.evicted_invocations += 1;
                outcome.freed_bytes += size;
                outcome.freed_jobs += jobs;
            }
        }
        Ok(outcome)
    }

    /// Helper: derive repo_id from base_dir's last component (the repo UUID).
    fn repo_id_or_empty(&self) -> String {
        self.base_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    }

    pub fn write_invocation_meta(&self, invocation_id: &str, meta: &InvocationMeta) -> Result<()> {
        let dir = self.base_dir.join(invocation_id);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create invocation dir: {}", dir.display()))?;
        let path = dir.join("invocation.json");
        let content = serde_json::to_string_pretty(meta)?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write invocation meta: {}", path.display()))?;
        Ok(())
    }

    pub fn read_invocation_meta(&self, invocation_id: &str) -> Result<InvocationMeta> {
        let path = self.base_dir.join(invocation_id).join("invocation.json");
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read invocation meta: {}", path.display()))?;
        let meta: InvocationMeta = serde_json::from_str(&content)?;
        Ok(meta)
    }

    pub fn list_invocations(&self) -> Result<Vec<InvocationMeta>> {
        let mut invocations = Vec::new();
        if !self.base_dir.exists() {
            return Ok(invocations);
        }
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let inv_id = entry.file_name().to_string_lossy().to_string();
                if let Ok(meta) = self.read_invocation_meta(&inv_id) {
                    invocations.push(meta);
                }
            }
        }
        invocations.sort_by_key(|a| a.created_at);
        Ok(invocations)
    }

    pub fn list_invocations_for_worktree(&self, worktree: &str) -> Result<Vec<InvocationMeta>> {
        let all = self.list_invocations()?;
        Ok(all.into_iter().filter(|m| m.worktree == worktree).collect())
    }

    pub fn find_invocations_by_prefix(
        &self,
        worktree: &str,
        prefix: &str,
    ) -> Result<Vec<InvocationMeta>> {
        let all = self.list_invocations_for_worktree(worktree)?;
        Ok(all
            .into_iter()
            .filter(|m| m.invocation_id.starts_with(prefix))
            .collect())
    }

    pub fn list_jobs_in_invocation(&self, invocation_id: &str) -> Result<Vec<PathBuf>> {
        let inv_dir = self.base_dir.join(invocation_id);
        let mut dirs = Vec::new();
        if !inv_dir.exists() {
            return Ok(dirs);
        }
        for entry in fs::read_dir(&inv_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            }
        }
        Ok(dirs)
    }

    pub fn list_distinct_worktrees(&self) -> Result<Vec<String>> {
        let invocations = self.list_invocations()?;
        let mut seen = std::collections::BTreeSet::new();
        for inv in &invocations {
            seen.insert(inv.worktree.clone());
        }
        Ok(seen.into_iter().collect())
    }

    /// Path to the repo-level cleanup policy sidecar.
    pub fn repo_policy_path(&self) -> PathBuf {
        self.base_dir.join("repo-policy.json")
    }

    /// Persist the repo-level cleanup policy. Field-merges with the on-disk
    /// values: explicit `Some(_)` in the new policy wins; `None` preserves the
    /// on-disk value. This prevents hooks without a `log:` block (which
    /// produce an all-`None` policy) from silently wiping persisted tuning.
    pub fn write_repo_policy(
        &self,
        policy: &crate::coordinator::clean_policy::RepoPolicy,
    ) -> Result<()> {
        fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("Failed to create base dir: {}", self.base_dir.display()))?;

        let on_disk = self.read_repo_policy();
        let merged = crate::coordinator::clean_policy::RepoPolicy {
            version: policy.version,
            max_total_size_bytes: policy.max_total_size_bytes.or(on_disk.max_total_size_bytes),
            keep_last: policy.keep_last.or(on_disk.keep_last),
            stale_running_after_seconds: policy
                .stale_running_after_seconds
                .or(on_disk.stale_running_after_seconds),
        };

        let json = serde_json::to_string_pretty(&merged)?;
        let path = self.repo_policy_path();
        fs::write(&path, json)
            .with_context(|| format!("Failed to write repo policy: {}", path.display()))?;
        Ok(())
    }

    /// Read the repo-level cleanup policy, falling back to defaults if the
    /// sidecar is missing or unreadable.
    pub fn read_repo_policy(&self) -> crate::coordinator::clean_policy::RepoPolicy {
        let path = self.repo_policy_path();
        match fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(policy) => policy,
                Err(err) => {
                    eprintln!(
                        "daft: warning: failed to parse repo policy at {}: {}; using defaults",
                        path.display(),
                        err
                    );
                    crate::coordinator::clean_policy::RepoPolicy::defaults()
                }
            },
            Err(_) => crate::coordinator::clean_policy::RepoPolicy::defaults(),
        }
    }
}

fn log_file_size(job_dir: &Path) -> u64 {
    LogStore::log_path(job_dir)
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn test_create_job_log_dir() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("abc123", "warm-build").unwrap();
        assert!(dir.exists());
        assert!(dir.ends_with("abc123/warm-build"));
    }

    #[test]
    fn test_write_and_read_meta() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("inv1", "job1").unwrap();
        let meta = JobMeta {
            name: "job1".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "/path/to/wt".to_string(),
            command: "cargo build".to_string(),
            working_dir: "/path/to/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Running,
            exit_code: None,
            pid: Some(12345),
            background: false,
            finished_at: None,
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();
        let loaded = store.read_meta(&dir).unwrap();
        assert_eq!(loaded.name, "job1");
        assert!(matches!(loaded.status, JobStatus::Running));
    }

    #[test]
    fn test_list_jobs_for_repo() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        store.create_job_dir("inv1", "job-a").unwrap();
        store.create_job_dir("inv1", "job-b").unwrap();
        store.create_job_dir("inv2", "job-c").unwrap();
        let jobs = store.list_job_dirs().unwrap();
        assert_eq!(jobs.len(), 3);
    }

    #[test]
    fn test_clean_old_logs() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("old-inv", "old-job").unwrap();
        let meta = JobMeta {
            name: "old-job".to_string(),
            hook_type: "post-clone".to_string(),
            worktree: "/tmp/wt".to_string(),
            command: "echo old".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now() - chrono::Duration::days(30),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: None,
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();
        let policy = crate::coordinator::clean_policy::CleanPolicy {
            retention_override: Some(chrono::Duration::days(7)),
            repo_policy: crate::coordinator::clean_policy::RepoPolicy {
                version: 1,
                keep_last: Some(0),
                ..crate::coordinator::clean_policy::RepoPolicy::defaults()
            },
            ..crate::coordinator::clean_policy::CleanPolicy::default()
        };
        let summary = store.clean(&policy).unwrap();
        assert_eq!(summary.removed_jobs, 1);
        assert!(!dir.exists());
    }

    #[test]
    fn clean_uses_per_job_retention_from_meta() {
        use crate::coordinator::clean_policy::{CleanPolicy, RepoPolicy};

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        // Two invocations, one with a 1-day retention (old), one with 30-day
        // retention (also old, but should survive).
        for (id, retention_secs) in &[("0001", 86_400i64), ("0002", 86_400 * 30)] {
            let inv_meta = InvocationMeta {
                invocation_id: id.to_string(),
                trigger_command: "post-create".into(),
                hook_type: "worktree-post-create".into(),
                worktree: "main".into(),
                created_at: now - chrono::Duration::days(10),
            };
            store.write_invocation_meta(id, &inv_meta).unwrap();

            let dir = store.create_job_dir(id, "build").unwrap();
            let meta = JobMeta {
                name: "build".into(),
                hook_type: "worktree-post-create".into(),
                worktree: "main".into(),
                command: "echo".into(),
                working_dir: "/tmp".into(),
                env: HashMap::new(),
                started_at: now - chrono::Duration::days(10),
                status: JobStatus::Completed,
                exit_code: Some(0),
                pid: None,
                background: false,
                finished_at: Some(now - chrono::Duration::days(10)),
                needs: vec![],
                retention_seconds: Some(*retention_secs),
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
            };
            store.write_meta(&dir, &meta).unwrap();
        }

        let policy = CleanPolicy {
            retention_override: None,
            dry_run: false,
            default_retention: chrono::Duration::days(7),
            repo_policy: RepoPolicy {
                version: 1,
                keep_last: Some(0), // disable sanity floor for this test
                ..RepoPolicy::defaults()
            },
        };
        let summary = store.clean(&policy).unwrap();

        // 0001 had 1d retention, started 10d ago → removed
        // 0002 had 30d retention, started 10d ago → kept
        assert_eq!(summary.removed_invocations, 1);
        assert!(!tmp.path().join("0001").exists());
        assert!(tmp.path().join("0002").exists());
    }

    #[test]
    fn clean_keep_last_floor_overrides_retention() {
        use crate::coordinator::clean_policy::{CleanPolicy, RepoPolicy};

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        // 5 invocations all >30 days old, retention 7 days.
        for i in 0..5 {
            let id = format!("000{i}");
            let inv_meta = InvocationMeta {
                invocation_id: id.clone(),
                trigger_command: "post-create".into(),
                hook_type: "worktree-post-create".into(),
                worktree: "main".into(),
                created_at: now - chrono::Duration::days(30 + i),
            };
            store.write_invocation_meta(&id, &inv_meta).unwrap();
            let dir = store.create_job_dir(&id, "build").unwrap();
            let meta = JobMeta {
                name: "build".into(),
                hook_type: "worktree-post-create".into(),
                worktree: "main".into(),
                command: "echo".into(),
                working_dir: "/tmp".into(),
                env: HashMap::new(),
                started_at: now - chrono::Duration::days(30 + i),
                status: JobStatus::Completed,
                exit_code: Some(0),
                pid: None,
                background: false,
                finished_at: Some(now - chrono::Duration::days(30 + i)),
                needs: vec![],
                retention_seconds: Some(86_400 * 7),
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
            };
            store.write_meta(&dir, &meta).unwrap();
        }

        let policy = CleanPolicy {
            retention_override: None,
            dry_run: false,
            default_retention: chrono::Duration::days(7),
            repo_policy: RepoPolicy {
                version: 1,
                keep_last: Some(3),
                ..RepoPolicy::defaults()
            },
        };
        store.clean(&policy).unwrap();

        // 5 invocations — sanity floor of 3 keeps the most recent 3.
        let remaining: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(remaining.len(), 3);
        assert!(
            remaining.contains(&"0000".to_string()),
            "expected most-recent invocation 0000 to survive, got: {remaining:?}"
        );
        assert!(
            remaining.contains(&"0001".to_string()),
            "expected 0001 to survive, got: {remaining:?}"
        );
        assert!(
            remaining.contains(&"0002".to_string()),
            "expected 0002 to survive, got: {remaining:?}"
        );
    }

    #[test]
    #[serial_test::serial]
    fn clean_detects_stale_running_when_socket_missing() {
        use crate::coordinator::clean_policy::{CleanPolicy, RepoPolicy};

        let tmp = TempDir::new().unwrap();
        // Point DAFT_STATE_DIR at our tempdir so coordinator_socket_path resolves
        // to a path that's guaranteed not to exist (no coordinator running here).
        // DAFT_STATE_DIR is process-global; #[serial] prevents cross-test interference.
        let prev_state_dir = std::env::var("DAFT_STATE_DIR").ok();
        std::env::set_var("DAFT_STATE_DIR", tmp.path());

        // Use a UUID-shaped base_dir component so coordinator_socket_path
        // produces a real-looking path (which won't exist on the test FS).
        let repo_dir = tmp.path().join("01900000-0000-7000-8000-000000000000");
        std::fs::create_dir(&repo_dir).unwrap();
        let store = LogStore::new(repo_dir.clone());
        let now = chrono::Utc::now();

        // One Running invocation, started 48h ago. stale_running_after = 24h.
        let inv_id = "0001";
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.into(),
            trigger_command: "post-create".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "main".into(),
            created_at: now - chrono::Duration::hours(48),
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        let dir = store.create_job_dir(inv_id, "long-running").unwrap();
        let meta = JobMeta {
            name: "long-running".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "main".into(),
            command: "sleep 86400".into(),
            working_dir: "/tmp".into(),
            env: HashMap::new(),
            started_at: now - chrono::Duration::hours(48),
            status: JobStatus::Running,
            exit_code: None,
            pid: Some(99999),
            background: true,
            finished_at: None,
            needs: vec![],
            retention_seconds: Some(60), // 1 minute, well exceeded
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();

        // No socket file is created — coordinator considered dead.
        let policy = CleanPolicy {
            retention_override: None,
            dry_run: false,
            default_retention: chrono::Duration::days(7),
            repo_policy: RepoPolicy {
                version: 1,
                keep_last: Some(0),
                stale_running_after_seconds: Some(86_400), // 24h
                ..RepoPolicy::defaults()
            },
        };
        let summary = store.clean(&policy).unwrap();

        assert_eq!(
            summary.stale_running_marked, 1,
            "expected exactly one stale-Running job to be detected"
        );
        // The job dir should be gone after reclassification + retention sweep.
        assert!(
            !dir.exists(),
            "stale-Running job dir should have been removed"
        );

        // Restore env.
        match prev_state_dir {
            Some(v) => std::env::set_var("DAFT_STATE_DIR", v),
            None => std::env::remove_var("DAFT_STATE_DIR"),
        }
    }

    #[test]
    fn test_write_and_read_invocation_meta() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        // Create the invocation directory (normally done by create_job_dir)
        std::fs::create_dir_all(tmp.path().join("inv1")).unwrap();

        let meta = InvocationMeta {
            invocation_id: "inv1".to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/tax-calc".to_string(),
            created_at: chrono::Utc::now(),
        };
        store.write_invocation_meta("inv1", &meta).unwrap();
        let loaded = store.read_invocation_meta("inv1").unwrap();
        assert_eq!(loaded.invocation_id, "inv1");
        assert_eq!(loaded.trigger_command, "worktree-post-create");
        assert_eq!(loaded.worktree, "feature/tax-calc");
    }

    #[test]
    fn test_list_invocations() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        for (inv_id, wt) in &[("inv1", "feature/a"), ("inv2", "feature/b")] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: wt.to_string(),
                created_at: chrono::Utc::now(),
            };
            store.write_invocation_meta(inv_id, &meta).unwrap();
        }

        let all = store.list_invocations().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_invocations_for_worktree() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        for (inv_id, wt) in &[
            ("inv1", "feature/a"),
            ("inv2", "feature/b"),
            ("inv3", "feature/a"),
        ] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: wt.to_string(),
                created_at: chrono::Utc::now(),
            };
            store.write_invocation_meta(inv_id, &meta).unwrap();
        }

        let filtered = store.list_invocations_for_worktree("feature/a").unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|m| m.worktree == "feature/a"));
    }

    #[test]
    fn test_list_jobs_in_invocation() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        // Create two jobs under inv1
        store.create_job_dir("inv1", "db-migrate").unwrap();
        store.create_job_dir("inv1", "warm-build").unwrap();
        // Create one job under inv2
        store.create_job_dir("inv2", "db-seed").unwrap();

        let jobs = store.list_jobs_in_invocation("inv1").unwrap();
        assert_eq!(jobs.len(), 2);
        let names: Vec<&str> = jobs
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(names.contains(&"db-migrate"));
        assert!(names.contains(&"warm-build"));
    }

    #[test]
    fn test_job_meta_background_and_finished_at() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("inv1", "bg-job").unwrap();
        let finished = chrono::Utc::now();
        let meta = JobMeta {
            name: "bg-job".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "echo hi".to_string(),
            working_dir: "/tmp".to_string(),
            env: HashMap::new(),
            started_at: finished - chrono::Duration::seconds(5),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: Some(1234),
            background: true,
            finished_at: Some(finished),
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();
        let loaded = store.read_meta(&dir).unwrap();
        assert!(loaded.background);
        assert!(loaded.finished_at.is_some());
    }

    #[test]
    fn test_list_invocations_empty_store() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let all = store.list_invocations().unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn skipped_status_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::new(dir.path().to_path_buf());
        let job_dir = store.create_job_dir("inv1", "dbsetup").unwrap();

        let meta = JobMeta {
            name: "dbsetup".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: String::new(),
            working_dir: String::new(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Skipped,
            exit_code: None,
            pid: None,
            background: false,
            finished_at: None,
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&job_dir, &meta).unwrap();

        let loaded = store.read_meta(&job_dir).unwrap();
        assert_eq!(loaded.status, JobStatus::Skipped);
    }

    #[test]
    fn invocation_meta_written_without_coordinator() {
        // Verifies that write_invocation_meta creates the file on its own;
        // no coordinator fork required.
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::new(dir.path().to_path_buf());
        let meta = InvocationMeta {
            invocation_id: "deadbeef".to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            created_at: chrono::Utc::now(),
        };

        store.write_invocation_meta("deadbeef", &meta).unwrap();
        let path = dir.path().join("deadbeef").join("invocation.json");
        assert!(path.exists());

        let loaded = store.read_invocation_meta("deadbeef").unwrap();
        assert_eq!(loaded.trigger_command, "worktree-post-create");
    }

    #[test]
    fn write_job_record_creates_meta_and_log_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::new(dir.path().to_path_buf());

        let meta = JobMeta {
            name: "pnpm-install".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "pnpm install".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: Some(chrono::Utc::now()),
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };

        let job_dir = store
            .write_job_record("inv42", &meta, b"installing...\ndone\n")
            .unwrap();

        let loaded_meta = store.read_meta(&job_dir).unwrap();
        assert_eq!(loaded_meta.name, "pnpm-install");

        let log_bytes = std::fs::read(LogStore::log_path(&job_dir)).unwrap();
        assert_eq!(log_bytes, b"installing...\ndone\n");
    }

    #[test]
    fn job_meta_needs_round_trips_through_json() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("inv-needs", "seeder").unwrap();
        let meta = JobMeta {
            name: "seeder".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "echo seed".to_string(),
            working_dir: "/tmp".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: None,
            needs: vec!["migrator".to_string()],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();
        let loaded = store.read_meta(&dir).unwrap();
        assert_eq!(loaded.needs, vec!["migrator".to_string()]);
    }

    #[test]
    fn job_meta_skipped_constructor_produces_correct_fields() {
        let meta = JobMeta::skipped(
            "test-job",
            "worktree-post-create",
            "feature/x",
            "echo test",
            false,
            vec!["dep".to_string()],
        );
        assert_eq!(meta.name, "test-job");
        assert_eq!(meta.hook_type, "worktree-post-create");
        assert_eq!(meta.worktree, "feature/x");
        assert_eq!(meta.command, "echo test");
        assert_eq!(meta.status, JobStatus::Skipped);
        assert_eq!(meta.needs, vec!["dep".to_string()]);
        assert!(!meta.background);
        assert!(meta.exit_code.is_none());
        assert!(meta.pid.is_none());
        assert!(meta.finished_at.is_none());
        assert!(meta.working_dir.is_empty());
        assert!(meta.env.is_empty());
    }

    #[test]
    fn job_meta_without_needs_field_deserializes_to_empty_vec() {
        let json = r#"{
            "name": "old-job",
            "hook_type": "worktree-post-create",
            "worktree": "feature/x",
            "command": "echo hi",
            "working_dir": "/tmp",
            "env": {},
            "started_at": "2026-04-11T12:00:00Z",
            "status": "completed",
            "exit_code": 0,
            "pid": null,
            "background": false,
            "finished_at": null
        }"#;
        let meta: JobMeta = serde_json::from_str(json).unwrap();
        assert!(meta.needs.is_empty());
    }

    #[test]
    fn list_distinct_worktrees_returns_unique_names() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        let now = chrono::Utc::now();
        for (inv_id, wt, offset) in &[
            ("inv1", "feature/a", 100i64),
            ("inv2", "feature/b", 50),
            ("inv3", "feature/a", 10),
            ("inv4", "feature/c", 5),
        ] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: wt.to_string(),
                created_at: now - chrono::Duration::seconds(*offset),
            };
            store.write_invocation_meta(inv_id, &meta).unwrap();
        }

        let worktrees = store.list_distinct_worktrees().unwrap();
        assert_eq!(worktrees.len(), 3);
        assert!(worktrees.contains(&"feature/a".to_string()));
        assert!(worktrees.contains(&"feature/b".to_string()));
        assert!(worktrees.contains(&"feature/c".to_string()));
    }

    #[test]
    fn find_invocations_by_prefix_returns_matching() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        let now = chrono::Utc::now();
        for (inv_id, wt, offset) in &[
            ("a3f200000000", "feature/a", 100i64),
            ("a3f200000001", "feature/a", 50),
            ("b7c100000000", "feature/a", 10),
        ] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: wt.to_string(),
                created_at: now - chrono::Duration::seconds(*offset),
            };
            store.write_invocation_meta(inv_id, &meta).unwrap();
        }

        let matches = store
            .find_invocations_by_prefix("feature/a", "a3f2")
            .unwrap();
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|m| m.invocation_id.starts_with("a3f2")));

        let matches = store
            .find_invocations_by_prefix("feature/a", "b7c1")
            .unwrap();
        assert_eq!(matches.len(), 1);

        let matches = store
            .find_invocations_by_prefix("feature/a", "zzzz")
            .unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn invocation_id_is_16_hex_chars() {
        let id = invocation_id_from_seed(1_234_567_890, 42);
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn invocation_id_is_deterministic_for_same_seed() {
        let a = invocation_id_from_seed(1_234_567_890, 42);
        let b = invocation_id_from_seed(1_234_567_890, 42);
        assert_eq!(a, b);
    }

    #[test]
    fn invocation_id_prefix_discriminates_within_same_millisecond() {
        // Regression: the previous `format!("{ts:012x}")` impl produced IDs
        // whose leading 4 hex chars reflected the top 16 bits of a ms
        // timestamp — stable for ~3 days at a time. Any two invocations in
        // the same window collided on the short-ID prefix.
        //
        // New impl seeds off nanoseconds and hashes, so two inputs 100 ns
        // apart must yield distinct leading prefixes.
        let ms = 1_700_000_000_000_u128;
        let ns_a = ms * 1_000_000 + 100;
        let ns_b = ms * 1_000_000 + 200;
        let a = invocation_id_from_seed(ns_a, 42);
        let b = invocation_id_from_seed(ns_b, 42);
        assert_ne!(a, b);
        assert_ne!(&a[..4], &b[..4]);
    }

    #[test]
    fn job_meta_round_trips_with_new_policy_fields() {
        let meta = JobMeta {
            name: "build".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/x".into(),
            command: "echo hi".into(),
            working_dir: "/tmp".into(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: None,
            needs: vec![],
            retention_seconds: Some(86_400 * 14),
            max_log_size_bytes: Some(20 * 1024 * 1024),
            log_truncated: false,
            original_size_bytes: None,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: JobMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.retention_seconds, Some(86_400 * 14));
        assert_eq!(back.max_log_size_bytes, Some(20 * 1024 * 1024));
        assert!(!back.log_truncated);
    }

    #[test]
    fn job_meta_back_compat_missing_new_fields() {
        let json = r#"{
            "name":"x","hook_type":"worktree-post-create","worktree":"main",
            "command":"echo","working_dir":"/tmp","env":{},
            "started_at":"2025-01-01T00:00:00Z","status":"completed",
            "exit_code":0,"pid":null,"background":false,"finished_at":null,
            "needs":[]
        }"#;
        let meta: JobMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.retention_seconds, None);
        assert_eq!(meta.max_log_size_bytes, None);
        assert!(!meta.log_truncated);
    }

    #[test]
    fn repo_policy_round_trip_via_log_store() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        let policy = RepoPolicy {
            version: 1,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(7),
            stale_running_after_seconds: Some(120),
        };
        store.write_repo_policy(&policy).unwrap();
        let back = store.read_repo_policy();
        assert_eq!(back, policy);
    }

    #[test]
    fn repo_policy_missing_returns_defaults() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let p = store.read_repo_policy();
        assert_eq!(p.max_total_size_resolved(), 500 * 1024 * 1024);
        assert_eq!(p.keep_last_resolved(), 3);
    }

    #[test]
    fn invocation_id_prefixes_discriminate_across_many_draws() {
        // 30 close-spaced timestamps, same pid. With the old impl these
        // would all share the same leading prefix. The hash-based impl
        // should spread them across the 2^16 prefix space with essentially
        // zero collisions at this size.
        let base = 1_700_000_000_000_000_000_u128;
        let prefixes: std::collections::HashSet<String> = (0..30)
            .map(|i| invocation_id_from_seed(base + i, 42)[..4].to_string())
            .collect();
        assert_eq!(
            prefixes.len(),
            30,
            "short-ID prefixes collided: {:?}",
            prefixes
        );
    }

    fn seed_inv_with_jobs(
        store: &LogStore,
        inv_id: &str,
        worktree: &str,
        started_at: chrono::DateTime<chrono::Utc>,
        n_jobs: usize,
        log_bytes: usize,
    ) {
        // Write the invocation sidecar.
        let inv_dir = store.base_dir.join(inv_id);
        std::fs::create_dir_all(&inv_dir).unwrap();
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.to_string(),
            trigger_command: "test".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: worktree.to_string(),
            created_at: started_at,
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        // Write each job's meta + a synthetic log file of `log_bytes` bytes.
        for i in 0..n_jobs {
            let name = format!("job-{i}");
            let job_dir = store.create_job_dir(inv_id, &name).unwrap();
            let meta = JobMeta {
                name: name.clone(),
                hook_type: "worktree-post-create".to_string(),
                worktree: worktree.to_string(),
                command: "echo x".to_string(),
                working_dir: "/tmp".to_string(),
                env: HashMap::new(),
                started_at,
                status: JobStatus::Completed,
                exit_code: Some(0),
                pid: None,
                background: true,
                finished_at: Some(started_at),
                needs: Vec::new(),
                retention_seconds: None,
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
            };
            store.write_meta(&job_dir, &meta).unwrap();
            let log_path = LogStore::log_path(&job_dir);
            std::fs::write(&log_path, vec![b'x'; log_bytes]).unwrap();
        }
    }

    #[test]
    fn enforce_budget_returns_freed_bytes_and_jobs() {
        use crate::coordinator::clean_policy::{BudgetOutcome, RepoPolicy};

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().join("jobs").join("test-repo"));

        let now = chrono::Utc::now();
        seed_inv_with_jobs(
            &store,
            "inv-old",
            "feat/x",
            now - chrono::Duration::days(2),
            2,
            1_048_576,
        );
        seed_inv_with_jobs(&store, "inv-new", "feat/x", now, 2, 1_048_576);

        let policy = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(1_500_000), // forces eviction of inv-old
            keep_last: Some(1),
            stale_running_after_seconds: None,
        };

        let outcome: BudgetOutcome = store.enforce_budget(&policy).unwrap();
        assert_eq!(outcome.evicted_invocations, 1);
        assert_eq!(outcome.freed_jobs, 2);
        assert!(outcome.freed_bytes >= 2 * 1_048_576);
    }

    #[test]
    fn budget_evicts_oldest_first_respects_keep_last() {
        use crate::coordinator::clean_policy::RepoPolicy;
        use std::collections::HashMap;
        use std::io::Write;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        // 5 invocations, each 200KB. Budget 500KB (= 2.5 invocations worth).
        // Keep_last = 1. Expected: oldest 3 evicted, newest 2 kept.
        for i in 0..5 {
            let id = format!("000{i}");
            let inv_meta = InvocationMeta {
                invocation_id: id.clone(),
                trigger_command: "post-create".into(),
                hook_type: "worktree-post-create".into(),
                worktree: "main".into(),
                created_at: now - chrono::Duration::hours(5 - i as i64),
            };
            store.write_invocation_meta(&id, &inv_meta).unwrap();
            let dir = store.create_job_dir(&id, "build").unwrap();
            let meta = JobMeta {
                name: "build".into(),
                hook_type: "worktree-post-create".into(),
                worktree: "main".into(),
                command: "echo".into(),
                working_dir: "/tmp".into(),
                env: HashMap::new(),
                started_at: now - chrono::Duration::hours(5 - i as i64),
                status: JobStatus::Completed,
                exit_code: Some(0),
                pid: None,
                background: false,
                finished_at: Some(now - chrono::Duration::hours(5 - i as i64)),
                needs: vec![],
                retention_seconds: None,
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
            };
            store.write_meta(&dir, &meta).unwrap();
            let mut f = std::fs::File::create(LogStore::log_path(&dir)).unwrap();
            f.write_all(&vec![b'.'; 200 * 1024]).unwrap();
        }

        let policy = RepoPolicy {
            version: 1,
            max_total_size_bytes: Some(500 * 1024),
            keep_last: Some(1),
            stale_running_after_seconds: None,
        };
        let outcome = store.enforce_budget(&policy).unwrap();
        assert!(
            outcome.evicted_invocations >= 3,
            "expected >=3 evicted, got {}",
            outcome.evicted_invocations
        );

        let remaining: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("000"))
            .collect();
        // The most recent invocation (0004) must always survive.
        assert!(remaining.contains(&"0004".to_string()));
        // The oldest (0000) should be evicted.
        assert!(!remaining.contains(&"0000".to_string()));
    }

    #[test]
    fn truncate_caps_oversized_log_with_footer() {
        use std::collections::HashMap;
        use std::io::Write;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let now = chrono::Utc::now();

        let inv_id = "0001";
        let inv_meta = InvocationMeta {
            invocation_id: inv_id.into(),
            trigger_command: "post-create".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "main".into(),
            created_at: now,
        };
        store.write_invocation_meta(inv_id, &inv_meta).unwrap();

        let dir = store.create_job_dir(inv_id, "spam").unwrap();
        let meta = JobMeta {
            name: "spam".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "main".into(),
            command: "yes".into(),
            working_dir: "/tmp".into(),
            env: HashMap::new(),
            started_at: now,
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: Some(now),
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: Some(1024),
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();

        // Write a 4KB log file
        let log_path = LogStore::log_path(&dir);
        let mut f = std::fs::File::create(&log_path).unwrap();
        f.write_all(&vec![b'x'; 4096]).unwrap();

        // Truncate with 1KB cap (from meta.max_log_size_bytes)
        let truncated = store.truncate_oversized_logs(None).unwrap();
        assert_eq!(truncated, 1);

        // File should be approximately 1KB (cap), with footer
        let len = log_path.metadata().unwrap().len();
        assert!(len <= 1024, "expected <=1024, got {len}");
        let contents = std::fs::read_to_string(&log_path).unwrap();
        assert!(contents.ends_with("[output truncated at 4096 bytes]\n"));

        // Meta should be updated
        let updated = store.read_meta(&dir).unwrap();
        assert!(updated.log_truncated);
        assert_eq!(updated.original_size_bytes, Some(4096));
    }

    #[test]
    fn total_size_bytes_skips_deleting_orphans() {
        use std::io::Write;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        // Real invocation dir with a 1KB log
        let dir = store.create_job_dir("0001", "build").unwrap();
        let log = LogStore::log_path(&dir);
        let mut f = std::fs::File::create(&log).unwrap();
        f.write_all(&[b'.'; 1024]).unwrap();
        drop(f);

        // Orphan trash dir with 5KB of garbage
        let trash = tmp.path().join(".deleting-orphan-001");
        std::fs::create_dir(&trash).unwrap();
        let mut f = std::fs::File::create(trash.join("output.log")).unwrap();
        f.write_all(&[b'.'; 5 * 1024]).unwrap();
        drop(f);

        let total = store.total_size_bytes().unwrap();
        // Should be ~1024 (the real log only); orphan must be excluded.
        assert!(total <= 1100, "orphan inflated total: {total}");
        assert!(total >= 1024, "real log undercounted: {total}");
    }

    #[test]
    fn write_repo_policy_preserves_unset_fields_from_on_disk() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().join("store"));

        // First write: user sets max_total_size + keep_last.
        let first = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(5),
            stale_running_after_seconds: None,
        };
        store.write_repo_policy(&first).unwrap();

        // Second write: a hook with no log block submits all-None.
        let second = RepoPolicy::defaults();
        store.write_repo_policy(&second).unwrap();

        // The on-disk policy should still have the user's values.
        let read = store.read_repo_policy();
        assert_eq!(read.max_total_size_bytes, Some(100 * 1024 * 1024));
        assert_eq!(read.keep_last, Some(5));
    }

    #[test]
    fn write_repo_policy_overrides_explicitly_set_fields() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().join("store"));

        let first = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(5),
            stale_running_after_seconds: None,
        };
        store.write_repo_policy(&first).unwrap();

        let second = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(200 * 1024 * 1024),
            keep_last: None,
            stale_running_after_seconds: None,
        };
        store.write_repo_policy(&second).unwrap();

        let read = store.read_repo_policy();
        assert_eq!(
            read.max_total_size_bytes,
            Some(200 * 1024 * 1024),
            "explicit set wins"
        );
        assert_eq!(read.keep_last, Some(5), "unset preserves on-disk");
    }

    #[test]
    fn dry_run_tallies_removed_invocations() {
        use crate::coordinator::clean_policy::{CleanPolicy, RepoPolicy};

        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().join("jobs").join("test-repo"));

        let now = chrono::Utc::now();
        // 1 invocation with 2 jobs, both far older than retention (override below).
        seed_inv_with_jobs(
            &store,
            "inv-old",
            "feat/x",
            now - chrono::Duration::days(30),
            2,
            100,
        );

        // keep_last=0 disables the sanity floor so the older invocation is
        // actually a candidate for removal.
        let policy = CleanPolicy {
            repo_policy: RepoPolicy {
                version: RepoPolicy::VERSION,
                max_total_size_bytes: None,
                keep_last: Some(0),
                stale_running_after_seconds: None,
            },
            dry_run: true,
            retention_override: Some(chrono::Duration::seconds(1)),
            default_retention: chrono::Duration::days(7),
        };
        let summary = store.clean(&policy).unwrap();
        assert_eq!(summary.removed_jobs, 2);
        assert_eq!(
            summary.removed_invocations, 1,
            "dry-run should tally would-be-removed invocations"
        );
        // Dry-run must not actually remove anything from disk.
        assert!(
            store.base_dir.join("inv-old").exists(),
            "dry-run must not remove invocation dir"
        );
    }
}
