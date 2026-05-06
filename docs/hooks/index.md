---
title: Hooks
description:
  daft hooks define clear boundaries as your code evolves — a local, parallel
  approach to GitHub Actions.
---

# Hooks

> **daft hooks define clear boundaries as your code evolves. They are a local,
> parallel approach to GitHub Actions — every stage of code's lifecycle gets a
> well-defined gate.**

Each stage in your code's journey through development has different needs:

- When you start isolated work, you want the right env booted up
- When you commit a change, you want format/lint/fast-tests to gate it
- When you merge, you want the equivalent of PR checks to gate it
- When you tear down a worktree, you want artifacts persisted and state
  reclaimed

daft models each of these as a hook stage. They share one configuration system,
one trust model, one job orchestrator.

## The boundaries

| Stage                             | Hook type                                      | Boundary semantics                                                                             | Status                                                      |
| --------------------------------- | ---------------------------------------------- | ---------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| End of clone setup                | `post-clone`                                   | One-shot bootstrap of a fresh repo                                                             | Shipped                                                     |
| Start of isolated dev             | Worktree hooks (`worktree-pre/post-create`)    | Set up local dev env (deps, services)                                                          | Shipped                                                     |
| Sealing a unit of change          | Commit hooks                                   | Progressive code-replication boundary — format, lint, fast tests before the change is recorded | Roadmap ([#468](https://github.com/avihut/daft/issues/468)) |
| Letting a change escape isolation | Merge hooks (`pre-merge`, `post-merge`)        | PR-check parity — full tests, integration, security gates before code leaves the branch        | Shipped                                                     |
| Reclaiming an isolated env        | Worktree teardown (`worktree-pre/post-remove`) | Teardown, persist artifacts, sync state                                                        | Shipped                                                     |

## How daft hooks differ from lefthook

Two distinctions:

1. **Lefthook is commit-time-only.** daft covers the full code-evolution
   lifecycle. Commit hooks are one stage among many — they share the trust
   model, the YAML schema, and the job orchestrator with worktree-lifecycle
   hooks. (See [#468](https://github.com/avihut/daft/issues/468) for the
   lefthook drop-in plan.)
2. **Boundaries before changes leave your machine.** CI traditionally runs
   _after_ code reaches the central repo; daft hooks run _before_. CI shifts
   left.

## Where to next

- **Reference:** [Lifecycle hooks](/hooks/lifecycle) — types, triggers, env
  vars, exit-code semantics
- **Reference:** [YAML reference](/hooks/yaml-reference) — the full `daft.yml`
  schema
- **Concept:** [Job orchestration](/hooks/job-orchestration) — parallelism,
  dependencies, conditions, OS/arch gating
- **Concept:** [Trust & security](/hooks/trust-and-security) — why hooks need
  trust and how the model works
- **Status:** [Roadmap](/hooks/roadmap) — what's coming for commit-stage hooks
- **Recipes:** [Recipes for Hooks](/recipes/?pillar=hooks)
