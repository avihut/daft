# Benchmarks

Two families of benchmarks live here. Both write their results to
`benches/results/` (gitignored) and follow a `:baseline`/`:compare` workflow so
you can pin a SHA's numbers and diff against them as work lands.

## Family 1 — daft vs git (command runtime)

For changes that affect how fast a single daft command runs (`clone`,
`checkout`, `prune`, ...). Each scenario uses [`hyperfine`] to compare
`daft <cmd>` against the equivalent raw-git invocation, three-way against the
gitoxide variant where applicable.

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
grep-friendly line per scenario:

```
[bench] scenario="Clone basic" elapsed_ms=412
```

The scaling sweep script uses this to compute p50/p95/max distributions. Outside
the bench harness, you can pipe the output through `grep ^\[bench\]` to capture
per-scenario timings yourself.

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

[`hyperfine`]: https://github.com/sharkdp/hyperfine
