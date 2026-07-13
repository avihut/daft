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

use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

    /// Lift the level 0 → 1 only, atomically, doing nothing if any cancel
    /// already landed. The post-TUI keypress path uses this: a raw-mode
    /// Ctrl+C never reached the signal handler, so the flag may still be
    /// 0 and must be raised — but a check-then-`escalate` would race the
    /// ctrlc handler thread (SIGTERM/SIGHUP via the `termination` feature)
    /// and could drive 0 → 1 → 2, forcing SIGKILL off a single graceful
    /// keypress (#8). The compare-exchange makes the 0 → 1 transition win
    /// or no-op, never compound.
    pub fn soft_escalate_once(&self) {
        let _ = self
            .0
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
    }
}

impl Default for CancelFlag {
    fn default() -> Self {
        Self::new()
    }
}

/// Poll cadence for cancellable child waits (mirrors exec's loop).
const TICK: Duration = Duration::from_millis(50);
/// How often escalation re-walks the process table while a cancel is
/// live. Re-walking catches groups spawned after the cancel began.
#[cfg(unix)]
const CASCADE_EVERY: Duration = Duration::from_millis(500);
/// How often the direct child's job-control state is probed.
///
/// Each probe forks one `ps -o stat= -p <pid>`; with N supervised
/// children that is at most 2N short-lived processes per second, and
/// only while a child is running with no cancel in flight (the probe
/// is skipped once a teardown starts, where a `T` state is expected
/// from the queued TERM). Fine at sync's typical worktree counts; a
/// shared snapshot would be the lever if N ever grows painful.
#[cfg(unix)]
const STOP_PROBE_EVERY: Duration = Duration::from_millis(500);
/// Consecutive stopped probes required before declaring a tty-stop, so a
/// transient stop/resume can't misfire the teardown.
///
/// Two probes at 500ms means a child must sit stopped for a full
/// second — a real tty-auth stop is indefinite, while SIGSTOP/SIGCONT
/// blips (debuggers, `kill -STOP` probes) resume in between and reset
/// the streak. The residual race is accepted: a child that stops for
/// both probes, resumes, and exits successfully before the SIGKILL
/// lands still reports StoppedOnTty — with these constants that takes
/// deliberate signal choreography, not anything git or a hook does on
/// its own.
#[cfg(unix)]
const STOP_STREAK: u8 = 2;

/// Typed marker error: a subprocess run was torn down because the shared
/// [`CancelFlag`] went active. Callers use `anyhow::Error::is` to tell
/// cancellation apart from real failures.
#[derive(Debug, thiserror::Error)]
#[error("operation cancelled")]
pub struct OperationCancelled;

/// Typed marker error: the subprocess job-control-stopped itself — the
/// signature of a background-group `/dev/tty` read, i.e. an interactive
/// auth prompt daft cannot forward. Its process tree has already been
/// killed by the time this surfaces.
#[derive(Debug, thiserror::Error)]
#[error(
    "git stopped waiting for terminal input (interactive auth prompt?); \
     run the command manually there, or configure an ssh-agent/credential helper"
)]
pub struct NeedsTerminal;

/// Typed marker error: the supervised push unit (git + pre-push hook)
/// exceeded its wall-clock budget and its tree was torn down (#678).
#[derive(Debug, thiserror::Error)]
#[error(
    "push timed out after {}s (pre-push hook still running?); \
     raise or disable daft.sync.pushTimeout if this push legitimately needs longer",
    limit.as_secs()
)]
pub struct OperationTimedOut {
    pub limit: Duration,
}

/// Once a unit is past its deadline: this long on the soft cascade
/// (TERM+CONT per group) before escalating to SIGKILL.
const TIMEOUT_HARD_GRACE: Duration = Duration::from_secs(10);

/// Pausable wall-clock budget for one supervised unit (#678 stage 4).
///
/// The supervisor polls [`UnitClock::overdue`] every tick. The resource
/// governor pauses the clock while it has the unit frozen — SIGSTOP'd
/// descendants make no progress, and counting that time would time out an
/// innocent hook. `overdue` subtracts the in-progress pause live, so a
/// unit frozen past its nominal deadline can never fire mid-freeze.
pub struct UnitClock {
    limit: Duration,
    started: Instant,
    pause: Mutex<PauseState>,
}

#[derive(Default)]
struct PauseState {
    since: Option<Instant>,
    accrued: Duration,
}

