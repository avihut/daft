---
title: daft list
description: List all worktrees with status information
---

# daft list

List all worktrees with status information

## Usage

```
daft list [OPTIONS]
```

## Description

Lists all worktrees in the current project with enriched status information
including uncommitted changes, ahead/behind counts vs. both the base branch
and the remote tracking branch, branch age, and last commit details.

Each worktree is shown with:

- A `>` marker for the current worktree
- Branch name, with `✦` for the default branch
- Relative path from the current directory
- Ahead/behind counts vs. the base branch (e.g. +3 -1)
- File status: +N staged, -N unstaged, ?N untracked
- Remote tracking status: ⇡N unpushed, ⇣N unpulled
- Branch age since creation (e.g. 3d, 2w, 5mo)
- Last commit: shorthand age + subject (e.g. 1h fix login bug)
- Owner: author name of the branch's commit range owner, per
  `daft.ownership.strategy` (available via `--columns owner`)

Ages use shorthand notation: `<1m`, `Xm`, `Xh`, `Xd`, `Xw`, `Xmo`, `Xy`.

Use `-b / --branches` to also show local branches without a worktree.
Use `-r / --remotes` to also show remote tracking branches.
Use `-a / --all` to show both (equivalent to `-b -r`).

Non-worktree branches are shown with dimmed styling and blank Path/Changes columns.

Use `--stat lines` to show line-level change counts (insertions and deletions)
instead of the default summary (commit counts for base/remote, file counts for
changes). This is slower as it requires computing diffs for each worktree.

Use `--format` for machine-readable output suitable for scripting. Supported
formats: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`. JSON
output includes fields like `is_default_branch`, `staged`, `unstaged`,
`untracked`, `remote_ahead`, `remote_behind`, `branch_age`, `owner_name`, and
`owner_email`. Use `--template '<tera>'` for custom output.

Use `--columns` to select which columns are shown and in what order.

### Two-section layout

When `git config user.email` is set, the output is split into two sections:

- **Your branches** — branches whose resolved owner email (per the
  `daft.ownership.strategy` setting) matches your `git config user.email`.
- **Other branches** — all remaining branches.

This makes it easy to identify your active work at a glance. The section
divider is only shown when both sections are non-empty.

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `--format <FORMAT>` | Output format: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown` | |
| `--template <STR>` | Tera template string for custom output | |
| `--no-headers` | Omit header row (tsv/csv only) | |
| `-v, --verbose` | Be verbose; show detailed progress | |
| `-b, --branches` | Also show local branches without a worktree | |
| `-r, --remotes` | Also show remote tracking branches | |
| `-a, --all` | Show all branches (equivalent to `-b -r`) | |
| `--stat <STAT>` | Statistics mode: `summary` or `lines` (default: from git config `daft.list.stat`, or `summary`) | |
| `--columns <COLUMNS>` | Columns to display (comma-separated). Replace mode: `branch,path,age`. Modifier mode: `+col,-col` | |
| `--sort <SORT>` | Sort order (comma-separated). `+col` ascending, `-col` descending. Sortable columns: `branch`, `path`, `size`, `age`, `owner`, `activity` (aliases: `commit`, `last-commit`). Default: `daft.list.sort` or `+branch`. | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Examples

```bash
# List all worktrees with status
daft list

# Also show local branches without a worktree
daft list --branches

# Show all branches including remote tracking branches
daft list --all

# Show line-level insertions/deletions instead of commit counts
daft list --stat lines

# Machine-readable JSON output
daft list --format json

# Pipe JSON to jq for filtering
daft list --format json | jq '.[] | select(.unstaged > 0)'

# Show only branch, path, and age columns (replace mode)
daft list --columns branch,path,age

# Remove annotation and last-commit from defaults (modifier mode)
daft list --columns -annotation,-last-commit

# Add the Owner column to the default layout
daft list --columns +owner

# Show branch, path, and owner only
daft list --columns branch,path,owner
```

## Structured Output

`daft list` supports machine-readable output via `--format`: `json`, `ndjson`,
`tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>` for custom
output.

```sh
# Two columns for awk / cut
daft list --format tsv --no-headers | cut -f2,3

# Pipe to jq
daft list --format json | jq '.[] | select(.is_current == true)'

# Custom one-liner per worktree
daft list --template '{% for r in items %}{{ r.name }} -> {{ r.path }}
{% endfor %}'
```

See the [Output Formats guide](../guide/output-formats.md) for format details
and Tera syntax.

## See Also

- [git worktree-list](./git-worktree-list.md) for the underlying git-native command
