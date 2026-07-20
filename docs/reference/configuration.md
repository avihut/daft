---
title: Configuration
description: All daft configuration options
---

# Configuration

daft reads configuration from Git's config system. Settings are loaded with
standard Git priority: repository-local config overrides global config, which
overrides built-in defaults.

## Setting Values

```bash
# Set for the current repository
git config daft.autocd false

# Set globally (all repositories)
git config --global daft.autocd false
```

## General Settings

| Key                 | Default    | Description                                                                               |
| ------------------- | ---------- | ----------------------------------------------------------------------------------------- |
| `daft.autocd`       | `true`     | CD into new worktrees when using shell wrappers                                           |
| `daft.remote`       | `"origin"` | Default remote name for all operations                                                    |
| `daft.updateCheck`  | `true`     | Show notifications when a new daft version is available                                   |
| `daft.gitoxide`     | `true`     | Use gitoxide for supported Git operations; `false` opts out to the git-subprocess backend |
| `daft.go.autoStart` | `false`    | Auto-create worktree when branch not found in `daft go`                                   |

## Layout Settings

Layout configuration uses `~/.config/daft/config.toml` (TOML format), not
`git config`. This is different from the other settings on this page.

See the [Layouts guide](/worktrees/layouts) for detailed explanations of each
layout.

| Key                | File                 | Description                                 |
| ------------------ | -------------------- | ------------------------------------------- |
| `defaults.layout`  | `config.toml`        | Global default layout name or template      |
| `layout`           | `daft.yml` (in repo) | Team-recommended layout for this repository |
| `[layouts.<name>]` | `config.toml`        | Custom layout definition                    |

### Global Default

```toml
# ~/.config/daft/config.toml
[defaults]
layout = "contained"
```

Or use the command: `daft layout default contained`

### Custom Layouts

```toml
# ~/.config/daft/config.toml
[layouts.my-team]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"

[layouts.isolated]
template = "~/worktrees/{{ repo }}/{{ branch | sanitize }}"
```

### Team Convention (daft.yml)

```yaml
# daft.yml (committed to repository)
layout: contained
```

## Remote Sync Settings

By default, daft does not contact the remote during worktree management. All
remote operations are opt-in. Use `daft config remote-sync` to toggle these
settings interactively, or set them directly with `git config`.

