//! Pure decision core for the sync push resource governor (#678).
//!
//! Functional core, imperative shell (see `ARCHITECTURE.md`): everything
//! here is deterministic state-machine logic over injected values — no
//! clocks, no syscalls, no I/O. The shell ([`crate::governor::SyncGovernor`])
//! probes the system, stamps monotonic time, and applies the decisions.

/// A point-in-time reading of system memory, in bytes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceSample {
    /// Total physical memory.
    pub mem_total: u64,
    /// Memory available for new allocations without swapping.
    pub mem_available: u64,
    /// Swap currently in use.
    pub swap_used: u64,
    /// Linux memory PSI `some avg10` (percent, 0–100). `None` where
    /// `/proc/pressure/memory` does not exist (macOS, older kernels).
    pub psi_some_avg10: Option<f32>,
}

/// Traffic-light pressure classification of the latest sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    /// Comfortable headroom — admit and grow.
    Green,
    /// Headroom shrinking (or swap growing) — hold admissions, keep running.
    Yellow,
    /// Below the reserve floor — shrink; containment may act (stage 3).
    Red,
}

/// Why [`Controller::try_admit`] held a unit back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldReason {
    /// At the hard concurrency cap (`--jobs` / `daft.governor.jobs`).
    AtCap,
    /// At the AIMD target (memory-derived, below the hard cap).
    AtTarget,
    /// Not enough headroom for the candidate's predicted peak, or the
    /// system is under pressure.
    Memory,
    /// Inside the slow-start stagger window after the previous launch.
    Stagger,
    /// Waiting out the cooldown after a governor kill (stage 3).
    KillCooldown,
}

/// Fallback predicted peak tree-RSS for a hook with no learned profile.
pub const DEFAULT_PEAK: u64 = 512 << 20;

/// Minimum gap between cold-start launches. An allocation storm shows in
/// the memory-availability derivative within a launch or two; the stagger
/// keeps the storm's blast radius to one unit instead of N.
pub const STAGGER_MS: u64 = 200;

/// AIMD target at construction (before anything is known about the hook).
const INITIAL_TARGET: usize = 2;

/// Yellow when available memory drops below `reserve * this`.
const YELLOW_HEADROOM_FACTOR: u64 = 3; // numerator over 2 → 1.5×

/// Red when Linux memory PSI `some avg10` reaches this percentage.
const PSI_RED: f32 = 10.0;

/// Consecutive green ticks required before the target grows (hysteresis
/// against flapping on a noisy sample).
const RAMP_GREEN_TICKS: u32 = 2;

/// A single-tick drop in available memory larger than `reserve / this`
/// classifies as an allocation storm (yellow) even while headroom is green.
const STORM_DROP_DIVISOR: u64 = 4;

/// Tunables for the admission controller, fixed at construction.
#[derive(Debug, Clone, Copy)]
pub struct GovernorParams {
    /// Hard cap on concurrently admitted units.
    pub cap: usize,
    /// Bytes of memory the governor keeps free at all times.
    pub reserve: u64,
    /// Predicted peak for unprofiled candidates.
    pub default_peak: u64,
    /// Minimum gap between cold-start launches, in milliseconds.
    pub stagger_ms: u64,
}

impl GovernorParams {
    /// Standard parameters for `cap` concurrent units and a `reserve` floor.
    pub fn new(cap: usize, reserve: u64) -> Self {
        Self {
            cap: cap.max(1),
            reserve,
            default_peak: DEFAULT_PEAK,
            stagger_ms: STAGGER_MS,
        }
    }
}

/// Deterministic admission state machine: slow-start + AIMD over a
/// traffic-light pressure signal.
///
/// Time is injected (`now_ms`, any monotonic origin); samples are injected;
/// the running count is injected. Nothing in here blocks or reads the world.
#[derive(Debug)]
pub struct Controller {
    params: GovernorParams,
    /// AIMD ceiling on concurrent admissions (never above `params.cap`).
    target: usize,
    /// True until the first red — the target doubles instead of +1.
    slow_start: bool,
    green_streak: u32,
    pressure: Pressure,
    last_admit_ms: Option<u64>,
    prev_swap: Option<u64>,
    prev_avail: Option<u64>,
}

impl Controller {
    /// A fresh controller in slow-start.
    pub fn new(params: GovernorParams) -> Self {
        Self {
            target: INITIAL_TARGET.min(params.cap),
            slow_start: true,
            green_streak: 0,
            pressure: Pressure::Green,
            last_admit_ms: None,
            prev_swap: None,
            prev_avail: None,
            params,
        }
    }

    /// The current AIMD target (test/observability surface).
    pub fn target(&self) -> usize {
        self.target
    }

    /// The most recent pressure classification.
    pub fn pressure(&self) -> Pressure {
        self.pressure
    }

