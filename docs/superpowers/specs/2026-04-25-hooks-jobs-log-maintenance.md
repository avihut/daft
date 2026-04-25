# Hooks Jobs Log Maintenance — Design Spec

**Sub-project:** Follow-up to the hooks-jobs basket **Parent:**
[`2026-04-11-hooks-jobs-basket-overview.md`](./2026-04-11-hooks-jobs-basket-overview.md)
**Depends on:** Universal hook logging (sub-project A, complete) **Branch:** TBD
— not yet started; this spec captures the design before work begins.

## Goal

Make `daft hooks jobs` log storage self-maintaining: respect the configured
`retention`, bound disk usage, run cleanup automatically without user
intervention, and expose enough visibility that a user can answer "what is daft
using my disk for, and what's about to be deleted?" without resorting to `du`.

The work splits into a v1 minimum-viable bundle (this branch / next branch) and
a v2 menu of compounding improvements (filed for later, deliberately not shipped
together).

## Motivation

Universal hook logging now writes one job dir per (invocation, job) under
`$XDG_STATE_HOME/daft/jobs/<repo-uuid>/<invocation-id>/<job-name>/` with a
`meta.json` and an `output.log`. The system has three real gaps today:

1. **Cleanup is manual only.** Nothing calls `LogStore::clean()` except the user
   explicitly typing `daft hooks jobs clean`. A user who never runs that command
   accumulates logs forever.

2. **The `retention` config is parsed but ignored.** `src/hooks/yaml_config.rs`
   accepts `log.retention: 14d` (top-level and per-job), it round-trips through
   `JobSpec.log_config.retention`, and `docs/guide/hooks.md:464` documents it.
   But `clean_logs` in `src/commands/hooks/jobs.rs:1293,1303` hardcodes
   `Duration::days(7)`, so the configured value never reaches the cleanup path.
   This is worse than not exposing the knob — the docs lie.

3. **No size protection.** A runaway background job that prints to stdout in a
   loop can fill the disk. There's no per-file cap, no per-repo budget, and no
   visibility (no size column, no total-usage view).

The fix borrows the proven `__hidden-subcommand` pattern from
`src/update_check.rs` and `src/trust_prune.rs`: every `daft` invocation calls a
`maybe_clean_logs()` entry point that consults a small JSON cache; if stale
(>24h), it spawns a detached `daft __clean-logs` child process and returns
immediately. Zero latency cost on the hot path.

---

# v1 — minimum viable bundle

Self-contained scope, intended to be merged as a single PR (or a small stack of
stacked commits) after the current `feat/background-hook-jobs` branch lands.

## Trigger surface

### Background trigger (primary)

New module `src/log_clean.rs` mirrors `src/trust_prune.rs`:

```rust
pub const NO_LOG_CLEAN_ENV: &str = "DAFT_NO_LOG_CLEAN";
const CACHE_TTL_SECONDS: i64 = 24 * 60 * 60;

pub fn maybe_clean_logs() { /* fire-and-forget, throttled */ }
pub fn run_clean_logs() -> Result<()> { /* the actual work */ }
```

Wired into `src/main.rs` next to the existing
`update_check::maybe_check_for_update()` and `trust_prune::maybe_prune_trust()`
calls. Same guards:

- `DAFT_NO_LOG_CLEAN` env-var opt-out.
- CI environment auto-disables (reuse `is_ci_environment()` from `trust_prune`).
- Refuse to run when own argv contains `__` (prevents fork bombs).
- `catch_unwind` wraps the entry point.

Cache file at `$XDG_CONFIG_HOME/daft/log-clean.json`:

```json
{
  "version": 1,
  "cleaned_at": 1745740800,
  "last_summary": {
    "removed_invocations": 23,
    "removed_jobs": 87,
    "freed_bytes": 4_823_220,
    "reason": "retention"
  }
}
```

The `last_summary` is written by every successful `run_clean_logs()` and
consumed by the listing footer (see Visibility below).

