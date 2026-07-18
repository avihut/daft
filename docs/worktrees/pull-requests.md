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

Add the `pr` column to `daft list`:

```bash
daft list --columns +pr
```

Two kinds of branches get a value:

- **Worktrees checked out from a PR/MR** show `#123` (GitHub) or `!45` (GitLab),
  read from local git config.
- **Your own branches with an open PR** show the PR opened _from_ them, matched
  by branch name against daft's forge-PR cache. Fork PRs whose head branch
  happens to share a local branch's name never match.

When the cache knows the PR's CI state, a glyph follows the number — `#123 ✓`
(passing, green), `#123 ✗` (failing, red), `#123 ●` (running, yellow). The glyph
carries the meaning; color only reinforces it, so piped and `NO_COLOR` output
stay readable. Persist the column with `git config daft.list.columns +pr`.

### The forge-PR cache

The PR numbers, titles, and CI states come from a per-repository snapshot of the
forge's open-PR listing, stored locally. It refreshes in the background — never
while you wait:

- checking out a `pr:`/`mr:` reference records that PR immediately;
- `daft update` and `daft sync` kick a detached refresh after they finish (they
  already talked to the remote);
- selecting the `pr` column in `daft list` kicks the same detached refresh, so
  the next listing is fresh.

A cold cache simply means undecorated cells — nothing blocks, nothing errors.
The cache also powers tab completion: `daft go pr:<Tab>` completes open PR
numbers with their titles, entirely from local data.

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
