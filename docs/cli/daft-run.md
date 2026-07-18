---
title: daft-run
description: Run a named task defined in daft.yml
---

# daft run

Run a named task defined in daft.yml

## Description

Run a named task from the current worktree's daft.yml.

Tasks live under a top-level `tasks:` section and reuse the hook job schema (jobs, parallel/piped/follow, needs, env, root, skip/only, tags). Bare `git daft run` executes the reserved task named `run`; passing a task name runs that task instead. A task resolving to a single job passes the terminal straight through to the command — no wrapping interface; a multi-job task renders one live row per job with the logs threaded beneath. Tasks run until they exit or you press Ctrl+C (press it twice to force-kill) — they have no execution timeout, which makes them the home for long-running dev servers, compose stacks, and watchers.

Use `run` for tasks committed in daft.yml; use `exec` for an ad-hoc command you type on the spot.

## Usage

```
daft run [OPTIONS] [TASK]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TASK>` | Task to run. Omit to run the reserved `run` task | No |

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

