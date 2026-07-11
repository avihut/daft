---
title: Progress Timeline
description:
  The plan-then-execute timeline daft renders for worktree create and remove
  commands
---

# Progress Timeline

On an interactive terminal, the worktree lifecycle commands — `daft go`,
`daft start`, `daft remove` (and their `git-worktree-*` forms), and `daft clone`
— narrate their work as a plan-then-execute timeline: the full ordered list of
steps renders up front, each step fills in place as daft works, and the finished
rail persists in your scrollback as a receipt.

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
in the pending plan, while running, and in the finished receipt.

- The header names the resolved intent (`Starting <branch> ← <base>`); the
  footer closes the rail with the outcome and total duration.
- With `daft.checkout.fetch` on, the remote fetch is planned work: `daft start`
  opens its rail with the `Fetch remote` and `Set up tracking` rows instead of
  running them as a spinner before it. A failed fetch turns its row yellow
  (`↓ Fetch remote  failed — continuing with local refs`) and the command
  proceeds on local refs. The header names the requested base; when the fetch
  reveals a fresher remote ref, the `Created branch` row carries the resolved
  provenance (`← origin/main`).
- The rail lists only work that happens. A step known to be off at planning time
  (push with `daft.checkout.push` off or `--local`) plans no row, and a step
  that resolves as a no-op (carry with a clean tree) removes its row — the
  finished rail is a receipt of what daft actually did. Attention-worthy skips
  are the exception and stay visible.
- Remote indicators appear only while remote interaction is in scope:
  `← origin/x` (created from the remote), `→ origin/x` (pushed),
  `tracking origin/x`, or remove's dim `no remote branch` note when remote
  deletion is on but the branch has no upstream. When configuration takes
  remotes out of scope — `daft config remote-sync` set to local only,
  `daft.branchDelete.remote` off (the default), or `--local` — the rail never
  mentions them, exactly as an unconfigured push plans no row.
- [Shared files](../cli/daft-shared.md) get their own section under a
  `├─ shared files` anchor: one receipt row per declared path stating its state.
  `✓` means the symlink landed; `○ already linked` and `○ materialized` are the
  quiet no-ops; a path never collected into shared storage renders the yellow
  `↓ … missing from shared storage` row with the `daft shared sync` remedy, and
  a real file in the way gets the `daft shared link` remedy. The section never
  silently ignores a declaration it could not honor.
- `daft remove` lists steps in true execution order — the remote branch is
  deleted first (it is the hardest to recreate), then the worktree, then the
  local branch. Multi-branch removals group rows under `├─` branch anchors.
- Lifecycle hooks appear as a plan row; when they actually run, the row becomes
  a `├─ post-create hooks` section with one receipt row per job. While a job
  runs, its latest output line rides the spinner as a dim annotation — one line
  of liveness per job. A finished job resolves green with the usual dim
  duration; a failed one turns its row red and its captured output prints below
  the rail footer. Jobs excluded with `--skip-hooks` (and jobs skipped because a
  dependency failed) render yellow `↓` rows; jobs skipped by their own
  `skip:`/`only:` conditions leave no trace, and a whole phase skipped that way
  vanishes with them. Background jobs get a blue `↻ name  background` receipt —
  `daft hooks jobs` manages them from there.
- Pass `-v` — or set `daft.hooks.output.verbose` — for the full hook block:
  version banner, rolling output tails, every job's complete output, and the
  summary, welded into the rail (`├────┐`) exactly as the block renders
  standalone. `daft.hooks.output.timerDelay` and `tailLines` only apply to this
  block. When nothing is configured to run, the hook row disappears; skips worth
  noticing (untrusted repository, `--skip-hooks all`) render the yellow `↓` row
  instead.
- If a step fails, later steps persist as dim `(not run)` rows and the footer
  reports `Failed after <t>` — the receipt shows exactly how far the command
  got.

## When the timeline does not render

The timeline is an interactive-terminal presentation. In every other mode daft
prints exactly the output it printed before the timeline existed:

- **Non-interactive stderr** (pipes, CI logs) — plain result lines.
- **`NO_COLOR`, `TERM=dumb`** — plain result lines (the live region requires
  color support; this matches the previous spinner's behavior).
- **`--quiet`** — warnings and errors only.
- **Navigation early-exits** — `daft go` to an existing worktree and `daft go -`
  remain single-line responses; there is no plan to show.

`daft prune`, `daft repo remove`, and multi-branch `daft clone`'s satellite
phase keep their inline operation table, which already shows all rows up front
and fills them in as work completes.

Pressing `Ctrl-C` mid-run collapses the live remainder of the rail and exits
with status 130; everything already completed stays in your scrollback.
