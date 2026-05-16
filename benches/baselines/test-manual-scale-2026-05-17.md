# Manual-test runner scaling — post-#510 baseline

Reference results captured after the parallelization work in #510 landed on this
branch. Pinned here so future #509 work can be measured against a defensible
starting point.

- **Captured:** 2026-05-17, 02:30–02:55 UTC+3
- **SHA:** `208c3603` (branch `daft-510/perf/parallelize-yaml-test-runner`) —
  includes both the `perf(test-runner): parallelize…` commit and the
  `fix(test-runner): suppress orphan log-clean spawns…` commit
- **Machine:** Apple M1 Max — 10 physical / 10 logical cores
- **macOS:** Darwin 25.4.0
- **Scenarios in corpus:** 572 (master at `fe3e9faf`, post-coordinator redesign)
- **Default cap (`--parallel`):** 5 (= `available_parallelism() / 2`)
- **Harness:** `mise run bench:tests:manual:scale`, hyperfine 1.20.0

## Headline

| Mode                          | Wall-clock | Speedup vs serial | Speedup vs pre-PR main† |
| ----------------------------- | ---------- | ----------------- | ----------------------- |
| pre-PR main (serial, no fix)  | ~1586 s    | —                 | 1.00×                   |
| this PR, `--jobs 1`           | 392 s      | 1.00×             | **4.05×**               |
| this PR, `--parallel` (cap=5) | 100 s      | **3.92×**         | **15.9×**               |
| this PR, `--jobs 10`          | 82 s       | 4.78×             | 19.3×                   |

† The pre-PR row is a one-shot measurement on the parent commit
(`xtask -- manual-test --ci`) before either the parallelization work or the
`DAFT_NO_LOG_CLEAN=1` test-env fix landed. Two effects combine: (a) parallelism
amortizes scenarios across workers, and (b) suppressing `daft __clean-logs`
orphan spawns prevents init-reparented daemons from stealing CPU as the corpus
grows. The fix alone is responsible for the 4.05× jump from
`pre-PR main → --jobs 1` — even users who don't opt into `--parallel` will see a
serial run drop from ~26 minutes to under 7.

## Phase 1 — wall-clock by `--jobs` (full sweep)

| --jobs | Mean     | σ       | Min      | Max      | Trials | Speedup | Efficiency |
| ------ | -------- | ------- | -------- | -------- | ------ | ------- | ---------- |
| 1      | 392.10 s | 1.51 s  | 390.47 s | 393.46 s | 3      | 1.00×   | 100%       |
| 2      | 217.22 s | 0.94 s  | 216.64 s | 218.30 s | 3      | 1.81×   | 90%        |
| 4      | 113.78 s | 0.46 s  | 113.26 s | 114.13 s | 3      | 3.45×   | 86%        |
| 5      | 100.00 s | 3.45 s  | 97.07 s  | 103.80 s | 3      | 3.92×   | 78%        |
| 8      | ~144 s   | (noisy) | 123.12 s | 165.91 s | 2      | 2.71×   | 34%        |
| 10     | 82.19 s  | —       | 82.19 s  | 82.19 s  | 1      | 4.78×   | 48%        |

Efficiency = `speedup / jobs` — ideal is 100%.

### What the curve says

- **Scaling is near-linear up to ~num_cpus/2 (jobs=4–5).** The default cap that
  `--parallel` picks is well-chosen on this class of machine.