impl UnitClock {
    /// A running clock with `limit` of countable wall time.
    pub fn new(limit: Duration) -> Self {
        Self {
            limit,
            started: Instant::now(),
            pause: Mutex::new(PauseState::default()),
        }
    }

    /// The configured budget.
    pub fn limit(&self) -> Duration {
        self.limit
    }

    /// Stop counting (idempotent). The governor calls this at freeze.
    pub fn pause(&self) {
        let mut pause = self.pause.lock().unwrap();
        if pause.since.is_none() {
            pause.since = Some(Instant::now());
        }
    }

    /// Resume counting (idempotent). The governor calls this at thaw.
    pub fn resume(&self) {
        let mut pause = self.pause.lock().unwrap();
        if let Some(since) = pause.since.take() {
            pause.accrued += since.elapsed();
        }
    }

    /// How far past the pause-adjusted deadline the unit is, if at all.
    pub fn overdue(&self, now: Instant) -> Option<Duration> {
        let paused = {
            let pause = self.pause.lock().unwrap();
            pause.accrued
                + pause
                    .since
                    .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
        };
        let counted = now
            .saturating_duration_since(self.started)
            .saturating_sub(paused);
        (counted > self.limit).then(|| counted - self.limit)
    }
}

/// How a supervised child's tree is torn down on cancel — and whether a
/// job-control stop is read as an interactive-auth signal.
///
/// The distinction exists because process-group isolation and interactive
/// terminal auth are mutually exclusive: a child in its own (background)
/// group cannot read the controlling `/dev/tty`, so any credential /
/// passphrase prompt there SIGTTIN-stops it. We only pay that cost where
/// group teardown is actually needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisionMode {
    /// Child spawned in its own process group (`process_group(0)`). Cancel
    /// tears the whole tree down by pgid cascade, and a job-control stop is
    /// read as a background-group `/dev/tty` read (interactive auth daft
    /// cannot forward) → the tree is killed and the run reports
    /// [`Verdict::StoppedOnTty`]. Required for `git push`, whose pre-push
    /// hook chain (`lefthook → mise → cargo`) escapes into its own groups
    /// that terminal job-control signals never reach (#663).
    Isolated,
    /// Child stays in the caller's (foreground) process group. Cancel is a
    /// direct SIGTERM→SIGKILL of the child pid — killpg is off-limits (it
    /// would hit daft's own group). No stop-detection: a foreground child
    /// reads the controlling tty directly, so interactive credential /
    /// passphrase prompts work and never SIGTTIN-stop. For
    /// fetch/pull/rebase/ls-remote, which spawn no group-escaping
    /// descendants; a plain `child.kill()` plus pipe-EOF reaps them.
    Direct,
}

/// Final state of a supervised child.
#[derive(Debug)]
pub enum Verdict {
    /// Ran to completion (any exit status) with no cancel in flight.
    Completed(ExitStatus),
    /// The cancel flag went active while the child ran; its tree was
    /// torn down and the child is reaped.
    Cancelled,
    /// The child job-control-stopped itself (tty auth); its tree was
    /// killed and the child is reaped.
    StoppedOnTty,
    /// The unit outran its [`UnitClock`] budget while still running; its
    /// tree was torn down and the child is reaped (#678).
    TimedOut,
}

/// Poll-based replacement for `Child::wait()` that keeps watching the
/// shared [`CancelFlag`] and the child's job-control state.
///
/// Supervision is opt-in via the flag: with `cancel: None` the wait is
/// a classic blocking one — same process group, no stop probes, no
/// cascades — preserving pre-cancellation behavior for callers that
/// never asked for teardown (their terminal credential prompts must
/// keep working, and Ctrl+C must keep reaching the git subtree through
/// the caller's foreground group). With a flag attached, [`SupervisionMode`]
/// selects the teardown: [`Isolated`](SupervisionMode::Isolated) children
/// (spawned with `Command::process_group(0)`) are torn down by pgid
/// cascade and stop-detected; [`Direct`](SupervisionMode::Direct) children
/// stay in the caller's group and are killed by pid. The `drains_done`
/// gate exists because pipe write-ends are inherited by descendants
/// that can outlive the direct child — returning before the drains see
/// EOF would leave the caller blocked on a reader join with nobody
/// left watching the flag, which is exactly the #663 wedge.
pub struct ChildSupervisor<'a> {
    cancel: Option<&'a CancelFlag>,
    #[cfg(unix)]
    mode: SupervisionMode,
    cancelled_in_flight: bool,
    tty_stopped: bool,
    /// Wall-clock budget for the unit; `None` = no timeout.
    clock: Option<Arc<UnitClock>>,
    /// The clock expired while the child was still running.
    timed_out: bool,
    #[cfg(unix)]
    child_pid: u32,
    #[cfg(unix)]
    cascade: GroupCascade,
    /// Level acted on and when, so a level change cascades immediately
    /// and an unchanged level re-cascades every [`CASCADE_EVERY`].
    #[cfg(unix)]
    cascade_at: Option<(usize, std::time::Instant)>,
    #[cfg(unix)]
    stop_probe_at: std::time::Instant,
    #[cfg(unix)]
    stopped_streak: u8,
    /// Direct-mode: whether the child pid was already SIGTERM'd, so the
    /// soft signal is sent once rather than every poll tick.
    #[cfg(unix)]
    direct_termed: bool,
}

