//! Pure retention decision-making for job log cleanup.
//!
//! Given a snapshot of jobs (each with its current size on disk and
//! retention metadata) plus a [`CleanPolicy`], return a
//! [`RetentionDecision`] describing which jobs to mark stale-running and
//! which job directories to evict. The imperative shell ([`LogStore::clean`])
//! gathers the snapshot, calls [`retention`], then applies the decision
//! through the [`JobsStorePort`] (stale-running marks) and the filesystem
//! (atomic rename-then-remove).
//!
//! This module is the canonical FCIS extraction: pure function, value-in /
//! value-out, no `&dyn Port` callbacks during computation, no I/O, no
//! `Utc::now`. Tests construct [`RetentionInput`]s and assert directly
//! against the returned [`RetentionDecision`] — no traits, no mocks, no
//! tempdir setup.
//!
//! Reference: `ARCHITECTURE.md` "Functional core inside domain modules".
//!
//! [`CleanPolicy`]: crate::coordinator::clean_policy::CleanPolicy
//! [`LogStore::clean`]: crate::coordinator::log_store::LogStore::clean
//! [`JobsStorePort`]: crate::coordinator::ports::JobsStorePort

use crate::coordinator::clean_policy::CleanPolicy;
use crate::coordinator::log_store::JobStatus;
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One job's view from the snapshot the shell gathered. Carries
/// everything `retention` needs to decide; no filesystem or store
/// callbacks happen during computation.
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    pub invocation_id: String,
    pub name: String,
    pub worktree: String,
    pub status: JobStatus,
    pub started_at: DateTime<Utc>,
    pub retention_seconds: Option<i64>,
    pub log_size_bytes: u64,
}

/// Job that must be marked stale-running (was `Running` past its threshold
/// while the coordinator socket was absent). The shell flips the status to
/// `Cancelled` via [`JobsStorePort::upsert_job`].
///
/// [`JobsStorePort::upsert_job`]: crate::coordinator::ports::JobsStorePort::upsert_job
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleMarkTarget {
    pub invocation_id: String,
    pub name: String,
}

/// Job directory scheduled for eviction. `job_dir` is precomputed by the
/// pure function as `base_dir / invocation_id / job_name` so the shell
/// doesn't need to re-derive it.
#[derive(Debug, Clone)]
pub struct EvictionTarget {
    pub job_dir: PathBuf,
    pub invocation_id: String,
    pub job_name: String,
    pub worktree: String,
    pub size_bytes: u64,
}

/// Inputs to [`retention`]. Built by [`LogStore::clean`] from the
/// `JobsStorePort` listing + filesystem stats + the coordinator socket
/// probe + `Utc::now`.
///
/// [`LogStore::clean`]: crate::coordinator::log_store::LogStore::clean
#[derive(Debug, Clone)]
pub struct RetentionInput {
    /// Per-repo base dir; eviction paths are computed as
    /// `base_dir / invocation_id / name`.
    pub base_dir: PathBuf,
    pub jobs: Vec<JobSnapshot>,
    pub policy: CleanPolicy,
    /// `true` if the per-repo coordinator socket file exists. Drives the
    /// stale-running rule: a `Running` job older than
    /// `policy.repo_policy.stale_running_after_resolved()` is flipped to
    /// terminal only when the socket is absent (i.e. no live coordinator
    /// can plausibly still be running it).
    pub coordinator_socket_alive: bool,
    pub now: DateTime<Utc>,
}

/// Output of [`retention`]. The shell consumes this to produce a
/// `CleanSummary` and to drive port writes + filesystem ops.
#[derive(Debug, Clone, Default)]
pub struct RetentionDecision {
    /// Apply each via `JobsStorePort::upsert_job` with status set to
    /// `Cancelled` and `finished_at` set to `now`.
    pub stale_running_marks: Vec<StaleMarkTarget>,
    /// Job directories to remove. The shell uses
    /// rename-to-`.deleting-…` then `remove_dir_all` for atomicity; on
    /// crash the trash dirs are reaped by a later sweep.
    pub evictions: Vec<EvictionTarget>,
    /// Total job count per invocation. The shell compares this to
    /// [`Self::candidates_per_inv`] to decide whether the invocation
    /// directory itself can be removed.
    pub jobs_per_inv: BTreeMap<String, usize>,
    /// Eviction candidates per invocation. Subset of
    /// [`Self::jobs_per_inv`]; when equal, the entire invocation dir
    /// (including `invocation.json`) is removable.
    pub candidates_per_inv: BTreeMap<String, usize>,
    /// Pre-summed `size_bytes` across all evictions. Lets the shell
    /// short-circuit dry-run reporting without re-iterating.
    pub freed_bytes_predicted: u64,
}

