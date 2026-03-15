# Column Selection for Worktree Commands

## Overview

Add a `--columns` flag to `git-worktree-list`, `git-worktree-sync`, and
`git-worktree-prune` that controls which columns are displayed and in what
order. Supports both full replacement and additive/subtractive modification of
defaults. Backed by per-command git config.

## Column Registry

A canonical ordered list of all available columns. Each column has a fixed
position, a CLI name, and per-command default membership.

| Position | CLI Name      | `list` default | `sync`/`prune` default    |
| -------- | ------------- | -------------- | ------------------------- |
| 0        | `status`      | not available  | pinned (not controllable) |
| 1        | `annotation`  | yes            | yes                       |
| 2        | `branch`      | yes            | yes                       |
| 3        | `path`        | yes            | yes                       |
| 4        | `base`        | yes            | yes                       |
| 5        | `changes`     | yes            | yes                       |
| 6        | `remote`      | yes            | yes                       |
| 7        | `age`         | yes            | yes                       |
| 8        | `last-commit` | yes            | yes                       |
| 9        | `size`        | no             | no                        |

Future columns register here with their natural position and `default: no`.

### Pinned Columns

The `status` column on `sync` and `prune` is pinned: always shown as the first
column, not controllable via `--columns`. Referencing `status` in `--columns` on
these commands is an error. On `list`, `status` does not exist in the column
registry at all.

## CLI Flag

```
--columns <COLUMNS>    Columns to display (comma-separated)
```

Available on `git-worktree-list`, `git-worktree-sync`, and `git-worktree-prune`.

### Three Modes

**Replace mode** -- no prefixes, exact columns in exact order:

```bash
git worktree list --columns branch,path,age
# Output: Branch | Path | Age
```

**Modifier mode** -- all values prefixed with `+` or `-`, applied to the default
column set. Result follows canonical column ordering, not input order:

```bash
git worktree list --columns +size,-annotation
# Output: Branch | Path | Base | Changes | Remote | Age | Last Commit | Size

git worktree list --columns -annotation,-last-commit,-remote
# Output: Branch | Path | Base | Changes | Age
```

**Mixed mode = error** -- combining prefixed and unprefixed values is rejected:

```bash
git worktree list --columns branch,+size
# Error: cannot mix column names with +/- modifiers
```

### Validation Rules

- Unknown column name: error listing valid names for the command.
- `-` a column not in defaults: silently ignored (idempotent).
- `+` a column already in defaults: silently ignored (idempotent).
- Empty result after removals: error.
- `status` referenced on sync/prune: error ("cannot be controlled on this
  command").
- `status` referenced on list: unknown column error (not in registry).

## Git Config

Per-command config keys using the same syntax as the CLI flag:

```
daft.list.columns = branch,path,age            # replace mode
daft.sync.columns = +size,-annotation           # modifier mode
daft.prune.columns = -last-commit               # modifier mode
```

The `--columns` CLI flag overrides the config value entirely (not layered on
top).

When neither `--columns` nor the config key is set, the hardcoded per-command
defaults are used (identical to today's behavior).

## JSON Output

When `--columns` is specified alongside `--json`, the JSON output only includes
keys corresponding to the selected columns.

| Column        | JSON keys                                                                                      |
| ------------- | ---------------------------------------------------------------------------------------------- |
| `annotation`  | `is_current`, `is_default_branch`                                                              |
| `branch`      | `name`, `kind`                                                                                 |
| `path`        | `path`                                                                                         |
| `base`        | `ahead`, `behind` (+ `base_lines_inserted`, `base_lines_deleted` with `--stat lines`)          |
| `changes`     | `staged`, `unstaged`, `untracked` (+ `staged_lines_*`, `unstaged_lines_*` with `--stat lines`) |
| `remote`      | `remote_ahead`, `remote_behind` (+ `remote_lines_*` with `--stat lines`)                       |
| `age`         | `branch_age`                                                                                   |
| `last-commit` | `last_commit_age`, `last_commit_subject`                                                       |
| `status`      | `status` (sync/prune only)                                                                     |
| `size`        | `size` (future)                                                                                |

When no `--columns` is specified, JSON output includes all keys (backward
compatible with today's behavior).

## Shell Completions

The `--columns` flag needs completion support across all shells:

- **Bash/Zsh/Fish**: Complete column names after `--columns`. Include plain
  names (`branch`, `path`) and prefixed variants (`+branch`, `-branch`). Support
  comma-aware completion (complete after each comma).
- **Fig/Amazon Q**: Same column set, generated from the registry.

Available names vary by command: sync/prune exclude `status` (pinned), list
excludes `status` (not available).

## Interaction with Existing Features

### `--stat lines`

Orthogonal. `--stat` controls column _content_ (commit counts vs. line counts).
`--columns` controls column _visibility and order_. Both can be combined freely.

### `-b`/`-r`/`-a` (branch/remote/all)

Orthogonal. These control which _rows_ appear. `--columns` controls _columns_.

### Terminal Width Truncation (list)

The table truncates from the right when the terminal is narrow. With
`--columns`, the same truncation applies to the user's selected columns.

### TUI Responsive Column Hiding (sync/prune)

The TUI drops low-priority columns on narrow terminals. With explicit column
selection (replace mode), the user's choices are respected -- the TUI only
truncates at terminal edge. In modifier mode, the TUI can still responsively
drop non-pinned columns since the user didn't explicitly choose them.

## Error Messages

**Unknown column:**

```
error: unknown column 'foo'
  valid columns: annotation, branch, path, base, changes, remote, age, last-commit
```

**Mixed mode:**

```
error: cannot mix column names with +/- modifiers
  use either replace mode:   --columns branch,path,age
  or modifier mode:          --columns +size,-annotation
```

**Empty result after removals:**

```
error: no columns remaining after applying modifiers
  modifiers: -branch,-path,-base,-changes,-remote,-age,-last-commit,-annotation
```

**`status` on sync/prune:**

```
error: 'status' column cannot be controlled on this command
  it is always shown as the first column
```

**`status` on list:**

```
error: unknown column 'status'
  valid columns: annotation, branch, path, base, changes, remote, age, last-commit
```