### Foreground trigger (existing, extended)

`daft hooks jobs clean` continues to exist as the manual escape hatch, with new
flags:

```text
daft hooks jobs clean [--all] [--older-than <duration>] [--dry-run]
```

- `--older-than <duration>` overrides resolved retention for this run.
- `--dry-run` prints what would be removed without removing it.
- `--all` (existing) — sweep across all repos, not just the current one.

When invoked without `--older-than`, the foreground command resolves retention
from config the same way the background path does.

### On-coordinator-boot trigger (deferred to v2)

Not in v1. Coordinator forks add complexity — leave to v2.

## Retention resolution

`retention` is resolved per-job using the existing precedence chain in
`src/hooks/yaml_config_loader.rs`:

1. Built-in default (`7d`).
2. Global config (`~/.config/daft/config.yml` if present — currently unused for
   hooks; introduce only if needed).
3. Repo config (`daft.yml` `log.retention`).
4. Local config (`daft-local.yml` `log.retention`).
5. Per-job (`hooks.<type>.jobs[].log.retention`).

**Resolution happens at hook-fire time, not at cleanup time.** The merged value
is captured into `JobMeta.retention_seconds: Option<i64>` at the moment the hook
fires. Cleanup reads it directly from `meta.json` and never touches `daft.yml`.

Why this matters: (1) **predictable** — "the retention you had configured when
the hook ran governs that invocation's lifespan" matches user intuition better
than "we honor whatever's in daft.yml right now"; (2) **survives worktree
deletion, repo relocation, and config corruption** — none of these break the
cleanup path; (3) **removes a class of failure modes** around missing
`daft.yml`, failed re-parse, and races between config edits and cleanup runs.

The same capture-at-fire-time treatment applies to `max_log_size` (stored into
`JobMeta.max_log_size_bytes: Option<u64>`).

For repo-level fields (`max_total_size`, `keep_last`, `stale_running_after`),
see § Repo-level policy sidecar below.

**Parse failure handling:** `"7days"`, `"1week"`, malformed `"abc"` — log a
warning to stderr at hook-fire time (when we have a real terminal), fall back to
the default. Never abort cleanup over a config typo.

## Repo-level policy sidecar

The repo-level fields — `max_total_size`, `keep_last`, `stale_running_after` —
cannot live on a per-job `JobMeta` because they govern decisions across all
invocations under a repo. They're also too small (3 values) to justify a
database.

Each hook fire writes the resolved repo-level policy to a sidecar at
`<state>/jobs/<repo-uuid>/repo-policy.json`. Most-recent-write wins. Cleanup
reads this file once per repo at the start of its run; if the file is missing
(orphaned state dir whose repo no longer fires hooks), built-in defaults apply.

The sidecar contains:

```json
{
  "version": 1,
  "max_total_size_bytes": 524288000,
  "keep_last": 3,
  "stale_running_after_seconds": 86400
}
```

All three fields are `Option<T>` in the on-disk shape; `None` triggers the
built-in defaults at read time.

## Size limits

Two independent caps, both enforced by `run_clean_logs()`. Whichever fires first
wins.

### Per-log file cap (default 10 MB)

Configured via `log.max_log_size` (top-level, repo, local, per-job — same chain
as `retention`). When a finished job's `output.log` exceeds the cap:

- Truncate the file to `cap - footer_len`.
- Append a footer: `\n[output truncated at 10485760 bytes]\n`.
- Update `meta.json` with `"log_truncated": true` and
  `"original_size_bytes": N`.

Truncation only happens to **terminal-status** jobs (Completed, Failed,
Cancelled, Skipped). Running jobs are left alone — truncating a live writer
mid-stream invites corruption.

Implemented as a pre-pass at the top of `run_clean_logs()`: scan all
terminal-status job dirs, truncate any over-cap. Fast (one stat per file).

### Per-repo size budget (default 500 MB)