impl<'a> ChildSupervisor<'a> {
    #[cfg(unix)]
    pub fn new(
        child: &Child,
        cancel: Option<&'a CancelFlag>,
        mode: SupervisionMode,
        clock: Option<Arc<UnitClock>>,
    ) -> Self {
        Self {
            cancel,
            mode,
            cancelled_in_flight: false,
            tty_stopped: false,
            clock,
            timed_out: false,
            child_pid: child.id(),
            cascade: GroupCascade::new(child.id()),
            cascade_at: None,
            // First probe only after a full interval: ultra-short
            // children exit before ever being ps-probed.
            stop_probe_at: std::time::Instant::now() + STOP_PROBE_EVERY,
            stopped_streak: 0,
            direct_termed: false,
        }
    }

    #[cfg(not(unix))]
    pub fn new(
        _child: &Child,
        cancel: Option<&'a CancelFlag>,
        _mode: SupervisionMode,
        clock: Option<Arc<UnitClock>>,
    ) -> Self {
        Self {
            cancel,
            cancelled_in_flight: false,
            tty_stopped: false,
            clock,
            timed_out: false,
        }
    }

    /// Timeout escalation level for this tick: 0 = within budget, 1 = past
    /// the deadline (soft cascade), 2 = past deadline + grace (SIGKILL).
    /// Computed even when the child already exited — an expired deadline
    /// during the drain-wait still tears down orphaned pipe holders.
    fn timeout_level(&self, now: Instant) -> usize {
        match &self.clock {
            Some(clock) => match clock.overdue(now) {
                None => 0,
                Some(over) if over < TIMEOUT_HARD_GRACE => 1,
                Some(_) => 2,
            },
            None => 0,
        }
    }

    /// Drive the child until it is reaped *and* `drains_done` reports
    /// the output pipes closed. `std::process::Child::try_wait` is the
    /// sole reaper — no raw `waitpid` runs beside it.
    pub fn wait(
        &mut self,
        child: &mut Child,
        drains_done: impl Fn() -> bool,
    ) -> std::io::Result<Verdict> {
        if self.cancel.is_none() {
            // No flag, no supervision: block like Child::wait always
            // did, then wait out any pipe holders (the drains run on
            // the caller's scoped threads and see EOF exactly as they
            // would have pre-cancellation).
            let status = child.wait()?;
            while !drains_done() {
                std::thread::sleep(TICK);
            }
            return Ok(Verdict::Completed(status));
        }
        let mut exit: Option<ExitStatus> = None;
        loop {
            if exit.is_none() {
                exit = child.try_wait()?;
            }
            if let Some(status) = exit
                && drains_done()
            {
                // Priority: a tty stop is the most specific diagnosis; a
                // user cancel outranks a timeout; a timeout outranks the
                // exit status (the teardown forged it anyway).
                let verdict = if self.tty_stopped {
                    Verdict::StoppedOnTty
                } else if self.cancelled_in_flight {
                    Verdict::Cancelled
                } else if self.timed_out {
                    Verdict::TimedOut
                } else {
                    Verdict::Completed(status)
                };
                #[cfg(unix)]
                if !matches!(verdict, Verdict::Completed(_)) {
                    record_survivors(&self.survivors());
                }
                return Ok(verdict);
            }
            self.tick(child, exit.is_none());
            std::thread::sleep(TICK);
        }
    }