| Key                        | Default           | Description                                                                                                         |
| -------------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------- |
| `daft.checkout.fetch`      | `false`           | Fetch from remote before creating a worktree for an existing branch                                                 |
| `daft.checkout.push`       | `false`           | Push new branches to remote after creation                                                                          |
| `daft.pushVerify`          | `auto`            | When ref-only pushes (remote-branch deletes, the upstream push) run the pre-push hook — see [Git Hooks](#git-hooks) |
| `daft.checkout.pushVerify` | `daft.pushVerify` | Checkout-scoped override of `daft.pushVerify` for the automatic upstream push (`auto`, `always`, `never`)           |
| `daft.branchDelete.remote` | `false`           | Delete the remote branch when removing a local branch                                                               |

### Enabling Remote Sync

Use the interactive command to toggle remote sync settings:

```bash
daft config remote-sync         # Open interactive TUI
daft config remote-sync --on    # Enable all remote sync operations
daft config remote-sync --off   # Disable all remote sync operations
daft config remote-sync --status  # Show current settings
```

You can also set values directly:

```bash
# Enable all remote sync (opt in to old behavior)
git config daft.checkout.fetch true
git config daft.checkout.push true
git config daft.branchDelete.remote true
```

Per-command overrides are available via `--local` (skip remote operations) and
`--remote` (delete remote branch only, without removing local worktree or
branch).

## Checkout Settings

| Key                         | Default | Description                                                   |
| --------------------------- | ------- | ------------------------------------------------------------- |
| `daft.checkout.upstream`    | `true`  | Set upstream tracking for branches                            |
| `daft.checkout.carry`       | `false` | Carry uncommitted changes when checking out existing branches |
| `daft.checkoutBranch.carry` | `true`  | Carry uncommitted changes when creating new branches          |

## Forge Settings

For checking out pull/merge requests (`daft go pr:123`, `mr:45`, or a PR/MR
URL). daft shells out to `gh`/`glab`, which supply the authentication — daft
stores no tokens. These are the only knobs daft itself owns.

| Key                    | Default | Description                                                                                     |
| ---------------------- | ------- | ----------------------------------------------------------------------------------------------- |
| `daft.forge.platform`  |         | Force the platform for an ambiguous remote: `github` or `gitlab`. Unset means detect by remote. |
| `daft.forge.githubCli` | `gh`    | Override the GitHub CLI binary (for Enterprise wrappers)                                        |
| `daft.forge.gitlabCli` | `glab`  | Override the GitLab CLI binary                                                                  |
| `daft.forge.hostname`  |         | Forge hostname for self-hosted / Enterprise instances (passed to the CLI as `--hostname`)       |

Run `daft doctor` to check whether `gh`/`glab` are installed and authenticated.

## Update Settings

| Key                | Default       | Description                                                                    |
| ------------------ | ------------- | ------------------------------------------------------------------------------ |
| `daft.update.args` | `"--ff-only"` | Default arguments passed to `git pull` in update operations (same-branch mode) |

## List Settings

| Key                         | Default     | Description                                                                                                                                                                                                  |
| --------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `daft.list.stat`            | `"summary"` | Default statistics mode for list command (`summary` or `lines`)                                                                                                                                              |
| `daft.list.columns`         |             | Default column selection for list command (e.g., `branch,path,age` or `+size,-annotation`)                                                                                                                   |
| `daft.list.sort`            |             | Default sort order for list command (e.g., `+branch`, `-activity`, `+owner,-size`). Sortable: branch, path, size, age, owner, activity                                                                       |
| `daft.list.sizeConcurrency` |             | Max concurrent directory-size walks for `--columns +size` on both `daft list` and `daft repo list`. Default: CPU count. Lower on slow/network filesystems. The `DAFT_SIZE_WALK_JOBS` env var overrides this. |

## Prune Settings

| Key                   | Default     | Description                                                                                                                 |
| --------------------- | ----------- | --------------------------------------------------------------------------------------------------------------------------- |
| `daft.prune.cdTarget` | `"root"`    | Where to cd after pruning the current worktree. Values: `root` (project root) or `default-branch` (default branch worktree) |
| `daft.prune.stat`     | `"summary"` | Default statistics mode for prune command (`summary` or `lines`)                                                            |
| `daft.prune.columns`  |             | Default column selection for prune command                                                                                  |
| `daft.prune.sort`     |             | Default sort order for prune command (e.g., `+branch`, `-activity`)                                                         |

## Sync Settings

| Key                          | Default        | Description                                                                                                                                                                                                   |
| ---------------------------- | -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft.sync.stat`             | `"summary"`    | Default statistics mode for sync command (`summary` or `lines`)                                                                                                                                               |
| `daft.sync.columns`          |                | Default column selection for sync command                                                                                                                                                                     |
| `daft.sync.sort`             |                | Default sort order for sync command (e.g., `+branch`, `-activity`)                                                                                                                                            |
| `daft.sync.pushTimeout`      | `"30m"`        | Wall-clock budget per push unit (git + pre-push hook). A hung hook is torn down and the push fails with a hint; `off` (or `0`) disables                                                                       |
| `daft.sync.pushHookStrategy` | `"per-branch"` | Pre-push hook cadence for `sync --push`: `per-branch` runs the hook once per branch; `batched` pushes every branch in one `git push` so the hook fires once with all refs (one refusal fails the whole batch) |

## Governor Settings

`daft sync --push` over many branches runs the repo's pre-push hook once per
branch, concurrently. Heavy hooks (test suites, builds) are internally parallel,
so the aggregate footprint multiplies — enough to exhaust machine memory. The
resource governor keeps that fan-out inside the machine's memory budget: it caps
concurrent hook-bearing pushes, admits new ones only while memory headroom
allows, learns each hook's peak memory and duration across runs (so light hooks
get full parallelism and heavy ones start capped), and under sustained pressure
freezes — then kills and retries — the newest push rather than letting the
machine swap. It only exists while a `sync --push` with a pre-push hook runs; a
hook-less push pays nothing.

| Key                           | Default  | Description                                                                                                                                                                                                                           |
| ----------------------------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft.governor.mode`          | `"auto"` | The governor (`auto`) or no governing at all (`off`). `sync --no-throttle` disables it for one run                                                                                                                                    |
| `daft.governor.jobs`          | `"auto"` | Cap on concurrent hook-bearing pushes: `auto` = max(2, cores/4), or a number. `sync --jobs N` overrides per run                                                                                                                       |
| `daft.governor.memoryReserve` | `"auto"` | Memory the governor keeps free: `auto` = max(10% RAM, 2G), a size (`2G`, `512M`), or a percent (`15%`)                                                                                                                                |
| `daft.governor.jobserver`     | `"auto"` | Export a shared POSIX jobserver (`MAKEFLAGS`) so make/cargo/ninja inside concurrent hooks share one token pool. Note: a bare `make` in a hook picks up `-jN` and becomes (bounded) parallel — set `off` if a hook can't tolerate that |

## Merge Settings

Defaults for `daft merge` flags. Each key can be set globally, locally, or
system-wide; CLI flag arguments always override the configured default. The
canonical reference for these keys is the
[`daft merge` CLI page](/reference/cli/daft-merge#configuration).

| Key                                  | Default           | Description                                                                             |
| ------------------------------------ | ----------------- | --------------------------------------------------------------------------------------- |
| `daft.merge.style`                   | `merge`           | Default merge style: `merge`, `squash`, `rebase`, or `rebase-merge`                     |
| `daft.merge.cleanup`                 | `keep`            | Default post-merge cleanup: `keep` or `remove-branch`                                   |
| `daft.merge.edit`                    | _(git's default)_ | Default for the merge-message editor on a TTY (`true`/`false`)                          |
| `daft.merge.commit`                  | `true`            | Default commit-after-squash behavior (`false` is equivalent to `--no-commit`)           |
| `daft.merge.signoff`                 | `false`           | Default for `--signoff`                                                                 |
| `daft.merge.gpgSign`                 | _(unset)_         | Default for `--gpg-sign` (`true`, `false`, or `<keyid>`)                                |
| `daft.merge.verifySignatures`        | `false`           | Default for `--verify-signatures`                                                       |
| `daft.merge.allowUnrelatedHistories` | `false`           | Default for `--allow-unrelated-histories`                                               |
| `daft.merge.strategy`                | _(unset)_         | Default merge strategy (`-s` / `--strategy`)                                            |
| `daft.merge.strategyOption`          | _(unset)_         | Default strategy option (`-X` / `--strategy-option`); repeatable via multi-value config |
| `daft.merge.adoptTargetOnDemand`     | `prompt`          | How to handle merging into a branch with no worktree: `prompt`, `yes`, or `no`          |
| `daft.merge.requireCleanTarget`      | `true`            | Refuse to start the merge when the target worktree has uncommitted changes              |

`--set-default` writes `daft.merge.style` and `daft.merge.cleanup` after a
successful merge, so a failed or conflicted merge never silently changes your
defaults.

### Default squash + cleanup recipe

To make `daft merge <source>` always squash and remove the source branch on
success:

```bash
git config daft.merge.style squash
git config daft.merge.cleanup remove-branch
```

For non-interactive or CI use, also suppress the editor so the auto-generated
squash message is used verbatim:

```bash
git config daft.merge.edit false
```

`daft merge feature/done` then becomes a one-shot squash + commit + branch
cleanup. The migration table in the
[`daft merge` reference](/reference/cli/daft-merge#migration-from-the-old-flag-set)
covers the v1.9 → v1.10 key renames if you're migrating an older config.

See the [`daft merge` reference](/reference/cli/daft-merge) for flag-level
details and the [Lifecycle hooks reference](/hooks/lifecycle#merge-hooks) for
`pre-merge` / `post-merge` hook configuration.

## Ownership Settings

Controls how daft determines which branches are "yours" for the purposes of
`daft sync --rebase` / `--push`, the Owner column in `daft list` / `daft sync` /
`daft prune`, and the sync TUI divider between owned and unowned branches.

| Key                       | Default             | Description                                                                                                                                               |
| ------------------------- | ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft.ownership.strategy` | `recency-plurality` | Strategy for deducing branch ownership from the `base..branch` commit range. Values: `tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`. |

Each strategy decides the branch owner from the commits in the range
`base..branch` (your branch minus everything already in the default branch):

- **`tip`** — owner is the author of the newest commit. Fast and simple, but
  flips ownership whenever a teammate or bot pushes a single commit on top.
- **`any`** — owner is you if any commit in range is yours; otherwise the tip
  author. Most permissive.
- **`first`** — owner is the author of the oldest commit in range — "who started
  this branch."
- **`plurality`** — owner is the author with the most commits. Ties broken by
  most-recent-commit-of-tied-author.
- **`majority`** — owner is the author with strictly more than 50% of commits.
  No owner if no majority.
- **`recency-plurality`** (default) — owner is the author with the highest
  recency-weighted score. Each commit at rank `k` from the tip (`k=0` = tip)
  contributes weight `1/(k+1)`. Ties broken by most-recent-commit. Matches the
  intuition "favor recent work" while staying robust to drive-by commits on top.

## Multi-Remote Settings

| Key                              | Default    | Description                                          |
| -------------------------------- | ---------- | ---------------------------------------------------- |
| `daft.multiRemote.enabled`       | `false`    | Enable multi-remote directory organization           |
| `daft.multiRemote.defaultRemote` | `"origin"` | Default remote for new branches in multi-remote mode |

## Hooks Settings

| Key                        | Default                 | Description                                                                 |
| -------------------------- | ----------------------- | --------------------------------------------------------------------------- |
| `daft.hooks.enabled`       | `true`                  | Master switch for all hooks                                                 |
| `daft.hooks.defaultTrust`  | `"deny"`                | Default trust level for unknown repositories (`deny`, `prompt`, or `allow`) |
| `daft.hooks.userDirectory` | `~/.config/daft/hooks/` | Path to user-global hooks directory                                         |
| `daft.hooks.timeout`       | `300`                   | Hook execution timeout in seconds                                           |
| `daft.hooks.trustPrune`    | `true`                  | Auto-prune stale entries from the trust database (background, once per 24h) |

### Per-Hook Settings

Each hook type can be configured individually. The hook name uses camelCase.

| Key                              | Default | Description                            |
| -------------------------------- | ------- | -------------------------------------- |
| `daft.hooks.<hookName>.enabled`  | `true`  | Enable/disable a specific hook type    |
| `daft.hooks.<hookName>.failMode` | varies  | Behavior on failure: `abort` or `warn` |

Hook names: `postClone`, `worktreePreCreate`, `worktreePostCreate`,
`worktreePreRemove`, `worktreePostRemove`.

Default fail modes:

- `worktreePreCreate`: `abort` (setup must succeed before creating worktree)
- All others: `warn` (don't block operations)

### Hook Output Settings

| Key                            | Default | Description                                                                                                                                                                  |
| ------------------------------ | ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft.hooks.output.quiet`      | `false` | Suppress hook stdout/stderr                                                                                                                                                  |
| `daft.hooks.output.timerDelay` | `5`     | Seconds before a silent job shows an elapsed timer (verbose output only)                                                                                                     |
| `daft.hooks.output.tailLines`  | `6`     | Live rolling output lines per job in verbose output (0 = none); the persisted log is never windowed. Also sizes the live window of `daft exec`'s per-worktree output threads |
| `daft.hooks.output.verbose`    | `false` | Thread each hook job's log through the [progress timeline](/reference/progress-timeline) (`-v` per invocation does the same); in plain output, show each job's command line  |

### YAML Hooks Configuration

Hooks can also be configured through a `daft.yml` file for richer features
including multiple jobs, execution modes, job dependencies, and conditional
execution. The same file's top-level `tasks:` section defines named,
user-invoked task groups run with [`daft run`](/reference/cli/daft-run) — the
_serve on demand_ counterpart to lifecycle hooks.

See the [Hooks guide](/hooks/yaml-reference) for the complete `daft.yml`
reference, including the [Tasks](/hooks/yaml-reference#tasks) section.

## Examples

```bash
# Enable remote sync (fetch on checkout, push on start, delete remote on remove)
daft config remote-sync --on

# Or enable individual settings
git config daft.checkout.fetch true
git config daft.checkout.push true
git config daft.branchDelete.remote true

# Use a different remote
git config daft.remote upstream

# Disable auto-cd globally
git config --global daft.autocd false

# After pruning, cd to default branch worktree instead of root
git config daft.prune.cdTarget default-branch

# Auto-create worktree when branch not found in daft go
git config daft.go.autoStart true

# Use rebase-style update by default
git config daft.update.args "--rebase"

# Disable hooks globally
git config --global daft.hooks.enabled false

# Make post-create hooks abort on failure
git config daft.hooks.worktreePostCreate.failMode abort
```

## Environment Variables

| Variable                  | Description                                                                                  |
| ------------------------- | -------------------------------------------------------------------------------------------- |
| `DAFT_CD_FILE`            | Temp file path for shell wrapper CD communication (set by shell wrappers)                    |
| `DAFT_NO_HINTS`           | Set to suppress contextual hint messages                                                     |
| `DAFT_NO_TRUST_PRUNE`     | Set to disable automatic trust database pruning                                              |
| `DAFT_NO_UPDATE_CHECK`    | Set to disable version update notifications                                                  |
| `DAFT_NO_BACKGROUND_JOBS` | Set to promote all background hook jobs to foreground                                        |
| `DAFT_SIZE_WALK_JOBS`     | Override directory-size-walk concurrency (takes precedence over `daft.list.sizeConcurrency`) |
| `NO_COLOR`                | Standard variable to disable colored output                                                  |
| `PAGER`                   | Override the pager for `daft release-notes`                                                  |

## Git Hooks

Every daft-initiated `git push` honors the repository's `pre-push` hook --
native `.git/hooks`, or hook managers registered through `core.hooksPath`
(lefthook, husky, pre-commit). A pre-push secret scanner or test gate that fires
on `git push` fires on daft's pushes too. The hook run is reported as a
`pre-push` phase with the hook's output, using the same surface as daft's
lifecycle hooks.

Pushes that provably carry no content are the conditional sites: the hook runs
only when there is something for a content gate to validate. Two kinds of push
qualify as ref-only:

- **The automatic upstream push** on `daft start` / `git worktree-checkout -b`,
  when branching off a fully-pushed base -- a new branch name pointing at
  commits the remote already has.
- **Remote-branch deletes** -- `daft remove` / `git worktree-branch -d` with
  remote deletion enabled, `daft rename`'s old-name cleanup, and
  `daft multi-remote move --delete-old`. A delete pushes a null ref update and
  zero objects, so it is ref-only by construction.

Control this with `daft.pushVerify` (the base setting every such site reads;
`daft.checkout.pushVerify` overrides it for the upstream push alone):

| Value    | Behavior                                                      |
| -------- | ------------------------------------------------------------- |
| `auto`   | (default) Run the hook only when the push carries new commits |
| `always` | Run the hook on every such push, including ref-only ones      |
| `never`  | Never run the hook on these pushes                            |

`always` is for repositories whose pre-push hooks act on the ref rather than the
content (branch-naming policies, protected-branch guards that reject deleting
`release/*` and friends): daft cannot classify an opaque hook, so the automatic
skip is based purely on pushed content. Content-carrying pushes -- `daft push`,
`daft sync --push`, and `daft rename`'s new-name push -- always verify and do
not consult these settings.

Pass `--no-verify` to skip the hook for one invocation. Every command that can
push accepts it: `daft push`, `daft sync --push`, `daft start` / `daft go -b` /
`git worktree-checkout -b`, `daft rename`, `daft remove` /
`git worktree-branch -d/-D`, and `daft multi-remote move`.

Remote operations are disabled by default. When enabled (via
`daft config remote-sync --on` or by setting the individual keys), this affects:

- **`daft start` / `git worktree-checkout -b`** -- pushes the new branch to set
  upstream tracking (controlled by `daft.checkout.push`; pre-push hook gating
  per `daft.checkout.pushVerify` above)
- **`daft remove` / `git worktree-branch -d`** -- pushes `--delete` to remove
  the remote branch (controlled by `daft.branchDelete.remote`; pre-push hook
  gating per `daft.pushVerify` above)
- **`daft multi-remote move --push`** -- pushes an existing branch to a new
  remote

Push failures are graded by the gate. When a `pre-push` hook is installed and
the push fails, the command exits non-zero -- a gate saying no is a failure, not
a warning (any worktree the command created or moved is still completed and
usable, and the error names the manual recovery command). Without a hook, push
failures (network issues, auth errors, remote rejection rules) stay non-fatal:
the local worktree and branch remain usable and a warning is shown.

Use `--local` on any worktree command to skip all remote operations for that
invocation, regardless of config.

::: tip daft's [lifecycle hooks](/hooks/) (configured in `daft.yml` or
`.daft/hooks/`) are separate from git's own hooks and are always executed
normally. :::
