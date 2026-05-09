---
title: Job orchestration
description:
  How daft hooks orchestrate multiple jobs — parallelism, dependencies,
  conditions.
---

# Job orchestration

A single hook can fire multiple jobs. This page explains how those jobs are
orchestrated — how they start, when they wait, and when they're skipped.

## Why jobs, not just scripts

The original hook model for most tools is a single script file. You write bash,
it runs top to bottom, done. That simplicity has a cost: steps that could run in
parallel wait for each other, one failed command can bail out of unrelated work,
and platform-specific or environment-specific steps require manual `if` branches
scattered through the script.

daft models hook work as a list of **named jobs**. Each job declares what it
does, what it depends on, and when it applies. The orchestrator reads those
declarations and does the right thing — running independent work concurrently,
holding back downstream jobs until their inputs are ready, skipping jobs whose
conditions aren't met, and reporting each job's outcome individually.

This also makes partial failure honest. When a script bails mid-way, you only
know the script exited non-zero. With named jobs, you know exactly which step
failed and which later steps were blocked by it, so you can address the root
cause without guessing.

## Parallelism

Jobs run in parallel by default. When a hook fires, all its jobs start
simultaneously and daft waits for all of them to finish. This is the right
default for worktree setup — installing npm packages, setting up a Python
virtualenv, and copying config files are independent and together complete
faster than they would in sequence.

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install-npm
        run: npm install
      - name: install-pip
        run: pip install -r requirements.txt
      - name: copy-env
        run: cp .env.example .env
```

All three jobs above start at the same time. A machine with reasonable I/O
finishes the above in roughly the time the slowest of the three takes, rather
than the sum of all three.

When jobs are not independent — when one step produces output that the next step
consumes — you should declare that relationship explicitly with `needs:` (below)
rather than relying on sequential execution. Two alternative execution modes
exist for cases where you want a simple sequence: `piped: true` runs jobs one by
one and stops on the first failure, and `follow: true` runs jobs one by one
regardless of failure. Only one mode can be active at a time; if none is set,
parallel is used. See [YAML reference](/hooks/yaml-reference) for the full field
list.

## Dependencies (`needs:`)

`needs:` lets a job declare which other jobs must finish before it starts. This
expresses a directed acyclic graph (DAG) of dependencies rather than a flat
sequence. Jobs without `needs:` start immediately; jobs with `needs:` wait for
all their named dependencies to complete.

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install-npm
        run: npm install
      - name: install-pip
        run: pip install -r requirements.txt
      - name: build
        run: npm run build
        needs: [install-npm]
      - name: deploy
        run: ./deploy.sh
        needs: [build, install-pip]
```

In this example, `install-npm` and `install-pip` start in parallel. `build`
waits for `install-npm` alone and starts as soon as npm is ready. `deploy` waits
for both `build` and `install-pip`. The total wall-clock time is determined by
the critical path, not the total of all steps.

When a dependency fails, all jobs that declared `needs:` on it are marked
`dep-failed` and do not run. This is more honest than continuing and producing
confusing output downstream. If a dependency is **skipped** (because its own
conditions weren't met), dependent jobs still run — a skipped job is considered
satisfied, since it simply did not apply. Circular dependencies and references
to non-existent job names are both rejected during validation, so these problems
surface before any hooks fire at runtime. Every job that participates in a
`needs:` relationship must have a `name`.

## Conditional skipping (`skip:` / `only:`)

`skip:` and `only:` control whether a job runs. They can be set at the hook
level (applying to all jobs in the hook) or at the job level (applying to one
job). At the hook level, a matching `skip:` suppresses the entire hook; at the
job level, the rest of the hook continues without that job.

`skip:` accepts three forms. A boolean (`skip: true`) unconditionally skips. An
environment variable name (`skip: CI`) skips when that variable is set and
truthy. A list of structured conditions skips when any condition in the list
matches — useful for composing multiple triggers:

```yaml
skip:
  - merge # skip during git merge state
  - rebase # skip during git rebase state
  - ref: "release/*" # skip if branch matches a glob
  - env: SKIP_HOOKS # skip if env var is truthy
  - run: "test -f .skip-hooks" # skip if command exits 0
```

`only:` is the inverse: all conditions in the list must match for the job to
run. A job with both `skip:` and `only:` runs only when the `only:` conditions
are satisfied and none of the `skip:` conditions match.

Use conditional skipping to handle situations where a job applies in most
contexts but not all — for example, skipping a linter job during CI where a
dedicated pipeline already covers it, or skipping a service-startup job on
branches that don't need a running server. When a job is skipped, it is treated
as satisfied for downstream `needs:` references, so skipping one step in a chain
does not block the rest.

## OS and architecture gating

Individual jobs can declare `os:` and `arch:` constraints. A job with
`os: macos` only runs on macOS; a job with `arch: aarch64` only runs on ARM
machines. Both fields accept a single value or a list, and both are evaluated at
runtime — a job whose OS or architecture does not match is silently skipped.

```yaml
- name: install-brew
  os: macos
  run:
    /bin/bash -c "$(curl -fsSL
    https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  skip:
    - run: "command -v brew"
      desc: Brew is already installed
```

This matters in cross-platform teams. A shared `daft.yml` committed to the repo
can describe the full setup for all platforms, and each developer's machine
simply runs the jobs that apply to it. There is no need to split hook
configuration by OS or maintain separate files for different environments. OS
and arch gating is a job-level feature; it cannot be set at the hook level.

## Trust and side effects

Job orchestration only runs once a hook is **trusted**. Hooks from untrusted
repositories do not run automatically during lifecycle events. See
[Trust & security](/hooks/trust-and-security) for the model.

When you run `git daft hooks run` manually, trust checks are bypassed — you are
explicitly invoking the hook, so the orchestrator runs regardless of trust
state.

## Where to next

- **Schema:** [YAML reference](/hooks/yaml-reference) — every field
- **Trust:** [Trust & security](/hooks/trust-and-security)
- **Recipes:** [Recipes for Hooks](/recipes/?pillar=hooks)
