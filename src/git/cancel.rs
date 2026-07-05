//! Two-stage cancellation primitives and unix process-*tree* teardown.
//!
//! [`CancelFlag`] is the shared escalation state that Ctrl+C / SIGTERM
//! handlers flip and subprocess poll loops observe (`daft exec` and
//! `daft sync` both drive it). The unix-only remainder tears down an
//! entire descendant tree **by process group**, which plain
//! parent-kills cannot do (#663): a pre-push hook chain like
//! `lefthook → mise → cargo test` puts its stages in their *own*
//! process groups, and a tty-triggered job-control stop (`T` state)
//! freezes such a group so that only SIGKILL — or SIGCONT after a
//! queued SIGTERM — makes it act on anything at all.
//!
//! Teardown therefore works on pgids, not pids:
//! - soft cancel sends `SIGTERM` then `SIGCONT` to each group (the CONT
//!   delivers the queued TERM to stopped members);
//! - hard cancel sends `SIGKILL`, which stopped processes act on
//!   directly;
//! - groups are discovered by walking a `ps` snapshot from the direct
//!   child, and every group ever signaled stays in a cumulative set —
//!   after TERM kills intermediate parents, orphans reparent to PID 1
//!   and a fresh walk can no longer reach them.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Cancellation level for in-flight subprocess runs.
///
/// 0 = running normally.
/// 1 = soft-cancel: children get SIGTERM; we wait for them to exit.
/// 2 = hard-cancel: children get SIGKILL.
///
/// Escalation is monotonic — the flag never goes down.
pub struct CancelFlag(AtomicUsize);

impl CancelFlag {
    pub fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    pub fn level(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.level() >= 1
    }

    pub fn escalate(&self) {
        // 0 → 1, 1 → 2, 2 → 2 (saturates). Atomic compare-and-swap so
        // concurrent escalations can't regress the level under contention.
        let _ = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |cur| {
                (cur < 2).then_some(cur + 1)
            });
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
pub use unix::*;

#[cfg(unix)]
mod unix {
    use nix::sys::signal::{Signal, kill, killpg};
    use nix::unistd::Pid;
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::process::Stdio;

    fn pg(pgid: u32) -> Pid {
        Pid::from_raw(pgid as i32)
    }

    /// Whether the process group is still alive. `killpg(pgid, 0)` is the
    /// canonical probe: ESRCH means gone; EPERM means it exists but isn't
    /// ours (still alive).
    pub fn group_alive(pgid: u32) -> bool {
        !matches!(killpg(pg(pgid), None), Err(nix::errno::Errno::ESRCH))
    }

    /// Job-control state of a single pid via `ps -o stat=`.
    ///
    /// Returns `Some(true)` when the process is stopped (`T`/`t` — the
    /// state a background-group `/dev/tty` read lands in), `Some(false)`
    /// when it is running, and `None` when it cannot be observed (already
    /// reaped, or `ps` failed). Callers must treat `None` as "unknown",
    /// not "not stopped".
    pub fn pid_stopped(pid: u32) -> Option<bool> {
        let out = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stat = stdout.trim();
        if stat.is_empty() {
            return None;
        }
        Some(stat.starts_with('T') || stat.starts_with('t'))
    }

    struct ProcessRow {
        ppid: u32,
        pgid: u32,
    }

    /// Point-in-time view of the system process table, sufficient to walk
    /// parent→child links and group membership.
    struct ProcessSnapshot {
        rows: HashMap<u32, ProcessRow>,
    }