Configured via `log.max_total_size` (top-level / repo / local — _not_ per-job;
this is a global ceiling). After retention sweeps, if the repo's
`<state>/jobs/<repo-uuid>/` total still exceeds budget:

- List remaining invocations sorted by `created_at` ascending (oldest first).
- Subject to the **sanity floor** (below), evict invocations one at a time until
  under budget.

Enforced as a post-pass after retention. The two layers compose: retention is a
"you wouldn't want this anyway" sweep; the budget is a hard ceiling.

## Sanity floor

Always retain the **last 3 invocations per worktree**, regardless of retention
or budget. Prevents the worst UX paper cut: "I haven't run a hook in 30 days, my
retention is 7d, now I have no record of what happened."

Implementation: when computing the eviction candidate set, group by `worktree`,
sort each group by `created_at` desc, drop the head 3, return the tail. Apply to
both retention and budget passes.

Configurable via `log.keep_last` (top-level / repo / local), default `3`.

## Stale-`Running` detection

Today, a job stuck in `Running` is permanently un-cleanable
(`!matches!(meta.status, JobStatus::Running)` in `LogStore::clean`). This
becomes a real problem if a coordinator dies without flipping its child jobs to
terminal status — the job dirs become immortal.

**Heuristic:** treat a job as stale-Running if all three are true:

1. `started_at` is older than 24 hours (configurable: `log.stale_running_after`,
   default `24h`).
2. The job's coordinator socket
   (`coordinator::coordinator_socket_path(repo_id)`) does not exist.
3. The job's recorded PID, if any, is not alive (`kill(pid, 0)` returns
   `ESRCH`).

When all three hold, treat as `Cancelled` for cleanup purposes. Optionally write
`"stale_running_marked_at": <timestamp>` into the meta. Log a debug line.

Out of scope here: actually fixing whatever caused the stale Running state.
That's a separate bug, captured in v2.

## Concurrency safety

**Single-flight lock.** The 24h throttle alone is insufficient if two `daft`
invocations fire within the same second. Add an advisory file lock on the cache
file's lock sibling, using `fs2::FileExt::try_lock_exclusive` for its
cross-platform behavior:

```rust
use fs2::FileExt;

let lock_path = cache_path.with_extension("lock");
let lock_file = OpenOptions::new()
    .write(true)
    .create(true)
    .truncate(false)
    .open(&lock_path)?;
match lock_file.try_lock_exclusive() {
    Ok(_) => { /* proceed */ }
    Err(_) => return Ok(()),  // another cleanup is running, defer to it
}
```

`fs2` was chosen over `nix::fcntl::flock` because its API is portable across
macOS, Linux, and Windows (we're Unix-only today, but cross-platform is cheap to
keep) and avoids the BSD `flock(2)` NFS quirks on macOS. `fs2` is already a
transitive dependency via test infrastructure; promoting to direct adds zero
binary size.

**Atomic remove.** Replace `fs::remove_dir_all(&job_dir)` with
rename-then-remove:

```rust
let trash = job_dir.with_file_name(format!(".deleting-{}", job_dir.file_name()));
fs::rename(&job_dir, &trash)?;
fs::remove_dir_all(&trash)?;
```

Avoids a reader observing half-deleted state. Cheap, no observable behavior
change in the happy path.

**Custom-path safety.** The hook config supports `log.path` for custom log file
locations. Cleanup must touch only paths under the canonical
`<state>/jobs/<repo-uuid>/`. Add a defensive `path.starts_with(state_dir)?`
check in `clean()` and explicit log warnings if any non-canonical path is
referenced. Already documented as user-managed in `docs/guide/hooks.md:472`, but
enforce it in code.

## Visibility

### `Size` column in human listing

`daft hooks jobs` (and `--all`) gain a `Size` column showing the log file's
bytes formatted human-readable (`4.2 KB`, `1.1 MB`). Computed as
`fs::metadata(log_path).len()` per row.

