# Benchmark Suite Design

**Date:** 2026-02-20 **Branch:** chore/benchmark

## Goals

- Compare daft commands against equivalent git scripting, with no concessions to
  the git side
- Test realistic scenarios including hook-equivalent setup (environment
  bootstrap, dependency install, etc.)
- Track performance over time to detect regressions and confirm improvements
- Publish results publicly on the docs site
- Later: compare against competing tools (git-town, shell aliases, etc.)

## Tool

**Hyperfine** — CLI benchmarking tool with statistical analysis, warmup runs,
command comparison, and JSON/markdown export. Used by git, delta, and other
serious CLI tools.

## Directory Structure

```
benches/
  bench_framework.sh        # shared setup/teardown, hyperfine wrappers, result helpers
  fixtures/
    create_repo.sh          # creates synthetic repos of configurable size (small/medium/large)
    real_repos.sh           # downloads/caches real repos (git, linux kernel, etc.)
  scenarios/
    clone.sh                # daft clone vs. git clone + worktree setup
    clone_with_hooks.sh     # daft clone (with post-clone hook) vs. git + manual hook equivalent
    checkout.sh             # daft checkout-branch vs. git worktree add
    checkout_with_hooks.sh  # daft checkout (with worktree-post-create hook) vs. manual equivalent
    init.sh                 # daft init vs. git init + worktree setup
    prune.sh                # daft prune vs. git worktree prune + rm
    fetch.sh                # daft fetch vs. git fetch --all (parallelized where possible)
    branch_delete.sh        # daft branch-delete vs. git worktree remove + git branch -d
    workflow_full.sh        # end-to-end: clone -> checkout -> work -> prune cycle
    vs_competition.sh       # opt-in: daft vs. git-town, shell aliases, etc.
  results/
    latest.json             # most recent run (gitignored locally, committed by CI)
    baseline.json           # pinned baseline for regression comparison
  history/                  # dated JSON files committed by CI for trend tracking
  run_all.sh                # orchestrates all scenarios, aggregates to markdown

mise-tasks/bench/
  (tasks mirroring test structure: bench, bench:clone, bench:checkout, etc.)

docs/benchmarks/
  index.md                  # auto-generated results page, deployed to docs site
```

## Scenarios

### Core command comparisons

Each scenario runs against three synthetic repo sizes:

- **small:** 100 files, 50 commits
- **medium:** 1,000 files, 500 commits
- **large:** 10,000 files, 2,000 commits

And one real-repo variant (the git project itself, cached locally).

| Scenario        | daft                                    | Git equivalent                                                          |
| --------------- | --------------------------------------- | ----------------------------------------------------------------------- |
| clone           | `git-worktree-clone <url>`              | `git clone --bare` + `git worktree add` + initial branch setup          |
| checkout        | `git-worktree-checkout <branch>`        | `git worktree add <path> <branch>`                                      |
| checkout-branch | `git-worktree-checkout-branch <branch>` | `git worktree add -b <branch> <path>`                                   |
| init            | `git-worktree-init`                     | `git init --bare` + `git worktree add`                                  |
| prune           | `git-worktree-prune`                    | `git worktree list --porcelain` + `git worktree remove` per stale entry |
| fetch           | `git-worktree-fetch`                    | `git fetch --all` (parallelized with `&` + `wait` across remotes)       |
| branch-delete   | `git-worktree-branch-delete`            | `git worktree remove` + `git branch -d`                                 |

### Hook scenarios

These measure daft's hook system against a competent manual equivalent. The git
side uses parallelism where applicable (`&`, `wait`, `xargs -P`) — no strawman
comparisons.

**clone_with_hooks:** Repos that require post-clone setup (e.g. installing
dependencies, writing `.envrc`, running `mise trust`). daft runs the
`post-clone` hook; git side runs the same commands manually in a shell script
after cloning.

**checkout_with_hooks:** Repos where each worktree needs environment bootstrap
on creation (e.g. `mise install`, `direnv allow`). daft runs
`worktree-post-create`; git side scripted.

The point is to show what daft's hooks cost vs. save compared to doing it
manually — including cases where the manual script can parallelize steps that
daft runs sequentially.

### Full workflow

`workflow_full.sh` benchmarks a realistic daily workflow with no daft
equivalent:

1. Clone a repo
2. Check out 3 feature branches as worktrees
3. Run a build/install hook in each
4. Prune two of them

The git side uses maximum parallelism (steps 2-3 parallelized with `&` +
`wait`). This gives the most honest picture of end-to-end time.

### Competitor comparison (opt-in)

`vs_competition.sh` compares against tools like git-town and common shell alias
patterns. Not run in CI by default — requires competitors to be installed.
Intended for published comparisons on the docs site, run manually and results
committed.

## Results & Publishing

### Local

Each hyperfine run exports JSON to `benches/results/`. `run_all.sh` aggregates
into a markdown table printed to stdout. A pinned `baseline.json` enables
`mise run bench:compare` to diff current results against baseline.

### CI

A new `bench.yml` GitHub Actions workflow triggers on push to `master`. It:

1. Runs all synthetic-repo scenarios (no network dependency)
2. Uploads JSON as a workflow artifact
3. Generates `docs/benchmarks/index.md` and commits it
4. The existing Cloudflare Pages pipeline picks it up and deploys to the docs
   site

Benchmarks are informational only — they never fail or block a build.

### Historical tracking

CI commits dated JSON to `benches/history/YYYY-MM-DD-vX.Y.Z.json`. The docs page
shows a table of recent runs with version tags, giving the "performance over
time" view without an external service.

## Mise Tasks

```
bench               # Run all benchmark scenarios
bench:clone         # Run only clone scenario
bench:checkout      # Run only checkout scenarios
bench:hooks         # Run hook scenarios
bench:workflow      # Run full workflow scenario
bench:competition   # Run competitor comparison (opt-in, requires competitors installed)
bench:compare       # Compare latest results against baseline
bench:baseline      # Pin current results as new baseline
```
