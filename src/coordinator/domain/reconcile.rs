//! Crash recovery: mark jobs still tagged `Running`/`Cancelling` as
//! `Crashed` when the recorded process group is gone.
//!
//! Pure logic — talks to storage via [`JobsStorePort`], probes the system
//! via [`ProcessControl`], and reads time via [`Clock`]. The unit tests
//! below drive it with in-memory fakes implementing those traits.

use crate::coordinator::ports::{Clock, JobsStorePort, ProcessControl};
use anyhow::Result;

/// Statuses (as stored on disk) that the reconciler attempts to repair.
/// Mirrors `list_active_jobs` on the port.
pub const ACTIVE_STATUSES: &[&str] = &["running", "cancelling"];

/// On-disk value the reconciler writes for jobs whose process group is gone.
pub const CRASHED_STATUS: &str = "crashed";

/// Probe every active job for `repo_hash`. Returns the number of jobs
/// updated to [`CRASHED_STATUS`]. Errors writing an individual update are
/// logged to stderr; we don't abort the whole pass on one bad row.
pub fn reconcile_active_jobs(
    store: &dyn JobsStorePort,
    process: &dyn ProcessControl,
    clock: &dyn Clock,
    repo_hash: &str,
) -> Result<usize> {
    let active = store.list_active_jobs(repo_hash)?;
    if active.is_empty() {
        return Ok(0);
    }

    let now = clock.now();
    let mut marked = 0usize;
    for mut row in active {
        let alive = matches!(row.pgid, Some(pgid) if pgid > 0)
            && process.process_group_alive(row.pgid.expect("just matched Some"));
        if alive {
            continue;
        }
        row.status = CRASHED_STATUS.to_string();
        row.finished_at = Some(now);
        match store.upsert_job(&row) {
            Ok(()) => marked += 1,
            Err(e) => eprintln!(
                "daft: reconcile failed to mark '{}' as crashed: {e}",
                row.name
            ),
        }
    }
    if marked > 0 {
        eprintln!("daft: reconcile marked {marked} crashed job(s) from a previous coordinator");
    }
    Ok(marked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::clean_policy::RepoPolicy;
    use crate::store::models::{InvocationRow, JobRow};
    use chrono::{DateTime, TimeZone, Utc};
    use std::collections::HashMap;
    use std::sync::Mutex;

    // ---- Fakes --------------------------------------------------------

    #[derive(Default)]
    struct FakeStore {
        jobs: Mutex<Vec<JobRow>>,
    }

    impl FakeStore {
        fn with_jobs(rows: Vec<JobRow>) -> Self {
            Self {
                jobs: Mutex::new(rows),
            }
        }

        fn snapshot(&self) -> Vec<JobRow> {
            self.jobs.lock().unwrap().clone()
        }
    }

    impl JobsStorePort for FakeStore {
        fn upsert_invocation(&self, _row: &InvocationRow) -> Result<()> {
            Ok(())
        }

        fn get_invocation(&self, _r: &str, _i: &str) -> Result<Option<InvocationRow>> {
            Ok(None)
        }

        fn upsert_job(&self, row: &JobRow) -> Result<()> {
            let mut guard = self.jobs.lock().unwrap();
            if let Some(slot) = guard.iter_mut().find(|r| {
                r.repo_hash == row.repo_hash
                    && r.invocation_id == row.invocation_id
                    && r.name == row.name
            }) {
                *slot = row.clone();
            } else {
                guard.push(row.clone());
            }
            Ok(())
        }

        fn get_job(&self, r: &str, i: &str, n: &str) -> Result<Option<JobRow>> {
            Ok(self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .find(|row| row.repo_hash == r && row.invocation_id == i && row.name == n)
                .cloned())
        }

        fn list_jobs_for_repo(&self, repo_hash: &str) -> Result<Vec<JobRow>> {
            Ok(self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.repo_hash == repo_hash)
                .cloned()
                .collect())
        }

        fn list_jobs_for_invocation(&self, r: &str, i: &str) -> Result<Vec<JobRow>> {
            Ok(self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|row| row.repo_hash == r && row.invocation_id == i)
                .cloned()
                .collect())
        }

        fn list_active_jobs(&self, repo_hash: &str) -> Result<Vec<JobRow>> {
            Ok(self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .filter(|r| {
                    r.repo_hash == repo_hash && ACTIVE_STATUSES.contains(&r.status.as_str())
                })
                .cloned()
                .collect())
        }

        fn read_repo_policy(&self, _repo_hash: &str) -> Result<RepoPolicy> {
            Ok(RepoPolicy::defaults())
        }

        fn write_repo_policy(&self, _repo_hash: &str, _policy: &RepoPolicy) -> Result<()> {
            Ok(())
        }
    }

    struct FakeProcess {
        alive_pgids: Vec<u32>,
    }

    impl ProcessControl for FakeProcess {
        fn process_group_alive(&self, pgid: u32) -> bool {
            self.alive_pgids.contains(&pgid)
        }

        fn signal_process_group(&self, _pgid: u32, _signal: i32) -> Result<()> {
            Ok(())
        }
    }

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn job(name: &str, pgid: Option<u32>, status: &str) -> JobRow {
        JobRow {
            repo_hash: "r".into(),
            invocation_id: "inv".into(),
            name: name.into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/x".into(),
            command: "echo".into(),
            working_dir: "/tmp".into(),
            env: HashMap::new(),
            started_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            finished_at: None,
            status: status.into(),
            exit_code: None,
            pid: pgid,
            pgid,
            background: true,
            needs: vec![],
            tags: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
        }
    }

    // ---- Tests --------------------------------------------------------

    #[test]
    fn marks_dead_running_as_crashed() {
        let store = FakeStore::with_jobs(vec![job("dead", Some(99), "running")]);
        let process = FakeProcess {
            alive_pgids: vec![],
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        let n = reconcile_active_jobs(&store, &process, &FixedClock(now), "r").unwrap();
        assert_eq!(n, 1);
        let saved = store.snapshot().into_iter().next().unwrap();
        assert_eq!(saved.status, CRASHED_STATUS);
        assert_eq!(saved.finished_at, Some(now));
    }

    #[test]
    fn leaves_alive_running_intact() {
        let store = FakeStore::with_jobs(vec![job("alive", Some(42), "running")]);
        let process = FakeProcess {
            alive_pgids: vec![42],
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        let n = reconcile_active_jobs(&store, &process, &FixedClock(now), "r").unwrap();
        assert_eq!(n, 0);
        let saved = store.snapshot().into_iter().next().unwrap();
        assert_eq!(saved.status, "running");
        assert!(saved.finished_at.is_none());
    }

    #[test]
    fn job_without_pgid_is_treated_as_crashed() {
        let store = FakeStore::with_jobs(vec![job("ghost", None, "running")]);
        let process = FakeProcess {
            alive_pgids: vec![],
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        let n = reconcile_active_jobs(&store, &process, &FixedClock(now), "r").unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn cancelling_status_is_also_reconciled() {
        let store = FakeStore::with_jobs(vec![job("cancelling", Some(7), "cancelling")]);
        let process = FakeProcess {
            alive_pgids: vec![],
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        let n = reconcile_active_jobs(&store, &process, &FixedClock(now), "r").unwrap();
        assert_eq!(n, 1);
        let saved = store.snapshot().into_iter().next().unwrap();
        assert_eq!(saved.status, CRASHED_STATUS);
    }

    #[test]
    fn terminal_statuses_are_not_touched() {
        let store = FakeStore::with_jobs(vec![
            job("done", Some(99), "completed"),
            job("oops", Some(99), "failed"),
            job("stop", Some(99), "cancelled"),
        ]);
        let process = FakeProcess {
            alive_pgids: vec![],
        };
        let now = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
        let n = reconcile_active_jobs(&store, &process, &FixedClock(now), "r").unwrap();
        assert_eq!(n, 0);
        // None of the rows were rewritten with `crashed`.
        let saved = store.snapshot();
        for row in saved {
            assert_ne!(row.status, CRASHED_STATUS, "row {} was mutated", row.name);
        }
    }

    #[test]
    fn other_repos_are_left_alone() {
        let mut other = job("other", Some(99), "running");
        other.repo_hash = "other-repo".into();
        let store = FakeStore::with_jobs(vec![other]);
        let process = FakeProcess {
            alive_pgids: vec![],
        };
        let now = Utc::now();
        let n = reconcile_active_jobs(&store, &process, &FixedClock(now), "r").unwrap();
        assert_eq!(n, 0);
        let saved = store.snapshot().into_iter().next().unwrap();
        assert_eq!(saved.status, "running");
    }
}