    #[cfg(unix)]
    fn tick(&mut self, _child: &mut Child, child_running: bool) {
        match self.mode {
            SupervisionMode::Isolated => self.tick_isolated(child_running),
            SupervisionMode::Direct => self.tick_direct(child_running),
        }
    }

    /// Isolated-mode tick: pgid-cascade teardown plus tty-stop detection.
    #[cfg(unix)]
    fn tick_isolated(&mut self, child_running: bool) {
        let now = std::time::Instant::now();
        let flag_level = self.cancel.map(CancelFlag::level).unwrap_or(0);
        // Only a cancel seen *while the child is still running* cancels
        // this run. If the child already exited (its status is in hand) a
        // later cancel is just tearing down leftover pipe holders — the
        // run's real outcome is its exit status. (#7: a `git push` that
        // exited 0 succeeded even if a backgrounded hook child kept the
        // pipe write-end open past the cancel; reporting it Cancelled would
        // tell the user their commits never pushed when they did.)
        if flag_level > 0 && child_running {
            self.cancelled_in_flight = true;
        }

        // Same gate for the timeout verdict (#7 shape): a deadline expiring
        // during the drain-wait doesn't relabel a finished run — but its
        // escalation level still folds in below, so orphaned pipe holders
        // are torn down instead of wedging the drain.
        let timeout_level = self.timeout_level(now);
        if timeout_level > 0 && child_running {
            self.timed_out = true;
        }

        // Stop detection runs only while nothing else is going on: once a
        // cancel, a detected stop, or a timeout starts a teardown, the T
        // state is expected (queued TERM) and must not re-trigger.
        if flag_level == 0
            && timeout_level == 0
            && !self.tty_stopped
            && child_running
            && now >= self.stop_probe_at
        {
            self.stop_probe_at = now + STOP_PROBE_EVERY;
            match pid_stopped(self.child_pid) {
                Some(true) => {
                    self.stopped_streak += 1;
                    if self.stopped_streak >= STOP_STREAK {
                        self.tty_stopped = true;
                    }
                }
                Some(false) => self.stopped_streak = 0,
                None => {}
            }
        }

        // A tty-stopped child can never make progress — skip straight to
        // the kill cascade regardless of the flag.
        let level = if self.tty_stopped {
            2
        } else {
            flag_level.max(timeout_level)
        };
        if level == 0 {
            return;
        }
        let due = match self.cascade_at {
            None => true,
            Some((acted, at)) => acted != level || now.duration_since(at) >= CASCADE_EVERY,
        };
        if due {
            if level == 1 {
                self.cascade.soft_tick();
            } else {
                self.cascade.hard_tick();
            }
            self.cascade_at = Some((level, now));
        }
    }

    /// Direct-mode tick: no isolation, no stop-detection. The child shares
    /// daft's (foreground) process group, so it reads the controlling tty
    /// for interactive auth and never SIGTTIN-stops; killpg is therefore
    /// off-limits (it would tear daft's own group down). Cancel is a direct
    /// two-stage kill of the child pid; git's helper children exit on pipe
    /// EOF once the parent dies.
    #[cfg(unix)]
    fn tick_direct(&mut self, child_running: bool) {
        let flag_level = self.cancel.map(CancelFlag::level).unwrap_or(0);
        let timeout_level = self.timeout_level(std::time::Instant::now());
        // See #7 above: a cancel arriving after the child already exited
        // doesn't cancel a finished run, and there is nothing to kill.
        if flag_level == 0 && timeout_level == 0 || !child_running {
            return;
        }
        if flag_level > 0 {
            self.cancelled_in_flight = true;
        }
        if timeout_level > 0 {
            self.timed_out = true;
        }
        if flag_level.max(timeout_level) >= 2 {
            kill_pid(self.child_pid, true);
        } else if !self.direct_termed {
            kill_pid(self.child_pid, false);
            self.direct_termed = true;
        }
    }

    #[cfg(not(unix))]
    fn tick(&mut self, child: &mut Child, child_running: bool) {
        // No process groups off unix: two-stage degrades to a plain kill
        // of the direct child on any cancel level (or an expired budget).
        if self.cancel.is_some_and(CancelFlag::is_cancelled) {
            self.cancelled_in_flight = true;
            if child_running {
                let _ = child.kill();
            }
        }
        if self.timeout_level(Instant::now()) > 0 && child_running {
            self.timed_out = true;
            let _ = child.kill();
        }
    }

