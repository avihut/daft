# Benchmarks

Two families of benchmarks live here. Both write their results to
`benches/results/` (gitignored) and follow a `:baseline`/`:compare` workflow so
you can pin a SHA's numbers and diff against them as work lands.

## Family 1 — daft vs git (command runtime)

For changes that affect how fast a single daft command runs (`clone`,
`checkout`, `prune`, ...). Each scenario uses [`hyperfine`] to compare
`daft <cmd>` against the equivalent raw-git invocation, three-way against the
git-subprocess opt-out variant where applicable.

```bash
mise run bench                 # All scenarios
mise run bench:clone           # Just clone
mise run bench:baseline        # Pin the current results
mise run bench:compare         # Diff latest against the baseline
```

Implementation:

- `benches/scenarios/*.sh` — one per command
- `benches/bench_framework.sh` — shared `bench_compare` helper
- `mise-tasks/bench/*` — task wiring

When to run: before merging anything that touches the hot path of an existing
command, or when adding a new command.

## Family 2 — test-runner performance (issue #509)

For changes that affect how fast the YAML manual-test suite itself runs.
Distinct from the command benchmarks above — here the unit under test is
`xtask manual-test`, not `daft <cmd>`. Two sub-benchmarks:

```bash
mise run bench:tests:manual              # Single-trial wall-clock check (~30 min serial)
mise run bench:tests:manual:scale        # Scaling sweep across --jobs values (~2-4 hours)
mise run bench:tests:manual:scale-baseline   # Pin scale results
mise run bench:tests:manual:scale-compare    # Diff against baseline
```

`bench:tests:manual` is the cheap "did we regress the serial baseline" check —
one run with default flags. `bench:tests:manual:scale` is the deep "how does
parallelism scale" check, sweeping `--jobs` over a range and emitting
per-scenario p50/p95/max distributions.

Implementation:

- `benches/scenarios/test_manual_scale.sh` — driver
- `mise-tasks/bench/tests/manual/*` — task wiring

### Configuring the scaling sweep

```bash
BENCH_JOBS=1,2,4,8     # comma-separated --jobs values to sweep (default: 1,2,4,8)
BENCH_RUNS=3           # trials per jobs value (default: 3)
BENCH_SKIP_TIMING=1    # skip per-scenario distribution phase (faster)
```

### Per-scenario timing

When `DAFT_MANUAL_TEST_EMIT_TIMING=1` is set, the manual-test runner emits one
grep-friendly line per scenario, broken down by phase:

```
[bench] scenario="Clone basic" elapsed_ms=412 setup_ms=0 fixture_ms=244 template_ms=5
```

The four fields, in order:

- **`elapsed_ms`** — the **step phase**: actual scenario commands running
  through the executor (every `daft …` / `git-worktree-* …` / `bash -c …`
  invocation defined in the scenario YAML). First on the line so the existing
  `awk -F'elapsed_ms='` percentiles pipeline in `test_manual_scale.sh` stays
  compatible.
- **`setup_ms`** — `Sandbox::create_at`: making the per-scenario `/tmp` dirs,
  registering with the cleanup set.
- **`fixture_ms`** — `repo_gen::generate_repo` calls (one per `repos:` entry):
  the bare-git fixture construction that happens before the scenario's steps
  run.
