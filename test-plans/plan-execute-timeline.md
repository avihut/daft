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
- [ ] The rail opens the moment the command starts: header + grey planning row
      (`⠹ Resolving branch` for go, `⠹ Resolving base branch` for start,
      `⠹ Validating branches` for remove) + ticking stopwatch — no legacy
      spinner anywhere before or under the rail; the committed plan replaces the
      middle in place (no duplicated header, no flicker)
- [ ] The pending footer opens as a dim ticking elapsed counter from the moment
      the rail opens (`└ 142ms`, `└ 1.2s` — never an ellipsis), and resolves
      into the outcome footer
- [ ] Header shows the requested base immediately: `┌ Starting <name> ← <base>`;
      when the fetch resolves a fresher remote ref, the branch row carries it —
      `✓ Created branch  ← origin/<base>`
- [ ] Remote sync on (`daft.checkout.fetch`): the rail appears immediately,
      opening with `Fetch remote  <remote>` then `Set up tracking` rows — no
      spinners before the rail
- [ ] Fetch with remote unreachable: yellow
      `↓ Fetch remote  failed — continuing with local refs` (and the same for
      `Set up tracking`), warning above the rail, worktree still created
- [ ] Fetch off (default config or `--local`): no fetch rows at all
- [ ] Push off (default) or `--local`: no push row at all — the plan lists only
      steps that will run
- [ ] With a remote + `daft.checkout.push=true`: `✓ Pushed  → origin/<name>`
      with a dim duration when ≥ 1s
- [ ] `daft go <existing-remote-branch>`: `✓ Checked out branch ← origin/<b>`
- [ ] `daft go` with `daft.checkout.fetch` on: the plan commits before the
      network — the `Fetch remote` row leads it, the `Check out branch` row
      starts bare, and the provenance (`← origin/<b>`, `tracking origin/<b>`,
      `local only`) lands on the pending row when the fetch resolves
- [ ] `daft go <missing>` with fetch on: the committed rail closes as a Failed
      receipt — `✓ Fetched remote`, later rows `(not run)`,
      `└ Failed after <t>`, error + tip below; with fetch off the planning face
      collapses and only today's plain error + tip print (no residue)
- [ ] `daft go <local-branch>` (worktree exists): single "Switched to existing
      worktree" line, **no rail** — the planning face collapses without a trace
      above it
- [ ] `daft go -`: previous-worktree navigation unchanged, no rail
- [ ] `daft go <missing> --start`: morphs into the start rail (exactly one rail
      for the whole invocation)
- [ ] Carry: with uncommitted changes `✓ Carried changes`; with a clean tree the
      carry row vanishes once execution reaches it
- [ ] Shared files (`shared:` in daft.yml + collected storage): a `│` spacer
      then `├─ shared files` anchor with one gutter row per path (`│  ✓  .env` —
      tucked inside the rail), placed between Carry/Push and the hooks section;
      the pending plan already shows the tree shape (`│  ○  .env` under the dim
      anchor)
- [ ] Shared file declared but never collected: yellow `│  ↓  <path>` gutter row
      saying `missing from shared storage` with the `daft shared sync` remedy —
      never silent
- [ ] Shared path materialized in this worktree: dim gutter row
      `│  ○  <path>  materialized`; already-linked:
      `│  ○  <path>  already linked`
- [ ] Section planned from the source config but the target branch's daft.yml
      drops `shared:`: rows and anchor vanish — no stranded anchor above the
      hook weld
- [ ] Shared path conflicting with a real file (tracked file also declared
      shared): yellow `↓ <path>` row carrying the `daft shared link` remedy
- [ ] Shared files in Plain mode (`2>&1 | cat`): legacy `Linked <path>` lines
      plus the `warning: … missing from shared storage` line for uncollected
      paths
- [ ] `daft remove <branch>`: execution order (remote → worktree → branch),
      `Deleted branch` annotated `was merged into <default>`
- [ ] `daft remove` plans hook rows only for phases with discoverable hooks:
      pre-remove hooks configured but no post-remove → only the
      `pre-remove hooks` row (and its `│` frame); no hooks at all → neither row,
      no vanish churn; per-branch on multi-remove (each worktree's own config
      decides its pre-remove row)
- [ ] `daft remove .` (worktree-path shorthand): header names the resolved
      branch — `Removing <branch>`, never `.` (the planning face shows the seed
      header, the committed plan swaps in the resolved one)
