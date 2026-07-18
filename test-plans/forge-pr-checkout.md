---
branch: daft-127/feat/forge-pr-checkout
---

# Forge PR/MR Checkout

Automated coverage exists (unit tests + `checkout/pr-*.yml` scenarios with a
fake `gh`, plus verified end-to-end against real `gh` on octocat/Hello-World).
This plan covers what CI can't: live forges, `glab` (not installed on the dev
machine — its command shapes are doc-derived, not run), and the interactive TUI.

## Setup

- [ ] `mise run dev` to install the current build.
- [ ] `gh auth login` (GitHub) and, if testing GitLab, `glab auth login`.
- [ ] `daft doctor` shows a "Forge integration" category: `gh`/`glab`
      installed + authenticated (pass), or a Warning to run `<tool> auth login`
      when unauthenticated, or Skipped when not installed.

## GitHub (live)

- [ ] In a clone of a repo you can read, `daft go pr:<open-PR>` creates a
      worktree on the PR's source branch and cd's into it.
- [ ] For a **fork** PR: `git config branch.<src>.merge` is
      `refs/pull/<n>/head`, and `git pull` in the worktree fast-forwards to new
      commits pushed to the PR.
- [ ] For a **same-repo** PR: the branch tracks `origin/<src>` normally.
- [ ] A pasted PR URL (`daft go https://github.com/o/r/pull/<n>`) resolves the
      same as `pr:<n>`.
- [ ] A closed/merged PR still checks out, with a note in the output.
- [ ] `daft go pr:<nonexistent>` errors with "not found" and a hint.
- [ ] `daft list --columns +pr` shows `#<n>` for the PR worktree; `master` shows
      nothing.
- [ ] Fork-workflow: in a repo whose `origin` is your fork and `upstream` is the
      base, `daft go pr:<n>` still resolves (via `gh repo set-default` or the
      upstream remote).

## GitLab (live — needs glab installed)

- [ ] `daft go mr:<open-MR>` on a GitLab repo checks out the MR's source branch;
      `git config branch.<src>.merge` is `refs/merge-requests/<n>/head` for a
      fork MR.
- [ ] `pr:<n>` on a GitLab repo resolves the MR (aliases), and `mr:<n>` on a
      GitHub repo resolves the PR.
- [ ] A pasted MR URL resolves the same as `mr:<n>`.

## Disambiguation & config

- [ ] In a repo with both a GitHub and a GitLab remote, `pr:<n>` picks one;
      `git config daft.forge.platform gitlab` forces the MR path.
- [ ] `daft.forge.githubCli`/`gitlabCli` override the invoked binary (point at a
      wrapper script and confirm it's called).

## Guards & edge cases

- [ ] `daft start pr:5` and `daft go -b pr:5` are refused with a hint to use
      `daft go pr:5`.
- [ ] `daft go pr:<n> --local` is refused ("requires the network").
- [ ] Collision: with a local `main`, checking out a fork PR whose source branch
      is `main` is refused (does not hijack your `main`); the message suggests
      renaming/deleting or `daft go main`.

## TUI

- [ ] `daft list --columns +pr` in an interactive terminal: the PR column shows
      a loading shimmer briefly, then `#<n>` / blank, and the table doesn't
      jump.

## Forge-PR cache (live forge)

- [ ] In a repo with open PRs, `daft list --columns +pr` twice: the first run
      may be undecorated (cold cache) but kicks the background refresh; the
      second shows `#<n>` on branches with open PRs, with the CI glyph
      (`✓`/`✗`/`●`) colored green/red/yellow in the TUI.
- [ ] `NO_COLOR=1 daft list --columns +pr | cat`: glyphs survive, no ANSI.
- [ ] A fork PR whose head branch name matches one of your local branches does
      NOT decorate that branch.
- [ ] `daft update` (or `daft sync`) refreshes the cache in the background:
      `daft __dump-store forge-prs` shows fresh `fetched_at` afterwards.
- [ ] `daft go pr:<Tab>` in bash AND zsh completes open PR numbers with titles;
      in bash, accepting a completion inserts `pr:<n>` exactly once (no
      duplicated `pr:` — the colon-wordbreak handling).
- [ ] `daft go <Tab>` on an empty word offers the `pr:`/`mr:` syntax tokens
      after the branch groups.
- [ ] GitLab: `mr:` completion and `!<n>` column decoration from a `glab`
      listing (CI stays blank — the REST listing carries no pipeline status).