    /// Signaled process groups that still have live members after the
    /// teardown — input for the caller's manual-recovery report.
    #[cfg(unix)]
    pub fn survivors(&self) -> Vec<u32> {
        self.cascade.survivors()
    }

    #[cfg(not(unix))]
    pub fn survivors(&self) -> Vec<u32> {
        Vec::new()
    }
}

/// Process groups that outlived a teardown cascade, recorded by
/// supervisors as they conclude. Command layers read this at exit (via
/// [`surviving_groups`]) to print a manual-recovery hint — the #663
/// incident's `kill -KILL -<pgid>` forensics as first-class UX.
#[cfg(unix)]
static LEFTOVER_GROUPS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

#[cfg(unix)]
fn record_survivors(pgids: &[u32]) {
    if pgids.is_empty() {
        return;
    }
    if let Ok(mut set) = LEFTOVER_GROUPS.lock() {
        set.extend(pgids.iter().copied());
    }
}

/// Ever-recorded survivor groups that are *still alive right now* —
/// each is re-probed on read, so groups that died since teardown fall
/// away instead of producing stale warnings.
#[cfg(unix)]
pub fn surviving_groups() -> Vec<u32> {
    let Ok(mut set) = LEFTOVER_GROUPS.lock() else {
        return Vec::new();
    };
    set.retain(|&p| group_alive(p));
    set.iter().copied().collect()
}

#[cfg(not(unix))]
pub fn surviving_groups() -> Vec<u32> {
    Vec::new()
}

/// Per-invocation options for [`supervise_command`] beyond the cancel
/// flag itself.
pub(crate) struct SuperviseOpts<'a> {
    /// Process-group placement and teardown strategy.
    pub mode: SupervisionMode,
    /// Invoked with the child's pid immediately after spawn.
    pub on_spawn: Option<&'a (dyn Fn(u32) + Send + Sync)>,
    /// Wall-clock budget for the unit (#678); expiry escalates through
    /// the same cascade as a cancel and yields [`Verdict::TimedOut`].
    pub clock: Option<Arc<UnitClock>>,
}

impl SuperviseOpts<'_> {
    /// Plain supervision in `mode`: no spawn observer, no budget.
    pub(crate) fn new(mode: SupervisionMode) -> Self {
        Self {
            mode,
            on_spawn: None,
            clock: None,
        }
    }
}