- [ ] `daft remove` validation failure (dirty worktree, unmerged): the planning
      face collapses without a trace, then the plain `error: cannot delete …`
      lines and the aborting summary print
- [ ] Consolidation prompt (refined visitor daft file, unforced remove): the
      face suspends for the prompt (summary + `[c/d/A]` on a clear terminal),
      and answering redraws the region, which then commits the plan and proceeds
- [ ] `daft remove` with remote deletion on (`daft.branchDelete.remote` true):
      `✓ Deleted remote branch`, or dim `○ no remote branch` when the branch has
      no upstream
- [ ] Remote deletion off (default, `daft config remote-sync` local only, or
      `--local`): no remote row or note anywhere in the rail — the remote is
      never mentioned
- [ ] Multi-branch remove: one rail, `├─` branch-name anchors each with a `│`
      spacer above (the first leans on the header's spacer — never doubled),
      every branch's step rows and notes in the gutter —
      `│  ✓  Removed worktree`, `│  ○  no remote branch` — count footer;
      current-worktree branch deferred to last
- [ ] `daft clone <url>` single-branch: `✓ Cloned repository ← <url>` as a
      pre-completed row (bare-clone spinner runs before the layout prompt), then
      `Create worktree`, hooks, footer
- [ ] `daft clone --branch a,b,c` (multi-branch): rail closes with
      `└ Base worktree ready in <t>` BEFORE the satellite table; hooks render
      after the table exactly as before

## Hook sections (succinct default)

- [ ] Hook step pending as `○ post-create hooks` (on the spine), already framed
      by its section's `│` gaps — the committed plan shows the receipt's rail
      rhythm (remove: both phases framed; start: one gap above the hook row;
      none doubled with the header's, the footer's, or a group's own spacer)
- [ ] When the phase runs, the pending row becomes the `├─ post-create hooks`
      anchor in place with one gutter row per job (`│  ✓  <job>`) — no row below
      shifts (only job rows grow the section); pending rows + the ticking footer
      stay visible below while jobs run
- [ ] Active job row: `│  ⠹  name  <latest output>` — gutter, spinner, and the
      job's latest output line as a dim annotation updating in place; long lines
      truncate, never wrap
- [ ] Job description shows as the annotation until the first output line
      arrives
- [ ] Finished jobs resolve in place: `│  ✓  name` with dim duration only at ≥
      1s, seated in the shared annotation column; parallel jobs persist in
      completion order
- [ ] Failed job (failMode warn): red `✗ name`, command completes, footer
      `Finished with failures…`, and the job's full captured output prints BELOW
      the footer as `error: hook job '<name>' failed:` + indented lines; the
      runner's `Job '<name>' failed…` line does not appear
- [ ] Failed job (failMode abort, `worktree-pre-create`): command aborts, dump
      still prints after the abort footer, before the command error
- [ ] `--skip-hooks <job>`: yellow
      `│  ↓  <job>  skipped — requested (--skip-hooks)` gutter row inside the
      section; dependents render `skipped — depends on …`
- [ ] Job with `skip:`/`only:` condition false: no row at all (check
      `daft hooks jobs` still records it)
- [ ] Hook-level `skip:`/`only:` condition false: the whole hook row vanishes
      silently
- [ ] Background job: blue `│  ↻  name  background` receipt row; the
      `⟳ N background job(s) running` notice rides the gutter as section content
      (`│  ⟳ …`)
- [ ] `daft.hooks.output.quiet`: job rows and durations still render, but no
      live output annotation and no failure dump
- [ ] Multi-phase (pre-create AND post-create in one run): two sections, each
      with its own spacer + anchor, no doubled spacers between them
- [ ] Sequential (piped) hooks: receipt rows may persist before a later, wider
      job name raises the alignment column — accepted cosmetic limit
- [ ] No hooks configured: `daft start` / `daft go`'s hook row vanishes silently
      when execution reaches it, taking its planned `│` gaps with it — no stray
      blank rail lines (`daft remove` never plans the row; see Rail basics)
- [ ] Untrusted repo: `↓ post-create hooks  skipped — Repository not trusted`
      keeping the planned `│` frame around the row, and the contextual
      `Untrusted repo — …` notice (#654: trust + replay suggestions) persists
      above the rail, not torn through the live bars
- [ ] `--skip-hooks all`: yellow ↓ row on the hook step
- [ ] Pre-push hook (git hook in repo) during `daft start` with push on:
      `├─ pre-push hooks` section under the active Push row; on rejection
      `✗ Push` + worktree still completes + non-zero exit (#599 semantics)
- [ ] Remove with remote deletion + pre-push hook: per-branch
      `├─ pre-push hooks` section under each active `Delete remote branch` row

## Threaded log (verbose)

- [ ] `-v`: the hook section threads each job's log under its row —
      `│  │    <line>` hanging from the glyph column; the job rows themselves
      are byte-identical to the succinct dialect (glyphs, floods, durations)
- [ ] `daft.hooks.output.verbose=true` without `-v`: same threaded log
- [ ] Anchor annotation: bold label then grey
      `worktree-post-create · daft v<version>`; no `on: <target>` segment
      anywhere (the header / branch anchor already names the target)
- [ ] Each thread opens with the dim `❯ <command>` provenance line, and that
      line survives into the receipt
- [ ] Live: rolling window of `tailLines` grey lines per running job; the
      persisted receipt keeps the full log (never windowed)
- [ ] Each job's thread closes with an empty thread line (`│  │`) — after every
      persisted log, and under every running job's live block (parallel blocks
      never fuse); the rail's lone `│` appears only at section boundaries
- [ ] Job with a paragraph-long `description:`: the live row truncates at the
      terminal edge — never wraps or tears the region — including while the
      elapsed counter is promoted
- [ ] Live annotation keeps the job's description while output rolls in the
      thread (no duplicated newest line)
- [ ] Success log recedes grey; a failed job's log keeps default ink
- [ ] Failed job: single `error: hook job '<name>' failed (exit code: N)` line
      after the footer — no output dump repeat (check failMode warn AND abort)
- [ ] Job that prints nothing: dark-grey `(no output)` thread line
- [ ] Section closes with its own rail end — the grey `└ all jobs in <t>` note
      (before the reconnect spacer; skipped when no jobs ran)
- [ ] `daft remove -v` with pre-remove hooks but no post-remove hooks: exactly
      one rail end at line start — the pending footer never strands above
      `└ Removed in <t>` (#651 field test: the unplanned phase's debug line
      leaked to raw stdout mid-region)
- [ ] Job silent past `timerDelay`: dim `(<elapsed>)` joins the spinner row;
      first output retires it
- [ ] `daft.hooks.output.quiet` + verbose: threads vanish entirely (rows, note,
      and the after-footer exit fact remain)
- [ ] No `├───┐` banner box anywhere on the timeline; `┌ daft hooks v…┐` remains
      only in standalone contexts (`daft hooks run`, merge hooks)
- [ ] Plain mode (`2>&1 | cat`) with `-v`: each job's command line appears
      (`daft.hooks.output.verbose` plain-mode behavior); without it, absent

## Ink grammar

- [ ] Committed plan is readable at a glance: pending rows show a dim `○` with a
      plain (default-ink) label — never a whole-row grey slab
- [ ] Section headings (`shared files`, `post-create hooks`, remove's branch
      anchors) render bold, in the plan and in the receipt
- [ ] Identity inks constant across states: `origin` / `← origin/master` /
      `→ origin/x` cyan, worktree path manila, shared file paths violet —
      pending, active, and done alike
- [ ] Hook job names wear their outcome: green succeeded, red failed, yellow
      skipped, blue background — matching the verbose block's summary colors
- [ ] Failure details and skip reasons render plain (never the stage's identity
      ink); dimmed rows (expected skips, `(not run)`) drop identity inks
      entirely
- [ ] `⟳ N background jobs` notice and `○ no remote branch` notes sit one grey
      tier below content, above the pending glyph's dark grey
- [ ] `NO_COLOR` / piped: byte-identical plain output, zero ANSI

## Failure states

- [ ] Mid-plan failure (e.g. worktree dir exists): `✗` row with detail,
      remaining rows `○ … (not run)`, `└ Failed after <t>`, error line after
- [ ] Remove with a failing remote delete: `✗ Delete remote branch` row, later
      steps still run, `└ Finished with failures in <t>`, errors after
- [ ] Carry conflict: red Carry row pointing at `git stash pop`
- [ ] Ctrl-C mid-run: region collapses once, no stranded frames, no duplicate
      footer (after step 7 lands)
- [ ] Ctrl-C while the planning face is up: the face collapses, nothing
      persists, exit 130
- [ ] Ctrl-C after a consolidation prompt resolved (mid-hook): the region still
      collapses cleanly — the prompt restores the region's interrupt behavior
      instead of clearing it

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
