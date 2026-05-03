---
title: Comparison
description: daft vs nearby tools — git worktree, lefthook, gitup, gh worktree.
---

# Comparison

How daft relates to nearby tools.

## vs plain `git worktree`

`git worktree` is the foundation daft is built on. daft adds:

- **Layout management.** `git worktree` makes you place worktrees manually; daft
  enforces a chosen geometry (sibling, contained, nested, custom).
- **Lifecycle automation.** `daft.yml` hooks fire on create/remove; plain
  `git worktree` has no hook surface.
- **Shell integration.** daft's shell wrapper auto-`cd`s into new worktrees;
  plain `git worktree` leaves you in the source.
- **Maintenance commands.** `daft prune`, `daft sync`, `daft list`,
  `daft doctor` — orchestrated workflows that you'd otherwise script yourself.

When to pick plain `git worktree`: occasional, one-off worktree usage where the
daft layout would be overkill.

## vs lefthook

[Lefthook](https://github.com/evilmartians/lefthook) is a popular git hook
manager focused on commit-stage hooks (pre-commit, commit-msg, pre-push).

Today, daft hooks are scoped to worktree-lifecycle stages — they don't replace
lefthook. The full git-hooks drop-in
([#468](https://github.com/avihut/daft/issues/468)) is on the roadmap; once
shipped, daft will be a viable lefthook replacement.

When that ships, the comparison will be:

- **daft** covers the full code-evolution lifecycle (worktree → commit → merge →
  teardown) under one config and one trust model.
- **lefthook** covers commit-stage only, but is mature and battle-tested.

When to pick lefthook today: you only need commit-stage hooks. Revisit when #468
ships.

## vs gitup / `gh worktree` / `git-town`

These are smaller-scope tools targeting specific workflow gaps:

- **[gitup](https://github.com/jonas/gitup)** is a TUI for `git worktree`. daft
  is a CLI with a richer feature set (layouts, hooks, multi-remote).
- **[`gh worktree`](https://github.com/cli/cli)** (planned in github/cli) is a
  thin GitHub CLI extension over `git worktree`. daft is broader (not
  GitHub-specific).
- **[git-town](https://www.git-town.com/)** automates branch sync workflows on a
  single working tree. daft solves the parallel-branches problem instead.

When to pick one of those: you have a narrow workflow gap that one of them fills
better than daft, or you don't need worktrees at all.

## vs GitHub Actions PR checks

(Speculative — fully realized once
[#330](https://github.com/avihut/daft/issues/330) and
[#468](https://github.com/avihut/daft/issues/468) ship.)

GitHub Actions runs PR checks **after** code reaches the central repo. daft
hooks (when the full set is shipped) run **before** code leaves your machine.

These are complementary: fast checks shift left to daft hooks (faster feedback,
no minutes consumed); slow/secrets-bound checks stay in Actions.

When to lean on Actions over daft hooks: deployment, release pipelines, artifact
publishing, integration with external secret stores.
