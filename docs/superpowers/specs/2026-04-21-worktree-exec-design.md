# daft worktree-exec — Design

**Date:** 2026-04-21 **Branch:** `feat/execute-on-worktree` **Status:**
Approved, pending implementation plan

## Problem

Users routinely want to run a command inside a worktree other than the one they
are currently in, without `cd`-ing there. They also want to fan the same command
out across many worktrees — a batch build, a batch test, a lint pass — with
sensible parallelism and failure handling.

Today this requires:

- `cd ../<branch>/ && cmd && cd -` by hand, per worktree, per command, or
- Shell `for`-loops that lose output, lose exit codes, and break on the first
  failure, or
- Writing a `daft.yml` hook purely to run a single ad-hoc command — which is too
  heavy for one-offs.

The `-x` / `--exec` option on `clone` / `init` / `checkout` covers first-time
post-setup execution but is scoped to the worktree being created, not arbitrary
existing worktrees.

## Goal

A first-class `daft worktree-exec` command that runs one or more commands
against one or more selected worktrees, with:

- A single-target fast path that feels like `cd <wt> && cmd`.
- A multi-target batch mode with parallel execution by default, sequential on
  request, fail-fast by default, continue-through-failures on request.
- A list-mode UI inspired by `daft sync` for readability, with failed-target
  output dumped at the end.
- Full completion, man-page, and documentation coverage consistent with the rest
  of daft.

## Non-goals (v1)

Recording these so they are not silently added later.

- Template expansion (`{branch}`, `{worktree_path}`) in command strings. Users
  reach the same values via injected environment variables.
- `--subdir` / `--cwd` to run inside a sub-path of the worktree. Users can
  `cd sub && cmd` inside the `-x` shell form.
- Ownership selectors (`--owned`, `--others`) or `--except <branch>`
  subtraction. The `--all` selector and positional globs cover v1.
- Per-worktree env-file auto-sourcing (e.g. automatic `.envrc` / `mise`
  activation). Users opt in explicitly in their command.
- Timeouts. Ctrl-C cancels.
- JSON / machine-readable output for the result table.

These are all non-breaking to add later.

## CLI surface

```
daft worktree-exec [TARGETS]... [--all]
                   [--sequential | --keep-going] [-v|--verbose]
                   (-- CMD [ARGS]... | -x 'CMD' [-x 'CMD']...)
```

Verb alias: **`daft exec`**. Git subcommand form: **`git worktree-exec`** (via
symlink on PATH).

### Positionals

Each positional is interpreted in this order against the set of worktrees:

1. If it contains any of `*`, `?`, or `[`, it is treated as a **glob** and
   matched against worktree **branch names**.
2. Otherwise, exact match against **branch name**, then against worktree
   **directory name** (same precedence as `daft worktree-carry`).

If a positional matches nothing, `daft exec` errors listing the unmatched tokens
and runs no commands.

### Selectors

- **`--all`** — expand to every worktree returned by `git worktree list`. No
  exclusions. Mutually exclusive with positionals.
- No other selectors in v1.

At least one of positionals or `--all` must be provided.

### Command forms (mutually exclusive)

- **`-- CMD [ARGS]...`** — everything after `--` is an argv vector. Executed
  directly via `std::process::Command::new(CMD).args(ARGS)`. No shell, no
  globbing, no env expansion in the user's command string. Preferred for the
  common case and for commands whose own flags collide with daft's (the `--`
  delimiter is unambiguous).
- **`-x 'CMD'`** (repeatable) — each string is a shell pipeline run via
  `$SHELL -c 'CMD'`, falling back to `sh -c` if `$SHELL` is unset. Multiple `-x`
  values run in order per worktree.

The two forms cannot be mixed in one invocation.

**Deliberate divergence from existing daft `-x`:** `clone` / `init` /
`checkout`'s `-x` uses `$SHELL -i -c`. This new command drops `-i`. Rationale:
interactive mode loads rcfiles, which is slow, non-deterministic across
machines, and causes surprising interactions when daft is itself invoked from a
hook or CI. We do not change the legacy `-x` on the other commands in this
design.

### Execution flags

| Flag               | Meaning                                                        |
| ------------------ | -------------------------------------------------------------- |
| _(default)_        | Parallel across worktrees, fail-fast at the process-exit level |
| `--sequential`     | One worktree at a time, stop on first failure                  |
| `--keep-going`     | One worktree at a time, continue through failures              |
| `-v` / `--verbose` | Hook-style live "windows" TUI instead of list-mode table       |

