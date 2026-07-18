---
title: Checking Out Pull Requests
description:
  Open a GitHub PR or GitLab MR directly into a worktree with daft go pr:123
---

# Checking Out Pull Requests

daft can check out a GitHub pull request or GitLab merge request straight into a
worktree, so reviewing a PR is one command and never disturbs your current work:

```bash
daft go pr:123
```

This resolves PR #123, creates a worktree on its source branch, and drops you
into it. Fork PRs work the same way, with no need to add a remote.

## How authentication works

daft never stores tokens or talks to a forge over HTTP. Every forge lookup
shells out to the [GitHub CLI](https://cli.github.com/) (`gh`) or the
[GitLab CLI](https://gitlab.com/gitlab-org/cli) (`glab`), which supply the
authentication. Log in once:

```bash
gh auth login      # GitHub
glab auth login    # GitLab
```

From then on daft inherits that auth — including SSO, Enterprise, and
self-hosted instances, which the CLIs already handle. Run `daft doctor` to check
whether `gh`/`glab` are installed and authenticated.

## Accepted forms

| Form                                                 | Resolves          |
| ---------------------------------------------------- | ----------------- |
| `daft go pr:123`                                     | GitHub PR #123    |
| `daft go mr:45`                                      | GitLab MR !45     |
| `daft go https://github.com/o/r/pull/123`            | The pasted PR URL |
| `daft go https://gitlab.com/g/r/-/merge_requests/45` | The pasted MR URL |

`pr:` and `mr:` are interchangeable aliases — the platform is detected from the
repository's remote, so `pr:45` on a GitLab repo resolves the merge request.

## What daft configures

- **Same-repo PR/MR** — the source branch is fetched from the base remote and
  checked out as an ordinary tracking branch.
- **Fork PR/MR** — the head is fetched from the base repo's `refs/pull/123/head`
  (GitHub) or `refs/merge-requests/45/head` (GitLab), and the new branch is
  configured so `git pull` updates it from the PR head. `git push` back to a
  contributor's fork is not configured yet.

A closed or merged PR still checks out (with a note) — inspecting merged work is
legitimate.

## Seeing PRs and their CI in `daft list`

In a repository with a GitHub or GitLab remote, `daft list` shows the `pr`
column by default. Two silent rules govern it, so it never nags:

- Repositories with no forge remote (and no `daft.forge.platform` override)
  never show the column.
- If the background refresh fails in a way only you can fix — `gh`/`glab` not
  installed, authentication expired, repository access lost — the column
  disappears from the next `daft list` on, and stays gone. Fix the underlying
  issue (say, `gh auth login`) and the next refresh detects it and restores the
  column, again persistently. `daft doctor` explains a hidden column under
  "Forge integration".

Transient trouble (network down, rate limits) never hides the column. Force it
regardless with `--columns +pr`, or drop it with `--columns -pr` (persist either
with `git config daft.list.columns`).

Two kinds of branches get a value:

- **Worktrees checked out from a PR/MR** show `#123` (GitHub) or `!45` (GitLab),
  read from local git config.
- **Your own branches with an open or merged PR** show the PR opened _from_
  them, matched by branch name against daft's forge-PR cache. An open PR wins
  over a merged one (a reused branch shows its live PR); fork PRs whose head
  branch happens to share a local branch's name never match.

In a color terminal, the number's color is the PR's fate: green/red/yellow for
CI passing/failing/running, purple for merged — the "this worktree is done,
prune it" signal — and dim for closed-without-merge. When color is off
(`NO_COLOR`, piped output), the same states trail the number as a glyph instead:
`#123 ✓`/`✗`/`●` for CI, `#123 ◆` merged, `#123 ○` closed — the signal never
exists as color alone. Where the table is printed as plain text, supporting
terminals also make the number a clickable link to the PR.

The live table never presents a cached status as current. While a background
refresh is in flight, PR numbers render without any fate; the colors arrive the
moment the refresh lands — the table holds its final frame a few seconds for the
verdict when needed. On the very first run in a repository, before any snapshot
exists, the PR cells show a loading skeleton (like the size column) until that
first refresh concludes.

### The forge-PR cache

The PR numbers, titles, states, and CI come from a per-repository snapshot of
the forge's open and recently-merged PR listings, stored locally. It refreshes
in the background — never while you wait:

- checking out a `pr:`/`mr:` reference records that PR immediately;
- `daft update` and `daft sync` kick a detached refresh after they finish (they
  already talked to the remote);
- a `daft list` with the `pr` column in play kicks the same detached refresh —
  and while the live table is open, the fresh data slots into the PR column the
  moment it lands, so even a first-ever run decorates within a couple of
  seconds.

Refreshes are throttled to about one per minute per repository, so back-to-back
lists reuse the just-taken snapshot instead of re-asking the forge. A cold cache
never blocks and never errors — cells simply wait for the refresh. The cache
also powers tab completion: `daft go pr:<Tab>` completes cached PR numbers with
their titles, open PRs first, entirely from local data.

## Mixed-remote repositories

If a repository has both a GitHub and a GitLab remote, tell daft which forge a
bare `pr:`/`mr:` reference means:

```bash
git config daft.forge.platform gitlab
```

A pasted URL is always unambiguous and ignores this setting. See
[Configuration](/reference/configuration#forge-settings) for the full list of
`daft.forge.*` keys, including CLI-binary and hostname overrides for Enterprise
instances.