- **Past num_cpus/2, returns diminish quickly.** jobs=8 actually _regresses_
  below jobs=5 (longer wall-clock, higher variance, and scenario failures
  appeared in one of two trials at jobs=8 — likely contention on a shared
  resource we haven't traced). jobs=10 recovers and improves to 4.78× but with
  much lower efficiency.
- **Don't push `--jobs` above `num_cpus/2` casually.** The default cap exists
  for a reason.

## Phase 2 — per-scenario distribution

572 scenarios, captured via `DAFT_MANUAL_TEST_EMIT_TIMING=1`.

| Mode                 | p50    | p95     | max     | Cumulative scenario time |
| -------------------- | ------ | ------- | ------- | ------------------------ |
| `--jobs 1` (serial)  | 498 ms | 1111 ms | 3213 ms | 300.0 s                  |
| `--jobs 5` (default) | 567 ms | 1293 ms | 3502 ms | 342.5 s                  |

Notes:

- Per-scenario times grow ~14% (p50) under `--jobs 5` due to CPU contention.
  Acceptable trade-off for the 3.92× wall-clock win.
- Cumulative scenario time (300 s) is less than the serial wall-clock (392 s);
  the ~92 s gap is xtask-level overhead per scenario — sandbox `mkdir`/`cp -a`
  setup, `TestEnv::Drop` teardown, output buffer serialization — that lives
  outside `run_non_interactive` and isn't counted in the per-scenario timer.
- That fixed overhead is what caps efficiency: at `--jobs 5` the cumulative work
  is 342.5 s spread across 5 workers (perfect scheduling → 68.5 s); we measure
  100 s, leaving ~31 s for the same fixed overhead amortized across the parallel
  run.

## Failure rate

| Mode                 | Scenarios | Steps | Passed | Failed              |
| -------------------- | --------- | ----- | ------ | ------------------- |
| `--jobs 1` (serial)  | 572       | 2193  | 2193   | 0                   |
| `--jobs 5` (default) | 572       | 2193  | 2193   | 0                   |
| `--jobs 8`           | 572       | —     | varies | 1+ in 1 of 2 trials |
| `--jobs 10`          | 572       | —     | varies | flaky               |

Parallelism at the default cap surfaces zero new failures across multiple runs —
the acceptance criterion's "no new flakes" is met for that knob. Pushing past
`num_cpus/2` does introduce occasional failures; this is consistent with the
"diminishing returns" reading of the wall-clock curve and is the reason the
default cap is `num_cpus/2` rather than `num_cpus`.

## Methodology

1. Build release binaries:
   `cargo build --release && cargo build -p xtask --release`.
2. Clean `/tmp`:
   `find -L /tmp -maxdepth 1 -name 'daft-manual-test-*' -type d -exec rm -rf {} +`.
3. Confirm no orphan daft processes:
   `ps -e -o pid,ppid,command | awk '$2==1 && /target\/release\/daft/'` should
   be empty.
4. Run:
   ```bash
   BENCH_JOBS=1,2,4,5,8,10 BENCH_RUNS=3 mise run bench:tests:manual:scale
   ```
5. To re-baseline this file on a new machine or after a corpus change, run the
   same command and copy `benches/results/test-manual-scale.md` into
   `benches/baselines/` with a date-stamped filename.

### Variance and noise sources

- `--jobs 5` showed σ = 3.45 s (~3.4% of the mean) vs σ ≤ 1.5 s at jobs={1,2,4}.
  The default cap _is_ the contention boundary on this machine; variance climbs
  when workers approach saturation.
- `--jobs 8` ran with only 2 trials in this baseline (the bench harness exited
  mid-sweep — see "Known harness issue" below); the wide range (123–166 s)
  suggests it'd need 5+ trials to converge.
- `--jobs 10` is a single trial. The number is suggestive, not rigorous.

### Known harness issue

`hyperfine` exited mid-sweep twice during baseline capture with
`Error: No such file or directory (os error 2)` between consecutive parameter
values. Root cause not isolated; the script now catches a non-zero hyperfine
exit so partial JSON is preserved (`benches/scenarios/test_manual_scale.sh`).
When this happens, the later parameter values are missing — re-run those
individually:

```bash
hyperfine --warmup 0 --runs 3 --ignore-failure \
    "target/release/xtask manual-test --ci --jobs N"
```

## What this enables for #509

The umbrella's purpose is making the YAML manual-test runner fast enough to run
on every PR without slowing down the dev loop. Today:

- Pre-#510 + `DAFT_NO_LOG_CLEAN`: 392 s = 6.5 min serial. Workable but not
  joyful.
- This PR with `--parallel`: 100 s = 1.7 min. Below the "context-switch cost"
  threshold — fast enough that engineers will actually wait for it on a PR.

Going forward, the bench should fire on any change that touches:

- `xtask/src/manual_test/` (the runner itself)
- `tests/manual/scenarios/` (new scenarios that don't parallelize well)
- The hot path of `daft` startup (a 50 ms regression per invocation = ~30 s
  added to the serial baseline across 572 scenarios)
- `src/coordinator/` or `src/hooks/` (the highest-fanout subsystems per
  scenario)

Re-baseline this file when the corpus grows by ~10%+, when MSRV changes (codegen
variance), or when a structural change to the runner lands.