```text
> master

<1m -- worktree-post-create [907e]
  Job              Status        Started    Duration   Size
  → install         ✓ completed   10:49:15   11s        4.2 KB
  ↪ warm-build      ✗ failed      10:49:26   3s         812 B
  → direnv-allow    ✓ completed   10:49:15   42ms       0 B
  ↪ db-seed         ✗ failed      10:49:26   88ms       214 B
  ↪ db-migrate      ✗ failed      10:49:26   36ms       198 B
```

Trailing footer when `--all`:

```text
Total: 124 invocations, 47.3 MB across 8 worktrees
Last log cleanup 4h ago: removed 23 job log(s) (4.2 MB freed)
```

Last-cleanup line reads from the `last_summary` field in `log-clean.json`.

### `size_bytes` column in `--format json`

Add to the `Tabular` payload schema in `build_jobs_payload`:

```rust
"size_bytes",   // int|null  — bytes of output.log (null when log file absent)
```

Update `docs/cli/daft-hooks-jobs.md` schema section.

### `daft hooks jobs clean --dry-run` output

```text
Would remove 23 jobs across 5 invocations (would free 4.2 MB):
  feature/auth      worktree-post-create [a3f2]   2026-04-15  ~1.1 MB
  feature/auth      worktree-post-create [b711]   2026-04-16  ~890 KB
  ...
Reason: retention (>14d, configured)
```

## Configuration schema

Extend the existing `log:` block in `daft.yml`:

```yaml
log:
  retention: 14d # already exists; honored as of v1
  max_log_size: 10MB # NEW — per-file cap
  max_total_size: 500MB # NEW — per-repo budget
  keep_last: 3 # NEW — sanity floor
  stale_running_after: 24h # NEW — stale-Running cutoff
```

All existing top-level / repo / local / per-job override semantics apply, except
`max_total_size` and `keep_last` which are global only (per-job overrides for
these would be incoherent).

Parser change: `src/hooks/yaml_config.rs` `LogConfig` gains four new optional
fields. `src/hooks/yaml_config_loader.rs` `merge_log_configs` extends the merge
order.

Parsing helpers: `parse_size("10MB")` returns `u64`; `parse_duration("24h")`
already exists in the codebase (find and reuse).

## Files to add / modify

```
src/log_clean.rs                       NEW   ~250 LOC
src/lib.rs                             MOD   pub mod log_clean
src/main.rs                            MOD   wire maybe_clean_logs(); add __clean-logs hidden subcommand
src/coordinator/log_store.rs           MOD   extend clean() to take a CleanPolicy struct; add size + stale-Running logic
src/commands/hooks/jobs.rs             MOD   --dry-run, --older-than flags; Size column; build_jobs_payload size_bytes
src/hooks/yaml_config.rs               MOD   LogConfig adds 4 fields
src/hooks/yaml_config_loader.rs        MOD   merge new fields
src/executor/mod.rs                    MOD   plumb new fields onto JobSpec.log_config
docs/cli/daft-hooks-jobs.md            MOD   new flags, Size column, schema update
docs/guide/hooks.md                    MOD   new config knobs, behavior section
SKILL.md                               MOD   update hooks jobs row
tests/manual/scenarios/hooks/          NEW   ~6-8 scenarios (see below)
```

Estimated LOC: 700-900 including tests.

## Test scenarios

Manual YAML scenarios under `tests/manual/scenarios/hooks/`:

1. **`log-cleanup-respects-retention.yml`** — set retention to 1d, fast-forward
   filesystem mtime, run cleanup, assert old invocation removed.
2. **`log-cleanup-honors-keep-last.yml`** — create 5 invocations all older than
   retention; cleanup must keep most recent 3.
3. **`log-cleanup-per-file-cap.yml`** — produce a 20MB log, cleanup truncates to
   10MB with footer; meta.json shows `log_truncated: true`.
4. **`log-cleanup-budget-evicts-oldest.yml`** — create 600MB worth of logs under
   a 500MB budget; oldest invocations evicted until under budget; sanity floor
   still respected.
