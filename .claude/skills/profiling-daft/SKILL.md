---
name: profiling-daft
description: Use when profiling or optimizing the runtime of daft or its test suites — finding where time goes, choosing a profiler on macOS, or A/B-validating a perf change. Covers the benchmark-vs-profile split (and the existing bench infra), the macOS Apple-Silicon toolchain (samply, hyperfine, why dtrace is out), idle-gating on a shared machine, the shared-bin/DAFT_BINARY_DIR A/B trick, the EMIT_TIMING-first method, and a baseline map of where the manual-test suite's time actually goes.
---

# Profiling daft

How to investigate **where daft's runtime goes** — the binary and the YAML test
suite — and how to A/B-validate a fix. Read before any perf/optimization work.

> **Benchmark vs profile.** daft already has rich *benchmarking* infra (compare
> wall-clock, prove a change is faster). Do **not** reinvent it — use it to
> validate. This skill covers *profiling* (find the bottleneck), which daft did
> not document.
>
> Existing benchmarking infra (for validation):
> - `mise run bench:<cmd>` — per-command vs competition/baseline (`benches/`).
> - `mise run bench:tests:integration` — TUI bash-vs-YAML; `bench:tests:manual` — YAML timing.
> - `benches/scenarios/test_manual_scale.sh` — percentiles over the manual suite.
> - `DAFT_MANUAL_TEST_EMIT_TIMING=1` — per-scenario `[bench]` lines (see below).

## Method (cheapest, highest-signal first)

1. **Test the presupposition before chasing it.** Do the arithmetic first:
   `wall × workers ÷ steps` ≈ per-step work. For the manual suite that's
   ~57s × 10 ÷ 2217 ≈ **~250ms/step** — git-operation territory, not
   process-startup territory. A "turn off feature X" hunch is often refuted by
   one division.
2. **Mine the existing timing before instrumenting.** Run
   `DAFT_MANUAL_TEST_EMIT_TIMING=1 mise run test:manual -- --jobs 1` and aggregate
   the `[bench] scenario="…" elapsed_ms=N setup_ms=N fixture_ms=N template_ms=N`
   lines. This buckets per-scenario cost for free and ranks the slow tail.
3. **Only then add probes.** Reuse the `DAFT_MANUAL_TEST_EMIT_TIMING` gate for new
   per-scenario timers; env-gate any daft-internal probe (e.g. a counter at a
   `gix::discover()` chokepoint) so it ships disabled.
4. **Earn an "it's intrinsic" verdict — don't assume it.** If you conclude a hot
   path can't be cut, prove it by looking *inside* (sample CPU, count calls), not
   by inspecting its shape. Redundant per-invocation work hides behind "git is
   just slow."
5. **CPU sampling is load-robust; wall-clock is not.** A flamegraph's *relative*
   breakdown survives background load; any wall-clock number (hyperfine, suite
   Duration) does not — see idle-gating.

## macOS Apple-Silicon toolchain

| Tool | Use for | Notes |
|---|---|---|
| `hyperfine` | wall-clock A/B of a CLI | Runs each command in a *block* (not interleaved) — **idle-gate it**. `--warmup`, `-N` (no shell), `--export-json`. |
| `samply` | CPU flamegraph of daft / the runner | `cargo binstall samply` (or `cargo install`). Needs debug symbols → build `--profile profiling`. Browser-based; follows child processes. |
| `/usr/bin/sample` | quick text call-tree | Built-in, no install; needs a process living long enough to attach. |
| `cargo-instruments` | off-CPU / syscall / exec trace | Needs **full Xcode** (Command Line Tools / `xcode-select --install` is not enough). Only when CPU sampling proves the cost is "spawn + wait." |
| `criterion` / `divan` | in-process microbench | For isolating one op (e.g. `generate_repo`). Per-process sampling is hopeless at tens-of-ms — bench the op directly. |
| ~~`dtrace` / `dtruss`~~ | — | **SIP-restricted on macOS; do not rely on it.** Use samply. |

