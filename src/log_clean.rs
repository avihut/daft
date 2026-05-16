//! Background log cleanup.
//!
//! Mirrors the trust_prune.rs pattern: every daft invocation calls
//! maybe_clean_logs(), which checks a 24h cache and spawns a detached
//! `daft __clean-logs` child if stale. Single-flight enforced via flock.
//! Zero latency on the hot path.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::coordinator::clean_policy::{CleanPolicy, CleanSummary};

pub const NO_LOG_CLEAN_ENV: &str = "DAFT_NO_LOG_CLEAN";
const CACHE_TTL_SECONDS: i64 = 24 * 60 * 60;
const CACHE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct LogCleanCache {
    pub version: u32,
    pub cleaned_at: i64,
    #[serde(default)]
    pub last_summary: Option<LastSummary>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LastSummary {
    pub removed_invocations: usize,
    pub removed_jobs: usize,
    pub freed_bytes: u64,
    pub reason: String,
}

pub fn maybe_clean_logs() {
    let _ = std::panic::catch_unwind(maybe_clean_logs_inner);
}

fn maybe_clean_logs_inner() {
    if env::args().any(|a| a.starts_with("__")) {
        return;
    }
    if is_disabled() {
        return;
    }
    let path = match cache_path() {
        Ok(p) => p,
        Err(_) => return,
    };
    let cache = load_cache(&path);
    match &cache {
        Some(c) if !is_cache_stale(c) => {}
        _ => {
            let _ = spawn_background();
        }
    }
}

pub fn run_clean_logs() -> Result<()> {
    use crate::coordinator::adapters::SqliteJobsStore;
    use crate::coordinator::log_store::LogStore;
    use crate::coordinator::ports::JobsStorePort;

    // Single-flight lock.
    let lock_path = cache_path()?.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let lock_file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .context("Failed to open lock file")?;
    use fs2::FileExt;
    if lock_file.try_lock_exclusive().is_err() {
        return Ok(()); // another cleanup is running
    }

    // Iterate all repos under the state dir.
    let jobs_dir = crate::daft_state_dir()?.join("jobs");
    if !jobs_dir.exists() {
        write_cache_with_summary(None)?;
        return Ok(());
    }

    let mut total_summary = CleanSummary {
        reason: "auto".into(),
        ..CleanSummary::default()
    };

    for entry in fs::read_dir(&jobs_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if uuid::Uuid::parse_str(&name).is_err() {
            continue;
        }

        let store = LogStore::for_repo(&name)?;
        let job_store = match SqliteJobsStore::for_repo_base(&store.base_dir) {
            Ok(js) => js,
            Err(_) => continue, // can't retention-sweep without the store
        };
        let repo_policy = job_store
            .read_repo_policy(&name)
            .unwrap_or_else(|_| crate::coordinator::clean_policy::RepoPolicy::defaults());

        // 1. Retention sweep.
        let policy = CleanPolicy {
            repo_policy: repo_policy.clone(),
            ..CleanPolicy::default()
        };
        let s = store.clean(&job_store, &name, &policy).unwrap_or_default();
        total_summary.removed_invocations += s.removed_invocations;
        total_summary.removed_jobs += s.removed_jobs;
        total_summary.freed_bytes += s.freed_bytes;
        total_summary.stale_running_marked += s.stale_running_marked;

        // 2. Budget post-pass.
        let bo = store.enforce_budget(&repo_policy).unwrap_or_default();
        total_summary.removed_invocations += bo.evicted_invocations;
        total_summary.removed_jobs += bo.freed_jobs;
        total_summary.freed_bytes += bo.freed_bytes;
    }

    let last_summary = LastSummary {
        removed_invocations: total_summary.removed_invocations,
        removed_jobs: total_summary.removed_jobs,
        freed_bytes: total_summary.freed_bytes,
        reason: total_summary.reason,
    };
    write_cache_with_summary(Some(last_summary))?;

    Ok(())
}

fn cache_path() -> Result<PathBuf> {
    Ok(crate::daft_config_dir()?.join("log-clean.json"))
}

fn load_cache(path: &PathBuf) -> Option<LogCleanCache> {
    let s = fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn write_cache_with_summary(summary: Option<LastSummary>) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("System clock error")?
        .as_secs() as i64;
    let cache = LogCleanCache {
        version: CACHE_VERSION,
        cleaned_at: now,
        last_summary: summary,
    };
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let s = serde_json::to_string_pretty(&cache)?;
    fs::write(&path, s)?;
    Ok(())
}

fn is_cache_stale(cache: &LogCleanCache) -> bool {
    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => return true,
    };
    let age = now - cache.cleaned_at;
    !(0..=CACHE_TTL_SECONDS).contains(&age)
}

fn spawn_background() -> Result<()> {
    let exe = env::current_exe().context("Could not determine current executable")?;
    Command::new(exe)
        .arg("__clean-logs")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn background log cleanup")?;
    Ok(())
}

fn is_disabled() -> bool {
    if env::var(NO_LOG_CLEAN_ENV).is_ok() {
        return true;
    }
    crate::trust_prune::is_ci_environment()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trip() {
        let c = LogCleanCache {
            version: 1,
            cleaned_at: 1745740800,
            last_summary: Some(LastSummary {
                removed_invocations: 3,
                removed_jobs: 12,
                freed_bytes: 123_456,
                reason: "auto".into(),
            }),
        };
        let s = serde_json::to_string(&c).unwrap();
        let back: LogCleanCache = serde_json::from_str(&s).unwrap();
        assert_eq!(back.version, 1);
        assert_eq!(back.cleaned_at, 1745740800);
        assert_eq!(back.last_summary.unwrap().removed_jobs, 12);
    }

    #[test]
    fn is_cache_stale_for_old() {
        let c = LogCleanCache {
            version: 1,
            cleaned_at: 0,
            last_summary: None,
        };
        assert!(is_cache_stale(&c));
    }

    #[test]
    fn is_cache_stale_for_future() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let c = LogCleanCache {
            version: 1,
            cleaned_at: now + 100_000,
            last_summary: None,
        };
        assert!(is_cache_stale(&c));
    }

    #[test]
    fn test_maybe_clean_logs_does_not_panic() {
        // The catch_unwind wrapper inside maybe_clean_logs() should absorb any
        // panics. This test simply ensures the function completes (success or
        // controlled return) without unwinding.
        super::maybe_clean_logs();
    }
}