- **`template_ms`** — `Sandbox::create_template`: snapshot of `remotes/` →
  `remotes-template/` so `reset()` can restore from it (post-#511 this is a
  reflink, so it's typically 1–10ms; was 100s+ms with `cp -a`).

The total per-scenario wall-clock at jobs=1 is approximately the sum of all four
(drop/teardown is the residual, typically <20ms). The scaling sweep script's
percentile output uses `elapsed_ms` only; for phase attribution use the full
line.

## Methodology notes

### Wall-clock is machine-relative

Numbers from the bench harness depend on CPU, disk, and concurrent load. The
`summary.md` headers always capture the machine the run was taken on so future
readers know what they're comparing.

**For SHA-over-SHA comparison on the same machine,** the absolute numbers are
directly comparable. Regressions in the daft binary's startup or per-scenario
overhead show up as a slower serial baseline.

**For parallelism quality across SHAs,** use the speedup ratio
(`serial / parallel`) rather than absolute parallel wall-clock. The ratio
abstracts over machine speed; only the curve shape matters.

### Variance

The scaling sweep defaults to 3 trials per jobs value. hyperfine reports mean ±
stddev. A standard deviation above ~5% of the mean is a sign of contention
(background processes, disk pressure) — re-run on a quiet machine before drawing
conclusions.

`bench:tests:manual` is a single trial by design — it's the everyday "did
anything regress?" check, not a publication-quality measurement. Use the scaling
sweep for that.

### Baselines

Two baselines coexist:

- **Local baseline** — what `mise run bench:tests:manual:scale-baseline`
  produces: a copy of your last run pinned to
  `benches/results/test-manual-scale-baseline.md` (gitignored). Use for
  day-to-day "did my change regress my own machine?" checks.
- **Reference baseline** — committed under `benches/baselines/` with a
  date-stamped filename (`test-manual-scale-YYYY-MM-DD.md`). Captures the
  numbers a maintainer measured on a specific machine; future PRs doing #509
  work can compare the _curve shape_ (speedup × efficiency) against it, even
  when their machine differs in absolute speed. Add a new dated file when a PR
  materially moves the numbers — don't edit prior baselines in place.

### Long-form review

When a PR is meant to move the needle on #509:

1. Pin the pre-change baseline: `mise run bench:tests:manual:scale-baseline` on
   the parent commit.
2. Apply the change.
3. Run `mise run bench:tests:manual:scale`.
4. `mise run bench:tests:manual:scale-compare` for the diff.
5. Paste the diff into the PR description, and consider whether to add a fresh
   `benches/baselines/test-manual-scale-<date>.md` reference snapshot.

### Phase attribution

For PRs that move per-scenario cost between phases (e.g. clonefile shrinks
`template_ms`; a fixture cache shrinks `fixture_ms`; a daft-startup amortization
shrinks `elapsed_ms`), wall-clock at high parallelism alone often misses the
real signal — the per-scenario win can get masked by the parallel floor
(contention, scheduler tail, etc.).

The remedy is to run with `DAFT_MANUAL_TEST_EMIT_TIMING=1` at `--jobs 1`,
extract the `[bench]` lines, and build a per-phase cumulative table:

```bash
DAFT_MANUAL_TEST_EMIT_TIMING=1 ./target/release/xtask manual-test --jobs 1 \
    > /tmp/bench.txt
grep '^\[bench\] scenario=' /tmp/bench.txt > /tmp/bench-lines.txt
# Then parse with a small awk/python script — see #511 PR description for an
# example. Sum each phase across all scenarios; report p50/p95/max of the
# non-zero values per phase.
```

The cumulative table in the PR description should look like:

| Phase               | Sum (s) |   % | p50 | p95 | max | scenarios non-zero |
| ------------------- | ------: | --: | --: | --: | --: | -----------------: |
| Step (`elapsed`)    |       A |  A% |   … |   … |   … |                  … |
| Fixture gen         |       B |  B% |   … |   … |   … |                  … |
| Template snapshot   |       C |  C% |   … |   … |   … |                  … |
| Setup (`create_at`) |       D |  D% |   … |   … |   … |                  … |

`A + B + C + D` should match the serial wall-clock minus a few seconds of
drop/teardown. If they don't, something's miscounted. Add the table before/after
the change so reviewers can see which phase moved.

Top-N slowest scenarios per phase is a useful diagnostic addendum when a
particular phase has high p95 or max — surfaces the specific scenarios driving
that tail. Not required, but cheap to include.

## PR description checklist for #509 sub-tasks

Every PR landing under #509's umbrella must include all of these in its
description so future contributors can build on the measurement, not just the
code:

- [ ] **Wall-clock numbers from `mise run bench:tests:manual:scale`** — at
      minimum `BENCH_JOBS=10 BENCH_RUNS=3` (the default-cap measurement), more
      jobs values if the PR is changing parallel scaling specifically. Mean ±
      stddev. State the machine (CPU, OS).
- [ ] **Comparison against a reference baseline** — either the pinned
      `benches/baselines/test-manual-scale-<date>.md` on master, or the
      pre-change SHA captured via `scale-baseline` / `scale-compare`. State the
      diff explicitly; don't make the reader compute it.
- [ ] **Per-phase cumulative table** (see "Phase attribution" above) at
      `--jobs 1`. Even when the PR's headline gain is at high parallelism, the
      phase table is what attributes the change correctly across `setup_ms` /
      `fixture_ms` / `template_ms` / `elapsed_ms`.
- [ ] **Honest assessment of what moved vs what's left**. If the suite-level
      wall-clock at `--jobs 10` didn't change measurably, say so and explain why
      (e.g., "the phase this PR shrank was already a 0.6% slice; the win becomes
      visible after [other sub-task] lands").
- [ ] **Coverage of #509's umbrella checklist** — which sub-tasks remain after
      this lands, and whether any new gaps were surfaced during measurement
      (file fresh issues, link them).

If the PR materially changes the curve shape (parallel efficiency, p95 of any
phase, etc.), add a fresh `benches/baselines/test-manual-scale-<date>.md`
snapshot capturing the post-change numbers — future PRs will compare against it.
Don't edit prior baselines in place.

[`hyperfine`]: https://github.com/sharkdp/hyperfine
