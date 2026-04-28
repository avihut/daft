//! Cleanup policy types and string parsers shared by hook-fire and cleanup paths.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// What the cleanup pass should do on this run.
#[derive(Debug, Clone)]
pub struct CleanPolicy {
    /// Override retention for all jobs to this value. None = use per-job
    /// `retention_seconds` from JobMeta.
    pub retention_override: Option<chrono::Duration>,
    /// If true, list candidates but do not remove anything.
    pub dry_run: bool,
    /// Default retention when JobMeta has no `retention_seconds`. Falls back
    /// to 7 days.
    pub default_retention: chrono::Duration,
    /// Repo-level policy for sanity floor and stale-Running detection.
    pub repo_policy: RepoPolicy,
}

impl Default for CleanPolicy {
    fn default() -> Self {
        Self {
            retention_override: None,
            dry_run: false,
            default_retention: chrono::Duration::days(7),
            repo_policy: RepoPolicy::defaults(),
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct CleanSummary {
    pub removed_invocations: usize,
    pub removed_jobs: usize,
    pub freed_bytes: u64,
    pub truncated_logs: usize,
    pub stale_running_marked: usize,
    /// One-line human reason: "retention", "budget", "stale-running", "mixed".
    pub reason: String,
    /// Set of (worktree, invocation_id, job_name) candidates considered for
    /// removal — used by `--dry-run`.
    pub candidates: Vec<(String, String, String)>,
}

/// Repo-level cleanup policy persisted to `<state>/jobs/<repo-uuid>/repo-policy.json`.
/// Written on every hook fire (most-recent wins). Read by cleanup at run time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoPolicy {
    pub version: u32,
    #[serde(default)]
    pub max_total_size_bytes: Option<u64>,
    #[serde(default)]
    pub keep_last: Option<usize>,
    #[serde(default)]
    pub stale_running_after_seconds: Option<i64>,
}

impl RepoPolicy {
    pub const VERSION: u32 = 1;
    pub const DEFAULT_MAX_TOTAL_SIZE: u64 = 500 * 1024 * 1024;
    pub const DEFAULT_KEEP_LAST: usize = 3;
    pub const DEFAULT_STALE_RUNNING_AFTER_SECONDS: i64 = 86_400;

    pub fn defaults() -> Self {
        Self {
            version: Self::VERSION,
            max_total_size_bytes: None,
            keep_last: None,
            stale_running_after_seconds: None,
        }
    }

    pub fn max_total_size_resolved(&self) -> u64 {
        self.max_total_size_bytes
            .unwrap_or(Self::DEFAULT_MAX_TOTAL_SIZE)
    }

    pub fn keep_last_resolved(&self) -> usize {
        self.keep_last.unwrap_or(Self::DEFAULT_KEEP_LAST)
    }

    pub fn stale_running_after_resolved(&self) -> i64 {
        self.stale_running_after_seconds
            .unwrap_or(Self::DEFAULT_STALE_RUNNING_AFTER_SECONDS)
    }
}

/// Outcome of a single budget-enforcement pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BudgetOutcome {
    pub evicted_invocations: usize,
    pub freed_bytes: u64,
    pub freed_jobs: usize,
}

/// Build a `RepoPolicy` by inspecting the merged `LogConfig` of any job in the
/// hook fire. The repo-level fields (`max_total_size`, `keep_last`,
/// `stale_running_after`) should already be merged identically across jobs by
/// `merge_log_configs`, so first-non-None wins.
pub fn build_repo_policy(specs: &[crate::executor::JobSpec]) -> RepoPolicy {
    let mut policy = RepoPolicy::defaults();

    for spec in specs {
        let Some(lc) = spec.log_config.as_ref() else {
            continue;
        };

        if policy.max_total_size_bytes.is_none() {
            if let Some(s) = lc.max_total_size.as_deref() {
                if let Ok(n) = parse_size(s) {
                    policy.max_total_size_bytes = Some(n);
                }
            }
        }
        if policy.keep_last.is_none() {
            if let Some(n) = lc.keep_last {
                policy.keep_last = Some(n);
            }
        }
        if policy.stale_running_after_seconds.is_none() {
            if let Some(s) = lc.stale_running_after.as_deref() {
                if let Ok(n) = parse_duration_str(s) {
                    policy.stale_running_after_seconds = Some(n as i64);
                }
            }
        }
    }

    policy
}

/// Parse a size string into bytes. Accepts: `1024`, `1KB`, `10MB`, `2GB`.
/// Case-insensitive, no spaces. Plain integer = bytes.
pub fn parse_size(input: &str) -> Result<u64> {
    let s = input.trim();
    let upper = s.to_ascii_uppercase();
    let (num_str, multiplier): (&str, u64) = if let Some(n) = upper.strip_suffix("GB") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = upper.strip_suffix("MB") {
        (n, 1024 * 1024)
    } else if let Some(n) = upper.strip_suffix("KB") {
        (n, 1024)
    } else if let Some(n) = upper.strip_suffix('B') {
        (n, 1)
    } else {
        (upper.as_str(), 1)
    };
    let n: u64 = num_str
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid size: {input}"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| anyhow!("size overflow: {input}"))
}

/// Parse a duration string into seconds. Accepts: `30m`, `24h`, `7d`.
pub fn parse_duration_str(input: &str) -> Result<u64> {
    let s = input.trim();
    let (num_str, multiplier): (&str, u64) = if let Some(n) = s.strip_suffix('d') {
        (n, 86_400)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1)
    } else {
        return Err(anyhow!(
            "invalid duration: {input} (expected suffix d/h/m/s)"
        ));
    };
    let n: u64 = num_str
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid duration: {input}"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| anyhow!("duration overflow: {input}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_plain_integer() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }

    #[test]
    fn parse_size_with_units() {
        assert_eq!(parse_size("1KB").unwrap(), 1024);
        assert_eq!(parse_size("10MB").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("2GB").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_case_insensitive() {
        assert_eq!(parse_size("10mb").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("10Mb").unwrap(), 10 * 1024 * 1024);
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("abc").is_err());
        assert!(parse_size("10XB").is_err());
        assert!(parse_size("").is_err());
    }

    #[test]
    fn parse_duration_basic() {
        assert_eq!(parse_duration_str("30s").unwrap(), 30);
        assert_eq!(parse_duration_str("5m").unwrap(), 300);
        assert_eq!(parse_duration_str("24h").unwrap(), 86_400);
        assert_eq!(parse_duration_str("7d").unwrap(), 7 * 86_400);
    }

    #[test]
    fn parse_duration_rejects_no_suffix() {
        assert!(parse_duration_str("60").is_err());
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert!(parse_duration_str("abc").is_err());
        assert!(parse_duration_str("5y").is_err());
    }

    #[test]
    fn parse_size_rejects_overflow() {
        assert!(parse_size("99999999999999GB").is_err());
    }

    #[test]
    fn parse_size_handles_leading_whitespace() {
        assert_eq!(parse_size("  10MB  ").unwrap(), 10 * 1024 * 1024);
    }

    #[test]
    fn parse_duration_rejects_negative() {
        assert!(parse_duration_str("-5m").is_err());
    }

    #[test]
    fn parse_duration_rejects_overflow() {
        // u64::MAX is ~1.84e19; multiplying by 86_400 overflows for any value >= ~2.1e14.
        assert!(parse_duration_str("999999999999999999d").is_err());
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    use crate::executor::{JobSpec, LogConfig};

    #[test]
    fn repo_policy_defaults_resolve() {
        let p = RepoPolicy::defaults();
        assert_eq!(p.max_total_size_resolved(), 500 * 1024 * 1024);
        assert_eq!(p.keep_last_resolved(), 3);
        assert_eq!(p.stale_running_after_resolved(), 86_400);
    }

    #[test]
    fn repo_policy_round_trips() {
        let p = RepoPolicy {
            version: 1,
            max_total_size_bytes: Some(1024 * 1024 * 1024),
            keep_last: Some(5),
            stale_running_after_seconds: Some(3_600),
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: RepoPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn build_repo_policy_first_log_config_wins() {
        let s1 = JobSpec {
            name: "a".into(),
            log_config: Some(LogConfig {
                keep_last: Some(5),
                ..Default::default()
            }),
            ..Default::default()
        };
        let s2 = JobSpec {
            name: "b".into(),
            log_config: Some(LogConfig {
                keep_last: Some(99),
                ..Default::default()
            }),
            ..Default::default()
        };
        let policy = build_repo_policy(&[s1, s2]);
        assert_eq!(policy.keep_last, Some(5));
    }

    #[test]
    fn build_repo_policy_skips_jobs_without_log_config() {
        let s1 = JobSpec {
            name: "a".into(),
            log_config: None,
            ..Default::default()
        };
        let s2 = JobSpec {
            name: "b".into(),
            log_config: Some(LogConfig {
                keep_last: Some(7),
                ..Default::default()
            }),
            ..Default::default()
        };
        let policy = build_repo_policy(&[s1, s2]);
        assert_eq!(policy.keep_last, Some(7));
    }

    #[test]
    fn build_repo_policy_empty_specs_returns_defaults() {
        let policy = build_repo_policy(&[]);
        assert_eq!(policy, RepoPolicy::defaults());
    }

    #[test]
    fn build_repo_policy_ignores_garbage_values() {
        let spec = JobSpec {
            name: "j".into(),
            log_config: Some(LogConfig {
                max_total_size: Some("not-a-size".into()),
                stale_running_after: Some("nope".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let policy = build_repo_policy(std::slice::from_ref(&spec));
        assert_eq!(policy.max_total_size_bytes, None);
        assert_eq!(policy.stale_running_after_seconds, None);
    }
}
