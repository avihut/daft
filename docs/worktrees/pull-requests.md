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

## Seeing which worktrees track a PR

Add the `pr` column to `daft list`:

```bash
daft list --columns +pr
```

Worktrees checked out from a PR/MR show `#123` (GitHub) or `!45` (GitLab). The
value comes from local git config, so it needs no network. Persist the column
with `git config daft.list.columns +pr`.

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