    /// Fold a fresh sample into the controller: classify pressure and
    /// adapt the AIMD target. Call once per probe tick with the number of
    /// currently admitted units.
    pub fn tick(&mut self, sample: &ResourceSample, running: usize) -> Pressure {
        let swap_rising = self.prev_swap.is_some_and(|prev| sample.swap_used > prev);
        let storm_drop = self
            .prev_avail
            .map_or(0, |prev| prev.saturating_sub(sample.mem_available))
            > self.params.reserve / STORM_DROP_DIVISOR;
        self.prev_swap = Some(sample.swap_used);
        self.prev_avail = Some(sample.mem_available);

        let pressure = self.classify(sample, swap_rising, storm_drop);
        self.pressure = pressure;

        match pressure {
            Pressure::Green => {
                self.green_streak += 1;
                if self.green_streak >= RAMP_GREEN_TICKS && self.target < self.params.cap {
                    self.target = if self.slow_start {
                        (self.target * 2).min(self.params.cap)
                    } else {
                        self.target + 1
                    };
                    self.green_streak = 0;
                }
            }
            Pressure::Yellow => {
                // Hold: no growth, no shrink. Yellow deliberately does not
                // end slow-start — only red proves the ramp overshot.
                self.green_streak = 0;
            }
            Pressure::Red => {
                self.green_streak = 0;
                self.slow_start = false;
                // Halve relative to what actually runs: a target far above
                // `running` would otherwise take several reds to bite.
                self.target = (running.min(self.target) / 2).max(1);
            }
        }
        pressure
    }

    fn classify(&self, sample: &ResourceSample, swap_rising: bool, storm_drop: bool) -> Pressure {
        let reserve = self.params.reserve;
        if sample.mem_available < reserve || sample.psi_some_avg10.is_some_and(|psi| psi >= PSI_RED)
        {
            return Pressure::Red;
        }
        let yellow_line = reserve / 2 * YELLOW_HEADROOM_FACTOR;
        if sample.mem_available < yellow_line || swap_rising || storm_drop {
            return Pressure::Yellow;
        }
        Pressure::Green
    }

