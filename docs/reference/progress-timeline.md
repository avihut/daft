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
✓  Created branch
✓  Checked out branch
✓  Created worktree   ../daft-652/cool-feature
✓  Pushed             → origin/daft-652/cool-feature  (1.8s)
│  shared files
✓  .env
✓  .claude/settings.json
│
├──────────────────────────────────────────────────────────────────┐
│ daft hooks v1.18.0  worktree-post-create  on: daft-652/cool-feature │
└──────────────────────────────────────────────────────────────────┘
┃  bun-install ❯
┃  installed 214 packages
│
└  Ready in 6.3s
```

## Reading the rail

| Glyph | Meaning                                                        |
| ----- | -------------------------------------------------------------- |
| `○`   | Pending (dim), or an expected skip with its reason             |
| `⠹`   | The step currently running (spinner)                           |
| `✓`   | Done — past-tense label, dim duration when the step took ≥ 1s  |
| `✗`   | Failed — the label stays imperative (the fact never happened)  |
| `↓`   | Skipped for an attention-worthy reason (e.g. repo not trusted) |

- The header names the resolved intent (`Starting <branch> ← <base>`); the
  footer closes the rail with the outcome and total duration.
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
- [Shared files](../cli/daft-shared.md) get their own section under a dim
  `shared files` anchor: one receipt row per declared path stating its state.
  `✓` means the symlink landed; `○ already linked` and `○ materialized` are the
  quiet no-ops; a path never collected into shared storage renders the yellow
  `↓ … missing from shared storage` row with the `daft shared sync` remedy, and
  a real file in the way gets the `daft shared link` remedy. The section never
  silently ignores a declaration it could not honor.
- `daft remove` lists steps in true execution order — the remote branch is
  deleted first (it is the hardest to recreate), then the worktree, then the
  local branch. Multi-branch removals group rows under dim branch anchors.
- Lifecycle hooks appear as a plan row; when they actually run, the row expands
  in place into the familiar hook block. The rail welds into the banner's top
  corner (`├────┐`) while the banner closes below (`└────┘`), leaving the job
  lines hanging beneath it. When nothing is configured to run, the row
  disappears; skips worth noticing (untrusted repository, `--skip-hooks`) render
  the yellow `↓` row instead.
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
