# `daft list` empty state + `daft repo remove` copy refinement

**Date:** 2026-05-02 **Issue:**
[#444](https://github.com/avihut/daft/issues/444) **Surfaced by:** field-testing
of [#421](https://github.com/avihut/daft/issues/421) (`daft repo remove`)

## Problem

Two related but distinct copy issues in the `daft` CLI surface from the same
underlying field test.

### `daft list` empty state

Running `daft list` in a bare-layout repo with no checked-out worktrees prints
just the column headers and an empty body:

```
❯ daft list
  Branch  Path  Base  Changes  Remote  Age  Owner  Commit
```

The user can't tell whether the command failed silently, the layout is
misconfigured, or there are genuinely no worktrees. The blocking path
(`src/commands/list.rs::print_table`) early-returns silently on
`infos.is_empty()` and prints nothing. The default TTY path
(`src/output/tui/render.rs::render_table`) renders the header row only — the
artefact in the issue.

The empty result is filtered into existence at:

- `src/commands/list_live.rs:130-132` — porcelain seed skips bare entries
- `src/core/worktree/list.rs:856-858` — `collect_worktree_info` skips bare
  entries

In bare layouts (`contained`, `contained-flat`, or any custom layout where
`Layout::needs_bare()` is true), zero worktrees is a **legitimate, normal
state**. The bare repo itself isn't a worktree. In non-bare layouts (`sibling`,
`nested`, `centralized`, `contained-classic`), the primary checkout is always a
worktree, so an empty list signals an anomaly — but the same hints (`go` /
`start`) are still the right next action either way.

### `daft repo remove` technical copy

The confirm prompt and dry-run plan output both leak internal terminology ("bare
git dir", "trust DB entry") that means nothing to a user who hasn't read the
codebase:

```
This will delete the bare git dir (no worktrees to remove).
```

```
Would remove:
  worktree  /path/to/feature  (feature)
  bare      /path/to/.git
  trust DB entry for /path
```

The structure and detail level are right — the words are wrong.

## Goals

1. `daft list` renders a clear, actionable empty state when the worktree set is
   empty.
2. The empty state guides the user toward the two relevant next actions:
   `daft go` (existing branch) and `daft start` (new branch).
3. `daft repo remove`'s prompt and plan output use plain terminology that
   doesn't require reading source code.
4. No behavior change in either command — copy and rendering only.

## Non-goals

- Layout-conditional empty-state messaging. Same copy regardless of layout.
- `daft doctor` integration. Layout-aware diagnostics belong elsewhere.
- Restructuring `daft repo remove`'s prompt/plan format — only the strings.
- Changes to `daft list`'s structured output (`--format json|csv|...`). Empty
  rows array remains the contract.

## Design

### Part 1: `daft list` empty state

#### Output

When the final list of `WorktreeInfo` is empty (after any `-b`/`-r`/`-a` branch
enumeration), render this in place of the table:

```
No worktrees yet.

  daft go <branch>     switch to an existing branch
  daft start <branch>  create a new branch
```

Styling (when `styles::colors_enabled()` is true):

- `daft` — `styles::dim` (chrome)
- `go` / `start` — `styles::cyan` + `styles::bold` (verb, what the user types)
- `<branch>` — `styles::dim` (placeholder)
- right-hand description — `styles::dim`

When colors are disabled, the same text without escape sequences. The right-hand
descriptions are aligned by the longest command syntax (`daft start <branch>`).

#### Module shape

New module `src/commands/list_empty.rs`:

```rust
/// Render the `daft list` empty-state hint as a styled string.
pub fn render(use_color: bool) -> String;

/// Write the empty-state hint to the given writer.
pub fn print(out: &mut impl std::io::Write, use_color: bool) -> std::io::Result<()>;
```

`render` returns a styled string; `print` writes it. Keeping render pure makes
unit testing trivial — no stdout capture, no terminal-size mocking. The module
is wired into `src/commands/mod.rs` (private to the crate, used only by `list`
and `list_live`).

#### Render path integration

**Blocking path** (`src/commands/list.rs::run_blocking` → `print_table`):

Replace the silent early-return at the top of `print_table`:

```rust
if infos.is_empty() {
    return;
}
```

with a call to
`list_empty::print(&mut std::io::stdout(), styles::colors_enabled())`.

The structured-output branch (`if args.emit.is_structured()` higher up in
`run_blocking`) is reached before `print_table` and is unchanged.

**TUI path** (`src/commands/list_live.rs::run_live`):

After porcelain parsing produces the initial `worktree_infos` and (if
`-b`/`-r`/`-a` is set) `collect_branch_info` runs synchronously, check whether
the merged set is empty. If so:

1. Skip the streaming collector spawn, the renderer, the SIGINT handler install,
   and the raw-mode guard.
2. Call `list_empty::print(&mut std::io::stdout(), styles::colors_enabled())`.
3. Return `Ok(())`.

This avoids TUI bringup/teardown for a 3-line static message and prevents
flicker.

The synchronous `collect_branch_info` call already exists in `run_live` for
`-b`/`-r`/`-a`; we just check `worktree_infos.is_empty()` after it merges
results. No new I/O, no new git invocations.

#### Tests

**Unit** (`src/commands/list_empty.rs`):

- `render_contains_both_commands_no_color`: assert `render(false)` contains
  `"daft go <branch>"`, `"daft start <branch>"`, and `"No worktrees yet."`.
- `render_contains_color_escapes_when_enabled`: assert `render(true)` contains
  ANSI escape sequences (`\x1b[`).
- `render_alignment`: assert the two suggestion lines are aligned (right-hand
  descriptions start at the same column when stripped of ANSI).

**Unit** (`src/commands/list.rs`):

- Add a smoke test that `print_table` invoked on `&[]` writes non-empty output
  to a captured writer. (Refactor `print_table` to take a `&mut impl Write` for
  testability, or extract the empty-state branch into a helper that's invoked by
  `print_table` and is itself testable.)

**YAML scenarios:**

- `tests/manual/scenarios/list/empty-bare.yml`: clone with `--layout contained`,
  run `daft list`, assert stdout contains `"No worktrees yet."` and both hint
  lines.
- `tests/manual/scenarios/list/empty-format-json.yml`: same setup, run
  `daft list --format json`, assert empty rows array, assert stdout does NOT
  contain `"No worktrees yet."`.
- `tests/manual/scenarios/list/empty-with-branches-flag.yml`: same setup, run
  `daft list -b`, assert empty-state copy is shown (no local branches exist yet
  either).

### Part 2: `daft repo remove` copy refinement

Pure string replacement. No structural changes.

#### `confirm_prompt` suffix (`src/commands/repo/remove.rs:200-204`)

| `n` | Current                                                       | Proposed                                              |
| --- | ------------------------------------------------------------- | ----------------------------------------------------- |
| 0   | `This will delete the bare git dir (no worktrees to remove).` | `No worktrees to remove — this will delete the repo.` |
| 1   | `This will delete 1 worktree and the bare git dir.`           | `This will delete 1 worktree and the repo.`           |
| n   | `This will delete {n} worktrees and the bare git dir.`        | `This will delete {n} worktrees and the repo.`        |

#### `print_plan` (`src/commands/repo/remove.rs:182-193`)

| Element      | Current                       | Proposed                      |
| ------------ | ----------------------------- | ----------------------------- |
| header       | `Would remove:`               | `Would remove:` _(unchanged)_ |
| worktree row | `  worktree  /path  (branch)` | _(unchanged)_                 |
| bare row     | `  bare      /path/to/.git`   | `  git dir   /path/to/.git`   |
| trust row    | `  trust DB entry for /path`  | `  trust marker for /path`    |

Width of the leading column changes from `worktree`/`bare` (max 8 chars) to
`worktree`/`git dir` (max 8 chars) — alignment is preserved. The `trust marker`
row uses the same `for /path` format; no realignment needed.

#### Tests

Update existing scenarios that assert the old strings:

- `tests/manual/scenarios/repo/remove-dry-run.yml`
- `tests/manual/scenarios/repo/remove-basic.yml`

Other scenarios (`remove-from-inside.yml`, `remove-vanilla.yml`,
`remove-non-git-fails.yml`, `remove-force.yml`,
`remove-with-hooks-cwd-outside.yml`, `remove-with-hooks.yml`) need a quick scan
for any string they assert on; update if needed.

No new scenarios needed — pure copy change.

## Architecture

### File map

**New:**

- `src/commands/list_empty.rs` — empty-state rendering helper

**Modified:**

- `src/commands/mod.rs` — register `list_empty` module
- `src/commands/list.rs` — replace `print_table` empty-return with
  `list_empty::print`; refactor `print_table` to take `&mut impl Write` so the
  empty path is unit-testable
- `src/commands/list_live.rs` — short-circuit before TUI bringup when
  worktree+branch sets are both empty
- `src/commands/repo/remove.rs` — string replacements in `confirm_prompt` and
  `print_plan`
- `tests/manual/scenarios/list/empty-bare.yml` (new)
- `tests/manual/scenarios/list/empty-format-json.yml` (new)
- `tests/manual/scenarios/list/empty-with-branches-flag.yml` (new)
- `tests/manual/scenarios/repo/remove-dry-run.yml` (asserts updated)
- `tests/manual/scenarios/repo/remove-basic.yml` (asserts updated, if needed)

### Data flow

`list_empty` is leaf — no dependencies beyond `crate::styles`. Both call sites
(`list.rs`, `list_live.rs`) call `list_empty::print` synchronously after they
determine the merged result is empty. No async, no channels, no streaming.

### Error handling

`list_empty::print` returns `io::Result<()>`. Callers propagate via `?`. Stdout
write failures are vanishingly rare and propagating is consistent with the rest
of `daft list`'s output handling.

## Testing strategy

- Unit tests for `list_empty::render` cover color/no-color, content, and
  alignment.
- Unit smoke test for `print_table` on `&[]` confirms the call site is wired.
- YAML scenarios cover the three real-world empty paths (TTY, JSON, branches
  flag).
- Existing `repo remove` scenarios catch any string drift in the refinement.
- Run `mise run test:unit`, `mise run test:integration`, `mise run clippy`,
  `mise run fmt:check` before committing — required by `CLAUDE.md`.

## Rollout

Single PR, single squash merge. No flag, no migration. The change is purely
cosmetic from the user's perspective:

- Empty `daft list` output goes from "headers only / nothing" to "3-line hint" —
  strictly better UX.
- `repo remove` copy goes from technical to plain — strictly clearer.

No release-notes-worthy behavior change; mention under "fixes" in the next
release.

## Open questions

None.