5. **`log-cleanup-skips-running.yml`** — start a long-running bg job, run
   cleanup with retention=0; running job survives.
6. **`log-cleanup-stale-running-detected.yml`** — write meta.json with
   status=Running, started_at 48h ago, no live socket; cleanup treats as stale
   and removes.
7. **`log-cleanup-dry-run.yml`** — `clean --dry-run` lists what would be removed
   and removes nothing.
8. **`log-cleanup-custom-path-untouched.yml`** — config has
   `log.path: /tmp/custom`; cleanup never touches that path.

Plus unit tests in `src/log_clean.rs` for size parsing, single-flight lock,
retention resolution chain, and the candidate-set computation.

## Compat / migration

- **Existing log dirs.** No migration needed — the meta format is unchanged
  except for the new optional fields.
- **Behavior change.** Users who currently rely on "logs accumulate forever"
  will see automatic cleanup after upgrade. CHANGELOG note required:
  `feat(hooks): automatic log cleanup respects log.retention; default 7d`.
- **Opt-out.** `DAFT_NO_LOG_CLEAN=1` for users who want fully manual control.

## Out of scope (in v1)

- Compression of cold logs.
- Search / grep over log content.
- Per-hook-type retention overrides.
- Pinning / TTL extension on access.
- Failed-run retention bias.
- Cleanup audit history beyond the single-line `last_summary`.
- Disk quota stderr warnings.
- Anything below in the v2 catalog.

---

# v2 — deferred catalog

Captured here so the design context isn't lost. Each entry is a sketch; full
spec to follow when work begins. Ranked by user-value-per-LOC, not
implementation order.

## Tier 1 — high value, low cost

### Failed-run retention bias

Keep failed and cancelled invocations for `2 × retention`. Failed runs are the
highest-signal logs (debugging value) and usually rarer than successes.

**Sketch:** in the candidate-set computation, multiply effective age by 0.5 when
any job in the invocation has `Failed` or `Cancelled` status. Subject to budget
eviction same as everything else, just deprioritized.

**Cost:** ~30 LOC. Lives entirely in `LogStore::clean()`.

### Pinning

`daft hooks jobs pin <invocation>` marks an invocation immune to automatic
cleanup. Companion `unpin <invocation>`. Pinned invocations also bypass the
budget pass (pinning is the user saying "I will manage this one myself").

**Sketch:** add `pinned: bool` to `InvocationMeta` (default false). Two new
subcommands. Cleanup skips pinned invocations entirely. Listing shows a `📌`
glyph or similar in the invocation header (or `[pinned]` in non-color mode).

**Cost:** ~80 LOC + a few tests.

### Cleanup audit history

After each background cleanup, append a one-line entry to a small
`log-clean-history.json` (last 50 runs, ring buffer). Surface as
`daft hooks jobs cleanup-history` and a one-liner in the listing footer.

The v1 `last_summary` field is the seed; v2 promotes it to a ring buffer and
exposes it explicitly. Defangs "retention surprised me" complaints.

**Sketch:** `Vec<CleanupSummary>` in the cache file, push-and-truncate on each
run. New `cleanup-history` subcommand prints it. ~120 LOC.

**Cost:** ~120 LOC.

### TTL extension on access

When `daft hooks jobs logs <id>` opens a log, bump that invocation's
`retention_until` by `now + retention_period`. The user is actively
investigating; cleanup shouldn't yank the file mid-debug.

**Sketch:** add `retention_until: Option<DateTime>` to `InvocationMeta`,
overriding the default `created_at + retention` when set. Touched on log open.
~50 LOC.

**Cost:** ~50 LOC.

## Tier 2 — high value, moderate cost

### Compression of cold logs

Gzip `output.log` for any job in terminal status older than 1 day.
`daft hooks jobs logs` decompresses on read transparently. Largest space win for
users with long-lived state dirs (text logs compress 10-20×).

