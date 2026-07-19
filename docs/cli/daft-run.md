---
title: daft-run
description: Run a named task defined in daft.yml
---

# daft run

Run a named task defined in daft.yml

## Description

Run a named task from the current worktree's daft.yml.

Tasks live under a top-level `tasks:` section and reuse the hook job schema (jobs, parallel/piped/follow, needs, env, root, skip/only, tags). Bare `git daft run` executes the reserved task named `run`; a first word that names a task runs that task, and any words after it are forwarded to the task as arguments. A first word that names no task is itself forwarded — the whole word list goes to the reserved `run` task. Words after the first are passed through verbatim, flags included, so this command reads its own flags (--list, --job, --tag) only before the first word; write `--` before the first word to forward every word without task-name matching.

Forwarded words are shell-escaped and appended to the task's command, which requires the task to resolve to exactly one foreground job (narrow a multi-job task with --job). A task resolving to a single job passes the terminal straight through to the command — no wrapping interface; a multi-job task renders one live row per job with the logs threaded beneath. Tasks run until they exit or you press Ctrl+C (press it twice to force-kill) — they have no execution timeout, which makes them the home for long-running dev servers, compose stacks, and watchers.

Use `run` for tasks committed in daft.yml; use `exec` for an ad-hoc command you type on the spot.

## Usage

```
daft run [OPTIONS] [TASK]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TASK>` | Task to run, then arguments forwarded to it. A first word naming no task is forwarded to the reserved `run` task along with the rest | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--list` | List the tasks defined in daft.yml and exit |  |
| `--job <NAME>` | Run only the named job within the task |  |
| `--tag <TAG>` | Run only jobs carrying this tag (repeatable) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

