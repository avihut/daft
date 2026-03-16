# Branch Ownership & Local Branch Sync

## Problem

The prune mechanism (reused by sync) deletes both worktrees and local-only
branches whose remote tracking branches have been deleted. However, the TUI
table only shows worktrees — local-only branches are pruned silently with no
visibility.

Additionally, sync's `--rebase` and `--push` flags apply to all worktrees
unconditionally. This is overly permissive: users may unintentionally rebase or
push branches they don't own, especially in shared repositories. Extending
rebase/push to local-only branches (which this design also enables) makes that
over-permissiveness impossible to ignore.

## Design

### 1. Owner Column

A new **Owner** column is added to all list interfaces (list, sync, prune).

- **Value:** The email of the author of the branch tip commit, obtained via
  `git log -1 --format='%ae' <branch>`.
- **Ownership comparison:** The branch tip author email is compared against
  `git config user.email` to determine whether a branch "belongs to" the current
  user. This matches GitHub's heuristic for the "Your branches" feature in the
  branch dropdown — a commonly understood industry convention.
- **Column position:** Between Age and Last Commit (canonical position 8; Last
  Commit shifts to position 9).
- **CLI name:** `owner`.
- **Added to both column systems:**
  - `ListColumn::Owner` in `core/columns.rs` — user-facing, supports `--columns`
    selection/modifier modes.
  - `Column::Owner` in `output/tui/columns.rs` — TUI-specific, with dynamic
    sizing and priority-based dropping.
- **Included in defaults** for all three commands (list, sync, prune).
- **Non-worktree rows** receive dimmed styling, consistent with other columns.

### 2. Local Branches in Prune/Sync Table

Currently, `collect_worktree_info` (used by sync/prune TUI) only collects actual
worktrees from `git worktree list --porcelain`. Local-only branches (those with
`EntryKind::LocalBranch`) are never included.

**Change:** After fetch identifies gone branches, any that are local-only (no
associated worktree) are added as rows in the TUI table:

- They use `EntryKind::LocalBranch` (already exists in the data model).
- The path column is blank.
- They flow through the same DAG as worktree prune tasks:
  `TaskId::Prune(branch_name)`, with final status "Pruned".
- The owner column shows the branch tip email like any other row.

The existing `TaskStarted` handler in `TuiState::apply_event` already
auto-creates rows for dynamically discovered branches via `WorktreeInfo::empty`.
The change ensures local-only gone branches are seeded as initial rows with
proper `EntryKind` metadata rather than appearing as empty placeholders.

### 3. Ownership-Gated Rebase & Push

**Default behavior change:** Rebase and push only apply to branches where the
tip author email matches `git config user.email`.

- **Update phase:** Always applies to all branches. Pulling upstream changes
  (fast-forward or merge) is non-destructive and safe regardless of ownership.
- **Rebase phase:** Only applies to owned branches (plus any explicitly included
  via `--include`). Unowned branches show their update status but receive no
  rebase task.
- **Push phase:** Same ownership gate as rebase. Unowned branches are not
  pushed.
- **Prune:** Unaffected by ownership — always prunes all gone branches. Prune
  reflects the state of the remote, not a user's intent.
- **DAG integration:** The ownership check happens at DAG build time. Unowned
  branches (unless included) do not receive `Rebase` or `Push` task nodes. This
  is cleaner than creating tasks that immediately skip.

### 4. The `--include` Flag

A parameterized, repeatable flag that moves branches from "update only" into the
full sync pipeline (rebase + push).

**Accepted values:**

| Value               | Effect                                   |
| ------------------- | ---------------------------------------- |
| `unowned`           | Include all branches regardless of owner |
| `alice@example.com` | Include branches owned by this email     |
| `feat/login`        | Include this specific branch by name     |

**Syntax:** Repeatable flag. Multiple values use multiple flags:
`--include alice@example.com --include feat/x`.

**Resolution:** If the value contains `@`, treat as an email. If it equals
`unowned`, treat as the special keyword. Otherwise, treat as a branch name.

**Effect:** Matched branches move from the bottom (update-only) section to the
top (full sync) section and receive rebase/push tasks.