**Sketch:** new pass in `run_clean_logs()` before retention sweep: `output.log`
→ `output.log.gz`. Reader code in `render_single_job_log` and
`render_invocation_logs` checks for `.gz` and pipes through `flate2`.
`size_bytes` column shows compressed bytes; tooltip / `--full-size` shows
uncompressed.

**Cost:** ~150 LOC + flate2 dep (already optional in some daft features —
verify).

### `daft hooks jobs grep <pattern>`

Full-text search across all log files for a repo (or `--all`). Returns hits as
`worktree:invocation:job:line — content`. Bigger value than it sounds: today the
only way to find "which run had the error about port 5432" is to open logs one
by one.

**Sketch:** new subcommand. Iterate job dirs (or stream from `InvocationMeta`),
`BufReader` over each `output.log` (decompressing if gzipped), regex match
line-by-line, format hits. Optional `--context N` flag for surrounding lines
(mimic `grep -C`). Can use `regex` and `grep-searcher` crates from the Rust
ecosystem; `grep-searcher` handles the streaming and is already battle-tested by
ripgrep.

**Cost:** ~200 LOC.

### Job stats / `daft hooks jobs stats <hook>`

Aggregate over historical invocations: count, success rate, p50/p95 duration,
last-failed timestamp, "flaky" flag (jobs that intermittently fail). Reveals
slow regressions, and "is this hook actually doing anything" questions.

**Sketch:** new subcommand. Compute per-hook-type and per-job-name aggregates
from existing `meta.json` files. Output shape: a table with one row per
(hook_type, job_name). Bonus: tiny inline sparkline of recent durations using
`unicode-block-elements`. Mostly a presentation of data already on disk.

**Cost:** ~250 LOC.

### Cross-invocation diff

`daft hooks jobs diff <inv1> <inv2> [--job <name>]` shows the unified diff
between two invocations of the same job. Powerful for "what changed in our setup
output between yesterday and today" investigations.

**Sketch:** new subcommand. `similar` crate (already a transitive dep) for the
diff; format with `daft`'s standard color scheme. Without `--job`, diffs all
common jobs side-by-side.

**Cost:** ~100 LOC.

## Tier 3 — power-user controls

### Verbose `clean -v` mode

Per-removal lines so users can audit decisions when retention surprises them.
Cheap once v1 ships; folded in only because the existing flag plumbing is
already there.

**Cost:** ~40 LOC.

### Disk quota warning

When per-repo usage approaches 80% of `max_total_size`, the next `daft`
invocation prints a one-line stderr warning. Avoids surprise eviction.

**Sketch:** in `maybe_clean_logs()`, if usage >= 0.8 × budget, write a sentinel;
main path reads sentinel and prints once.

**Cost:** ~50 LOC.

### "Important" marker from within hook scripts

A hook script can run `daft hooks mark-important` (or
`echo 1 > $DAFT_JOB_IMPORTANT`) to flag its own log for extended retention. Lets
the hook author decide what's worth keeping.

**Sketch:** new env var passed to hook child processes pointing at a sentinel
path; presence of the sentinel before exit promotes the invocation's effective
retention by 4×. ~80 LOC.

**Cost:** ~80 LOC.

### Per-hook-type retention

A `worktree-pre-remove` log might only matter for an hour, while a `post-clone`
log that ran a 30-minute setup may be worth keeping for a month. Resolution
chain: built-in → global → repo → hook-type → per-job.

**Sketch:** insert a new layer in the merge order. ~40 LOC.

**Cost:** ~40 LOC.

### Per-repo retention overrides (cap-raising)

A repo with strict compliance ("we keep all build logs for 90 days") pins its
retention floor in `daft.yml` so it ignores lower global defaults. Inverse of
the merge order: repo can raise but not lower.

**Sketch:** new `log.retention_floor` field, takes precedence as a lower bound.
~60 LOC + careful test coverage.

**Cost:** ~60 LOC.

### Configurable cleanup schedule

