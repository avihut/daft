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

| Field              | Type        | Description                                                                        |
| ------------------ | ----------- | ---------------------------------------------------------------------------------- |
| `min_version`      | string      | Minimum daft version required (e.g., `"1.5.0"`)                                    |
| `colors`           | bool        | Enable/disable colored output                                                      |
| `no_tty`           | bool        | Disable TTY detection                                                              |
| `rc`               | string      | Shell RC file to source before running hooks                                       |
| `output`           | bool / list | `false` to suppress all output, or list of hook names to show output for           |
| `extends`          | list        | Additional config files to merge (e.g., `["shared.yml"]`)                          |
| `source_dir`       | string      | Directory for script files (default: `".daft"`)                                    |
| `source_dir_local` | string      | Directory for local (gitignored) script files (default: `".daft-local"`)           |
| `copy`             | list / map  | Gitignored paths copied into each new worktree (see [Copied paths](#copied-paths)) |
| `env`              | object      | Derived per-worktree env values (see [Environment values](#environment-values))    |
| `hooks`            | map         | Hook definitions, keyed by hook name                                               |
| `tasks`            | map         | Named, user-invoked task definitions (see [Tasks](#tasks))                         |
| `log`              | object      | Log configuration (see [Log configuration](#log-configuration))                    |
| `relations`        | list        | Related repositories (see [Relations](#relations))                                 |
| `merge`            | object      | Committed merge gate policy (see [Merge gate policy](#merge-gate-policy))          |

## Copied paths

A top-level `copy:` key declares gitignored paths — build caches such as
`target/`, `node_modules/`, `.gradle/` — that daft replicates into every new
worktree, so a fresh worktree starts warm instead of paying a full build. On a
filesystem with copy-on-write support (APFS, btrfs, XFS with `reflink=1`,
OpenZFS 2.2+, bcachefs, ReFS) the replica costs almost nothing until the two
copies diverge.

Not every cache is a good candidate: a directory that records its own absolute
path (a Python `.venv/` above all) breaks when copied elsewhere. See
[what actually stays warm](/worktrees/copying-caches#what-actually-stays-warm)
before declaring one.

```yaml
copy:
  - target/
  - node_modules/
  - "**/dist/"
```

The map form adds knobs:

```yaml
copy:
  paths: [target/, node_modules/]
  fallback: copy # copy | skip (default: copy)
  max_size: 5GB # optional per-entry cap on the byte-copy fallback
```

| Field      | Type            | Description                                                                                                                                                    |
| ---------- | --------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `paths`    | list            | Entries to copy, relative to the worktree root. Files or directories; a trailing `/` is cosmetic, a **leading** `/` is refused — write `target`, not `/target` |
| `fallback` | `copy` / `skip` | What to do when the filesystem cannot reflink an entry. `copy` (the default) pays for a real byte copy; `skip` leaves the entry out                            |
| `max_size` | string / int    | Per-**entry** size cap (`5GB`, `500MB`, `1048576`). Gates the byte-copy fallback only — a reflink is near-free and is never size-checked                       |

Sizes are case-insensitive and use binary multiples (`1KB` = 1024 bytes); a
plain byte count works too, quoted or not. Both `fallback` spellings are matched
case-insensitively, but lowercase is canonical.

`daft hooks validate` rejects a `max_size` it cannot parse, and a map form that
declares no `paths:` at all (which is how a misspelled `paths:` key surfaces).
Both are errors rather than warnings: each would otherwise degrade quietly into
an uncapped copy or a section that looks configured and does nothing.

An entry containing `*`, `?`, or `[` is a glob, expanded against the source
worktree at copy time. Expansion ignores git's ignore rules (`copy:` entries are
gitignored by definition) and stops descending below a match — `**/dist/`
reports `web/dist`, not `web/dist/assets`.

**Entries must be gitignored.** daft checks each one with `git check-ignore`
_and_ verifies nothing underneath it is tracked, so a force-added file inside an
otherwise-ignored directory still disqualifies the entry. A violation is a
per-entry warning; the worktree is still created.

**Where it runs.** The copy stage sits between the `worktree-pre-create` and
`worktree-post-create` hooks — before `shared:` symlinking, before post-create
hooks fire — so a hook-driven `npm install` or `cargo build` hits a warm cache.
Caches first and daft-managed links on top: linking creates the parent
directories it needs, so the other order let a `shared:` path _inside_ a copied
cache manufacture an empty scaffold the copy then skipped as `already present`.
It is a creation-time optimization and never aborts creation: every failure
(tracked entry, unreadable source, full disk) is a warning row, never a fatal
error. `daft clone` does not run it — a fresh clone has no source worktree to
copy from.

The gitignored check asks the **source** worktree only, so the destination's own
`.gitignore` never gets a vote — copying into a branch that does not ignore the
entry leaves it as untracked content in that worktree's `git status`.

Existing destination entries are never overwritten, which makes the stage
idempotent and safe to re-run; [`daft warm`](/reference/cli/daft-warm) replays
it on demand, and `daft warm --force` replaces what is already there — except
content the **target** worktree tracks, which it refuses to delete.

Unlike most keys here, `copy:` accepts two different YAML shapes, so a mistyped
knob (`fallback: symlink`) fails the whole file with a generic
`data did not match any variant of untagged enum CopyConfig` rather than a
message naming the bad value. Check the `copy:` block first when you see it.

For the practical guide — what actually stays warm per toolchain, and what a
copied cache cannot promise — see
[Copying build caches into new worktrees](/worktrees/copying-caches).

## Environment values

The `env:` section declares deterministic per-worktree values — ports and
templated names — derived from the worktree's slug. No allocation, no registry:
the same worktree name yields the same values on every machine, even before the
worktree exists. Query them with [`daft env`](/reference/cli/daft-env); hooks,
tasks, and `daft exec` receive them in their environment automatically. The
`DAFT_*` prefix is reserved for daft's own
[job variables](/hooks/lifecycle#environment-provided-to-hooks): a name in that
namespace is refused here and never injected, because a derived port under
`DAFT_BRANCH_NAME` would overwrite the real branch name in every job. Reading
them is fine — `daft env DAFT_BRANCH_NAME` answers from live worktree state,
alongside the declared set in the listing.

```yaml
env:
  salt: myapp # optional; default = the repo directory's name.
  # Pin it so values match across machines and clone locations.
  ports:
    - WEBAPP_PORT # offset 0 — enum semantics: a bare name is previous + 1
    - STORYBOOK_PORT # offset 1
    - API_PORT: 8 # an explicit offset resets the counter
  values:
    COMPOSE_PROJECT_NAME: "myapp-{worktree_slug}"
    API_URL: "http://localhost:{env:API_PORT}"
  write: .env # optional default target for `daft env --write`
  range: 20000-32767 # optional; the default shown
  block_size: 16 # optional; ports per worktree block
```

Each worktree hashes to its own contiguous block of `block_size` ports inside
`range`; declared names take their offsets inside it, so one worktree's ports
are consecutive. Keep the ports list **append-only** — inserting a bare name
mid-list renumbers everything after it (pin load-bearing names with explicit
offsets). Undeclared names resolve in a disjoint ad-hoc region, but only until
you declare a schema: declaring opts the repo into strictness, where an unknown
name is an error (`daft env --ad-hoc` escapes).

`values:` are templates over `{worktree_slug}`, `{worktree_path}`,
`{worktree_root}`, `{branch}`, `{repo}`, plus `{env:PORT_NAME}` to embed a
declared port. Unresolved placeholders are errors here (unlike hook command
templates, which leave them intact).

Merge semantics: the scalar knobs (`salt`, `range`, `block_size`, `write`) merge
field-level — a `daft.local.yml` overriding just `salt:` is the local "reroll
everything" lever — while `ports:` and `values:` replace wholesale when an
overlay declares them.

`env.write` must not also appear in `shared:`: a shared dotenv is one central
file symlinked into every worktree, so per-worktree values would overwrite each
other. Validation refuses the pair.

::: warning Four meanings of "env"

This schema uses the word at four nesting depths with different semantics:
top-level `env:` declares _derived_ values (this section); a job-level `env:` is
a _literal_ K→V map for that job; `skip:`/`only:` take an `env:` that names a
variable as a _truthiness predicate_; and `DAFT_*` variables are _computed_ by
daft at hook time. When reading a config, the nesting depth tells you which one
you are looking at.

:::

## Relations

The Graph pillar's [relations manifest](/graph/concepts) lives in `daft.yml` as
a top-level `relations:` list — directed edges to the repositories this one
coordinates with:

```yaml
relations:
  - url: git@github.com:acme/api-client.git # required — the resolution key
    name: client # optional friendly label
    kind: consumer # optional, free-form
```

| Field  | Type   | Description                                                        |
| ------ | ------ | ------------------------------------------------------------------ |
| `url`  | string | Remote URL of the related repo (required; normalized for matching) |
| `name` | string | Friendly label used in output (optional)                           |
| `kind` | string | Free-form relationship kind, e.g. `client`, `library` (optional)   |

Manage this list with [`daft repo link`](/reference/cli/daft-repo-link) and
[`daft repo unlink`](/reference/cli/daft-repo-unlink) instead of editing it by
hand — they resolve names, paths, or URLs to the portable remote URL and edit
only the `relations:` block. Consumed by `daft exec --related`,
`daft start --with-related`, and `daft repo info`. Older daft versions ignore
the key.

## Merge gate policy

A top-level `merge:` block commits team policy on what `daft merge` may land —
the local equivalent of a branch protection rule, in git's own vocabulary:

```yaml
merge:
  ff: only # refuse merges that cannot fast-forward
  source_worktree: clean # source worktree must exist and be clean
```

| Field             | Values  | Description                                                     |
| ----------------- | ------- | --------------------------------------------------------------- |
| `ff`              | `only`  | Refuse any merge whose source does not contain the target's tip |
| `source_worktree` | `clean` | Refuse a source with a missing or dirty worktree                |

Enforced natively by `daft merge` (before pre-merge hooks fire, re-verified when
the ref moves) and relaxed only by explicit per-invocation flags
(`--no-ff-only`, `--source-worktree any`) — the YAML deliberately has no relax
spellings, so an overlay config can tighten policy but never loosen it. See
[Merge gate policy](/reference/cli/daft-merge#merge-gate-policy) for the full
semantics, including the single-source rule pre-merge hooks activate.

## Tasks

A top-level `tasks:` map defines named, user-invoked job groups, run with
[`daft run`](/reference/cli/daft-run). Tasks are the _serve on demand_ half of
the workflow: provisioning stays finite and unattended in
`worktree-post-create`, while starting dev servers, `docker compose` stacks, and
watchers becomes an explicit, attended `daft run`.

```yaml
tasks:
  run: # reserved default — bare `daft run`
    parallel: true
    jobs:
      - name: backend
        run: docker compose up
        env:
          COMPOSE_PROJECT_NAME: "api-{worktree_slug}"
      - name: web
        run: pnpm dev
        root: frontend
  seed-db: # `daft run seed-db`
    jobs:
      - name: seed
        run: ./scripts/seed.sh
```

A task body is a [hook entry](#hook-entries): it takes the same
`parallel`/`piped`/`follow`, `jobs`, and `skip`/`only` fields, and each job
takes the same [job entry](#job-entries) fields. Keeping tasks in their own
section (rather than as custom hook names) keeps hook-name validation strict.

Task-specific rules:

- **Names** must start with a letter or digit and contain only letters, digits,
  `.`, `_`, or `-`. The reserved name `run` is what bare `daft run` executes.
- **Arguments forward.** Words after the task name are shell-escaped and
  appended to the task's command (`daft run seed-db --reset` runs
  `./scripts/seed.sh --reset`); a first word naming no task forwards every word
  to the reserved `run` task, and a leading `--` forces forwarding past the name
  match. Forwarding requires the task to resolve to a single foreground job with
  a single-line command — narrow multi-job tasks with `--job`.
- **Jobs only** — the deprecated `commands:` form is rejected in tasks.
- **No execution timeout** — a task job runs until it exits or is cancelled
  (lifecycle-hook jobs keep the 300-second default). This makes tasks the right
  home for long-running processes.
- **Foreground** — a task that resolves to a single job passes the terminal
  straight through: the job inherits daft's stdio and its raw output is the
  whole interface, exactly as if you ran the command yourself. A multi-job task
  renders one live row per job with the logs threaded beneath. Either way,
  Ctrl+C cancels (twice to force-kill); there is no detached mode.
- **Trust** — an explicit `daft run` runs even in an untrusted repo (it counts
  as consent), unlike lifecycle hooks which are skipped until the repo is
  trusted.

The `daft-local.yml` overlay layers machine-local tasks on top of the committed
`daft.yml`, merged by name exactly like hooks.

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
| `exclude`      | list                 |         | Glob patterns appended to every file-aware job's `exclude` list            |
| `skip`         | bool / string / list |         | Skip condition (see [Skip and only conditions](#skip-and-only-conditions)) |
| `only`         | bool / string / list |         | Only condition (see [Skip and only conditions](#skip-and-only-conditions)) |
| `jobs`         | list                 |         | Jobs to execute                                                            |
| `fail_mode`    | `abort` / `warn`     | varies  | Behavior when this hook fails (see [Failure mode](#failure-mode))          |

Only one of `parallel`, `piped`, or `follow` can be set at a time.

### Failure mode

`fail_mode` controls what happens when a hook exits non-zero:

- `abort` — the failure is fatal and the daft operation stops.
- `warn` — the failure is reported and the operation continues.

Defaults are per hook type: `worktree-pre-create`, `worktree-post-create`, and
`pre-merge` default to `abort`; every other hook defaults to `warn`. Committing
`fail_mode:` ships that choice to every clone, so a repo can mark a best-effort
hook (a warmup, an optional setup step) non-fatal for everyone.

A local git config `daft.hooks.<hookName>.failMode` overrides the committed
value, so a developer can always change the mode on their own machine:

```bash
git config daft.hooks.worktreePostCreate.failMode warn
```

Both `fail_mode:` and the git config accept `abort` or `warn`
case-insensitively. If the git config holds an unrecognized value, daft ignores
it (the committed `fail_mode:` or the default applies) and prints a warning, so
a typo does not silently change the failure behavior.

`fail_mode` has no effect under `tasks:` — `daft run` stops on the first failing
job regardless.

## Job entries

Each job in the `jobs` list supports:

| Field               | Type                 | Description                                                                                                  |
| ------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------ |
| `name`              | string               | Job name (used for display, merging, and dependency references)                                              |
| `description`       | string               | Human-readable description (shown in dry-run and completions)                                                |
| `run`               | string               | Inline shell command to execute                                                                              |
| `script`            | string               | Script file to run (relative to `source_dir`)                                                                |
| `runner`            | string               | Interpreter for script files (e.g., `"bash"`, `"python"`)                                                    |
| `args`              | string               | Arguments to pass to the script                                                                              |
| `root`              | string               | Working directory / cwd, relative to worktree root (see [Working directory](#working-directory-root))        |
| `tags`              | list                 | Tags for filtering with `exclude_tags`                                                                       |
| `glob`              | string / list        | Changed-file patterns gating this job (see [Changed-file filters](#changed-file-filters-glob-exclude-files)) |
| `exclude`           | list                 | Changed-file patterns removed from this job's list                                                           |
| `files`             | string               | Shell command producing this job's file list (one path per line)                                             |
| `skip`              | bool / string / list | Skip condition                                                                                               |
| `only`              | bool / string / list | Only condition                                                                                               |
| `arch`              | string / list        | Target architecture (`x86_64`, `aarch64`); skips if no match                                                 |
| `env`               | map                  | Extra environment variables                                                                                  |
| `fail_text`         | string               | Custom failure message                                                                                       |
| `interactive`       | bool                 | Job needs TTY/stdin (forces sequential execution)                                                            |
| `priority`          | int                  | Execution ordering (lower runs first)                                                                        |
| `needs`             | list                 | Names of jobs that must complete before this job runs                                                        |
| `tracks`            | list                 | Worktree attributes this job depends on: `path`, `branch`                                                    |
| `group`             | object               | Nested group of jobs (see [Groups](#groups))                                                                 |
| `background`        | bool                 | Run this job in the background (see [Background jobs](#background-jobs))                                     |
| `background_output` | `log` / `silent`     | Output behavior for background jobs (default: `log`)                                                         |
| `log`               | object               | Log configuration (`retention`, `max_log_size`) for this job                                                 |

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

### Changed-file filters (`glob`, `exclude`, `files`)

A job can scope itself to the files the operation actually changed. For merge
hooks (`pre-merge`, `post-merge`) the changed set is what the merge sources
changed relative to the target — the three-dot `target...source` diff, unioned
across sources. Other hook types have no built-in changed set; a job there may
supply its own with `files:`.

```yaml
hooks:
  pre-merge:
    exclude:
      - "**/*.lock" # hook-level: appended to every file-aware job below
    jobs:
      - name: build-check
        glob: ["src/**", "Cargo.*"]
        run: cargo check --all-targets
      - name: lint-changed
        glob: "*.{js,ts}"
        exclude: ["web/generated/**"]
        run: eslint {changed_files}
      - name: docs-build
        glob: "docs/**"
        run: mise run docs:site:build
```

Semantics:

- `glob` selects files (string or list); `exclude` removes files and wins over
  `glob`. Hook-level `exclude` is appended to every file-aware job's list.
- Patterns match **repository-root-relative** paths — `root:` moves the job's
  cwd, never what its patterns see. Matching uses standard doublestar rules: `*`
  and `?` stop at `/`, `**` spans zero or more directories (`**/*.js` matches
  `app.js` and `src/app.js` alike), braces expand (`*.{js,ts}`), and matching is
  case-sensitive.
- **Empty means skip.** When no changed file survives the filter, the job is
  skipped as a first-class outcome with the reason recorded (even when the
  command references no file template). A docs-only merge skips the
  `src/**`-gated build ring instead of running it against nothing.
- `{changed_files}` in the `run` command expands to the filtered list,
  shell-quoted and space-joined.
- `files:` replaces the operation's changed set with the output of a shell
  command (run via `sh -c` in the hook's working directory, one
  repository-root-relative path per line). An empty result skips the job; a
  non-zero exit fails the hook.
- A job that declares `glob`/`exclude` (or uses `{changed_files}`) on a hook
  type with no changed set and no `files:` command is a **configuration error**
  — the hook fails loudly rather than guessing.
- `exclude` alone (no `glob`) selects every changed file outside the excluded
  paths: the job runs unless _only_ excluded paths changed, which makes it the
  natural "skip on docs-only changes" spelling.

### Template variables

Job `run`/`script` commands **and** job `env:` values support template variables
that are replaced with values from the execution context (this applies to both
lifecycle hooks and `daft run` tasks):

| Variable              | Description                                                                                                                                     |
| --------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `{branch}`            | Target branch name (alias for `{worktree_branch}`)                                                                                              |
| `{worktree_path}`     | Path to the target worktree                                                                                                                     |
| `{worktree_root}`     | Project root directory                                                                                                                          |
| `{worktree_slug}`     | Sanitized worktree name — `[a-z0-9-]`, capped at 63                                                                                             |
| `{worktree_branch}`   | Target branch name                                                                                                                              |
| `{source_worktree}`   | Path to the source worktree (where command was invoked)                                                                                         |
| `{git_dir}`           | Path to the `.git` directory                                                                                                                    |
| `{remote}`            | Remote name (usually `"origin"`)                                                                                                                |
| `{job_name}`          | Name of the current job                                                                                                                         |
| `{base_branch}`       | Base branch name (for `checkout -b` commands)                                                                                                   |
| `{commit}`            | Pinned commit OID (anonymous sandbox worktrees only)                                                                                            |
| `{repository_url}`    | Repository URL (for `post-clone`)                                                                                                               |
| `{default_branch}`    | Default branch name (for `post-clone`)                                                                                                          |
| `{changed_files}`     | The job's filtered changed-file list, shell-quoted (file-aware jobs only; see [Changed-file filters](#changed-file-filters-glob-exclude-files)) |
| `{merge_source_path}` | Merge hooks: the source's worktree path (single worktree-backed source only)                                                                    |
| `{merge_target_path}` | Merge hooks: the target's worktree path                                                                                                         |

`{worktree_slug}` is the worktree's path relative to the project root (falling
back to the directory name), lowercased with every run of non-`[a-z0-9]`
characters collapsed to a single `-` and the result capped at the 63-character
DNS-label limit. Because it is keyed off the worktree rather than the branch, it
is unique per worktree and stable even when the worktree is not on a branch —
use it to keep per-worktree names collision-free, e.g.
`COMPOSE_PROJECT_NAME: "api-{worktree_slug}"`.

For anonymous sandbox worktrees (`daft go <commit-ish>`, `daft start --fork`)
the branch variables substitute to the empty string — the contract is "empty
means no branch" — and `{commit}` (env: `DAFT_COMMIT`) carries the commit the
sandbox is pinned at. `{worktree_slug}` works unchanged, which makes it the
right handle for per-worktree resources in hooks that must serve both branch and
sandbox worktrees.

The merge-path templates resolve only when the corresponding worktree exists
(`{merge_source_path}` additionally requires exactly one source). They are legal
in `root:` — the canonical gate shape runs each ring in the source worktree:

```yaml
hooks:
  pre-merge:
    jobs:
      - name: build-check
        root: "{merge_source_path}"
        run: cargo check --all-targets
```

`root:` resolution is fail-closed: a `{merge_…}` template that cannot resolve
(worktree-less source, multiple sources, non-merge hook) aborts the hook rather
than running the job in the hook's own directory.

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
  - changed: "docs/**" # Changed: skip if a changed file matches
```

Named conditions:

| Name     | Triggers when                                                      |
| -------- | ------------------------------------------------------------------ |
| `merge`  | Git is in a merge state (`MERGE_HEAD` exists)                      |
| `rebase` | Git is in a rebase state (`rebase-merge` or `rebase-apply` exists) |

Structured condition fields:

| Field     | Description                                                                                                                       |
| --------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `ref`     | Glob pattern matched against the current branch name                                                                              |
| `env`     | Environment variable name; truthy = condition met                                                                                 |
| `run`     | Shell command; exit code 0 = condition met                                                                                        |
| `changed` | Glob pattern(s); any changed file matching = condition met (see [Changed-file filters](#changed-file-filters-glob-exclude-files)) |
| `desc`    | Human-readable reason shown when the condition triggers a skip                                                                    |

A `changed:` rule reads the same changed-file set as the job-level `glob:`
field, so `skip: [{changed: "docs/**", desc: docs-only change}]` skips a job
when the merge touches docs, and `only: [{changed: "docs/**"}]` runs it only
then. Using `changed:` on a hook type with no changed-file source is a
configuration error.

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
- **`copy`**: replaced **wholesale**. An overlay that restates `copy:` replaces
  the paths _and_ the knobs — there is no element-wise union, so a local
  override is always a complete restatement (a base `fallback: skip` does not
  survive into an overlay that omits it)

The `copy:` key is read through this merge, so a `daft.local.yml` overlay or an
`extends:` file can declare or override it without touching the committed
config.

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

There is no `os:` field. OS targeting lives in `run:`, which may be an OS-keyed
map (`macos`, `linux`, `windows`) instead of a string; a job whose map has no
entry for the current OS is skipped. `arch:` is separate and constrains the
architecture.

```yaml
- name: install-brew
  description: Install Homebrew package manager
  run:
    macos:
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
