---
branch: daft-665/list-live-hidden-size-walk
---

# List Live Field Gating

Fix for #665: the live `daft list` path requested `FieldSet::ALL`, so hidden
SIZE/MTIME collection (a full recursive walk of every worktree) kept the process
alive for seconds after the table was fully rendered.

## Exit latency

- [x] `daft list` (TTY, default columns) exits with no post-render tail — wall
      time ≈ the `DAFT_NO_LIVE=1` baseline (0.23–0.27 s vs 0.36 s in the
      220k-file fixture)
- [x] Pre-fix vs fixed binary in a 220k-file fixture: 1.8–2.0 s → 0.23 s
- [x] Real repo (6 worktrees, ~45 GB of build artifacts): 3.5 s cold / 1.1 s
      warm → ~0.3 s
- [ ] Interactive terminal: shell prompt timer no longer reports multi-second
      `daft list` in a large project

## Opt-in expensive fields still work

- [x] `--columns +size` walks again (1.7 s in the fixture) and renders the Size
      column plus the TOTAL summary footer
- [x] `--sort=-size` collects sizes even without the Size column (regression
      unit test + fixture run)
- [x] `--sort=-activity` collects mtime and renders "Sorted by Activity"
- [x] YAML list scenarios: 53 scenarios / 167 steps pass
- [ ] Interactive glance: `daft list --stat lines` live view fills Base /
      Changes / Remote with line counts (field request covered by unit test;
      blocking path covered by YAML scenarios)

## Measurement note

Timings were taken through a PTY harness that answers the terminal's
cursor-position query (`ESC[6n`). Bare `script(1)` with no terminal behind it
stalls crossterm's inline-viewport bring-up for its ~2 s DSR timeout, which
masks the walk entirely — worth remembering for any future live-path timing.