/// The single spawn → supervise → verdict skeleton shared by every
/// cancellable git seam. Sets up piped stdio, isolates the child into its
/// own process group when the mode is [`SupervisionMode::Isolated`] and a
/// flag is attached, then drains both pipes on scoped threads with the
/// caller's closures while [`ChildSupervisor`] polls the flag and the
/// child's job-control state. Returns the [`Verdict`] alongside whatever
/// the two drain closures produced.
///
/// [`output_with_cancel`] (fetch/pull/rebase/ls-remote, Direct mode) and
/// [`GitCommand::run_push`](crate::git::GitCommand) (Isolated mode) are
/// the two callers; centralizing the skeleton means a change to the
/// supervision contract (the drains-done gate, a new [`Verdict`] arm) is
/// made once instead of drifting between two copies.
pub(crate) fn supervise_command<O, E, FO, FE>(
    cmd: &mut Command,
    cancel: Option<&CancelFlag>,
    opts: SuperviseOpts<'_>,
    drain_out: FO,
    drain_err: FE,
) -> anyhow::Result<(Verdict, O, E)>
where
    O: Send + Default,
    E: Send + Default,
    FO: FnOnce(std::process::ChildStdout) -> O + Send,
    FE: FnOnce(std::process::ChildStderr) -> E + Send,
{
    use anyhow::Context;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    if cancel.is_some() && opts.mode == SupervisionMode::Isolated {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn()?;
    // Surface the pid before the first supervisor tick so an observer (the
    // resource governor, #678) knows the unit's root from its first instant.
    if let Some(on_spawn) = opts.on_spawn {
        on_spawn(child.id());
    }
    let mut supervisor = ChildSupervisor::new(&child, cancel, opts.mode, opts.clock);
    let stdout_pipe = child
        .stdout
        .take()
        .context("Failed to capture child stdout")?;
    let stderr_pipe = child
        .stderr
        .take()
        .context("Failed to capture child stderr")?;

    let (verdict, out, err) = std::thread::scope(|scope| {
        let out = scope.spawn(move || drain_out(stdout_pipe));
        let err = scope.spawn(move || drain_err(stderr_pipe));
        let verdict = supervisor.wait(&mut child, || out.is_finished() && err.is_finished());
        // The wait gate guarantees both drains saw EOF; joins can't block.
        (
            verdict,
            out.join().unwrap_or_default(),
            err.join().unwrap_or_default(),
        )
    });

    Ok((verdict?, out, err))
}

/// Cancellation-aware stand-in for `Command::output()`.
///
/// Uses [`SupervisionMode::Direct`]: the child stays in the caller's
/// process group so an interactive credential / passphrase prompt on
/// `/dev/tty` works, and a cancel escalation kills the child pid directly
/// (fetch/pull/rebase/ls-remote spawn no group-escaping descendants that
/// would need a pgid cascade). Returns [`OperationCancelled`] as a typed
/// error on cancel; a run that merely failed still returns `Ok` with the
/// non-success status, matching `Command::output()` semantics.
///
/// With `cancel: None` this is `Command::output()` with extra steps: the
/// wait blocks classically. Callers that never opted into cancellation
/// keep their exact pre-cancellation behavior.
pub fn output_with_cancel(
    cmd: &mut Command,
    cancel: Option<&CancelFlag>,
) -> anyhow::Result<Output> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err(OperationCancelled.into());
    }
    let read_to_end = |mut pipe: std::process::ChildStdout| {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        buf
    };
    let read_to_end_err = |mut pipe: std::process::ChildStderr| {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut buf);
        buf
    };
    let (verdict, stdout, stderr) = supervise_command(
        cmd,
        cancel,
        SuperviseOpts::new(SupervisionMode::Direct),
        read_to_end,
        read_to_end_err,
    )?;

    match verdict {
        Verdict::Completed(status) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        Verdict::Cancelled => Err(OperationCancelled.into()),
        Verdict::StoppedOnTty => Err(NeedsTerminal.into()),
        // Unreachable in practice: this path never attaches a UnitClock.
        Verdict::TimedOut => Err(OperationTimedOut {
            limit: Duration::ZERO,
        }
        .into()),
    }
}