`--sequential` and `--keep-going` are mutually exclusive. `--keep-going` implies
sequential.

### Single-target pass-through

When all selectors resolve to exactly one worktree, `daft exec` spawns the child
with **fully inherited stdio** (no capture, no prefixing, no UI). The child's
exit code is propagated verbatim. Interactive programs — `claude`, `vim`, `fzf`
— work the same as if the user had `cd`-ed first.

This includes `daft exec --all -- cmd` when the repo has exactly one worktree.

### Completions

Follow the worktree-completions convention used by `carry`, `fetch`, and
`branch -d`:

- Positional completer = current worktrees' branch names + directory names.
- `--all` is hinted as a mutually-exclusive alternative where the shell supports
  it.
- `-x` is repeatable.
- The `--` terminator stops daft's completion and hands off to the child
  command's own completion (native behavior in fish/zsh; bash needs an explicit
  early-return on `--`).

All five completers must be updated in lockstep: `mod.rs`, `bash.rs`, `zsh.rs`,
`fish.rs`, `fig.rs`.

## Target resolution

Given positional tokens plus `--all`, resolution produces an ordered,
de-duplicated `Vec<ResolvedTarget { worktree_path, branch_name }>`.

- **Deduplication** is by absolute worktree path.
- **Ordering** is stable:
  - Positionals in user-supplied order.
  - Within a single positional that is a glob, expansions are sorted by branch
    name.
  - `--all` sorts by branch name.
- **Orphan branches** (branches with no worktree) that match via glob are
  skipped with a single aggregated warning line. This keeps globs usable in
  repos with many stale local branches, while still surfacing that a skip
  happened.
- The **default branch** is not treated specially and is not excluded.
- The **current worktree** is not treated specially and is not excluded.
- **Zero positionals and no `--all`** → error.
- **`--all` combined with positionals** → error.
- **Unmatched exact positional or zero-expansion glob** (ignoring the
  orphan-skip warning) → error listing the unmatched token(s). No commands run.

Invocation must be from anywhere inside the daft-managed repo. Invocation
outside the repo produces the standard "Not inside a Git repository" error.

## Execution semantics

### Working directory

The worktree root. No `--subdir` in v1.

### Spawn strategy

| Form       | How                                                       |
| ---------- | --------------------------------------------------------- |
| `-- CMD…`  | Direct argv exec of `CMD` with `ARGS`, no shell.          |
| `-x 'CMD'` | `$SHELL -c 'CMD'`, or `sh -c 'CMD'` if `$SHELL` is unset. |

### Injected environment variables

Per child, in addition to the parent environment:

| Var                  | Value                                  |
| -------------------- | -------------------------------------- |
| `DAFT_WORKTREE_PATH` | Absolute path to the target worktree   |
| `DAFT_BRANCH_NAME`   | Branch name                            |
| `DAFT_PROJECT_ROOT`  | Project root (shared git dir's parent) |
| `DAFT_GIT_DIR`       | Git common dir                         |
| `DAFT_COMMAND`       | Literal string `"exec"`                |

Hook-specific variables (`DAFT_HOOK`, `DAFT_IS_NEW_BRANCH`,
`DAFT_SOURCE_WORKTREE`, etc.) are **not** set.

### Per-worktree failure policy

When `-x` is repeated, commands run sequentially per worktree and **stop on
first failure within that worktree**. Remaining `-x` commands for that worktree
are not run. The worktree is marked failed with the failing command's exit code.
This matches `daft.yml` `piped` semantics and shell `set -e` expectations.

Other worktrees are unaffected at this layer; cross-worktree behavior is
controlled by `--sequential` / `--keep-going`.

### Cross-worktree failure policy

- **Parallel (default):** All worktrees start. Each runs to its own completion
  or first-`-x`-failure. `daft exec`'s own exit code is `1` if any worktree
  failed, `0` otherwise.
- **`--sequential`:** Worktrees run one at a time in resolved order. First
  failing worktree terminates the run. `daft exec` exits `1`.
- **`--keep-going`:** Sequential, but every worktree runs regardless of prior
  failures. Exit code `1` if any failed, `0` otherwise.

### Exit code aggregation

- Single-target (pass-through): exact child exit code.
- Multi-target: `0` if all succeeded, else `1`. Per-worktree exit codes are
  surfaced in the list-mode table and failed-output dump.

### Signal handling

`SIGINT` to `daft exec` propagates to all running children via `SIGTERM`, then
waits for them to exit (bounded — a second `SIGINT` escalates to `SIGKILL`). The
final table is rendered with cancelled rows marked. No detached-zombie footgun.

## Output UI

Three modes, selected automatically.

### Mode A — Single target (pass-through)

No UI. Stdio inherited. Exit code verbatim.

### Mode B — Multi-target, default verbosity (list mode)

Modeled on `daft sync`:

```
daft exec --all -- cargo test
────────────────────────────────────────────────────────────
Commands
  1. cargo test
────────────────────────────────────────────────────────────
Worktrees
  ✓  master         (0.8s)
  ⠸  feat/a           running cargo test…
  ⠼  feat/b           running cargo test…
  ⠴  fix/crash        queued
  ✗  feat/dirty     (1.2s)   cargo test → exit 101
────────────────────────────────────────────────────────────
```

Each worktree row shows: spinner, branch name, current stage (command index when
`-x` is repeated), elapsed time, and on failure a one-line `<cmd> → exit <N>`
marker.

After all worktrees finish, **failed worktrees' captured output is dumped**,
each preceded by a header:

```
─── feat/dirty ── cargo test → exit 101 ────────────────────
running 42 tests
test utils::check_path ... FAILED
...
```

Successful worktrees' captured output is discarded. Each child's stdout + stderr
is captured into a **bounded ring buffer (1 MB tail)**; the fixed size is an
internal constant, not a user-facing flag.

### Mode C — Multi-target, `-v` (windows mode)

Reuses the hooks TUI. One live panel per currently-running (worktree × command)
pair, streaming output, collapsing when finished. If the terminal has fewer rows
than running pairs, the renderer degrades gracefully — same behavior as hooks.

The final failed-output dump still happens after the TUI tears down; in windows
mode everything was already shown live, so the dump acts as a
scrollback-friendly summary.

## Code structure

### New files

- **`src/commands/exec.rs`** — clap `Args` struct, argument validation, entry
  point. Shape modeled on `src/commands/carry.rs`; ≈150 lines.
- **`src/core/worktree/exec.rs`** — pure logic: target resolution, per-worktree
  pipeline runner, scheduler (parallel / sequential / keep-going), `ExecReport`
  data type. ≈250–350 lines.
- **`tests/manual/scenarios/exec/`** — YAML scenarios covering: single-target
  pass-through; multi-target parallel happy path; glob expansion plus `--all`;
  failure dump rendering; `-x` pipeline stop-on-failure within a worktree;
  sequential + keep-going policies; unmatched positional; orphan branch skip
  warning; `-v` windows mode.
- **`tests/integration/test_worktree_exec.sh`** — bash integration tests
  following the patterns in the existing `tests/integration/test_*.sh` suite.
  Named distinctly from the existing `test_exec.sh` (which exercises `-x` on
  clone / init / checkout). Creates a temp repo with several worktrees and
  exercises the main code paths end-to-end.
- **`docs/cli/daft-exec.md`** and **`docs/cli/git-worktree-exec.md`** — one
  reference page each, template = `docs/cli/daft-doctor.md`.
- **`docs/guide/running-commands-across-worktrees.md`** — narrative guide: when
  to use `daft exec` vs hooks vs `-x` on clone/init/checkout, worked examples
  for common CI-like batches, debugging a failed target.
- **`man/daft-exec.1`** — generated via `mise run man:gen`.

### Modified files

- **`src/commands/mod.rs`** — register the `exec` module.
- **`src/main.rs`** — routing for `git-worktree-exec`, `daft exec`,
  `daft worktree-exec`.
- **`xtask/src/main.rs`** — add `exec` to `COMMANDS` and
  `get_command_for_name()`.
- **`src/commands/docs.rs`** — add to `get_command_categories()`.
- **`src/commands/completions/{mod,bash,zsh,fish,fig}.rs`** — register `exec` /
  `worktree-exec` / `git-worktree-exec`, verb alias group, positional
  branch-name completer, `-x` repeatability, `--` terminator behavior.
- **`src/core/worktree/mod.rs`** — expose the new `exec` submodule.
- **`SKILL.md`** — row in the Management table, row in the invocation-forms and
  verb-alias tables, subsection in Workflow Guidance mapping "run my build on
  these worktrees" → `daft exec`.
- **`CHANGELOG.md`** —
  `feat: add daft exec for running commands across worktrees`.

### Reused (unchanged)

- **`src/exec.rs`** — existing `run_exec_commands` used by `clone` / `init` /
  `checkout` `-x`. Not used by this command (different shell semantics,
  §Execution). Left intact for backward compatibility.
- **`src/core/worktree/list.rs`** — worktree enumeration for `--all`.
- Branch-resolution helpers used by `carry` and `fetch`.

### Shared UI components

- **Sync's list-mode table renderer** — inspect and, if low-risk, extract a
  `TargetTableRenderer` trait into `src/core/progress.rs`; otherwise mirror the
  minimal bits needed in `exec.rs`. Decision made during implementation.
- **Hooks' live-windows TUI renderer** — reuse for `-v` mode. If buried inside
  the hooks module today, extract via a thin shim; extraction is an acceptable
  scope bump on this feature.

Both of these are tracked as implementation-plan open items, not design gaps.

## Data flow

1. `exec::run` parses args and validates mutual exclusions (`--all` vs
   positionals, `--` vs `-x`, `--sequential` vs `--keep-going`).
2. Resolves targets to `Vec<ResolvedTarget>`. Errors early on unmatched
   positionals.
3. Single target → pass-through: inherit stdio, propagate exit code.
4. Else → construct scheduler (parallel / sequential / keep-going). Spawn
   per-worktree tasks; each runs its command pipeline; stdout + stderr go into a
   bounded ring buffer; status events feed the renderer.
5. Renderer = list-mode table (default) or hook-windows TUI (`-v`).
6. On SIGINT: send SIGTERM to children, wait, mark cancelled rows, render final
   state. A second SIGINT escalates to SIGKILL.
7. After all finish: dump failed children's captured output. Exit `0` if all
   succeeded, else `1`.

## Testing

- **Unit tests:** target resolution (exact, dir-name, glob, dedup, unmatched,
  orphan-skip); scheduler state machine (parallel, sequential, keep-going,
  SIGINT); exit-code aggregation; ring-buffer truncation.
- **YAML scenarios** in `tests/manual/scenarios/exec/` cover the UX matrix
  listed under "New files."
- **Integration test:** a real temp repo with three worktrees; run
  `daft exec --all -- true` and `daft exec --all -- false`; assert exit codes,
  failed-output dump presence, row counts.

Per CLAUDE.md, before committing: `mise run fmt`, `mise run clippy`,
`mise run test:unit` all pass. Every bug discovered during implementation gets a
regression YAML scenario.

## Deliverables checklist

1. **Completions — worktree-completions convention.** Positional branch + dir
   name completer reused from existing commands. `--all` hinted as mutually
   exclusive where possible. `-x` repeatable. `--` terminator hands off to the
   child command's own completions (manual per-shell verification). All five
   completers updated in lockstep.

2. **Man page with examples.** `man/daft-exec.1` generated from clap. The `Args`
   struct carries `#[command(long_about = …)]` and `#[command(after_help = …)]`
   that include a real EXAMPLES section:

   ```
   EXAMPLES
       Run a single command across all worktrees:
           daft exec --all -- npm test
       Run on specific branches (glob and exact mix):
           daft exec feat/auth 'feat/ui-*' -- cargo build
       Sequential with fail-fast:
           daft exec --all --sequential -- pnpm lint
       Pipeline of commands per worktree:
           daft exec --all -x 'mise install' -x 'pnpm build' -x 'pnpm test'
       Pass-through to an interactive program (single target):
           daft exec feat/auth -- claude
       Live "windows" output (like hooks):
           daft exec --all -v -- cargo test
   ```

   `mise run man:verify` keeps it in sync (CI-enforced).

3. **Documentation.** `docs/cli/daft-exec.md` and
   `docs/cli/git-worktree-exec.md` reference pages. A narrative
   `docs/guide/running-commands-across-worktrees.md` guide. `SKILL.md` updates.
   `CHANGELOG.md` entry.

## Open implementation questions (deferred to the plan)

- Whether the hooks-TUI windows renderer is extractable cleanly today, or needs
  a thin shim PR alongside this feature.
- Whether sync's list-mode table is reusable via a `TargetTableRenderer` trait,
  or whether `exec` gets a minimal local mirror.
- Whether the 1 MB captured-output cap is sufficient in practice, or needs to
  become a config setting.
