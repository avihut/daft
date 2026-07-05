---
branch: daft-651/feat/output/plan-execute-timeline
---

# Plan-Execute Timeline

All checks below are interactive-terminal checks — run them in a real TTY in a
scratch repository (`mktemp -d`, local git config only; never this repo). The
engine spike (`cargo run --example timeline_spike [fail|skip]`) renders a
synthetic rail + a real embedded hook block for quick visual iteration.

## Rail basics

- [ ] `daft start <name>`: full plan appears at once (pending rows dim), steps
      fill in place top-to-bottom, footer `└ Ready in <t>` persists
- [ ] Header shows the resolved base: `┌ Starting <name> ← <base>`
- [ ] Push off (default) or `--local`: no push row at all — the plan lists only
      steps that will run
- [ ] With a remote + `daft.checkout.push=true`: `✓ Pushed  → origin/<name>`
      with a dim duration when ≥ 1s
- [ ] `daft go <existing-remote-branch>`: `✓ Checked out branch ← origin/<b>`
- [ ] `daft go <local-branch>` (worktree exists): single "Switched to existing
      worktree" line, **no rail**
- [ ] `daft go -`: previous-worktree navigation unchanged, no rail
- [ ] `daft go <missing> --start`: morphs into the start rail (exactly one rail
      for the whole invocation)
- [ ] Carry: with uncommitted changes `✓ Carried changes`; with a clean tree the
      carry row vanishes once execution reaches it
- [ ] `daft remove <branch>`: execution order (remote → worktree → branch),
      `Deleted branch` annotated `was merged into <default>`
- [ ] `daft remove` remote fate always explicit: `✓ Deleted remote branch`, or
      `○ kept on origin — daft.branchDelete.remote off` / `--local` /
      `○ no remote branch`
- [ ] Multi-branch remove: one rail, dim branch-name group anchors, count
      footer; current-worktree branch deferred to last
- [ ] `daft clone <url>` single-branch: `✓ Cloned repository ← <url>` as a
      pre-completed row (bare-clone spinner runs before the layout prompt), then
      `Create worktree`, hooks, footer
- [ ] `daft clone --branch a,b,c` (multi-branch): rail closes with
      `└ Base worktree ready in <t>` BEFORE the satellite table; hooks render
      after the table exactly as before

## Hook composition (the weld)

- [ ] Hook step pending as `○ post-create hooks`; expands in place into the hook
      block headed by a single `├─ daft hooks …` branch row (no banner box on
      the rail); job/summary interior byte-identical to the standalone renderer
- [ ] Rail spacers (`│`) separate the block from rows above and below, never
      doubled
- [ ] Pending rows + `└ …` stay visible below the block while jobs run
- [ ] No hooks configured: the hook row vanishes silently
- [ ] Untrusted repo: `↓ post-create hooks  skipped — Repository not trusted`,
      and the contextual `Untrusted repo — …` notice (#654: trust + replay
      suggestions) persists above the rail, not torn through the live bars
- [ ] `--skip-hooks all`: yellow ↓ row
- [ ] Pre-push hook (git hook in repo) during `daft start` with push on: block
      welds under the active Push row; on rejection `✗ Push` + worktree still
      completes + non-zero exit (#599 semantics)

## Failure states

- [ ] Mid-plan failure (e.g. worktree dir exists): `✗` row with detail,
      remaining rows `○ … (not run)`, `└ Failed after <t>`, error line after
- [ ] Remove with a failing remote delete: `✗ Delete remote branch` row, later
      steps still run, `└ Finished with failures in <t>`, errors after
- [ ] Carry conflict: red Carry row pointing at `git stash pop`
- [ ] Ctrl-C mid-run: region collapses once, no stranded frames, no duplicate
      footer (after step 7 lands)

## Modes

- [ ] Piped stderr (`2>&1 | cat`): byte-identical legacy output, no rail
- [ ] `DAFT_TESTING=1`: no rail; result lines as before
- [ ] `-q`: silent except warnings/errors
- [ ] `-v`: free-text steps appear as dim transient sub-lines under the active
      row
- [ ] `NO_COLOR=1` on a TTY: plain legacy output (indicatif hides its draw
      target under NO_COLOR — matches the pre-timeline spinner behavior)
- [ ] Narrow terminal (< 60 cols): rows truncate, never wrap; region clips from
      the bottom and recovers
- [ ] merge / sync / rename / prune / repo remove: output unchanged
