---
title: Hooks
description: Automate worktree lifecycle events with project-managed hooks
---

# Hooks

daft provides a hooks system that runs automation at worktree lifecycle events.
Hooks are stored in the repository and shared with your team, with a trust-based
security model.

The recommended approach is a **YAML configuration file** (`daft.yml`) that
supports multiple jobs, parallel execution, dependencies, conditional skipping,
and more. For simple cases, you can also use **executable shell scripts** in
`.daft/hooks/`.

## Hook Types

| Hook                   | Trigger                              | Runs From                            |
| ---------------------- | ------------------------------------ | ------------------------------------ |
| `post-clone`           | After `git worktree-clone` completes | New default branch worktree          |
| `worktree-pre-create`  | Before new worktree is added         | Source worktree (where command runs) |
| `worktree-post-create` | After new worktree is created        | New worktree                         |
| `worktree-pre-remove`  | Before worktree is removed           | Worktree being removed               |
| `worktree-post-remove` | After worktree is removed            | Current worktree (where prune runs)  |

### Execution Order During Clone

When running `git worktree-clone`, hooks fire in this order:

1. **`post-clone`** -- one-time repo bootstrap (install toolchains, global
   setup)
2. **`worktree-post-create`** -- per-worktree setup (install dependencies,
   configure environment)

This lets `post-clone` install foundational tools (pnpm, bun, uv, etc.) that
`worktree-post-create` may depend on.

## Trust Model

For security, hooks from untrusted repositories don't run automatically. Trust
is managed per-repository.

### Trust Levels

| Level            | Behavior                                    |
| ---------------- | ------------------------------------------- |
| `deny` (default) | Hooks are never executed                    |
| `prompt`         | User is prompted before each hook execution |
| `allow`          | Hooks run without prompting                 |

### Managing Trust

```bash
# Trust the current repository
git daft hooks trust

# Prompt before running hooks
git daft hooks prompt

# Revoke trust (sets explicit deny entry)
git daft hooks deny

# Remove trust entry (returns to default deny, no record kept)
git daft hooks trust reset

# Check current status
git daft hooks status

# List all trusted repositories
git daft hooks trust list

# Clear all trust settings
git daft hooks trust reset all
```

## Quick Start

1. **Scaffold a configuration file:**

   ```bash
   git daft hooks install
   ```

   This creates a `daft.yml` at your worktree root with placeholder jobs for all
   hook types.

2. **Edit `daft.yml`** with your actual commands:

   ```yaml
   hooks:
     worktree-post-create:
       jobs:
         - name: install-deps
           run: npm install
         - name: setup-env
           run: cp .env.example .env
   ```

3. **Trust the repository** so hooks can run:

   ```bash
   git daft hooks trust
   ```

4. **Validate your configuration:**

   ```bash
   git daft hooks validate
   ```

That's it. The next time a worktree is created, your hooks will run
automatically.

## YAML Configuration

### Config File Locations

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

### Top-Level Settings

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

