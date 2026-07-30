---
title: Progress Timeline
description:
  The plan-then-execute timeline daft renders for worktree create and remove
  commands
---

# Progress Timeline

On an interactive terminal, the worktree lifecycle commands — `daft go`,
`daft start`, `daft remove` (and their `git-worktree-*` forms), `daft clone`,
and multi-worktree `daft exec` — narrate their work as a plan-then-execute
timeline: the full ordered list of steps renders up front, each step fills in
place as daft works, and the finished rail persists in your scrollback as a
receipt.

```
┌  Starting daft-652/cool-feature ← main
│
✓  Fetched remote     origin  (1.1s)
✓  Set up tracking
✓  Created branch     ← origin/main
✓  Checked out branch
✓  Created worktree   ../daft-652/cool-feature
✓  Pushed             → origin/daft-652/cool-feature  (1.8s)
│
├─ copied paths from 'main'
│  ✓  target/         12 paths · 1.4 GB · reflinked · 0.4s
│  ○  node_modules/   nothing to copy yet
│
├─ shared files
│  ✓  .env
│  ✓  .claude/settings.json
│
├─ post-create hooks
│  ✓  prepare-db    (2.1s)
│  ✓  bun-install   (2.9s)
│  ↻  check-todos   background
│
└  Ready in 6.3s
```

## Reading the rail

