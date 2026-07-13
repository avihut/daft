---
branch: daft-533/feat/exec-rail-output
---

# Exec Rail Output

All checks are interactive-terminal checks — run them in a real TTY in a scratch
repository (`mktemp -d`, local git config only; never this repo) with two or
more worktrees. The plain-mode (non-TTY) behavior is covered by the
`tests/manual/scenarios/worktree-exec/` YAML scenarios; this plan covers the
live rail that only renders on a TTY.

## Rail basics (multi-target, default density)

- [ ] `daft exec --all -- <cmd>` with ≥2 worktrees: header
      `┌ Running <cmd> in N worktrees`, one row per worktree, footer
      `└ Done in <t>` — no legacy `──── N worktrees · M commands ────` divider,
      no compact rows promoted to the top
- [ ] Rows fill in plan order even though workers finish out of order (run a
      fast command in one worktree and a slow one in another; the fast row still
      waits, in scrollback, behind a plan-earlier slow row — its `✓` shows in
      place immediately)
- [ ] A running worker's latest output line rides its row, dim, updating in
      place; a successful worker resolves to a compact `✓ <label> (<t>)` row (no
      threaded output)
- [ ] A **failed** worker resolves `✗ <label>  exit N` and threads its captured
      output under the row (`│    <line>`, default ink, `│` closer); a
      **silent** failure stays a bare `✗ <label> exit N` with no `(no output)`
      line
- [ ] Single command, no worktree match for a glob (e.g. `daft exec 'feat/*'`
      where one match has no worktree): the orphan shows as a yellow
      `↓ <branch>  no worktree` row, not a pre-header warning

## Verbose (`-v`)

- [ ] `daft exec --all -v -- <cmd>`: every worker threads its full log — grey
      under a success, default ink under a failure, `(no output)` for a silent
      worker; a rolling window (default 6 lines, `daft.hooks.output.tailLines`)
      shows while it runs
- [ ] Nothing prints below the footer (no dump repeat) on an interactive
      terminal; the footer is the last line

## Pipelines (`-x` × 2+)

- [ ] `daft exec --all -x '<cmd1>' -x '<cmd2>'`: a `├─ <worktree>` group per
      worktree, one row per command; a failed command stops its worktree and the
      rest of that group persists as dim `○ … (not run)` while other worktrees
      keep running

## Scheduling + footers

- [ ] `--sequential` with a failing worktree: the run stops, later worktrees
      persist as `○ … (not run)`, footer `└ Failed after <t>`
- [ ] `--keep-going` with a failing worktree: all worktrees run, footer
      `└ Finished with failures in <t>`
- [ ] All succeed: footer `└ Done in <t>`; parallel run with one failure (all
      ran): `└ Finished with failures in <t>`

## Cancellation (two-stage Ctrl-C)

- [ ] Long-running `daft exec --all -- <slow>`: first `Ctrl-C` — running workers
      resolve `⊘ <label>  cancelled`, the rail keeps rendering, footer
      `└ Cancelled after <t>`; process does not exit on the first `Ctrl-C`
- [ ] Second `Ctrl-C` during a worker ignoring SIGTERM: the child is SIGKILL'd
      and the run tears down; exit code reflects the aggregate

## Single-target passthrough (no rail)

- [ ] `daft exec <one-worktree> -- claude` (or `vim`, `fzf`): stdio inherited,
      interactive program works, no rail, exit code propagated verbatim
- [ ] Bare `daft exec --repo <name> -- claude` (default-branch worktree, no
      positional): same passthrough — stdio inherited, no rail
- [ ] `Ctrl-C` in a single-target passthrough behaves like the child's own
      SIGINT (no rail collapse)
- [ ] **#533 [3] regression:** a fan-out that resolves to exactly **one** live
      worktree still rails, it does **not** collapse to passthrough —
      `daft exec 'feat/*' -- <cmd>` where only one matched branch has a worktree
      (the rest are orphans) renders `┌ Running <cmd> in 1 worktree` with the
      one `✓`/`✗` row plus `↓ <branch> no worktree` rows, not an inherited-stdio
      run. Likewise `daft exec --all -- <cmd>` in a single-worktree repo renders
      a one-row rail, not passthrough

## Fleet scopes

- [ ] `daft exec --related -- <cmd>`: rows labeled `repo:branch`, header
      `in N related worktrees`; two related repos sharing a branch name render
      as distinct rows (not collapsed)
- [ ] `daft exec --all-repos -- <cmd>`: header `in N repos`; not-cloned /
      missing-repo notices print as plain warnings above the rail

## Output redirection

- [ ] `daft exec --all -- <cmd> > out.txt`: the rail narrates on stderr, and the
      failure dump lands in `out.txt` (with `-v`, every worker's output) — the
      rail is not duplicated into the file
- [ ] `daft exec --all -- <cmd> 2>/dev/null`: no rail (stderr not a TTY), the
      plain summary rows + dump behave exactly as before this feature

## Non-TTY parity

- [ ] `daft exec --all -- <cmd> | cat`: plain output byte-identical to
      pre-feature (the YAML scenarios pin this; re-run
      `mise run test:manual tests/manual/scenarios/worktree-exec`)

## Scale

- [ ] `daft exec --all -- <cmd>` with ~20 worktrees: the live region clamps to
      the terminal window (no runaway), receipts persist above in plan order, no
      stranded frames after the footer
