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

## Seeing every open PR in `daft list`

In a repository with a GitHub or GitLab remote, `daft list` shows the repo's
working state, not just your worktrees: the `pr` column is on by default, and
**every open PR gets a row**. Concretely:

- A worktree whose branch has an open PR shows it in place — `#123` (GitHub) or
  `!45` (GitLab) — no extra row.
- A local branch with an open PR is listed even without `--branches`.
- An open PR with no local presence at all — a colleague's branch, any fork PR —
  appears as a dimmed row built from the forge data: the PR title where a commit
  subject would be, the PR's last activity as its age. Fork PRs render
  `owner:branch` (GitHub's own notation), because per-fork branch names collide
  — two contributors' `patch-1`s, a fork's `main` versus yours.
- Merged and closed PRs decorate existing rows (the purple "this branch is done"
  signal) but never add one.
- Wherever a row has a PR, the Owner column shows the **PR author** rather than
  an owner deduced from commit history — the forge's answer is canonical, and
  for foreign rows there is no local history to deduce from.

The open-PR rows and the `pr` column are one unit, governed by the same silent
rules, so the list never nags:

- Repositories with no forge remote (and no `daft.forge.platform` override)
  never show either.
- If the background refresh fails in a way only you can fix — `gh`/`glab` not
  installed, authentication expired, repository access lost — column and rows
  disappear from the next `daft list` on, and stay gone. Fix the underlying
  issue (say, `gh auth login`) and the next refresh detects it and restores
  them, again persistently. `daft doctor` explains a hidden column under "Forge
  integration".

Transient trouble (network down, rate limits) never hides anything. Force the
forge overlay regardless with `--columns +pr`, or remove it — rows and column
together — with `--columns -pr`. To prefer just your local worktrees permanently
in a repository (say, a large open-source project with hundreds of open PRs),
persist the opt-out there:

```bash
git config -- daft.list.columns -pr
```

`--branches` remains a superset — it shows _all_ local branches, PR-bearing or
not — and structured output marks the foreign-PR rows with `"kind": "pr"`.

In a color terminal, the number's color is the PR's fate: green/red/yellow for
CI passing/failing/running, purple for merged — the "this worktree is done,
prune it" signal — and dim for closed-without-merge. When color is off
(`NO_COLOR`, piped output), the same states trail the number as a glyph instead:
`#123 ✓`/`✗`/`●` for CI, `#123 ◆` merged, `#123 ○` closed — the signal never
exists as color alone. Where the table is printed as plain text, supporting
terminals also make the number a clickable link to the PR.

The live table never presents a cached status as current: every run re-verifies
against the forge. PR numbers and rows render immediately but fateless, and the
fresh verdict lands as one repaint — colors arrive, rows for PRs that opened
since the snapshot appear, rows for PRs that closed drop — typically a second or
two; the table holds its final frame briefly for it when needed. On the very
first run in a repository, before any snapshot exists, the PR cells show a
loading skeleton (like the size column) until that first refresh concludes and
its open PRs take their rows. The one next-run case: a PR freshly opened from a
branch that already exists locally surfaces on the following list (its row needs
local git data the live table doesn't gather mid-run).

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

Every list re-verifies: a rendered status always has a verification behind it
that the run itself started — or attached to, since lists fired while a refresh
is already running share its verdict instead of stacking forge calls. A cold
cache never blocks and never errors — cells simply wait for the refresh. The
cache also powers tab completion: `daft go pr:<Tab>` completes the cached
**open** PR numbers, each with its status as of the last refresh (the same
`✓`/`✗`/`●` glyphs as the column), its author, and its title — entirely from
local data. Merged and closed PRs don't complete — their branch has usually been
deleted on the forge — except ones you already have checked out, whose local
branch keeps them a navigation target regardless.

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
