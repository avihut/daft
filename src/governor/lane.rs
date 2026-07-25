//! Cross-process gate lane: a repo-scoped, width-1 admission lane for
//! gated merges (#775).
//!
//! The in-process governor cannot serialize two `daft merge` processes —
//! each process's min-one-runner liveness guarantee admits its own single
//! piped gate. But two concurrent gates in one repository are a
//! *correctness* problem, not just load: test suites with fixed scratch
//! paths corrupt each other and produce false reds. The lane makes the
//! serial-merge discipline construction instead of convention.
//!
//! Mechanism: an fs2 advisory `flock` on a sidecar file next to the repo's
//! coordinator DB (never the DB itself — #666's sidecar rule). Advisory
//! flock is kernel-owned, so a crashed or killed holder releases
//! automatically; the holder JSON inside the file is best-effort context
//! for the waiting announcement and may be stale, but the lock cannot be.
//! The coordinator deliberately plays no part: it is on-demand and
//! single-threaded, and a blocking acquire parked on it would starve its
//! accept loop.

use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::Path;

/// Filename of the lane's lock sidecar, next to the per-repo coordinator DB.
const LANE_LOCK_FILE: &str = "gate-lane.lock";

/// Best-effort holder record written into the lock file while held.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Holder {
    pid: u32,
    worktree: String,
    started_at: String,
}

/// An exclusively held gate lane. Dropping the guard (closing the file)
/// releases the lock.
pub struct GateLane {
    file: File,
}

impl GateLane {
    /// Acquire the repository's gate lane, blocking until it is free.
    ///
    /// When the lane is busy, announces who holds it (best-effort, from the
    /// holder record) on stderr before waiting, so a queued merge explains
    /// its own silence.
    pub fn acquire(repo_hash: &str, worktree: &Path) -> Result<Self> {
        let db_path = crate::store::paths::for_repo(repo_hash)
            .context("failed to resolve the repo's state directory for the gate lane")?;
        let lock_path = db_path
            .parent()
            .expect("for_repo always returns a path with a parent")
            .join(LANE_LOCK_FILE);

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&lock_path)
            .with_context(|| format!("failed to open gate lane {}", lock_path.display()))?;

        if file.try_lock_exclusive().is_err() {
            // Busy: read whoever wrote the holder record and announce the
            // wait. Reading file content needs no lock — advisory flock
            // gates nothing but other flocks.
            let mut contents = String::new();
            let holder = file
                .read_to_string(&mut contents)
                .ok()
                .and_then(|_| serde_json::from_str::<Holder>(&contents).ok());
            match holder {
                Some(h) => eprintln!(
                    "waiting for the merge gate lane — held by a merge in {} (pid {})",
                    h.worktree, h.pid
                ),
                None => eprintln!("waiting for the merge gate lane — held by another merge"),
            }
            file.lock_exclusive()
                .with_context(|| format!("failed to lock gate lane {}", lock_path.display()))?;
        }

        // Held. Stamp our holder record (advisory — the flock is the truth).
        let record = Holder {
            pid: std::process::id(),
            worktree: worktree.display().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        let _ = file.set_len(0);
        let _ = file.rewind();
        let _ = file.write_all(
            serde_json::to_string(&record)
                .unwrap_or_default()
                .as_bytes(),
        );
        let _ = file.flush();

        Ok(Self { file })
    }
}

impl Drop for GateLane {
    fn drop(&mut self) {
        // Explicit for clarity; closing the file would release it anyway.
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    #[serial]
    fn second_acquire_blocks_until_the_first_releases() {
        let _iso = crate::store::paths::IsolatedStateDir::new();
        let repo_hash = "lane-test-repo";
        let first =
            GateLane::acquire(repo_hash, Path::new("/wt/one")).expect("first acquire succeeds");

        let (tx, rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let lane =
                GateLane::acquire("lane-test-repo", Path::new("/wt/two")).expect("second acquire");
            tx.send(()).unwrap();
            drop(lane);
        });

        // The waiter must NOT get through while the first holds the lane.
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "second acquire must block while the lane is held"
        );

        drop(first);
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "second acquire must proceed once the lane is released"
        );
        waiter.join().unwrap();
    }

    #[test]
    #[serial]
    fn holder_record_is_written_while_held() {
        let _iso = crate::store::paths::IsolatedStateDir::new();
        let repo_hash = "lane-holder-repo";
        let lane = GateLane::acquire(repo_hash, Path::new("/wt/holder")).expect("acquire");

        let lock_path = crate::store::paths::for_repo(repo_hash)
            .unwrap()
            .parent()
            .unwrap()
            .join(LANE_LOCK_FILE);
        let contents = std::fs::read_to_string(&lock_path).unwrap();
        let holder: Holder = serde_json::from_str(&contents).expect("holder JSON parses");
        assert_eq!(holder.pid, std::process::id());
        assert_eq!(holder.worktree, "/wt/holder");
        drop(lane);
    }

    #[test]
    #[serial]
    fn distinct_repos_have_independent_lanes() {
        let _iso = crate::store::paths::IsolatedStateDir::new();
        let a = GateLane::acquire("lane-repo-a", Path::new("/wt/a")).expect("repo a");
        // A different repo's lane must not block.
        let b = GateLane::acquire("lane-repo-b", Path::new("/wt/b")).expect("repo b");
        drop(a);
        drop(b);
    }
}
