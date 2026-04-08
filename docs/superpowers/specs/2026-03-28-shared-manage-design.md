# Shared Manage TUI — Design Spec

## Goal

A full-featured interactive TUI for managing shared files across worktrees,
invoked via `daft shared manage`. Provides a one-stop interface for viewing
status, toggling materialization, collecting uncollected files, adding new
shared files, removing files from sharing, and comparing worktree copies — with
full parity to the `daft shared` CLI subcommands.

## Architecture

### Trait-Based TUI Abstraction

The existing collect picker TUI and the new manager share the same visual shell:
tabbed file navigation, split worktree list + preview panel, footer with help
legend. What differs is business logic — what happens on user actions and how
state is populated.

A `PickerMode` trait (or similar) defines the contract between the TUI shell and
the business logic:

- **Populate tabs** — what files/entries appear (uncollected files vs all shared
  files)
- **On action** — what happens when the user presses `m`, `d`, `r`, etc. (batch
  a decision vs execute immediately)
- **Footer content** — what buttons/status to show (Submit/Cancel vs just
  keybinding hints)
- **Available actions** — which keybindings are active in this mode

The TUI shell handles rendering, navigation, keyboard event routing, preview
display, and the help legend. It delegates action semantics to the mode
implementation.

**Two mode implementations:**

1. **CollectMode** — the current sync picker behavior. Batch/prepare-and-submit
   mode. Produces `Vec<CollectDecision>` on submit. Used by `daft shared sync`.
2. **ManageMode** — the new manager. Immediate mode. Each action executes
   against the filesystem and updates state in place. Used by
   `daft shared manage`.

### File Organization

Reusable TUI components move from `src/output/tui/collect_picker/` to a shared
location (e.g., `src/output/tui/shared_picker/`). The collect picker and manager
each become thin wrappers that configure the shell with their mode.

## Entry Point

`daft shared manage` — new subcommand added to the `SharedCommand` enum.
Launches the TUI in manage mode. Requires an interactive terminal (stderr is
TTY); errors with a message if not.

## Layout

Same as the collect picker:

```
┌─────────────────────────────────────────────────────┐
│ Tab Bar (one tab per shared file)                   │
├─────────────────────────────────────────────────────┤
│ Warning/Info Bar (optional, 0-1 lines)              │
├──────────────────┬──────────────────────────────────┤
│ Worktree List    │ Preview Panel                    │
│ (30%)            │ (70%)                            │
│                  │                                  │
│ ▸ main    linked │ DB_HOST=localhost                 │
│   develop mat.   │ DB_PORT=5432                     │
│   feature linked │ SECRET=abc123                    │
│   hotfix  miss.  │                                  │
├──────────────────┴──────────────────────────────────┤
│ Footer (help legend)                                │
└─────────────────────────────────────────────────────┘
```

Navigation is identical to the collect picker: `jk/↑↓` for worktree list or
preview scroll, `hl/←→` for tab switching, `Tab` to toggle panels, `PgUp/PgDn`
for preview scrolling, scrollbar when preview is focused.

## Worktree States

Every worktree shows one of six states as a colored tag:

| State                 | Color      | Meaning                                          |
| --------------------- | ---------- | ------------------------------------------------ |
| **linked**            | green      | Symlink pointing to shared version               |
| **materialized**      | yellow     | Local copy, tracked in materialized.json         |
| **missing**           | dim        | No file or symlink present                       |
| **conflict**          | red        | Real file exists but not tracked as materialized |
| **broken**            | yellow     | Symlink exists but points to wrong target        |
| **not yet collected** | dim/italic | Declared in daft.yml but not in shared storage   |

## Actions

All actions execute immediately in manage mode.

### `m` — Toggle Materialized/Linked

Available on any worktree entry in the list.

- **linked → materialized**: copies the shared file into the worktree as a real
  file, marks as materialized in `materialized.json`.
- **materialized → linked**: deletes the local copy, creates a symlink to shared
  storage, removes from `materialized.json`.
- **missing**: materializes (copies shared file in) or links (creates symlink) —
  `m` materializes since it's the more explicit action.
- **conflict/broken**: no-op for `m` — use `i` to fix first.

After execution, the worktree state tag updates in place.

### `i` — Link (Fix/Create Symlink)

Available on worktree entries that need a symlink created or fixed.

- **missing**: creates symlink to shared version.
- **conflict**: replaces the real file with a symlink (with a confirmation
  prompt since this is destructive — the local file is deleted).
- **broken**: fixes the symlink to point to the correct target.
- **linked**: no-op (already correct).
- **materialized**: no-op (use `m` to toggle).

### `d` — Diff Mode

Toggles diff mode on/off.

1. Press `d` on a worktree entry — that entry's file becomes the **pivot**.
2. The preview panel switches to a colored diff view showing the delta between
   the pivot and whichever worktree the cursor is on.
3. Navigate the worktree list — the diff updates as the cursor moves.
4. Press `d` again or `Esc` to exit diff mode.

The diff is computed using a crate like `similar` for line-level diffing. Output
uses standard diff coloring: green for additions, red for deletions, dim for
context lines. For worktrees where the file is missing or is a symlink to the
same shared version, the diff shows "(identical)" or "(no file)".

The pivot can be any worktree — linked (shared version), materialized (local
copy), or even one with a conflict. This lets you compare any two copies. When
the cursor is on the pivot worktree itself, the preview shows "(pivot — select
another worktree to compare)" since a self-diff is empty.

### `r` / `Del` / `Backspace` — Remove from Sharing

Opens a confirmation modal:

```
┌─ Remove .env ─────────────────────────────┐
│                                            │
│ ▸ Materialize in all worktrees          →  │
│   Delete everywhere                        │
│                                            │
│ Enter confirm  Esc cancel                  │
└────────────────────────────────────────────┘
```

- **Navigate** with `jk/↑↓` between the two options.
- **Materialize option** can be expanded with `→` or `l` to show a per-worktree
  checklist:

```
┌─ Remove .env ─────────────────────────────┐
│                                            │
│ ▾ Materialize in worktrees:                │
│     [✓] main                               │
│     [✓] develop                            │
│     [ ] feature                            │
│     [✓] hotfix                             │
│   Delete everywhere                        │
│                                            │
│ Space toggle  ←/h collapse  Enter confirm  │
└────────────────────────────────────────────┘
```

- **Toggle** individual worktrees with `Space`.
- **Collapse** back with `←` or `h`.
- **Confirm** with `Enter` — executes removal:
  - Worktrees with checkmark get a materialized copy.
  - Worktrees without checkmark lose the file entirely.
  - Shared storage is deleted.
  - `daft.yml` and `materialized.json` are updated.
- **Delete everywhere** — removes the file from all worktrees and shared
  storage. Shows a confirmation prompt ("This will delete .env from all
  worktrees. Are you sure?") before executing.
- **Cancel** with `Esc`.

After removal, the tab is removed from the tab bar and focus moves to the
adjacent tab (or the manager shows "No shared files" if none remain).

### `a` — Add New File

Opens a centered overlay modal with a file tree browser rooted at the current
worktree.

```
┌─ Add Shared File ────────────────────────┐
│                                           │
│ Search: .env▌                             │
│                                           │
│ ▾ ./                                      │
│   ▸ src/                                  │
│   ▸ tests/                                │
│     .env              ◀ highlighted       │
│     .env.local                            │
│     .env.example                          │
│   ▸ .idea/                                │
│     Cargo.toml                            │
│                                           │
│ ↑↓ navigate  →/l expand  ←/h collapse    │
│ Enter select  Esc cancel                  │
└───────────────────────────────────────────┘
```

**File tree behavior:**

- Rooted at the current worktree directory.
- Directories can be expanded (`→`/`l`) and collapsed (`←`/`h`).
- `jk/↑↓` navigate the visible entries.
- The search bar at the top filters entries interactively. Any alphanumeric or
  punctuation keypress goes to the search bar (the bar always has input focus
  for character keys). `jk/↑↓` always control tree navigation regardless of
  search content. `Backspace` deletes from the search. This means `j` and `k`
  cannot be typed into search — this is acceptable since file paths rarely
  contain lone `j`/`k`.
- Git-ignored files and directories are shown (since shared files are typically
  untracked — `.env`, `.idea`, etc.).
- Already-shared files are shown but dimmed/marked.

**No-results behavior:**

When the search matches no existing files, the modal shows:

```
│ Search: .secrets.yml▌                     │
│                                           │
│ No matching files found                   │
│ Press Enter to declare .secrets.yml       │
│ as a new shared file                      │
└───────────────────────────────────────────┘
```

Pressing `Enter` on no results declares the file (equivalent to
`daft shared add --declare`).

**On selection:**

Selecting an existing file triggers `daft shared add` logic — the file is moved
to shared storage, symlinked, added to `daft.yml` and `.gitignore`. A new tab
appears in the manager for the newly shared file. If the file exists in multiple
worktrees, the collect flow (same as sync picker) activates for that single file
before returning to the manager.

### `Space`/`Enter` — Collect Uncollected File

Available when focused on a worktree entry whose file is in "not yet collected"
state (declared in `daft.yml` but not in shared storage). Only `Space` and
`Enter` trigger collection — `m` does not, since `m` is semantically "toggle
materialized/linked" which requires the file to already be collected.

**Worktree has the file**: pressing `Space`/`Enter` collects using that worktree
as the source. Materialization defaults are computed automatically (identical
copies in other worktrees → linked, different copies → materialized, missing →
linked). The collection executes immediately and the tab refreshes with updated
states.

**Worktree does not have the file**: shows an info message —
`"<name>: no copy of <file> — select a worktree that has it"`.

This avoids launching a separate picker — the manage TUI already shows all
worktrees, so the user just navigates to the one with the file and presses
`Enter`.

## Immediate Mode Execution

Each action that modifies state:

1. Executes the filesystem operation (materialize, link, remove, add).
2. Updates `materialized.json` and/or `daft.yml` as needed.
3. Refreshes the affected tab's worktree states by re-scanning.
4. Renders the updated state immediately.

Error handling: if an action fails (e.g., permission denied), show a temporary
error message in the info bar (between tabs and body) for a few seconds, then
clear it. The state is not corrupted — the re-scan will show the actual
filesystem state.

## Non-Interactive Fallback

If stderr is not a TTY or `DAFT_TESTING` is set, `daft shared manage` prints an
error: "The manage interface requires an interactive terminal. Use
`daft shared status` for non-interactive output." and exits with code 1.

## Syntax Highlighting and .env Detection

Reuses the existing `Highlighter` from the collect picker, including the `.env`
file detection for shell syntax highlighting. The diff mode uses its own
rendering (colored diff output) instead of the highlighter.

## Diff Crate

Use `similar` (https://crates.io/crates/similar) for line-level text diffing. It
provides `TextDiff` with `unified_diff` output and change iteration. For
directory diffs, list the files that differ and show a summary rather than
attempting to diff binary or nested content.