| Glyph | Meaning                                                                      |
| ----- | ---------------------------------------------------------------------------- |
| `○`   | Pending (dim), or an expected skip with its reason                           |
| `⠹`   | The step currently running (spinner)                                         |
| `✓`   | Done — past-tense label, dim duration when the step took ≥ 1s                |
| `✗`   | Failed — the label stays imperative (the fact never happened)                |
| `↓`   | Skipped for an attention-worthy reason (e.g. repo not trusted)               |
| `├─`  | A section anchor (shared files, hook phases, multi-branch remove's branches) |
| `↻`   | A hook job handed to the background coordinator                              |

Rows belonging to a section render tucked inside the rail (`│  ✓  .env`), so the
rail stays a continuous wire and each `├─` anchor visibly carries its children —
in the pending plan, while running, and in the finished receipt. A hook phase
that will open as a section already owns its blank rail lines in the committed
plan, so the plan carries the receipt's rhythm and no row shifts when the
section starts to fill.

Color follows one grammar. State lives in the glyph (green done, bold-red
failed, yellow attention, cyan spinner) and daft's own vocabulary stays plain,
with section headings bold. Subjects wear identity inks that never change with
state — so the committed plan is as readable as the receipt: remote names and
refs (`origin`, `← origin/master`, `→ origin/x`) are cyan, worktree paths are
manila, shared files are violet, and background work is blue. The exceptions are
deliberate: hook job names take their outcome's color (the scheme the standalone
hook renderer's summary also speaks), failure details and skip reasons always
render plain, and a dimmed row — pending glyphs, expected skips, `(not run)` —
never keeps an identity ink.

- The rail opens the moment the command starts (after any pre-flight prompts) as
  a single collapsed line — the active spinner, the header's own text, and the
  ticking stopwatch on its tail. That line is the whole rail until a plan gives
  it a body: the `┌ │ └` frame appears only when the command has resolved work
  worth framing, expanding the line in place with the committed plan beneath it.
  The text follows the resolve phase — `daft clone` runs its whole network clone
  under the collapsed line, flips to `Resolving branches`, and commits a plan
  led by the already-done `✓ Cloned repository` row; `daft go` flips to
  `Fetching origin` while a `daft.checkout.fetch` round-trip runs. A prompt that
  must own the terminal mid-resolve (the first-clone layout prompt) makes the
  line step aside tracelessly and return, on the same phase, once answered. A
  run that resolves into a navigation early-exit, a hop to another cataloged
  repo, or a resolve-phase error collapses the line without a trace and keeps
  its single-line response.
- The header names the resolved intent (`Starting <branch> ← <base>`); the
  footer closes the rail with the outcome and total duration. While the command
  runs, the pending footer is a stopwatch — a dim elapsed counter (`└ 1.2s`)
  ticking from the moment the rail opens until the outcome replaces it.
- With `daft.checkout.fetch` on, `daft go` runs the fetch under the collapsed
  line and commits its plan only once the branch is known to exist — leading
  with the already-done `✓ Fetched remote` row, the branch's provenance
  (`← origin/x`, `tracking origin/x`, `local only`) already resolved onto the
  `Check out branch` row. A name the fetch fails to reveal is never this rail's
  work: the line dissolves tracelessly, and whatever the name turns out to be —
  another cataloged repo, a tag or commit that opens a sandbox, a branch
  `--start` (or `daft.go.autoStart`) creates, or a plain error — owns the run's
  only output. A failed fetch warns, turns its row yellow
  (`↓ Fetch remote  failed — continuing with local refs`), and the command
  proceeds on the refs it has. With the fetch off, the branch probe precedes the
  plan and an unknown branch keeps the plain error. `daft start` and forge
  targets (`daft go pr:123`) plan the fetch as work committed before the
  round-trip instead — `daft start` opens its rail with the `Fetch remote` and
  `Set up tracking` rows, and a forge miss falls back to the PR/MR head ref
  rather than failing to find a branch. For `daft start` the header names the
  requested base; when the fetch reveals a fresher remote ref, the
  `Created branch` row carries the resolved provenance (`← origin/main`).
- The rail lists only work that happens. A step known to be off at planning time
  (push with `daft.checkout.push` off or `--local`) plans no row, and a step
  that resolves as a no-op (carry with a clean tree) removes its row — the
  finished rail is a receipt of what daft actually did. Attention-worthy skips
  are the exception and stay visible.
- Remote indicators appear only while remote interaction is in scope:
  `← origin/x` (created from the remote), `→ origin/x` (pushed),
  `tracking origin/x`, or remove's dim `no remote branch` note when remote
  deletion is on but the branch has no upstream. When configuration takes
  remotes out of scope — the `remote-sync` behavior set to `off`,
  `daft.branchDelete.remote` off (the default), or `--local` — the rail never
  mentions them, exactly as an unconfigured push plans no row.
- [Shared files](../cli/daft-shared.md) get their own section under a
  `├─ shared files` anchor, after the copied paths — daft-managed links go on
  top of whatever bulk content the copy brought: one receipt row per declared
  path stating its state. `✓` means the symlink landed; `○ already linked` and
  `○ materialized` are the quiet no-ops; a path never collected into shared
  storage renders the yellow `↓ … missing from shared storage` row with the
  `daft shared sync` remedy, and a real file in the way gets the
  `daft shared link` remedy. The section never silently ignores a declaration it
  could not honor.
- [Copied paths](/worktrees/copying-caches) get the same treatment under a
  `├─ copied paths from 'master'` anchor, immediately **before** the
  shared-files section. The anchor names the worktree the caches came from,
  because that is a ranked decision rather than something the command line shows
  — the base branch's worktree when it has one, otherwise any worktree at the
  identical commit (`· same commit`, plus `, where you are` or `, warmest` when
  a tie had to be broken), otherwise where you ran the command
  (`· the same-commit worktree is empty` when the ladder passed over a match
  that carried none of the declared caches). Below it, one row per declared
  **entry**, never per expanded glob match, so a `**/dist/` declaration stays
  one row and reports its fan-out in the annotation
  (`3 paths · 1.2 GB · reflinked · 0.3s`; `part reflinked` when only some
  matches cloned, `· 2 unreadable` when the expansion could not read
  everywhere). Exactly three skips are dim, because they are the stage working
  as designed: `nothing to copy yet`, `already present`, `matched nothing`.
  Every other outcome is yellow — a declaration daft refused
  (`must be gitignored — tracked content is never copied`), could not size
  (`2.1 GB — over the 1 GB max_size`), or could not carry out (`failed — …`).
  The row's label is the entry, so its phrase never repeats it; when a glob
  expands, the phrase names the one match that offended. No copy row is ever
  red: the stage is an optimization, and a cache that did not copy has not cost
  the user the worktree they asked for.
- `daft remove` lists steps in true execution order — the remote branch is
  deleted first (it is the hardest to recreate), then the worktree, then the
  local branch. Multi-branch removals group rows under `├─` branch anchors. Its
  hook rows are planned only when the phase has hooks discoverable at plan time:
  a repository configuring no `worktree-post-remove` hooks plans no
  `post-remove hooks` row at all.
- Lifecycle hooks appear as a plan row framed by its section's rail gaps; when
  they actually run, the row becomes a `├─ post-create hooks` section in place,
  with one receipt row per job. While a job runs, its latest output line rides
  the spinner as a dim annotation — one line of liveness per job. A finished job
  resolves green with the usual dim duration; a failed one turns its row red and
  its captured output prints below the rail footer. Jobs excluded with
  `--skip-hooks` (and jobs skipped because a dependency failed) render yellow
  `↓` rows; jobs skipped by their own `skip:`/`only:` conditions leave no trace,
  and a whole phase skipped that way vanishes with them. Background jobs get a
  blue `↻ name  background` receipt — `daft hooks jobs` manages them from there.
- `-x`/`--exec` commands are planned where they run: last, after the post-create
  hooks. One row per `-x` occurrence, labelled with the command exactly as you
  typed it, under an `├─ exec` anchor once there are two or more — so two
  identical commands stay two rows. A command owns the terminal for its whole
  run (`-x` inherits stdio and may be interactive), so its own output prints
  above the rail and its row then resolves green, or red with the exit status
  (`✗ npm ci  exit 1`). A failure stops the sequence, and the commands that
  never got their turn say so (`↓ cargo test  not run`). The worktree exists and
  your shell still moves into it, so the footer reads
  `Ready with failures in 2.4s` rather than claiming the creation failed.
  Navigating to an **existing** worktree commits no plan and so has no rail: the
  commands still run, with the single-line output that path has always had.
- The `pre-push` gate is git's hook, not daft's — git dispatches it inside
  `git push` and daft sees one output stream. When that stream is a **hook
  manager** (lefthook 2.x — every released 2.x version is covered), daft
  recognizes its line grammar and renders the manager's jobs as first-class rows
  under the `├─ pre-push hooks` anchor, and folds the manager's identity into
  that header (`├─ pre-push hooks  lefthook v2.1.10`) — a persisted line that
  stays on screen while the run works and in scrollback after. daft reads the
  manager's config for the job list, so every job — including one that runs for
  minutes — is a live spinner with a running elapsed timer from the moment the
  manager engages, not only once its output finally arrives. (The roster is read
  up front, so a manager configured to run serially still shows all its jobs
  from engagement: a not-yet-started job spins with a timer counting from the
  phase start, not its own — a pipe gives no per-job start signal to key it to.)
  The instant a job's output block flushes (in default piped mode that is the
  job's completion), its row stops spinning and settles to a neutral grey `✓` —
  finished running, verdict pending — so a job that finished early no longer
  looks busy until the whole run ends. Managers report per-job pass/fail and the
  official duration only in their end-of-run summary, so each grey `✓` resolves
  there to its confirmed receipt: green `✓` with the duration, or red `✗` with
  the failure dump scoped to that job
  (`error: hook job 'unit tests (related)' failed:`) instead of the whole run.
  The confirmed verdicts land together at the summary because that is the only
  moment the manager reveals them. In verbose mode the closing note carries the
  phase total (`└ all jobs in 42.4s`); the manager's identity already lives in
  the header, so the note no longer repeats it. Output daft does not recognize —
  a plain script, husky, an unknown tool — passes through untouched as the
  single `pre-push` job, exactly as before; jobs the manager never resolved
  (killed mid-run) settle with the push verdict so no row is left spinning, and
  the push verdict itself always comes from git's own result, never from
  recognized output. `daft.hooks.output.parseManagers=false` turns recognition
  off entirely. `daft sync --push` reports the same structure in its table: the
  manager's jobs as `-v` sub-rows, the hook line annotated with the manager's
  identity, and the post-run `Hooks:` report naming the failing job
  (`pre-push · unit tests (related) failed`) with only that job's output.
- Pass `-v` — or set `daft.hooks.output.verbose` — to thread each job's log
  under its row. The section anchor gains the hook key and engine version
  (`├─ post-create hooks  worktree-post-create · daft v1.18.1`), and each job's
  output hangs from its glyph column on an inner thread:

  ```
  │  ✓  prepare-db   (2.1s)
  │  │    ❯ ./scripts/prepare-db.sh
  │  │    applying migration 0
  │  │    applying migration 1
  │  │
  │  ✓  bun-install  (2.9s)
  │  │    ❯ bun install
  │  │    resolving package cluster 11
  ```

  The thread opens with the dim `❯ <command>` provenance line, shows a rolling
  window of `daft.hooks.output.tailLines` lines while the job runs, and the
  receipt keeps every line — grey under a job that succeeded, default ink under
  one that failed (evidence stays loud), `(no output)` when it printed nothing.
  Each thread closes with an empty thread line (`│  │`), so consecutive blocks
  keep their own air — live and in the receipt — while the rail's lone `│` stays
  a section boundary. The section closes with its own rail end — a dim
  `└ all jobs in <t>` total — and a job still silent after
  `daft.hooks.output.timerDelay` seconds shows a dim elapsed counter until its
  first output. A failed job's exit status still prints after the footer
  (`error: hook job '<name>' failed (exit code: N)`) — but not its output, which
  already sits inline. When nothing is configured to run, the hook row
  disappears — and `daft remove` goes further: its hook config sources are on
  disk and exact before the plan commits, so the row is never planned. Skips
  worth noticing (untrusted repository, `--skip-hooks all`) render the yellow
  `↓` row instead.

- If a step fails, later steps persist as dim `(not run)` rows and the footer
  reports `Failed after <t>` — the receipt shows exactly how far the command
  got.

## Running commands across worktrees

Multi-worktree `daft exec` renders on the same rail, with one row per targeted
worktree (or, for a `-x` pipeline of several commands, a `├─` group per worktree
with one row per command):

```
┌  Running mise clean in 4 worktrees
│
✓  master                                         (3.2s)
✓  daft-335/feat/visitor-config                   (4.5s)
✗  daft-518/feat/test-runner-output-improvements  exit 1
│    [clean:tests] rm -rf target/tmp
│    error: Permission denied (os error 13)
│
✓  daft-529/exec-show-output                      (12.1s)
│
└  Finished with failures in 12.4s
```

- Workers run concurrently, but the receipt persists in **plan order**: a row
  that finishes early shows its outcome in place immediately and waits, in the
  scrollback, for the rows ahead of it. The header names the resolved scope
  (`in N worktrees`, `in N repos` for `--all-repos`, `in N related worktrees`
  for `--related`); the footer reports `Done in t`,
  `Finished with failures in t` (all ran, some failed), `Failed after t` (a
  `--sequential` run stopped early), or `Cancelled after t`.
- While a worker runs, its latest output line rides its row, dim. A **failed or
  cancelled** worker always threads its full captured output under its row; a
  successful worker stays a compact row. A row cancelled by `Ctrl-C` shows the
  yellow `⊘` face; the `↓` face marks a matched branch with no worktree.
- Pass `-v` (`--verbose`) to thread **every** worker's output — grey under a
  success, a rolling window while it runs — using the same
  `daft.hooks.output.tailLines` window the hook rail uses. A worker's full
  output reaches scrollback only once the rows ahead of it in the plan drain;
  its `✓`/`✗`/`⊘` outcome is never delayed. Nothing prints below the footer. You
  can also press `v` mid-run to switch either way — see [Live keys](#live-keys).
- A **single explicit-target** run (`daft exec feat/auth -- claude`, or a bare
  `--repo`) inherits stdio directly and renders no rail, so interactive programs
  work unchanged. A fan-out — `--all`, a glob, `--all-repos`, `--related`, or
  several positionals — renders the rail even when it resolves to a single live
  worktree (any orphan branches ride along as `↓` rows), rather than collapsing
  to pass-through. When stdout is redirected, `daft exec` still writes its
  captured-output dump there (failures only, or every worker with `-v`) while
  the rail narrates on stderr.

## When the timeline does not render

The timeline is an interactive-terminal presentation. In every other mode daft
prints exactly the output it printed before the timeline existed:

- **Non-interactive stderr** (pipes, CI logs) — plain result lines.
- **`NO_COLOR`, `TERM=dumb`** — plain result lines (the live region requires
  color support; this matches the previous spinner's behavior).
- **`--quiet`** — warnings and errors only.
- **Navigation early-exits** — `daft go` to an existing worktree and `daft go -`
  remain single-line responses; there is no plan to show (the just-opened
  planning face collapses without leaving a trace).
- **Single explicit-target `daft exec`** — inherits stdio directly (so
  interactive programs work); a fan-out or multi-target run on a non-interactive
  stderr prints the same summary rows and output dump it always did.

`daft prune`, `daft repo remove`, and multi-branch `daft clone`'s satellite
phase keep their inline operation table, which already shows all rows up front
and fills them in as work completes. In `daft sync --push`, that table also
surfaces the push resource governor: a push held back under memory pressure
shows a dim `held: memory` (or `held: capped` / `held: frozen` / `held: retry`)
instead of running immediately, and a post-run summary line reports the total
("2 pushes throttled 14s to preserve memory headroom").

## Live keys

While the rail is on screen, the stopwatch footer offers what you can press:

```
└  4.2s   v verbose · ^C cancel
```

**`v` toggles verbose output for the run in progress.** Start terse and press
`v` when a job starts looking interesting, or start with `-v` and press `v` to
quiet it back down — verbosity is a decision you make while watching, not one
you commit to before the run starts.

The toggle takes effect immediately for rows still running and for every receipt
printed from then on. Rows that already finished are a different matter: the
rail is append-only, so their receipts stay exactly where they printed. Turning
verbose on re-emits the logs of finished rows that printed compactly as a
fold-out block below, headed by a repeat of the receipt line:

```
✓  feat/auth
○  verbose on — replaying 1 finished row
✓  feat/auth
│    cargo test --lib
│    test result: ok. 214 passed
│
```

The `verbose on` note appears only when there are finished rows to fold out. A
flip with nothing to replay — and every `verbose off` — changes the live rows
alone and adds no line to scrollback, so repeated toggling never piles up notes;
the footer hint (`v verbose` / `v quiet`) is what always shows the current
setting. Each log folds out once, so toggling back and forth never repeats it.
Failed rows are not replayed — their output already threaded when they failed.
Turning verbose off collapses the live windows and leaves everything already
printed alone.

The hint and the toggle are terminal-only: with output redirected, in CI, or
under `--quiet` there is no live region, no key listener, and no change to what
daft prints. The toggle also does not change the captured-output dump
`daft exec` writes to a redirected stdout — that follows the `-v` flag you
passed, so a script's output does not depend on what you pressed. Only
`daft exec`'s rows replay; hook-job rows (worktree create/remove, `daft run`)
follow the new density from the next line they print onward.

Pressing `Ctrl-C` mid-run collapses the live remainder of the rail and exits
with status 130; everything already completed stays in your scrollback. A
`daft exec` run interrupts cooperatively instead: the first `Ctrl-C` stops the
running commands (SIGTERM), a second forces them (SIGKILL), and the rail closes
as a `Cancelled` receipt. This is unchanged by the key listener: `Ctrl-C`
reaches daft as a real signal, not as a keystroke.
