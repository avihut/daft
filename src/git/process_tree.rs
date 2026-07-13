//! Point-in-time process-table snapshots and per-PID freeze/thaw sweeps.
//!
//! Two consumers with different jobs share this walk: the cancellation
//! cascade (`git::cancel`) collects descendant *process groups* to tear a
//! supervised child's tree down, and the resource governor (#678 stage 3)
//! collects descendant *pids* to SIGSTOP/SIGCONT a push unit's hook
//! subtree. The governor must never signal the `git push` leader itself —
//! a stopped leader reads `T` to the supervisor's tty-stop probe, which
//! would misfire the teardown as `NeedsTerminal` — hence per-PID sweeps
//! that exclude the root rather than group signals.

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::process::Stdio;

pub(crate) struct ProcessRow {
    pub(crate) ppid: u32,
    pub(crate) pgid: u32,
}

/// Point-in-time view of the system process table, sufficient to walk
/// parent→child links and group membership.
pub(crate) struct ProcessSnapshot {
    rows: HashMap<u32, ProcessRow>,
}

impl ProcessSnapshot {
    /// Best-effort capture via `ps`. `-axo pid=,ppid=,pgid=,stat=` is
    /// portable across macOS and Linux procps; `=` suppresses headers.
    pub(crate) fn capture() -> Option<Self> {
        let out = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid=,pgid=,stat="])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(Self::parse(&String::from_utf8_lossy(&out.stdout)))
    }

    pub(crate) fn parse(text: &str) -> Self {
        let mut rows = HashMap::new();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            // The stat column isn't stored, but requiring all four
            // fields rejects truncated/garbled rows wholesale.
            let (Some(pid), Some(ppid), Some(pgid), Some(_stat)) =
                (it.next(), it.next(), it.next(), it.next())
            else {
                continue;
            };
            let (Ok(pid), Ok(ppid), Ok(pgid)) =
                (pid.parse::<u32>(), ppid.parse::<u32>(), pgid.parse::<u32>())
            else {
                continue;
            };
            rows.insert(pid, ProcessRow { ppid, pgid });
        }
        Self { rows }
    }

    /// Distinct pgids of `root_pid` and every process transitively
    /// parented under it. Misses processes that already reparented to
    /// PID 1 — which is exactly why the cancel cascade keeps a cumulative
    /// record of everything it ever signaled.
    pub(crate) fn descendant_pgids(&self, root_pid: u32) -> BTreeSet<u32> {
        let mut pgids = BTreeSet::new();
        self.walk(root_pid, |pid, row| {
            let _ = pid;
            pgids.insert(row.pgid);
        });
        pgids
    }

    /// Pids of `root_pid` (when present in the table) and every process
    /// transitively parented under it. Same reparenting caveat as
    /// [`Self::descendant_pgids`]; the freeze sweep compensates with a
    /// cumulative stopped-set and repeated passes.
    pub(crate) fn descendant_pids(&self, root_pid: u32) -> BTreeSet<u32> {
        let mut pids = BTreeSet::new();
        self.walk(root_pid, |pid, _row| {
            pids.insert(pid);
        });
        pids
    }

    fn walk(&self, root_pid: u32, mut visit: impl FnMut(u32, &ProcessRow)) {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (&pid, row) in &self.rows {
            children.entry(row.ppid).or_default().push(pid);
        }
        let mut stack = vec![root_pid];
        // Pid reuse between `ps` rows can in principle produce a
        // parent cycle; the seen-set makes the walk immune.
        let mut seen = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            if let Some(row) = self.rows.get(&pid) {
                visit(pid, row);
            }
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids.iter().copied());
            }
        }
    }
}

/// How many snapshot→stop passes a freeze sweep makes before giving up on
/// convergence. A stopped process cannot fork, so each pass can only be
/// racing children forked before their parent froze; in practice one or
/// two passes suffice.
const MAX_FREEZE_PASSES: usize = 5;

/// SIGSTOP every live descendant of `root_pid` — never `root_pid` itself
/// (see the module doc) — accumulating stopped pids into `stopped` so a
/// later [`thaw_pids`] resumes exactly what was frozen. Re-snapshots until
/// a pass stops nothing new. Idempotent: already-recorded pids are
/// skipped, so calling again while frozen only picks up stragglers.
/// Returns how many pids this call newly stopped.
pub(crate) fn freeze_descendants(root_pid: u32, stopped: &mut BTreeSet<u32>) -> usize {
    let mut newly_stopped = 0;
    for _ in 0..MAX_FREEZE_PASSES {
        let Some(snapshot) = ProcessSnapshot::capture() else {
            break;
        };
        let mut pass_new = 0;
        for pid in snapshot.descendant_pids(root_pid) {
            if pid == root_pid || stopped.contains(&pid) {
                continue;
            }
            // ESRCH (gone between snapshot and signal) is fine — skip.
            if kill(Pid::from_raw(pid as i32), Signal::SIGSTOP).is_ok() {
                stopped.insert(pid);
                pass_new += 1;
            }
        }
        newly_stopped += pass_new;
        if pass_new == 0 {
            break;
        }
    }
    newly_stopped
}