    /// Decide whether one more unit may launch now. `running` is the number
    /// of currently admitted units; `predicted_peak` the candidate's
    /// expected peak tree-RSS (`None` = unprofiled → conservative default).
    ///
    /// `Ok` reserves nothing by itself — the caller tracks the running
    /// count — but it does stamp the stagger clock.
    pub fn try_admit(
        &mut self,
        now_ms: u64,
        running: usize,
        sample: &ResourceSample,
        predicted_peak: Option<u64>,
    ) -> Result<(), HoldReason> {
        // Liveness: something must always run. The containment tier never
        // takes the last unit, so this cannot oscillate with stage 3.
        if running == 0 {
            self.last_admit_ms = Some(now_ms);
            return Ok(());
        }
        if running >= self.params.cap {
            return Err(HoldReason::AtCap);
        }
        if running >= self.target {
            return Err(HoldReason::AtTarget);
        }
        if self.pressure != Pressure::Green {
            return Err(HoldReason::Memory);
        }
        let peak = predicted_peak.unwrap_or(self.params.default_peak);
        if sample.mem_available.saturating_sub(self.params.reserve) <= peak {
            return Err(HoldReason::Memory);
        }
        if let Some(last) = self.last_admit_ms
            && now_ms.saturating_sub(last) < self.params.stagger_ms
        {
            return Err(HoldReason::Stagger);
        }
        self.last_admit_ms = Some(now_ms);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    /// 32 GiB machine, no swap, no PSI — comfortably green.
    fn sample(avail_gib: u64) -> ResourceSample {
        ResourceSample {
            mem_total: 32 * GIB,
            mem_available: avail_gib * GIB,
            swap_used: 0,
            psi_some_avg10: None,
        }
    }

    /// cap 8, reserve 2 GiB.
    fn controller() -> Controller {
        Controller::new(GovernorParams::new(8, 2 * GIB))
    }

    /// Ticks until green, spaced admissions — the plain happy path.
    fn admit_ok(c: &mut Controller, now_ms: u64, running: usize) -> Result<(), HoldReason> {
        c.try_admit(now_ms, running, &sample(20), None)
    }

    #[test]
    fn zero_running_always_admits() {
        let mut c = controller();
        // Even under red pressure with no headroom at all.
        c.tick(&sample(1), 4);
        assert_eq!(c.pressure(), Pressure::Red);
        assert!(c.try_admit(0, 0, &sample(1), Some(64 * GIB)).is_ok());
    }

    #[test]
    fn holds_at_initial_target_then_at_cap() {
        let mut c = controller();
        c.tick(&sample(20), 0);
        // Initial target is 2: third unit holds.
        assert!(admit_ok(&mut c, 0, 1).is_ok());
        assert_eq!(admit_ok(&mut c, 300, 2), Err(HoldReason::AtTarget));
        // Grow the target past the cap boundary and the cap takes over.
        for _ in 0..20 {
            c.tick(&sample(20), 2);
        }
        assert_eq!(c.target(), 8);
        assert_eq!(admit_ok(&mut c, 600, 8), Err(HoldReason::AtCap));
    }

    #[test]
    fn stagger_spaces_cold_start_launches() {
        let mut c = controller();
        c.tick(&sample(20), 0);
        assert!(admit_ok(&mut c, 0, 0).is_ok());
        assert_eq!(admit_ok(&mut c, 100, 1), Err(HoldReason::Stagger));
        assert!(admit_ok(&mut c, 250, 1).is_ok());
    }

    #[test]
    fn insufficient_headroom_for_predicted_peak_holds() {
        let mut c = controller();
        // Arrive at the tight sample twice so the storm-drop classifier
        // settles and pressure reads green — this test is about the
        // per-candidate headroom predicate, not the pressure signal.
        let tight = sample(4);
        c.tick(&tight, 1);
        c.tick(&tight, 1);
        assert_eq!(c.pressure(), Pressure::Green);
        // 4 GiB available − 2 GiB reserve = 2 GiB headroom; a 6 GiB hook
        // must hold even though pressure reads green.
        assert_eq!(
            c.try_admit(1_000, 1, &tight, Some(6 * GIB)),
            Err(HoldReason::Memory)
        );
        // A 1 GiB hook fits.
        assert!(c.try_admit(1_000, 1, &tight, Some(GIB)).is_ok());
    }

    #[test]
    fn unprofiled_candidate_uses_default_peak() {
        let mut c = controller();
        // 2.4 GiB available: headroom over the reserve is ~0.4 GiB, less
        // than the 512 MiB default peak.
        let tight = ResourceSample {
            mem_available: 2 * GIB + 400 * (1 << 20),
            ..sample(20)
        };
        assert_eq!(c.try_admit(0, 1, &tight, None), Err(HoldReason::Memory));
    }

    #[test]
    fn yellow_holds_admissions() {
        let mut c = controller();
        // Below reserve*1.5 (3 GiB) but above reserve (2 GiB) → yellow.
        c.tick(&sample(20), 1);
        let p = c.tick(
            &ResourceSample {
                mem_available: 2 * GIB + GIB / 2,
                ..sample(20)
            },
            1,
        );
        assert_eq!(p, Pressure::Yellow);
        assert_eq!(admit_ok(&mut c, 1_000, 1), Err(HoldReason::Memory));
    }

    #[test]
    fn swap_growth_classifies_yellow() {
        let mut c = controller();
        c.tick(&sample(20), 1);
        let p = c.tick(
            &ResourceSample {
                swap_used: GIB,
                ..sample(20)
            },
            1,
        );
        assert_eq!(p, Pressure::Yellow);
    }

    #[test]
    fn psi_classifies_red() {
        let mut c = controller();
        let p = c.tick(
            &ResourceSample {
                psi_some_avg10: Some(25.0),
                ..sample(20)
            },
            1,
        );
        assert_eq!(p, Pressure::Red);
    }

    #[test]
    fn storm_drop_classifies_yellow_while_headroom_green() {
        let mut c = controller();
        c.tick(&sample(20), 1);
        // 1 GiB drop in one tick (> reserve/4 = 512 MiB) with plenty left.
        let p = c.tick(&sample(19), 1);
        assert_eq!(p, Pressure::Yellow);
    }

    #[test]
    fn slow_start_doubles_then_red_halves_then_additive() {
        let mut c = controller();
        // Two green ticks per doubling: 2 → 4 → 8 (cap).
        for _ in 0..2 {
            c.tick(&sample(20), 2);
        }
        assert_eq!(c.target(), 4);
        for _ in 0..2 {
            c.tick(&sample(20), 4);
        }
        assert_eq!(c.target(), 8);
        // Red with 6 running: halve to 3, slow-start over.
        c.tick(&sample(1), 6);
        assert_eq!(c.target(), 3);
        // Recovery is additive now: +1 per two green ticks.
        for _ in 0..2 {
            c.tick(&sample(20), 3);
        }
        assert_eq!(c.target(), 4);
        for _ in 0..2 {
            c.tick(&sample(20), 4);
        }
        assert_eq!(c.target(), 5);
    }

    #[test]
    fn red_halves_against_running_not_stale_target() {
        let mut c = controller();
        for _ in 0..20 {
            c.tick(&sample(20), 2);
        }
        assert_eq!(c.target(), 8);
        // Only 2 actually running when red hits: target must drop to 1,
        // not to 4.
        c.tick(&sample(1), 2);
        assert_eq!(c.target(), 1);
    }

    #[test]
    fn green_streak_hysteresis_requires_consecutive_ticks() {
        let mut c = controller();
        c.tick(&sample(20), 2); // green #1
        assert_eq!(c.target(), 2);
        c.tick(
            &ResourceSample {
                swap_used: GIB,
                ..sample(20)
            },
            2,
        ); // yellow resets
        c.tick(&sample(20), 2); // green #1 again
        assert_eq!(c.target(), 2);
        c.tick(&sample(20), 2); // green #2 → grow
        assert_eq!(c.target(), 4);
    }

    #[test]
    fn target_never_exceeds_cap() {
        let mut c = Controller::new(GovernorParams::new(3, 2 * GIB));
        for _ in 0..20 {
            c.tick(&sample(20), 3);
        }
        assert_eq!(c.target(), 3);
    }
}