    impl ProcessSnapshot {
        /// Best-effort capture via `ps`. `-axo pid=,ppid=,pgid=,stat=` is
        /// portable across macOS and Linux procps; `=` suppresses headers.
        fn capture() -> Option<Self> {
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

        fn parse(text: &str) -> Self {
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
        /// PID 1 — which is exactly why [`GroupCascade`] keeps a
        /// cumulative record of everything it ever signaled.
        fn descendant_pgids(&self, root_pid: u32) -> BTreeSet<u32> {
            let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
            for (&pid, row) in &self.rows {
                children.entry(row.ppid).or_default().push(pid);
            }
            let mut pgids = BTreeSet::new();
            let mut stack = vec![root_pid];
            // Pid reuse between `ps` rows can in principle produce a
            // parent cycle; the seen-set makes the walk immune.
            let mut seen = HashSet::new();
            while let Some(pid) = stack.pop() {
                if !seen.insert(pid) {
                    continue;
                }
                if let Some(row) = self.rows.get(&pid) {
                    pgids.insert(row.pgid);
                }
                if let Some(kids) = children.get(&pid) {
                    stack.extend(kids.iter().copied());
                }
            }
            pgids
        }
    }

    /// Drop pgids that must never be signaled: the kernel/init groups
    /// (`killpg(0)` would hit *our own* group; `1` is init's) and the
    /// calling process's group — daft must not tear itself down.
    fn signalable_pgids(pgids: BTreeSet<u32>, own_pgid: u32) -> BTreeSet<u32> {
        pgids
            .into_iter()
            .filter(|&p| p > 1 && p != own_pgid)
            .collect()
    }

    /// Escalating teardown of one spawned child's process tree.
    ///
    /// Construct with the direct child's pid (which is its own group
    /// leader — spawn sites use `Command::process_group(0)`), then call
    /// [`soft_tick`](Self::soft_tick) / [`hard_tick`](Self::hard_tick)
    /// from the caller's poll loop while the corresponding cancel level
    /// is active. Ticks re-walk the process table so groups spawned
    /// after cancellation began are still caught, and every group ever
    /// signaled is remembered so hard-cancel and survivor reporting
    /// cover groups the walk can no longer reach.
    pub struct GroupCascade {
        root_pid: u32,
        own_pgid: u32,
        signaled: BTreeSet<u32>,
        root_fallback_termed: bool,
    }

    impl GroupCascade {
        pub fn new(root_pid: u32) -> Self {
            Self {
                root_pid,
                own_pgid: nix::unistd::getpgrp().as_raw() as u32,
                signaled: BTreeSet::new(),
                root_fallback_termed: false,
            }
        }

        fn walk_targets(&self) -> BTreeSet<u32> {
            match ProcessSnapshot::capture() {
                Some(snap) => signalable_pgids(snap.descendant_pgids(self.root_pid), self.own_pgid),
                None => BTreeSet::new(),
            }
        }

        /// SIGTERM + SIGCONT every group in the child's tree not already
        /// signaled. The CONT is load-bearing: a stopped process only
        /// acts on the queued TERM once resumed.
        pub fn soft_tick(&mut self) {
            let targets = self.walk_targets();
            if targets.is_empty() && self.signaled.is_empty() {
                // Snapshot raced the spawn (child not in the table yet,
                // or its setpgid not applied). Try the would-be group
                // (pid == pgid); on ESRCH fall back to a direct kill
                // once and retry the group on the next tick.
                if killpg(pg(self.root_pid), Signal::SIGTERM).is_ok() {
                    let _ = killpg(pg(self.root_pid), Signal::SIGCONT);
                    self.signaled.insert(self.root_pid);
                } else if !self.root_fallback_termed {
                    let _ = kill(pg(self.root_pid), Signal::SIGTERM);
                    self.root_fallback_termed = true;
                }
                return;
            }
            for pgid in targets {
                if self.signaled.contains(&pgid) {
                    continue;
                }
                // ESRCH (group died between walk and signal) is fine —
                // skip; a later tick re-derives targets.
                if killpg(pg(pgid), Signal::SIGTERM).is_ok() {
                    let _ = killpg(pg(pgid), Signal::SIGCONT);
                    self.signaled.insert(pgid);
                }
            }
        }

        /// SIGKILL every group ever signaled plus whatever a fresh walk
        /// still finds. Stopped processes act on SIGKILL directly, so no
        /// CONT is needed here.
        pub fn hard_tick(&mut self) {
            let mut targets = self.walk_targets();
            targets.extend(self.signaled.iter().copied());
            if targets.is_empty() {
                targets.insert(self.root_pid);
            }
            for pgid in targets {
                let _ = killpg(pg(pgid), Signal::SIGKILL);
                self.signaled.insert(pgid);
            }
        }

        /// Signaled groups that still have live members — input for the
        /// "manual recovery needed" report (`kill -KILL -<pgid>`).
        pub fn survivors(&self) -> Vec<u32> {
            self.signaled
                .iter()
                .copied()
                .filter(|&p| group_alive(p))
                .collect()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::process::CommandExt;
        use std::process::Command;
        use std::time::{Duration, Instant};

        fn wait_for<F: FnMut() -> bool>(mut cond: F, timeout: Duration, what: &str) {
            let deadline = Instant::now() + timeout;
            while !cond() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }

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
        }

        #[test]
        fn walk_survives_ppid_cycles() {
            let snap = ProcessSnapshot::parse("200 201 200 S\n201 200 200 S\n");
            assert_eq!(snap.descendant_pgids(200), BTreeSet::from([200]));
        }

        #[test]
        fn signalable_excludes_init_kernel_and_own_group() {
            let all = BTreeSet::from([0, 1, 42, 777, 4242]);
            assert_eq!(signalable_pgids(all, 777), BTreeSet::from([42, 4242]),);
        }

        #[test]
        fn cancel_flag_monotonic_escalation() {
            let f = super::super::CancelFlag::new();
            assert_eq!(f.level(), 0);
            assert!(!f.is_cancelled());
            f.escalate();
            assert_eq!(f.level(), 1);
            assert!(f.is_cancelled());
            f.escalate();
            assert_eq!(f.level(), 2);
            f.escalate(); // saturates
            assert_eq!(f.level(), 2);
        }

        #[test]
        fn cascade_terminates_live_isolated_group() {
            let mut child = Command::new("sh")
                .args(["-c", "sleep 30"])
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn sleeper");
            let mut cascade = GroupCascade::new(child.id());
            cascade.soft_tick();
            wait_for(
                || child.try_wait().expect("try_wait").is_some(),
                Duration::from_secs(5),
                "TERM+CONT cascade to kill the child",
            );
            assert!(cascade.survivors().is_empty());
        }

        #[test]
        fn cascade_unsticks_a_stopped_group() {
            // The #663 wedge in miniature: the child stops its own
            // isolated process group (`kill -STOP 0`), the state where
            // SIGTERM alone queues forever. soft_tick's TERM+CONT must
            // resume it into acting on the TERM.
            let mut child = Command::new("sh")
                .args(["-c", "kill -STOP 0; sleep 30"])
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn self-stopper");
            wait_for(
                || pid_stopped(child.id()) == Some(true),
                Duration::from_secs(5),
                "child to job-control-stop itself",
            );
            let mut cascade = GroupCascade::new(child.id());
            cascade.soft_tick();
            wait_for(
                || child.try_wait().expect("try_wait").is_some(),
                Duration::from_secs(5),
                "cascade to unstick and kill the stopped group",
            );
            assert!(cascade.survivors().is_empty());
        }

        #[test]
        fn hard_tick_kills_a_term_immune_group() {
            let mut child = Command::new("sh")
                .args(["-c", "trap '' TERM; sleep 30"])
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn TERM-trapping child");
            // Give the shell a beat to install the trap, then verify
            // soft-cancel alone does not kill it.
            std::thread::sleep(Duration::from_millis(300));
            let mut cascade = GroupCascade::new(child.id());
            cascade.soft_tick();
            std::thread::sleep(Duration::from_millis(300));
            assert!(
                child.try_wait().expect("try_wait").is_none(),
                "TERM-trapping child should survive soft cancel"
            );
            cascade.hard_tick();
            wait_for(
                || child.try_wait().expect("try_wait").is_some(),
                Duration::from_secs(5),
                "SIGKILL to reap the TERM-immune child",
            );
            assert!(cascade.survivors().is_empty());
        }
    }
}

#[cfg(all(test, not(unix)))]
mod tests {
    use super::CancelFlag;

    #[test]
    fn cancel_flag_monotonic_escalation() {
        let f = CancelFlag::new();
        f.escalate();
        f.escalate();
        f.escalate();
        assert_eq!(f.level(), 2);
    }
}
