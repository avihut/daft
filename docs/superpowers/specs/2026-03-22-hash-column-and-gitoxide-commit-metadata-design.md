# Hash Column and Gitoxide Commit Metadata

## Overview

Add a `hash` column to list, prune, and sync commands showing the abbreviated
(7-char) commit hash of each worktree's HEAD. The column is optional (not in
defaults), added via `--columns +hash`, and sortable. Additionally, introduce a
gitoxide fast path for all commit metadata retrieval (timestamp, hash, subject),
replacing subprocess calls when `daft.experimental.gitoxide` is enabled.

## Data Layer

### New field on `WorktreeInfo`

Add `pub last_commit_hash: Option<String>` to the `WorktreeInfo` struct in
`src/core/worktree/list.rs`. Stores the 7-character abbreviated SHA of the
worktree's HEAD commit. `None` when unavailable (consistent with other optional
fields like `last_commit_timestamp` and `owner_email`).

### Subprocess path changes

Modify `get_last_commit_info` and `get_last_commit_info_for_ref` to include the
abbreviated hash:

- Format string changes from `%ct\x1f%s` to `%ct\x1f%h\x1f%s`
- Return type changes from `(Option<i64>, String)` to
  `(Option<i64>, String, String)` — `(timestamp, hash, subject)`
- All call sites updated to destructure the new 3-tuple

The `refresh_dynamic_fields` method also calls `get_last_commit_info` and will
populate the hash field.

### Gitoxide fast path

New functions in `src/git/oxide.rs`:

- `get_commit_metadata_for_head(repo, worktree_path)` — opens repo at worktree
  path, reads HEAD commit, returns `(timestamp, short_hash, subject)`
- `get_commit_metadata_for_ref(repo, ref_name)` — resolves a named ref to its
  commit, returns the same tuple

Implementation uses `gix` to:

- Resolve ref to commit object
- Read `commit.time()` for the Unix timestamp
- Read `commit.id().to_hex()[..7]` for the abbreviated hash
- Read first line of `commit.message_raw_sloppy()` for the subject

### Integration via `&GitCommand`

The `collect_worktree_info` and `collect_branch_info` functions already receive
`&GitCommand`. Add `&GitCommand` to `refresh_dynamic_fields` as well. The commit
metadata helpers check `git.use_gitoxide`:

- When enabled: call the gitoxide functions via `git.gix_repo()`
- When disabled (or on gitoxide failure): fall back to the subprocess path

Fallback is silent — if gitoxide fails (e.g., shallow clone edge case), the
subprocess path runs without user-visible error.

## Column System

### `ListColumn` enum (`src/core/columns.rs`)

Add `Hash` variant between `Owner` and `LastCommit`:

| Position | CLI Name      | Default |
| -------- | ------------- | ------- |
| 1        | `annotation`  | yes     |
| 2        | `branch`      | yes     |
| 3        | `path`        | yes     |
| 4        | `size`        | no      |
| 5        | `base`        | yes     |
| 6        | `changes`     | yes     |
| 7        | `remote`      | yes     |
| 8        | `age`         | yes     |
| 9        | `owner`       | yes     |
| **10**   | **`hash`**    | **no**  |
| 11       | `last-commit` | yes     |

- `list_defaults()` and `tui_defaults()` exclude Hash (like Size)
- `all()` includes Hash in canonical order
- `FromStr`: `"hash"` maps to `ListColumn::Hash`
- `cli_name`: `ListColumn::Hash` maps to `"hash"`

### TUI `Column` enum (`src/output/tui/columns.rs`)

Add `Hash` variant:

- Priority: 10 (LastCommit becomes 11)
- Header label: `"Hash"`
- `from_list_column` / `to_list_column` mappings added
- `ALL_COLUMNS` updated to include Hash before LastCommit
- `column_content_width`: Hash is always 7 chars wide

### `ColumnValues` struct (`src/output/format.rs`)

Add `pub hash: String` field.

### `SortColumn` enum (`src/core/sort.rs`)

Add `Hash` variant:

- CLI name: `"hash"`
- `to_list_column()` maps to `ListColumn::Hash`
- Display name: `"Hash"`
- Comparison: lexicographic on the hash string

Update `valid_names()` to include `hash`.

## Rendering

### CLI table (`src/commands/list.rs`)

- `ListColumn::Hash` maps to `column_values.hash` in the column-to-data switch
- No special formatting — raw 7-char hex string
- No summary footer (unlike Size, nothing to aggregate)

### TUI table (`src/output/tui/render.rs`)

- `Column::Hash` maps to `column_values.hash` in cell rendering
- Responsive dropping works via the priority system (drops before LastCommit)
- Plain text styling, no special treatment

### Shell completions (`src/commands/completions/`)

Add `hash` to:

- Column completions in bash, zsh, fish, and fig specs
- Sort completions in the same files

### Man pages

Regenerate with `mise run man:gen` after updating command help text.

## Testing

### Unit tests (`src/core/columns.rs`)

- `test_defaults_exclude_hash` — Hash not in `list_defaults()` or
  `tui_defaults()`, present in `all()`
- `test_modifier_add_hash` — `+hash` inserts Hash after Owner and before
  LastCommit
- `test_hash_cli_name_roundtrip` — `"hash"` parses to `ListColumn::Hash` and
  back

### Unit tests (`src/core/sort.rs`)

- `test_hash_sort_parse` — `"hash"` parses to `SortColumn::Hash`
- Hash sort comparison works lexicographically

### Integration tests (`tests/integration/`)

- Test `--columns +hash` produces a 7-char hex string in output
- Test `--columns hash,branch` (replace mode) works standalone

### YAML manual test scenarios (`tests/manual/scenarios/list/`)

- Scenario for `+hash` showing the hash column
- Scenario combining `--columns +hash --sort hash`

### Gitoxide path

- Verify subprocess and gitoxide paths produce identical output for the same
  repo state

## Documentation

- Update `--columns` help text in list, prune, and sync `Args` structs to
  mention `hash`
- Update docs site command reference pages (`docs/cli/`)
- Update `SKILL.md` if the changes affect agent interaction patterns