### 5. Two-Section Table Layout

The TUI table (sync/prune) and CLI table (list) split into two sections with a
visual separator:

**Top section — "Full sync":** Branches that receive the full operation
pipeline. In sync: your branches (and any `--include`d branches) that get
update + rebase + push. In prune: all gone branches (prune ignores ownership).

**Bottom section — "Update only":** Unowned branches shown for awareness. They
receive updates (fast-forward/pull) but no rebase or push. Each row shows its
update status normally (Updated, Up to date, Diverged) and progresses no
further.

**Separator:** A dim horizontal divider label between sections (e.g.,
`── other branches ──`). Exact label TBD during implementation.

**With `--include unowned`:** Both sections merge — no divider, single unified
list. The bottom section disappears entirely.

### 6. Entry Ordering

All list presentations (list, sync, prune) order entries with worktrees first,
then branches:

1. **Worktrees** (`EntryKind::Worktree`) — sorted alphabetically
   (case-insensitive)
2. **Local branches** (`EntryKind::LocalBranch`) — sorted alphabetically
3. **Remote branches** (`EntryKind::RemoteBranch`, list only with `-r`/`-a`) —
   sorted alphabetically

This ordering applies within each section of the two-section layout. The top
"full sync" section shows owned worktrees first, then owned local branches. The
bottom "update only" section follows the same pattern for unowned entries.

No visual separator between entry kind groups within a section — ordering alone
provides the grouping. The only visual separator is between the two ownership
sections.

### 7. Temp Worktrees for Local Branch Rebase

Local-only branches (no persistent worktree) need a working tree for rebase.

**Update phase:** No temp worktree needed.

- Fast-forward: `git branch -f <branch> <upstream>`.
- Diverged: skip, show "Diverged" status.

**Rebase phase (if `--rebase` and branch is owned/included):**

1. Fast-forward the local ref first (as above).
2. Create temp worktree: `git worktree add <tmp-path> <branch>` in
   `<bare-root>/.daft-tmp/<branch-name>/`.
3. Rebase: `git -C <tmp-path> rebase <base-branch>`.
4. Remove temp worktree: `git worktree remove <tmp-path>`.
5. On conflict: abort rebase (`git rebase --abort`), remove temp worktree, show
   "Conflict" — same behavior as regular worktrees.

**Push phase:** `git push origin <branch>` — no worktree needed.

**Daft hooks:** Skipped on temp worktrees. This is a scoping decision — the full
temp worktree lifecycle (with hooks) is a separate future feature. Git hooks
(pre-push, pre-commit, etc.) fire naturally from the shared hooks directory
since all worktrees in a bare repo share the same git hooks.

**Temp worktree cleanup — belt and suspenders:**

1. **Happy path:** Remove temp worktree immediately after rebase completes
   (success or conflict).
2. **Panic/crash guard:** Register a cleanup handler (Drop guard / scopeguard)
   at the start of the operation that removes all `.daft-tmp/` contents on any
   exit path.
3. **Stale cleanup on next run:** At the start of every sync/prune invocation,
   scan for and remove any leftover `.daft-tmp/` worktrees before doing anything
   else. This catches crashes so severe that even the guard didn't fire
   (SIGKILL, power loss).
4. **SIGINT/SIGTERM handling:** Existing signal handling triggers the cleanup
   guard before exiting.

The guarantee: even if sync crashes catastrophically, the next sync run cleans
up before proceeding. No orphaned temp worktrees survive across two invocations.

## Scope Boundaries

**In scope:**

- Owner column in list/sync/prune
- Local-only branches visible in prune/sync table
- Ownership-gated rebase/push with `--include` override
- Two-section table layout with separator
- Worktrees-first ordering in all list presentations
- Minimal temp worktree for local branch rebase (no daft hooks)
- Aggressive temp worktree cleanup

**Out of scope (future work):**

- Full temp worktree system as a user-facing feature (e.g.,
  `daft run feat/x -- make test`)
- Daft lifecycle hooks on temp worktrees
- Per-branch ownership tracking in daft metadata
- `daft checkout` integration with ownership
