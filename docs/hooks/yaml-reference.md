---
title: daft.yml YAML reference
description: Complete reference for daft.yml hook configuration schema.
---

# `daft.yml` YAML reference

Complete reference for the `daft.yml` schema. For the conceptual framing, see
[Hooks Overview](/hooks/). For lifecycle-specific behavior (env vars, exit
codes), see [Lifecycle hooks](/hooks/lifecycle).

## Config file locations

daft searches for configuration files in the following order (first match wins):

| File                | Location                   |
| ------------------- | -------------------------- |
| `daft.yml`          | Repo root                  |
| `daft.yaml`         | Repo root                  |
| `.daft.yml`         | Repo root (hidden)         |
| `.daft.yaml`        | Repo root (hidden)         |
| `.config/daft.yml`  | XDG-style config directory |
| `.config/daft.yaml` | XDG-style config directory |

Additionally:

- **Local overrides** (`daft-local.yml`) — same directory as the main config,
  not committed to git. Useful for machine-specific settings.
- **Per-hook files** (`worktree-post-create.yml`, `post-clone.yml`, etc.) — same
  directory as the main config. Each file defines a single hook and is merged
  into the main config.

## Top-level keys

| Field              | Type        | Description                                                              |
| ------------------ | ----------- | ------------------------------------------------------------------------ |
| `min_version`      | string      | Minimum daft version required (e.g., `"1.5.0"`)                          |
| `colors`           | bool        | Enable/disable colored output                                            |
| `no_tty`           | bool        | Disable TTY detection                                                    |
| `rc`               | string      | Shell RC file to source before running hooks                             |
| `output`           | bool / list | `false` to suppress all output, or list of hook names to show output for |
| `extends`          | list        | Additional config files to merge (e.g., `["shared.yml"]`)                |
| `source_dir`       | string      | Directory for script files (default: `".daft"`)                          |
| `source_dir_local` | string      | Directory for local (gitignored) script files (default: `".daft-local"`) |
| `hooks`            | map         | Hook definitions, keyed by hook name                                     |
| `log`              | object      | Log configuration (see [Log configuration](#log-configuration))          |

## Hook entries

Each hook is defined under the `hooks` key:

```yaml
hooks:
  worktree-post-create:
    parallel: true
    jobs:
      - name: install
        run: npm install
      - name: build
        run: npm run build
```

| Field          | Type                 | Default | Description                                                                |
| -------------- | -------------------- | ------- | -------------------------------------------------------------------------- |
| `parallel`     | bool                 | `true`  | Run jobs in parallel                                                       |
| `piped`        | bool                 |         | Run jobs sequentially, stop on first failure                               |
| `follow`       | bool                 |         | Run jobs sequentially, continue on failure                                 |
| `background`   | bool                 |         | Default background execution for all jobs in this hook                     |
| `exclude_tags` | list                 |         | Tags to exclude at hook level                                              |
| `exclude`      | list                 |         | Glob patterns to exclude                                                   |
| `skip`         | bool / string / list |         | Skip condition (see [Skip and only conditions](#skip-and-only-conditions)) |
| `only`         | bool / string / list |         | Only condition (see [Skip and only conditions](#skip-and-only-conditions)) |
| `jobs`         | list                 |         | Jobs to execute                                                            |

Only one of `parallel`, `piped`, or `follow` can be set at a time.

## Job entries

Each job in the `jobs` list supports:

| Field               | Type                 | Description                                                                                           |
| ------------------- | -------------------- | ----------------------------------------------------------------------------------------------------- |
| `name`              | string               | Job name (used for display, merging, and dependency references)                                       |
| `description`       | string               | Human-readable description (shown in dry-run and completions)                                         |
| `run`               | string               | Inline shell command to execute                                                                       |
| `script`            | string               | Script file to run (relative to `source_dir`)                                                         |
| `runner`            | string               | Interpreter for script files (e.g., `"bash"`, `"python"`)                                             |
| `args`              | string               | Arguments to pass to the script                                                                       |
| `root`              | string               | Working directory / cwd, relative to worktree root (see [Working directory](#working-directory-root)) |
| `tags`              | list                 | Tags for filtering with `exclude_tags`                                                                |
| `skip`              | bool / string / list | Skip condition                                                                                        |
| `only`              | bool / string / list | Only condition                                                                                        |
| `os`                | string / list        | Target OS (`macos`, `linux`, `windows`); skips if no match                                            |
| `arch`              | string / list        | Target architecture (`x86_64`, `aarch64`); skips if no match                                          |
| `env`               | map                  | Extra environment variables                                                                           |
| `fail_text`         | string               | Custom failure message                                                                                |
| `interactive`       | bool                 | Job needs TTY/stdin (forces sequential execution)                                                     |
| `priority`          | int                  | Execution ordering (lower runs first)                                                                 |
| `needs`             | list                 | Names of jobs that must complete before this job runs                                                 |
| `tracks`            | list                 | Worktree attributes this job depends on: `path`, `branch`                                             |
| `group`             | object               | Nested group of jobs (see [Groups](#groups))                                                          |
| `background`        | bool                 | Run this job in the background (see [Background jobs](#background-jobs))                              |
| `background_output` | `log` / `silent`     | Output behavior for background jobs (default: `log`)                                                  |
| `log`               | object               | Log configuration (`retention`, `max_log_size`) for this job                                          |

A job must have exactly one of `run`, `script`, or `group`.

### Working directory (`root`)

By default each job runs in the worktree root. Set `root` to run the job in a
subdirectory instead — useful in a monorepo where a job targets a single
package. The path is relative to the worktree root and sets the job's working
directory (cwd).

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install-web
        run: pnpm install
        root: apps/web
```

### Template variables

Commands (`run`) support template variables that are replaced with values from
the execution context:

| Variable            | Description                                             |
| ------------------- | ------------------------------------------------------- |
| `{branch}`          | Target branch name (alias for `{worktree_branch}`)      |
| `{worktree_path}`   | Path to the target worktree                             |
| `{worktree_root}`   | Project root directory                                  |
| `{worktree_branch}` | Target branch name                                      |
| `{source_worktree}` | Path to the source worktree (where command was invoked) |
| `{git_dir}`         | Path to the `.git` directory                            |
| `{remote}`          | Remote name (usually `"origin"`)                        |
| `{job_name}`        | Name of the current job                                 |
| `{base_branch}`     | Base branch name (for `checkout -b` commands)           |
| `{repository_url}`  | Repository URL (for `post-clone`)                       |
| `{default_branch}`  | Default branch name (for `post-clone`)                  |

**Move hooks only** (available when `DAFT_IS_MOVE` is `true`):

| Variable              | Description                                         |
| --------------------- | --------------------------------------------------- |
| `{old_worktree_path}` | Previous worktree path (before the move)            |
| `{old_branch}`        | Previous branch name (before the move, rename only) |

### Skip and only conditions

`skip` and `only` control whether a hook or job runs. They can be set at either
the hook level or the job level.

- **`skip`**: If any condition matches, the hook/job is skipped
- **`only`**: All conditions must match for the hook/job to run

**Boolean** — always skip or always run:

```yaml
skip: true # Always skip this job
only: false # Never run this job
```

**Environment variable** — skip/run based on an env var being set and truthy:

```yaml
skip: CI # Skip when $CI is set
only: DEPLOY_ENABLED # Only run when $DEPLOY_ENABLED is set
```

An env var is "truthy" if it is set, non-empty, not `"0"`, and not `"false"`.

**Structured rules** — a list of conditions:

```yaml
skip:
  - merge # Named: skip during merge
  - rebase # Named: skip during rebase
  - ref: "release/*" # Ref: skip if branch matches glob
  - env: SKIP_HOOKS # Env: skip if env var is truthy
  - run: "test -f .skip-hooks" # Run: skip if command exits 0
```

Named conditions:

| Name     | Triggers when                                                      |
| -------- | ------------------------------------------------------------------ |
| `merge`  | Git is in a merge state (`MERGE_HEAD` exists)                      |
| `rebase` | Git is in a rebase state (`rebase-merge` or `rebase-apply` exists) |

Structured condition fields:

| Field  | Description                                                    |
| ------ | -------------------------------------------------------------- |
| `ref`  | Glob pattern matched against the current branch name           |
| `env`  | Environment variable name; truthy = condition met              |
| `run`  | Shell command; exit code 0 = condition met                     |
| `desc` | Human-readable reason shown when the condition triggers a skip |

### Groups

A job can contain a nested `group` of sub-jobs instead of a `run` or `script`.
The group runs as a unit with its own execution mode.

```yaml
hooks:
  worktree-post-create:
    piped: true
    jobs:
      - name: checks
        group:
          parallel: true
          jobs:
            - name: lint
              run: cargo clippy
            - name: format
              run: cargo fmt --check
      - name: build
        run: cargo build
```

| Group field | Type | Description                                        |
| ----------- | ---- | -------------------------------------------------- |
| `parallel`  | bool | Run group jobs in parallel                         |
| `piped`     | bool | Run group jobs sequentially, stop on first failure |
| `jobs`      | list | Jobs within the group                              |

### Background jobs

Jobs marked `background: true` run in the background after the command returns.

The `background_output` field controls notification behavior:

| Value    | Log file                | Terminal notification on failure |
| -------- | ----------------------- | -------------------------------- |
| `log`    | Always written          | Yes                              |
| `silent` | Written only on failure | No                               |

Default is `log`. Set `DAFT_NO_BACKGROUND_JOBS=1` to promote all background jobs
to foreground.

## Log configuration

The `log` field at the top level sets defaults for background-job log storage
and cleanup. Individual jobs can override `retention` and `max_log_size`.

```yaml
# Top-level default
log:
  retention: 14d # how long to keep logs
  max_log_size: 10MB # per-log file cap
  max_total_size: 500MB # per-repo total budget (repo-only)
  keep_last: 3 # sanity floor — keep at least this many invocations per worktree
  stale_running_after: 24h # how long before a stuck Running job is treated as cancelled

hooks:
  worktree-post-create:
    jobs:
      - name: build
        run: cargo build
        background: true
        log:
          retention: 1d # per-job override
          max_log_size: 50MB # per-job override
```

| Field                 | Type   | Default | Scope     | Description                                                                                         |
| --------------------- | ------ | ------- | --------- | --------------------------------------------------------------------------------------------------- |
| `retention`           | string | `7d`    | per-job   | How long to keep logs (e.g., `7d`, `24h`, `30m`).                                                   |
| `max_log_size`        | string | `10MB`  | per-job   | Truncate `output.log` to this size with a footer marker.                                            |
| `max_total_size`      | string | `500MB` | repo-only | Total disk budget for all logs under this repo. LRU eviction when exceeded.                         |
| `keep_last`           | int    | `3`     | repo-only | Always retain at least this many invocations per worktree, regardless of retention or budget.       |
| `stale_running_after` | string | `24h`   | repo-only | A `Running` job older than this with no live coordinator socket is treated as cancelled by cleanup. |

`retention` and `max_log_size` are resolved at hook-fire time and captured into
the job's `meta.json`. Cleanup reads these directly — editing `daft.yml` after a
hook fires will not retroactively change retention for already-completed jobs.

`max_total_size`, `keep_last`, and `stale_running_after` are persisted to
`<state>/jobs/<repo-uuid>/repo-policy.json` on every hook fire (most-recent-
write wins). Cleanup reads this file at run time; if it's missing (orphaned
state dir whose repo no longer fires hooks), built-in defaults apply.

## Config merging

When multiple config sources exist, they are merged in this order (lowest to
highest precedence):

1. **Main config** (`daft.yml`)
2. **Extends files** (listed in `extends`)
3. **Per-hook files** (`worktree-post-create.yml`, etc.)
4. **Local override** (`daft-local.yml`)

Merging rules:

- **Scalar fields** (e.g., `min_version`, `colors`): higher-precedence value
  wins
- **Named jobs**: jobs with the same `name` are replaced by the
  higher-precedence version
- **Unnamed jobs**: appended from the overlay

Use `git daft hooks dump` to inspect the fully merged configuration:

```bash
git daft hooks dump
```

## Examples

### Minimal quick-start

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
      - name: setup-env
        run: cp .env.example .env
```

### Platform constraint with skip condition

```yaml
- name: install-brew
  description: Install Homebrew package manager
  os: macos
  run:
    /bin/bash -c "$(curl -fsSL
    https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
  skip:
    - run: "command -v brew"
      desc: Brew is already installed
```

### Inline command, script with runner, and env vars

```yaml
- name: lint
  run: cargo clippy -- -D warnings

- name: setup
  script: setup.sh
  runner: bash
  args: --verbose

- name: test
  run: npm test
  env:
    NODE_ENV: test
    CI: "true"
  fail_text: "Tests failed! Fix before continuing."
```

### Job dependencies

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

### Background jobs with hook-level default

```yaml
hooks:
  worktree-post-create:
    background: true
    jobs:
      - name: install deps
        run: pnpm install
        background: false # override: run in foreground
      - name: warm build cache
        run: cargo build # inherits background: true
      - name: precompile assets
        run: pnpm build:assets # inherits background: true
```

### Move-tracked jobs

```yaml
hooks:
  worktree-post-create:
    jobs:
      - name: link-build-output
        description: Symlink build artifacts to a shared directory
        run: ln -sf {worktree_path}/dist /opt/project/builds/current
        tracks: [path]

      - name: set-branch-env
        description: Write branch name to local env file
        run: echo "CURRENT_BRANCH={branch}" > .env.branch
        tracks: [branch]

      - name: install-deps
        description: Install project dependencies
        run: npm install
        # Not tracked -- only runs on initial worktree creation

  worktree-pre-remove:
    jobs:
      - name: unlink-build-output
        run: rm -f /opt/project/builds/current
        tracks: [path]

      - name: clear-branch-env
        run: rm -f .env.branch
        tracks: [branch]
```

## Running these in CI

The same `daft.yml` runs locally and in CI — that's the parity story. See
[Recipes → CI parity](/recipes/ci-parity) for invoking
`daft hooks run worktree-post-create` from GitHub Actions, GitLab CI, or a
generic shell-based runner, plus how to skip local-only steps in CI via
`skip: { env: { CI: "true" } }`.