/// SIGCONT every pid in `stopped`. ESRCH (already gone) is ignored; a
/// pid another actor stopped resumes too — acceptable, nothing daft
/// supervises is legitimately stopped while the governor thaws.
pub(crate) fn thaw_pids(stopped: &BTreeSet<u32>) {
    for &pid in stopped {
        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGCONT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_malformed_rows() {
        let snap = ProcessSnapshot::parse(
            "  100     1   100 Ss+\n\
             garbage line\n\
             200 100 200 T\n\
             201 200 200 t\n\
             x y z w\n\
             300 201\n",
        );
        assert_eq!(snap.rows.len(), 3);
        assert!(snap.rows.contains_key(&100));
        assert!(snap.rows.contains_key(&200));
        assert!(snap.rows.contains_key(&201));
        // Three-token row (stat missing) is rejected wholesale.
        assert!(!snap.rows.contains_key(&300));
    }

    #[test]
    fn descendant_walk_collects_foreign_groups_but_not_reparented_orphans() {
        // Incident-shaped tree: git(200) → hook sh(201) share the
        // child's group; mise(300) and cargo(301) each setpgid'd
        // into their own; 400 already reparented to init.
        let snap = ProcessSnapshot::parse(
            "  100     1   100 Ss\n\
             200 100 200 S\n\
             201 200 200 S\n\
             300 201 300 T\n\
             301 300 301 T\n\
             400 1 400 T\n",
        );
        let pgids = snap.descendant_pgids(200);
        assert_eq!(pgids, BTreeSet::from([200, 300, 301]));
        // The walk cannot see 400 — this is why GroupCascade keeps
        // the cumulative signaled set across ticks.
        assert!(!pgids.contains(&400));
        // The pid walk sees the same tree, by pid.
        assert_eq!(
            snap.descendant_pids(200),
            BTreeSet::from([200, 201, 300, 301])
        );
    }

    #[test]
    fn walk_survives_ppid_cycles() {
        let snap = ProcessSnapshot::parse("200 201 200 S\n201 200 200 S\n");
        assert_eq!(snap.descendant_pgids(200), BTreeSet::from([200]));
    }

    #[test]
    fn parse_rejects_garbled_rows_and_walks_descendants() {
        let snapshot = ProcessSnapshot::parse(
            "  1     0     1  Ss\n\
             10     1    10  S\n\
             11    10    10  S\n\
             12    11    12  R+\n\
             garbage row\n\
             13    notanum 13 S\n",
        );
        let pids = snapshot.descendant_pids(10);
        assert_eq!(pids.into_iter().collect::<Vec<_>>(), vec![10, 11, 12]);
        let pgids = snapshot.descendant_pgids(10);
        assert_eq!(pgids.into_iter().collect::<Vec<_>>(), vec![10, 12]);
        // A root absent from the table walks to nothing.
        assert!(snapshot.descendant_pids(999).is_empty());
    }

    #[test]
    fn freeze_excludes_root_and_thaw_resumes() {
        // A pipeline forces sh to fork children instead of exec'ing.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 2 | sleep 2"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let root = child.id();
        // Let the pipeline children spawn.
        std::thread::sleep(std::time::Duration::from_millis(150));

        let mut stopped = BTreeSet::new();
        let newly = freeze_descendants(root, &mut stopped);
        assert!(newly >= 1, "the pipeline children must freeze");
        assert!(
            !stopped.contains(&root),
            "the root pid must never be signaled"
        );
        assert_eq!(
            crate::git::cancel::pid_stopped(root),
            Some(false),
            "the root must still be running (supervisor probe safety)"
        );
        let frozen = *stopped.iter().next().unwrap();
        assert_eq!(crate::git::cancel::pid_stopped(frozen), Some(true));

        // Re-freezing while frozen is a no-op (cumulative set).
        assert_eq!(freeze_descendants(root, &mut stopped), 0);

        thaw_pids(&stopped);
        assert_eq!(crate::git::cancel::pid_stopped(frozen), Some(false));

        // Cleanup: no orphans past the test.
        for &pid in &stopped {
            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}