Override the 24h throttle. `daft.yml`'s `log.cleanup_interval: 6h` for
high-churn repos. Probably only matters for CI agents firing hundreds of
hooks/day.

**Sketch:** read from config; pass to `is_cache_stale()`. ~20 LOC.

**Cost:** ~20 LOC.

## Tier 4 — content awareness

### Content-aware retention floor

Sniff log content for `FATAL`, `panic:`, segfault traces, non-zero exits. Any
match triggers the failed-run bias, even if `meta.json` says succeeded. Catches
hooks that exit 0 but emit errors anyway.

**Sketch:** scan first/last 4KB of each `output.log` against a small regex set
when computing eviction candidates. Cheap (4KB read per file). ~80 LOC.

**Cost:** ~80 LOC.

## Tier 5 — operational / monitoring

### Export-before-evict

When a cleanup is about to delete data, optionally archive to
`~/.local/state/daft/jobs-archive/<date>.tar.gz` first. User-visible knob:
`log.archive_before_delete: true`. For users who want forensic recovery ability
without paying ongoing storage cost.

**Sketch:** pre-deletion pass writes a tarball. Tarballs themselves get their
own retention. ~150 LOC.

**Cost:** ~150 LOC.

### External-storage sync

`log.archive_to: s3://my-bucket/daft-logs/` and the cleanup job uploads
invocations being evicted instead of just deleting. Very nice-to-have, big
complexity, depends on a separate "daft cloud" story.

**Sketch:** plug-point in cleanup; per-target uploader. Each backend (S3, GCS,
Azure, generic HTTP) is its own ~200 LOC.

**Cost:** ~400-600 LOC depending on backends.

### Cleanup webhook

Configurable POST to a URL when cleanup runs (cleanup metadata as JSON body).
Lets organizations track aggregate hook-log lifecycle.

**Sketch:** `log.cleanup_webhook: https://...` config; HTTP POST after cleanup
completes. ~80 LOC.

**Cost:** ~80 LOC.

## Tier 6 — UX polish

### Interactive prune TUI

`daft hooks jobs clean --interactive` opens a `ratatui` view where the user can
browse invocations, mark for deletion, see size impact in real time. Same stack
the rest of daft uses.

**Sketch:** new TUI module under `src/output/tui/`. List of invocations,
checkbox per row, totals at bottom. Esc cancels, Enter commits. ~400 LOC.

**Cost:** ~400 LOC.

### Job-stats sparkline in `daft hooks jobs`

Tiny inline sparkline showing recent-runs duration trend per hook type. Cute,
surprisingly useful for noticing regressions. Stretch goal — only worth doing
once stats infrastructure exists.

**Sketch:** appends to invocation header row. ~50 LOC after stats land.

**Cost:** ~50 LOC.

---

## Recommended v2 prioritization

If the v2 work happens, ship the **Tier 1** items together as a single follow-up
release: failed-run bias, pinning, cleanup audit history, TTL extension on
access. These compound (audit history makes the others discoverable; pinning is
opt-in; TTL extension fixes a UX paper cut). All four together are ~280 LOC.

Tier 2 items (compression, grep, stats, diff) are individually larger but each
independently shippable. Pick based on user demand — the tracking issue should
request user upvotes.

Tiers 3–6 are "ship when there's a real complaint that justifies it" rather than
"ship proactively." Filed mainly so the design doesn't get re-derived three
months later.

## Out of scope (deliberately, in both v1 and v2)

- **Per-job log streaming to remote services in real time.** Logs are ephemeral
  working state; if you need real-time observability, your hook should write to
  your existing observability stack directly.
- **Encrypted log storage.** Hook output is the user's own data on their own
  machine; OS-level encryption (FileVault, LUKS) is the right layer.
- **Rotating logs of currently-running jobs.** Run-time rotation invites
  corruption and complicates the writer; the per-file cap is enforced only at
  cleanup time on terminal-status jobs.
- **Cross-repo log search / unified inbox.** `--all` exists for the listing;
  search across repos is a different product.