Short-lived processes (a daft invocation is tens of ms) yield too few samples for
per-process attribution — loop the op, or use hyperfine for wall + samply on the
aggregate suite run.

## daft-specific gotchas

- **Build with `[profile.profiling]`** (release + debug symbols), never plain
  `release` — the release profile is `strip = true` + `opt-level = "z"`, so samply
  frames come back blank. Don't `cargo clean` between build and profile (unpacked
  split-debuginfo lives in `target/**/*.o`).
- **Shared-bin hash invalidation.** Editing any `.rs` changes the shared-bin
  content hash, forcing a slow `opt-z`+fat-LTO release rebuild. To A/B a *runner*
  (`xtask`) change cheaply, bypass it: `DAFT_BINARY_DIR=<cached release dir>
  cargo run -p xtask -- manual-test` rebuilds only debug xtask.
- **Don't fork-count with a PATH `git` shim** — it perturbs daft and hangs
  (`git rev-parse` blocked). Count forks from code, or instrument the spawn site.
- **`gix::discover()` is cached per `GitCommand` instance, not across them**
  (`src/git/mod.rs`). A command builds several `GitCommand`s (settings, hooks,
  itself) → it discovers the repo 2–3×. Watch for this multiplier in any per-
  command path.
- **Replicate the test env for standalone profiling** or you profile a different
  code path: `DAFT_TESTING=1` (gates background daemons — see below), a
  `DAFT_CONFIG_DIR` sandbox, and cwd inside a real worktree.

## Idle-gating (shared / multi-agent machines)

Other agents may be building in sibling worktrees. **Re-verify idle immediately
before each wall-clock bench** (CPU sampling is exempt). A simple gate: 1-min
loadavg `< 5`, no `rustc > 40% CPU`, no `manual-test`/`cargo` process, sustained
~90s. A suite run drives its own load to 40–90, so back-to-back runs see decaying
self-inflicted averages — interpret accordingly.

## `[profile.profiling]`

Checked into the workspace `Cargo.toml`. Tuned for **readable** flamegraphs +
fast builds (clear frames + quick compile beat faithful-but-opaque fat-LTO for
finding redundant calls): `-O2`, no LTO, many codegen units, full DWARF. Build
with `cargo build --profile profiling`. For absolute-timing fidelity to the
shipped binary, profile the size-optimized `release` instead (slower, opaquer).

## Baseline map — where the manual suite's time goes

Measured on a 10-core Apple-Silicon Mac (post-#578). Re-measure after structural
changes; treat as orientation, not gospel.

- **Total:** 581 scenarios / 2217 steps. Reported parallel Duration ≈ **57s**;
  full `mise run test:manual` wall ≈ **64s**.
- **The suite is git-subprocess + filesystem bound (91%), not startup/feature bound.**
  Summed core-work (÷ workers ≈ wall):
  - step-loop (daft invocations + git assertions): **506s / 91%**
  - fixture provision: 45s / 8% (**40s is inline repos bypassing the fixture cache**)
  - template snapshot: 5.6s / 1% (**dead work** — `create_template()` runs every
    scenario but `reset()` is interactive-only)
  - sandbox dir setup: ~0
- **Per-command cost is git/gix work, not startup.** daft startup ≈ **5.5ms**
  (faster than `bash -c true`); `daft worktree-list` ≈ 86ms (raw `git worktree
  list` ≈ 7ms) — the gap is status-gathering + redundant discovery.
- **Ruled out:** worker oversubscription (Duration flat at `--jobs` 10/16/24 →
  CPU-saturated at `ncpu`); disabling startup features/daemons (already gated under
  `DAFT_TESTING`, the runner sets it); disabling WAL/coordinator/gitoxide/hooks
  (load-bearing → deletes test coverage). The expensive features are already off
  or are exactly what the scenarios assert.

The actionable wins from that map are tracked as perf issues (lineage #509):
redundant `gix::discover()` (a ships-to-users win, not just harness), the dead
template snapshot, and routing inline repos through the fixture cache.