/// Pure retention decision. See module doc.
pub fn retention(input: RetentionInput) -> RetentionDecision {
    let stale_threshold =
        Duration::try_seconds(input.policy.repo_policy.stale_running_after_resolved())
            .unwrap_or_else(|| Duration::seconds(86_400));

    let mut stale_running_marks: Vec<StaleMarkTarget> = Vec::new();
    let mut jobs_per_inv: BTreeMap<String, usize> = BTreeMap::new();
    // Group eviction-eligible jobs by worktree (the keep_last floor is
    // per-worktree). Running jobs are filtered out unless flipped to
    // stale-running, in which case they become eligible.
    let mut by_worktree: BTreeMap<String, Vec<JobSnapshot>> = BTreeMap::new();

    for job in &input.jobs {
        *jobs_per_inv.entry(job.invocation_id.clone()).or_default() += 1;

        let effective_status = if matches!(job.status, JobStatus::Running) {
            let age = input.now.signed_duration_since(job.started_at);
            if age > stale_threshold && !input.coordinator_socket_alive {
                stale_running_marks.push(StaleMarkTarget {
                    invocation_id: job.invocation_id.clone(),
                    name: job.name.clone(),
                });
                JobStatus::Cancelled
            } else {
                JobStatus::Running
            }
        } else {
            job.status.clone()
        };

        if matches!(effective_status, JobStatus::Running) {
            continue; // never evict still-running jobs
        }

        by_worktree
            .entry(job.worktree.clone())
            .or_default()
            .push(job.clone());
    }

    let keep_last = input.policy.repo_policy.keep_last_resolved();
    let mut evictions: Vec<EvictionTarget> = Vec::new();
    let mut candidates_per_inv: BTreeMap<String, usize> = BTreeMap::new();

    for (_worktree, entries) in by_worktree {
        // Group this worktree's eligible jobs by invocation so the
        // keep_last sanity floor counts invocations, not jobs.
        let mut by_inv: BTreeMap<String, Vec<JobSnapshot>> = BTreeMap::new();
        for job in entries {
            by_inv
                .entry(job.invocation_id.clone())
                .or_default()
                .push(job);
        }
        // Newest invocation first.
        let mut invs: Vec<(String, Vec<JobSnapshot>)> = by_inv.into_iter().collect();
        invs.sort_by_key(|(_, jobs)| {
            std::cmp::Reverse(jobs.iter().map(|j| j.started_at).max().unwrap_or(input.now))
        });

        for (idx, (inv_id, jobs)) in invs.into_iter().enumerate() {
            if idx < keep_last {
                continue; // sanity floor
            }
            for job in jobs {
                let retention = input.policy.retention_override.unwrap_or_else(|| {
                    job.retention_seconds
                        .and_then(Duration::try_seconds)
                        .unwrap_or(input.policy.default_retention)
                });
                if input.now.signed_duration_since(job.started_at) > retention {
                    let job_dir = input.base_dir.join(&inv_id).join(&job.name);
                    *candidates_per_inv.entry(inv_id.clone()).or_default() += 1;
                    evictions.push(EvictionTarget {
                        job_dir,
                        invocation_id: inv_id.clone(),
                        job_name: job.name.clone(),
                        worktree: job.worktree.clone(),
                        size_bytes: job.log_size_bytes,
                    });
                }
            }
        }
    }

    let freed_bytes_predicted = evictions.iter().map(|e| e.size_bytes).sum();

    RetentionDecision {
        stale_running_marks,
        evictions,
        jobs_per_inv,
        candidates_per_inv,
        freed_bytes_predicted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::clean_policy::RepoPolicy;
    use std::path::PathBuf;

    fn base() -> PathBuf {
        PathBuf::from("/state/jobs/repo")
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn job(name: &str, inv: &str, worktree: &str, age_secs: i64, size: u64) -> JobSnapshot {
        JobSnapshot {
            invocation_id: inv.into(),
            name: name.into(),
            worktree: worktree.into(),
            status: JobStatus::Completed,
            started_at: fixed_now() - Duration::seconds(age_secs),
            retention_seconds: None,
            log_size_bytes: size,
        }
    }

    fn default_policy() -> CleanPolicy {
        CleanPolicy {
            retention_override: None,
            dry_run: false,
            default_retention: Duration::days(7),
            repo_policy: RepoPolicy::defaults(),
        }
    }

    #[test]
    fn evicts_jobs_past_default_retention() {
        // keep_last default is 3, so we need at least 4 invocations
        // per worktree to push the oldest one past the floor.
        let jobs = vec![
            job("j", "inv1", "wt", 86_400 * 10, 100),
            job("j", "inv2", "wt", 86_400 * 5, 200),
            job("j", "inv3", "wt", 86_400 * 2, 300),
            job("j", "inv4", "wt", 60, 400),
        ];
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs,
            policy: default_policy(),
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        // Only inv1 (idx 3, oldest, age 10d > default 7d retention) gets
        // evicted. inv2/inv3/inv4 occupy the keep_last floor (3 newest).
        assert_eq!(decision.evictions.len(), 1);
        assert_eq!(decision.evictions[0].invocation_id, "inv1");
        assert_eq!(decision.freed_bytes_predicted, 100);
    }

    #[test]
    fn keep_last_floor_preserves_newest_invocations() {
        // All jobs are old enough to be evicted by retention, but
        // keep_last=2 must preserve the 2 newest invocations.
        let policy = CleanPolicy {
            repo_policy: RepoPolicy {
                keep_last: Some(2),
                ..RepoPolicy::defaults()
            },
            default_retention: Duration::seconds(1),
            ..default_policy()
        };
        let jobs = vec![
            job("j", "inv1", "wt", 1_000, 10),
            job("j", "inv2", "wt", 500, 20),
            job("j", "inv3", "wt", 100, 30),
        ];
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs,
            policy,
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        // inv2 + inv3 are the 2 newest; inv1 is past retention and below
        // the floor → evicted.
        assert_eq!(decision.evictions.len(), 1);
        assert_eq!(decision.evictions[0].invocation_id, "inv1");
    }

    #[test]
    fn per_job_retention_seconds_overrides_default() {
        // Default retention is 7d; per-job override gives this one
        // a 1-second retention. Three filler invocations occupy the
        // keep_last floor so inv-target can be a candidate.
        let mut target = job("j", "inv-target", "wt", 60, 50);
        target.retention_seconds = Some(1);
        let jobs = vec![
            target,
            job("j", "inv-keep1", "wt", 30, 0),
            job("j", "inv-keep2", "wt", 20, 0),
            job("j", "inv-keep3", "wt", 10, 0),
        ];
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs,
            policy: default_policy(),
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        assert_eq!(decision.evictions.len(), 1);
        assert_eq!(decision.evictions[0].invocation_id, "inv-target");
    }

    #[test]
    fn retention_override_supersedes_per_job_value() {
        let mut target = job("j", "inv1", "wt", 60, 0);
        target.retention_seconds = Some(86_400 * 30); // would normally keep
        let jobs = vec![
            target,
            job("j", "inv2", "wt", 50, 0),
            job("j", "inv3", "wt", 40, 0),
            job("j", "inv4", "wt", 30, 0),
        ];
        let policy = CleanPolicy {
            retention_override: Some(Duration::seconds(10)),
            ..default_policy()
        };
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs,
            policy,
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        // The override of 10s makes inv1 (60s old, idx=3 past floor) a
        // candidate even though its per-job retention was 30d.
        assert_eq!(decision.evictions.len(), 1);
        assert_eq!(decision.evictions[0].invocation_id, "inv1");
    }

    #[test]
    fn stale_running_marked_when_socket_missing() {
        let mut running = job("j", "inv1", "wt", 86_400 * 2, 0);
        running.status = JobStatus::Running;
        let policy = CleanPolicy {
            repo_policy: RepoPolicy {
                stale_running_after_seconds: Some(3_600),
                ..RepoPolicy::defaults()
            },
            ..default_policy()
        };
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs: vec![running],
            policy,
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        assert_eq!(decision.stale_running_marks.len(), 1);
        assert_eq!(decision.stale_running_marks[0].invocation_id, "inv1");
        assert_eq!(decision.stale_running_marks[0].name, "j");
    }

    #[test]
    fn stale_running_skipped_when_socket_alive() {
        let mut running = job("j", "inv1", "wt", 86_400 * 2, 0);
        running.status = JobStatus::Running;
        let policy = CleanPolicy {
            repo_policy: RepoPolicy {
                stale_running_after_seconds: Some(3_600),
                ..RepoPolicy::defaults()
            },
            ..default_policy()
        };
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs: vec![running],
            policy,
            coordinator_socket_alive: true,
            now: fixed_now(),
        });
        assert!(
            decision.stale_running_marks.is_empty(),
            "live coordinator must not flip Running → stale"
        );
        assert!(
            decision.evictions.is_empty(),
            "Running jobs must never be evicted"
        );
    }

    #[test]
    fn running_jobs_never_evicted_even_past_retention() {
        // Job is older than retention but still Running with live socket.
        let mut running = job("j", "inv1", "wt", 86_400 * 100, 0);
        running.status = JobStatus::Running;
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs: vec![
                running,
                job("j", "inv2", "wt", 50, 0),
                job("j", "inv3", "wt", 40, 0),
                job("j", "inv4", "wt", 30, 0),
            ],
            policy: default_policy(),
            coordinator_socket_alive: true,
            now: fixed_now(),
        });
        // No evictions: the three completed jobs are inside keep_last;
        // the running one is exempt regardless of age.
        assert!(decision.evictions.is_empty());
    }

    #[test]
    fn multi_worktree_keep_last_is_per_worktree() {
        // Two worktrees, each gets its own keep_last floor of 3.
        let policy = CleanPolicy {
            default_retention: Duration::seconds(1),
            ..default_policy()
        };
        let jobs = vec![
            job("j", "a1", "wt-a", 1_000, 1),
            job("j", "a2", "wt-a", 500, 2),
            job("j", "a3", "wt-a", 100, 3),
            job("j", "a4", "wt-a", 50, 4), // candidate (4th in wt-a)
            job("j", "b1", "wt-b", 1_000, 10),
            job("j", "b2", "wt-b", 100, 20),
            job("j", "b3", "wt-b", 50, 30), // safe (within floor)
        ];
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs,
            policy,
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        // wt-a's oldest (a1, idx=3 past floor) is the only candidate.
        // wt-b only has 3 jobs, all within keep_last=3.
        let inv_ids: Vec<_> = decision
            .evictions
            .iter()
            .map(|e| e.invocation_id.clone())
            .collect();
        assert_eq!(inv_ids, vec!["a1"]);
    }

    #[test]
    fn jobs_per_inv_and_candidates_per_inv_align_for_whole_inv_removal() {
        // Whole-invocation removal triggers when candidates == total for
        // a given invocation. Three jobs in inv1, all in wt past the
        // floor and past retention → all three are candidates → the
        // shell will see candidates_per_inv == jobs_per_inv and remove
        // the invocation directory itself.
        let policy = CleanPolicy {
            default_retention: Duration::seconds(1),
            repo_policy: RepoPolicy {
                keep_last: Some(0),
                ..RepoPolicy::defaults()
            },
            ..default_policy()
        };
        let jobs = vec![
            job("a", "inv1", "wt", 1_000, 10),
            job("b", "inv1", "wt", 1_000, 20),
            job("c", "inv1", "wt", 1_000, 30),
        ];
        let decision = retention(RetentionInput {
            base_dir: base(),
            jobs,
            policy,
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        assert_eq!(decision.jobs_per_inv.get("inv1"), Some(&3));
        assert_eq!(decision.candidates_per_inv.get("inv1"), Some(&3));
        assert_eq!(decision.freed_bytes_predicted, 60);
    }

    #[test]
    fn dry_run_does_not_affect_decision_shape() {
        // The pure function doesn't care about dry_run — that's a shell
        // concern. We just verify the same inputs produce the same
        // decision with dry_run flipped.
        let jobs = vec![
            job("j", "inv1", "wt", 86_400 * 10, 100),
            job("j", "inv2", "wt", 60, 0),
            job("j", "inv3", "wt", 40, 0),
            job("j", "inv4", "wt", 20, 0),
        ];
        let live = retention(RetentionInput {
            base_dir: base(),
            jobs: jobs.clone(),
            policy: default_policy(),
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        let dry = retention(RetentionInput {
            base_dir: base(),
            jobs,
            policy: CleanPolicy {
                dry_run: true,
                ..default_policy()
            },
            coordinator_socket_alive: false,
            now: fixed_now(),
        });
        assert_eq!(live.evictions.len(), dry.evictions.len());
        assert_eq!(live.freed_bytes_predicted, dry.freed_bytes_predicted);
    }
}
