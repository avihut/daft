---
title: git-worktree-list
description: List all worktrees with status information
---

# git worktree-list

List all worktrees with status information

::: tip
This command is also available as `daft list`. See [daft list](./daft-list.md).
:::

## Description

Lists all worktrees in the current project with enriched status information
including uncommitted changes, ahead/behind counts vs. both the base branch
and the remote tracking branch, branch age, and last commit details.

Give a cataloged repository as the positional argument to list that
repository's worktrees from anywhere (sugar for `--repo`; the name must be
in the repo catalog). Use --all-repos to sweep every cataloged repository.

Each worktree is shown with:
  - A `>` marker for the current worktree
  - Branch name, with `✦` for the default branch
  - Relative path from the current directory
  - Ahead/behind counts vs. the base branch (e.g. +3 -1)
  - File status: +N staged, -N unstaged, ?N untracked
  - Remote tracking status: ⇡N unpushed, ⇣N unpulled
  - Branch age since creation (e.g. 3d, 2w, 5mo)
  - Last commit: shorthand age + subject (e.g. 1h fix login bug)

Ages use shorthand notation: <1m, Xm, Xh, Xd, Xw, Xmo, Xy.

Use -b / --branches to also show local branches without a worktree.
Use -r / --remotes to also show remote tracking branches.
Use -a / --all to show both (equivalent to -b -r).

Non-worktree branches are shown with dimmed styling and blank Path/Changes columns.

A worktree keeps its branch name even while git has detached its HEAD to run
an operation: mid-rebase the row still reads `feat/x`, sorts in place, and
keeps its Base/Age/Owner cells. The annotation column gains a glyph for the
paused operation, and unresolved conflicts show as a red `!N` under Changes.
Add the `status` column (--columns +status) to spell the state out, e.g.
"rebasing · 2 conflicts" or "rebasing · resolved" when everything is resolved
and the operation is only waiting to be continued.

Only a detached checkout that no operation explains is treated as a scratch
sandbox. Where daft knows what branch a worktree was made for, even that keeps
its name, shown alongside the checked-out commit. A worktree whose checkout
disagrees with that record is flagged as drifted; `daft doctor --fix`
reconciles the record.

Use --stat lines to show line-level change counts (insertions and deletions)
instead of the default summary (commit counts for base/remote, file counts for
changes). This is slower as it requires computing diffs for each worktree.

Use --format to emit machine-readable output suitable for scripting.
Supported formats: json, ndjson, tsv, csv, yaml, toon, markdown. Use
--template '<tera>' for custom output. See the Structured Output guide
for details.

Use --columns to select which columns are shown and in what order.
  Replace mode:  --columns branch,path,age (exact set and order)
  Modifier mode: --columns -annotation,-last-commit (remove from defaults)
  Add optional:  --columns +size (add disk size column after path)
Defaults can be set in git config with daft.list.columns.

The size column is not shown by default. Add it with --columns +size to see the
disk size of each worktree folder in human-readable format (e.g. 42K, 1.3M, 2.5G).
A summary row at the bottom shows the total size across all worktrees.

The pr column shows the pull/merge request each row relates to (#123 for a
GitHub PR, !45 for a GitLab MR). It is on by default in repositories with a
GitHub or GitLab remote and disappears silently — persisting across runs —
when the forge integration is broken in a way that needs your intervention
(gh/glab missing or unauthenticated); it returns automatically once a
background refresh succeeds again. Repositories with no forge remote never
show it. Add --columns +pr to force the column regardless, or -pr to drop it.

While the pr column is shown, every open PR in the repository gets a row, not
just the ones your worktrees represent: a local branch with an open PR is
listed without --branches, and a PR with no local presence at all (a
colleague's branch, any fork PR) appears as a dimmed row built from the forge
data — fork PRs render owner:branch. Merged and closed PRs decorate existing
rows but never add one. Rows with a PR show the PR author in the Owner
column. The open-PR rows and the pr column are one unit: --columns -pr (or
the silent gate above) removes both, so prefer just your worktrees per-repo
with `git config -- daft.list.columns -pr`.

Use --sort to control the sort order. Prefix with + for ascending (default) or
- for descending. Multiple columns can be comma-separated for multi-level sort.
  Sort by branch descending:  --sort -branch
  Sort by owner then size:    --sort +owner,-size
  Most recent activity first: --sort -activity

Sortable columns: branch, path, size, age, owner, hash, activity, commit (alias:
last-commit). activity considers both commits and uncommitted file changes;
commit sorts by last commit time only. You can sort by columns not shown in
the output (e.g. --sort -size without --columns +size). Defaults can be set
with daft.list.sort.

## Usage

```
git worktree-list [OPTIONS] [REPO]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPO>` | Cataloged repository to list (same as --repo) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-b, --branches` | Also show local branches without a worktree |  |
| `-r, --remotes` | Also show remote tracking branches |  |
| `-a, --all` | Show all branches (equivalent to -b -r) |  |
| `--merging` | Only show worktrees with an in-progress merge |  |
| `--stat <STAT>` | Statistics mode: summary or lines (default: from git config daft.list.stat, or summary) |  |
| `--columns <COLUMNS>` | Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col. Available: branch, path, size, base, changes, remote, pr, age, annotation, status, owner, hash, last-commit |  |
| `--sort <SORT>` | Sort order (comma-separated). +col ascending, -col descending. Columns: branch, path, size, base, changes, remote, age, owner, hash, activity, commit |  |
| `--repo <REPO>` | List another cataloged repository's worktrees |  |
| `--all-repos` | List every cataloged repository's worktrees |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Structured Output

`git worktree-list` supports machine-readable output via `--format`: `json`,
`ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>`
for custom output.

```sh
# Two columns for awk / cut
daft list --format tsv --no-headers | cut -f2,5

# Pipe to jq
daft list --format json | jq '.[] | select(.is_current == true)'

# Custom template
daft list --template '{% for r in items %}{{ r.name }} -> {{ r.path }}
{% endfor %}'
```

See the [Output Formats guide](/reference/output-formats) for format details
and Tera syntax.

## See Also

- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-branch](./git-worktree-branch.md)