### Hook Definition

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
| `exclude_tags` | list                 |         | Tags to exclude at hook level                                              |
| `exclude`      | list                 |         | Glob patterns to exclude                                                   |
| `skip`         | bool / string / list |         | Skip condition (see [Skip and Only Conditions](#skip-and-only-conditions)) |
| `only`         | bool / string / list |         | Only condition (see [Skip and Only Conditions](#skip-and-only-conditions)) |
| `jobs`         | list                 |         | Jobs to execute                                                            |

### Jobs

Each job in the `jobs` list supports:

| Field         | Type                 | Description                                                     |
| ------------- | -------------------- | --------------------------------------------------------------- |
| `name`        | string               | Job name (used for display, merging, and dependency references) |
| `run`         | string               | Inline shell command to execute                                 |
| `script`      | string               | Script file to run (relative to `source_dir`)                   |
| `runner`      | string               | Interpreter for script files (e.g., `"bash"`, `"python"`)       |
| `args`        | string               | Arguments to pass to the script                                 |
| `root`        | string               | Working directory (relative to worktree root)                   |
| `tags`        | list                 | Tags for filtering with `exclude_tags`                          |
| `skip`        | bool / string / list | Skip condition                                                  |
| `only`        | bool / string / list | Only condition                                                  |
| `env`         | map                  | Extra environment variables                                     |
| `fail_text`   | string               | Custom failure message                                          |
| `interactive` | bool                 | Job needs TTY/stdin (forces sequential execution)               |
| `priority`    | int                  | Execution ordering (lower runs first)                           |
| `needs`       | list                 | Names of jobs that must complete before this job runs           |
| `group`       | object               | Nested group of jobs (see [Groups](#groups))                    |

A job must have exactly one of `run`, `script`, or `group`.

#### Example: Inline command

```yaml
- name: lint
  run: cargo clippy -- -D warnings
```

#### Example: Script with runner

```yaml
- name: setup
  script: setup.sh
  runner: bash
  args: --verbose
```

#### Example: Environment variables and failure text

```yaml
- name: test
  run: npm test
  env:
    NODE_ENV: test
    CI: "true"
  fail_text: "Tests failed! Fix before continuing."
```

## Execution Modes

Each hook runs its jobs in one of three modes. Only one can be set at a time.

| Mode     | Field            | Behavior                                          |
| -------- | ---------------- | ------------------------------------------------- |
| Parallel | `parallel: true` | All jobs run concurrently (default)               |
| Piped    | `piped: true`    | Jobs run sequentially; stop on first failure      |
| Follow   | `follow: true`   | Jobs run sequentially; continue even if one fails |

### Parallel (default)

```yaml
hooks:
  worktree-post-create:
    parallel: true
    jobs:
      - name: install-npm
        run: npm install
      - name: install-pip
        run: pip install -r requirements.txt
```

Both jobs start at the same time.

### Piped

```yaml
hooks:
  worktree-post-create:
    piped: true
    jobs:
      - name: install
        run: npm install
      - name: build
        run: npm run build
```

`build` only runs if `install` succeeds.

### Follow

```yaml
hooks:
  worktree-post-create:
    follow: true
    jobs:
      - name: optional-lint
        run: npm run lint
      - name: required-build
        run: npm run build
```

`required-build` runs even if `optional-lint` fails.

## Job Dependencies

For complex workflows where some jobs depend on others, use `needs` to declare
dependencies. Jobs with `needs` wait for all their dependencies to complete
before starting. Independent jobs still run in parallel.

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

In this example:

- `install-npm` and `install-pip` start immediately (in parallel)
- `build` starts after `install-npm` completes
- `deploy` starts after both `build` and `install-pip` complete

### Dependency rules

- `needs` requires each job to have a `name`
- Circular dependencies are rejected during validation
- References to non-existent job names are rejected
- If a dependency **fails**, all jobs that depend on it are marked as
  `dep-failed` and do not run
- If a dependency is **skipped**, downstream jobs still run (skipped deps are
  considered satisfied)

## Skip and Only Conditions

`skip` and `only` control whether a hook or job runs. They can be set at either
the hook level or the job level.

- **`skip`**: If any condition matches, the hook/job is skipped
- **`only`**: All conditions must match for the hook/job to run

### Three forms

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

### Named conditions

| Name     | Triggers when                                                      |
| -------- | ------------------------------------------------------------------ |
| `merge`  | Git is in a merge state (`MERGE_HEAD` exists)                      |
| `rebase` | Git is in a rebase state (`rebase-merge` or `rebase-apply` exists) |

### Structured condition fields

| Field | Description                                          |
| ----- | ---------------------------------------------------- |
| `ref` | Glob pattern matched against the current branch name |
| `env` | Environment variable name; truthy = condition met    |
| `run` | Shell command; exit code 0 = condition met           |

### Hook-level vs job-level

```yaml
hooks:
  worktree-post-create:
    skip:
      - merge # Skip ALL jobs in this hook during merge
    jobs:
      - name: lint
        run: cargo clippy
        skip: CI # Additionally skip this job when $CI is set
      - name: build
        run: cargo build
```

## Groups

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

In this example, `lint` and `format` run in parallel within the group. The outer
hook uses `piped` mode, so `build` only starts after the entire `checks` group
completes successfully.

| Group field | Type | Description                                        |
| ----------- | ---- | -------------------------------------------------- |
| `parallel`  | bool | Run group jobs in parallel                         |
| `piped`     | bool | Run group jobs sequentially, stop on first failure |
| `jobs`      | list | Jobs within the group                              |

## Template Variables

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
| `{base_branch}`     | Base branch name (for `checkout-branch` commands)       |
| `{repository_url}`  | Repository URL (for `post-clone`)                       |
| `{default_branch}`  | Default branch name (for `post-clone`)                  |

### Example

```yaml
jobs:
  - name: log
    run: echo "Setting up worktree for {branch} at {worktree_path}"
  - name: diff
    run: git diff {base_branch}...{branch}
```

## Config Merging

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

## Manual Hook Execution

Use `git daft hooks run` to manually trigger a hook outside the normal worktree
lifecycle. Trust checks are bypassed since you are explicitly invoking the hook.

```bash
# List all configured hooks
git daft hooks run

# Run all jobs in a hook
git daft hooks run worktree-post-create

# Run a single job by name
git daft hooks run worktree-post-create --job "mise install"

# Run only jobs with a specific tag
git daft hooks run worktree-post-create --tag setup

# Preview what would run without executing
git daft hooks run worktree-post-create --dry-run
```

This is useful for:

- **Re-running** a hook after a previous failure
- **Iterating** on hook scripts during development
- **Bootstrapping** existing worktrees that predate the hooks config

When run from an untrusted repository, a hint is shown suggesting
`git daft hooks trust`, but hooks still execute.

## Shell Script Hooks

For simple automation, you can use executable scripts in `.daft/hooks/` instead
of (or in addition to) YAML configuration. Shell scripts run before
YAML-configured jobs.

### Writing a shell script hook

Hooks are executable scripts placed in `.daft/hooks/` within your repository.
They can be written in any language.

```
my-project/
├── .daft/
│   └── hooks/
│       ├── post-clone            # Runs after cloning the repo
│       ├── worktree-post-create  # Runs after creating a worktree
│       └── worktree-pre-remove   # Runs before removing a worktree
└── src/
```

#### Example: Auto-allow direnv

```bash
#!/bin/bash
# .daft/hooks/worktree-post-create
if [ -f ".envrc" ] && command -v direnv &>/dev/null; then
    direnv allow .
fi
```

#### Example: Install dependencies

```bash
#!/bin/bash
# .daft/hooks/worktree-post-create
if [ -f "package.json" ]; then
    npm install
elif [ -f "Gemfile" ]; then
    bundle install
elif [ -f "requirements.txt" ]; then
    pip install -r requirements.txt
fi
```

#### Example: Use correct Node version

```bash
#!/bin/bash
# .daft/hooks/worktree-post-create
if [ -f ".nvmrc" ] && command -v nvm &>/dev/null; then
    nvm use
fi
```

Make hooks executable:

```bash
chmod +x .daft/hooks/worktree-post-create
```

## Environment Variables

Hooks receive context via environment variables. These are available to both
YAML jobs and shell script hooks.

### Universal (all hooks)

| Variable               | Description                                               |
| ---------------------- | --------------------------------------------------------- |
| `DAFT_HOOK`            | Hook type (e.g., `worktree-post-create`)                  |
| `DAFT_COMMAND`         | Command that triggered the hook (e.g., `checkout-branch`) |
| `DAFT_PROJECT_ROOT`    | Repository root (parent of `.git` directory)              |
| `DAFT_GIT_DIR`         | Path to the `.git` directory                              |
| `DAFT_REMOTE`          | Remote name (usually `origin`)                            |
| `DAFT_SOURCE_WORKTREE` | Worktree where the command was invoked                    |

### Worktree (creation and removal hooks)

| Variable             | Description                         |
| -------------------- | ----------------------------------- |
| `DAFT_WORKTREE_PATH` | Path to the target worktree         |
| `DAFT_BRANCH_NAME`   | Branch name for the target worktree |

### Creation (create hooks only)

| Variable             | Description                                               |
| -------------------- | --------------------------------------------------------- |
| `DAFT_IS_NEW_BRANCH` | `true` if the branch was newly created, `false` otherwise |
| `DAFT_BASE_BRANCH`   | Base branch (for `checkout-branch` commands)              |

### Clone (post-clone only)

| Variable              | Description                 |
| --------------------- | --------------------------- |
| `DAFT_REPOSITORY_URL` | The cloned repository URL   |
| `DAFT_DEFAULT_BRANCH` | The remote's default branch |

### Removal (remove hooks only)

| Variable              | Description                                                                  |
| --------------------- | ---------------------------------------------------------------------------- |
| `DAFT_REMOVAL_REASON` | Why the worktree is being removed: `remote-deleted`, `manual`, or `ejecting` |

## Fail Modes

Each hook type has a default fail mode that determines what happens when a hook
exits with a non-zero status:

| Hook                  | Default Fail Mode | Behavior                              |
| --------------------- | ----------------- | ------------------------------------- |
| `worktree-pre-create` | `abort`           | Operation is cancelled                |
| All others            | `warn`            | Warning is shown, operation continues |

Override per-hook:

```bash
# Make post-create hooks abort on failure
git config daft.hooks.worktreePostCreate.failMode abort

# Make pre-create hooks just warn
git config daft.hooks.worktreePreCreate.failMode warn
```

## User-Global Hooks

Place hooks in `~/.config/daft/hooks/` to run them for all repositories. Global
hooks run after project hooks.

Customize the directory:

```bash
git config --global daft.hooks.userDirectory ~/my-daft-hooks
```

## Configuration

| Key                              | Default                 | Description                           |
| -------------------------------- | ----------------------- | ------------------------------------- |
| `daft.hooks.enabled`             | `true`                  | Master switch for all hooks           |
| `daft.hooks.defaultTrust`        | `deny`                  | Default trust level for unknown repos |
| `daft.hooks.userDirectory`       | `~/.config/daft/hooks/` | Path to user-global hooks             |
| `daft.hooks.timeout`             | `300`                   | Hook execution timeout in seconds     |
| `daft.hooks.<hookName>.enabled`  | `true`                  | Enable/disable a specific hook type   |
| `daft.hooks.<hookName>.failMode` | varies                  | `abort` or `warn` on hook failure     |
| `daft.hooks.output.quiet`        | `false`                 | Suppress hook stdout/stderr           |
| `daft.hooks.output.timerDelay`   | `5`                     | Seconds before showing elapsed timer  |
| `daft.hooks.output.tailLines`    | `6`                     | Rolling output lines per job          |

Hook name config keys use camelCase: `postClone`, `worktreePreCreate`,
`worktreePostCreate`, `worktreePreRemove`, `worktreePostRemove`.

### Output Display

When hooks run, daft shows real-time progress with spinners and rolling output
windows. Each job gets a spinner that animates while it runs, and the last few
lines of output are shown beneath it.

When a job takes longer than the configured timer delay (default 5 seconds), an
elapsed timer appears next to the spinner. When a job finishes, its full output
scrolls into the terminal history and the spinner is replaced with a check mark
or cross.

In non-interactive environments (CI, pipes), spinners are disabled and output is
printed as plain text.

```bash
# Suppress all hook output (only show spinner and result)
git config daft.hooks.output.quiet true

# Show elapsed timer after 3 seconds instead of 5
git config daft.hooks.output.timerDelay 3

# Show 10 lines of rolling output per job instead of 6
git config daft.hooks.output.tailLines 10

# Disable rolling output window (only show spinner)
git config daft.hooks.output.tailLines 0
```

## Migration from Deprecated Names

In earlier versions, worktree hooks used shorter names (`pre-create`,
`post-create`, `pre-remove`, `post-remove`). These were renamed with a
`worktree-` prefix for clarity.

Old names still work with deprecation warnings until v2.0.0. To migrate:

```bash
git daft hooks migrate
```

This renames hook files in the current worktree from old names to new names.