#[cfg(all(test, unix))]
mod supervisor_tests {
    use super::*;
    use std::time::Instant;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    }

    fn read_out(mut p: std::process::ChildStdout) -> Vec<u8> {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut p, &mut b);
        b
    }

    fn read_err(mut p: std::process::ChildStderr) -> Vec<u8> {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut p, &mut b);
        b
    }

    /// Run `cmd` under Isolated supervision (the `git push` shape: own
    /// process group, pgid cascade, tty-stop detection).
    fn run_isolated(
        cmd: &mut Command,
        flag: Option<&CancelFlag>,
    ) -> anyhow::Result<(Verdict, Vec<u8>, Vec<u8>)> {
        supervise_command(
            cmd,
            flag,
            SuperviseOpts::new(SupervisionMode::Isolated),
            read_out,
            read_err,
        )
    }

    #[test]
    fn unit_clock_pause_excludes_frozen_time() {
        let clock = UnitClock::new(Duration::from_millis(50));
        clock.pause();
        std::thread::sleep(Duration::from_millis(120));
        assert!(
            clock.overdue(Instant::now()).is_none(),
            "paused time must not count against the budget"
        );
        clock.resume();
        std::thread::sleep(Duration::from_millis(80));
        assert!(
            clock.overdue(Instant::now()).is_some(),
            "unpaused time counts"
        );
        // Idempotence: double resume/pause must not corrupt accounting.
        clock.resume();
        clock.pause();
        clock.pause();
    }

    #[test]
    fn timeout_tears_down_and_reports_timed_out() {
        // The flag never escalates — only the clock expires. Soft TERM
        // suffices for sh+sleep, well inside the hard grace.
        let flag = CancelFlag::new();
        let clock = Arc::new(UnitClock::new(Duration::from_millis(150)));
        let started = Instant::now();
        let (verdict, _, _) = supervise_command(
            &mut sh("sleep 30"),
            Some(&flag),
            SuperviseOpts {
                mode: SupervisionMode::Isolated,
                on_spawn: None,
                clock: Some(clock),
            },
            read_out,
            read_err,
        )
        .unwrap();
        assert!(matches!(verdict, Verdict::TimedOut), "got {verdict:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "soft teardown must not wait out the hard grace"
        );
    }

    #[test]
    fn on_spawn_reports_child_pid_before_first_tick() {
        let seen = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&seen);
        let on_spawn = move |pid: u32| {
            *sink.lock().unwrap() = Some(pid);
        };
        let (verdict, out, _) = supervise_command(
            &mut sh("echo hi"),
            None,
            SuperviseOpts {
                mode: SupervisionMode::Direct,
                on_spawn: Some(&on_spawn),
                clock: None,
            },
            read_out,
            read_err,
        )
        .unwrap();
        assert!(matches!(verdict, Verdict::Completed(s) if s.success()));
        assert_eq!(String::from_utf8_lossy(&out).trim(), "hi");
        assert!(seen.lock().unwrap().is_some(), "pid callback must fire");
    }

    #[test]
    fn output_with_cancel_completes_and_captures() {
        let out = output_with_cancel(&mut sh("echo out; echo err >&2; exit 3"), None).unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "err");
    }

    #[test]
    fn output_with_cancel_rejects_pre_cancelled_without_spawning() {
        let flag = CancelFlag::new();
        flag.escalate();
        let err = output_with_cancel(&mut sh("echo never"), Some(&flag)).unwrap_err();
        assert!(err.is::<OperationCancelled>(), "got: {err:#}");
    }

    /// Direct mode (fetch/pull/rebase): a cancel kills the child pid
    /// directly, no process group needed. Spawns `sleep` as its own
    /// process — NOT via `sh -c` — because Direct mode kills only the
    /// direct child (killpg is Isolated mode's job); a shell that forks
    /// its command instead of exec'ing it (dash vs some /bin/sh) would
    /// leave the grandchild holding the pipe and the drains would block
    /// for the full sleep. Real fetch/pull children exit on pipe EOF when
    /// git dies, so this single-process shape models them faithfully.
    #[test]
    fn direct_cancel_tears_down_a_running_child() {
        let flag = CancelFlag::new();
        let started = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                flag.escalate();
            });
            let mut cmd = Command::new("sleep");
            cmd.arg("30");
            let err = output_with_cancel(&mut cmd, Some(&flag)).unwrap_err();
            assert!(err.is::<OperationCancelled>(), "got: {err:#}");
        });
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "teardown took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn isolated_cascade_reaches_an_orphaned_pipe_holder() {
        // The run_push wedge shape: the direct child exits but a
        // backgrounded descendant keeps the pipe write-ends open. The
        // Isolated cascade must reach it through the still-live child
        // *group* even though the tree walk can no longer see it (the
        // holder reparented to init when the child died). Proven by the
        // sub-5s return: without the cascade, the drains would block on
        // the `sleep 30` holder for its full duration. The verdict is
        // Completed — the child itself exited 0, so per #7 the cancel that
        // only tore down the leftover holder does not relabel the run.
        let flag = CancelFlag::new();
        let started = Instant::now();
        let (verdict, _o, _e) = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                flag.escalate();
            });
            run_isolated(&mut sh("( sleep 30 & ); exit 0"), Some(&flag)).unwrap()
        });
        assert!(
            matches!(verdict, Verdict::Completed(s) if s.success()),
            "got {verdict:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cascade did not reach the orphaned holder: took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn isolated_cancel_tears_down_a_running_child() {
        // Isolated (push) shape: a cancel arriving while the child is still
        // running tears its group down and reports Cancelled.
        let flag = CancelFlag::new();
        let started = Instant::now();
        let (verdict, _o, _e) = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                flag.escalate();
            });
            run_isolated(&mut sh("sleep 30"), Some(&flag)).unwrap()
        });
        assert!(matches!(verdict, Verdict::Cancelled), "got {verdict:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "teardown took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn isolated_self_stopping_child_is_detected_and_killed() {
        // Interactive-auth shape: the child stops its own group the way
        // a background-group /dev/tty read would. Two stopped probes at
        // 500ms cadence must flag it, kill the group, and surface
        // StoppedOnTty instead of hanging forever. The flag stays at
        // level 0 — stop detection is part of Isolated supervision, not
        // of cancellation. Isolation puts the child in its own group so
        // `kill -STOP 0` freezes only it, never this test process.
        let flag = CancelFlag::new();
        let started = Instant::now();
        let (verdict, _o, _e) =
            run_isolated(&mut sh("kill -STOP 0; sleep 30"), Some(&flag)).unwrap();
        assert!(matches!(verdict, Verdict::StoppedOnTty), "got {verdict:?}");
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "stop detection took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn completed_run_isnt_relabeled_cancelled_by_late_cancel() {
        // #7: the direct child exits 0 but a backgrounded descendant keeps
        // the pipe open; a cancel that lands in that drain-wait window
        // tears the holder down (cleanup) but must NOT relabel the
        // already-successful run as Cancelled — the exit status stands.
        let flag = CancelFlag::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                // Let the child exit 0 first, then cancel while the holder
                // still keeps the pipe open.
                std::thread::sleep(Duration::from_millis(200));
                flag.escalate();
            });
            let (verdict, _o, _e) =
                run_isolated(&mut sh("( sleep 3 & ) ; exit 0"), Some(&flag)).unwrap();
            assert!(
                matches!(verdict, Verdict::Completed(s) if s.success()),
                "a run that exited 0 must stay Completed, got {verdict:?}"
            );
        });
    }

    /// Without a flag there is no supervision: the child must stay in
    /// the caller's process group (terminal prompts and Ctrl+C keep
    /// their pre-cancellation reach), and the wait must block
    /// classically to completion.
    #[test]
    fn no_flag_keeps_child_in_callers_group() {
        let out = output_with_cancel(&mut sh("ps -o pgid= -p $$"), None).unwrap();
        assert!(out.status.success());
        let child_pgid: i32 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .expect("pgid parses");
        assert_eq!(child_pgid, nix::unistd::getpgrp().as_raw());
    }

    /// #1 regression: a *supervised* Direct run (fetch/pull) must keep the
    /// child in the caller's foreground group, so it can still read the
    /// controlling tty for interactive auth. Isolation here would
    /// SIGTTIN-stop a passphrase prompt and get it killed as NeedsTerminal.
    #[test]
    fn direct_supervision_keeps_child_in_callers_group() {
        let flag = CancelFlag::new();
        let out = output_with_cancel(&mut sh("ps -o pgid= -p $$"), Some(&flag)).unwrap();
        assert!(out.status.success());
        let child_pgid: i32 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .expect("pgid parses");
        assert_eq!(child_pgid, nix::unistd::getpgrp().as_raw());
    }

    /// Isolated mode (push) gets the child its own group — the
    /// precondition for every by-group escalation.
    #[test]
    fn isolated_supervision_gives_child_its_own_group() {
        let flag = CancelFlag::new();
        let (verdict, out, _e) = run_isolated(&mut sh("ps -o pgid= -p $$"), Some(&flag)).unwrap();
        assert!(matches!(verdict, Verdict::Completed(s) if s.success()));
        let child_pgid: i32 = String::from_utf8_lossy(&out)
            .trim()
            .parse()
            .expect("pgid parses");
        assert_ne!(child_pgid, nix::unistd::getpgrp().as_raw());
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

    /// Direct two-stage kill of a single pid: `SIGTERM`, or `SIGKILL` when
    /// `hard`. Used by [`SupervisionMode::Direct`](super::SupervisionMode)
    /// teardown, where the child shares the caller's process group so
    /// `killpg` is off-limits. ESRCH (already reaped) is ignored.
    pub fn kill_pid(pid: u32, hard: bool) {
        let sig = if hard {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let _ = kill(Pid::from_raw(pid as i32), sig);
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
        fn soft_escalate_once_only_lifts_from_zero() {
            use super::super::CancelFlag;
            // 0 → 1.
            let f = CancelFlag::new();
            f.soft_escalate_once();
            assert_eq!(f.level(), 1);
            // Already cancelled → no-op (never compounds to 2).
            f.soft_escalate_once();
            assert_eq!(f.level(), 1);
            // Already hard → must not regress and must not lift further.
            let g = CancelFlag::new();
            g.escalate();
            g.escalate();
            assert_eq!(g.level(), 2);
            g.soft_escalate_once();
            assert_eq!(g.level(), 2);
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

    #[test]
    fn soft_escalate_once_only_lifts_from_zero() {
        let f = CancelFlag::new();
        f.soft_escalate_once();
        assert_eq!(f.level(), 1);
        f.soft_escalate_once();
        assert_eq!(f.level(), 1);
    }
}
