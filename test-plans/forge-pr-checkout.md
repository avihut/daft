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

## Default PR column and forge health

- [ ] In a repo with a GitHub remote, bare `daft list` shows the PR column
      without asking for it; in a purely local repo (no forge remote), it
      doesn't — and `daft list --format json` still carries the `pr_*` fields in
      both.
- [ ] Break auth (`gh auth token` revoked / `gh auth logout`), run `daft list`,
      wait a beat for the background refresh, run `daft list` again: the PR
      column is gone, silently, and stays gone across runs.
      `daft __dump-store forge-health` shows `"healthy":false`; `daft doctor`
      explains the hidden column under "Forge integration".
- [ ] While hidden, `daft list --columns +pr` still shows the column
      (config-recorded refs render without the forge).
- [ ] `gh auth login`, then `daft list` twice (the first probes and restores
      health; the second shows): the column is back, persistently.
- [ ] A successful `daft go pr:<n>` also restores a hidden column on the next
      list (write-through flips health without waiting for a refresh).

## Forge-PR cache (live forge)

- [ ] In a repo with open PRs and a **cold cache** (`rm` the coordinator db or a
      fresh clone), bare `daft list`: the PR cells show the loading skeleton
      (like size), the table holds its final frame a moment, and the fresh
      statuses land before it settles — no second invocation needed. If the
      forge answers slowly (>~4s), the run finishes with plain numbers and the
      next run shows the data.
- [ ] Every `daft list` re-verifies: numbers render plain first and re-colorize
      in place when the fresh verdict lands (~1–2s), on every run — a cached
      fate is never presented as current. Two lists fired within a second share
      one refresh (the second attaches to the in-flight one and still
      re-colorizes).
- [ ] In the TUI, PR numbers are colored by fate: green/red/yellow for CI
      pass/fail/running, purple for a branch whose PR merged, dim for a
      closed-unmerged PR — with **no trailing glyph** (the number alone).
- [ ] `NO_COLOR=1 daft list --columns +pr | cat`: no ANSI, and the fates appear
      as trailing glyphs instead — `✓`/`✗`/`●` CI, `◆` merged, `○` closed.
- [ ] In a linking terminal (iTerm2/Kitty/WezTerm), a static-table render
      (`DAFT_NO_LIVE=1`, `+pr`) makes each PR number a clickable link to the PR
      (hover shows the URL; the live table does not link — ratatui buffers can't
      carry OSC 8).
- [ ] A branch whose PR just merged shows `#<n>` purple (◆ piped) — and an open
      PR reusing that branch name wins over the merged one.
- [ ] A fork PR whose head branch name matches one of your local branches does
      NOT decorate that branch (open or merged).
- [ ] `daft update` (or `daft sync`) refreshes the cache in the background:
      `daft __dump-store forge-prs` shows fresh `fetched_at` afterwards, and
      merged PRs appear with `"state":"merged"`.
- [ ] `daft list --columns +pr --format json` rows carry `pr_state`,
      `ci_status`, and `pr_url`.
- [ ] `daft go pr:<Tab>` in bash AND zsh completes open PR numbers; the zsh list
      shows aligned status/owner/title columns (status = last-fetched fate glyph
      `✓`/`✗`/`●`, blank for no CI; fish separates the same fields with `·`);
      merged/closed PRs do NOT complete unless checked out locally (then
      `◆`/`○` + `checked out: <branch>`); in bash, accepting a completion
      inserts `pr:<n>` exactly once (no duplicated `pr:` — the colon-wordbreak
      handling).
- [ ] `daft go <Tab>` on an empty word offers the `pr:`/`mr:` syntax tokens
      after the branch groups.
- [ ] `daft go p<Tab>` / `daft go pr:<Tab>` / `daft go pr:999<Tab>` (no such
      cached PR) never flash the "Fetching refs from origin…" fetch-on-miss
      spinner or scribble over the command line — forge candidates count as hits
      and `:`-words can't be branches; a genuinely unknown branch prefix (e.g.
      `daft go zzz<Tab>`) still fetches as before.
- [ ] GitLab: `mr:` completion and `!<n>` column decoration from a `glab`
      listing, including a merged MR in purple (CI stays blank — the REST
      listing carries no pipeline status).

## Default open-PR rows (live forge)

- [ ] In a repo with open PRs, bare `daft list` shows a row for every open PR:
      your PR-bearing local branches appear without `--branches` (real local
      age/commit, owner = the PR author), and PRs with no local presence appear
      dimmed with the PR title as the commit subject and last activity as age.
      Fork PRs render `owner:branch`.
- [ ] Merged and closed PRs decorate existing rows but never add one.
- [ ] Live reconcile: with the table open on a warm cache, a PR opened from a
      foreign branch since the last run pops its row in when the fresh verdict
      lands (same repaint as the colors); a PR closed since drops its row. On a
      **cold cache**, the first refresh populates the foreign rows mid-run,
      within the settle hold.
- [ ] A PR freshly opened from an existing local branch surfaces on the _next_
      list (the stated next-run case), not mid-run.
- [ ] `daft go pr:<fork-pr>`; the fork row disappears (the worktree row absorbs
      it via the tracking ref); `daft rm` the worktree but keep the branch — the
      branch row is surfaced instead, still no duplicate.
- [ ] `--columns +pr` in a huge OSS repo: the foreign-PR section is loud;
      `git config -- daft.list.columns -pr` silences rows + column in that repo
      only, and other repos keep the default.
- [ ] `daft list -b` shows all local branches with no duplicate row for the
      PR-bearing ones; `daft list -r` doesn't duplicate a synthesized row's
      branch as a remote row.
