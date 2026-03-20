# Progressive Adoption and Layout System

## Overview

Redesign daft to support progressive adoption by decoupling worktree management
from a specific repository layout. Users can start using daft on existing repos
with zero configuration, then progressively adopt more features. Layouts become
a configurable, named concept with templates controlling worktree placement.
Bare repos are an implementation detail inferred from template geometry, never
exposed as a user-facing concept.

## Motivation

daft currently requires the "contained" layout (bare repo + worktrees as
children of project root) for all operations. This is a high barrier to entry:
users must either `daft clone` (producing an unfamiliar layout) or `daft adopt`
(restructuring their repo) before using any features. Competitors in the
worktree management space show that users want easy parallelization on top of
their existing git workflow, not a layout migration as a prerequisite.

## Progressive Adoption Ladder

### Level 0 -- "I have a repo, I want a worktree"

User has `~/projects/myrepo` cloned with `git clone`. Runs
`daft start feature/login`. Worktree appears at
`~/projects/myrepo.feature-login/`. No new concepts introduced.

### Level 1 -- "Let me use daft to clone"

`daft clone <url>` respects the user's default layout. First-time hint about
layout options. `post-clone` hook fires if `daft.yml` exists. Concepts
introduced: `daft clone` as a better clone.

### Level 2 -- "I want a different layout"

Discovers layouts via `daft layout list`. Tries contained layout via
`daft layout transform contained` on existing repos or
`daft clone --layout contained <url>` for new ones. Sets global default in
config. Concepts introduced: layouts.

### Level 3 -- "I want automation"

Adds `daft.yml` with hooks. `post-clone` for repo setup, `worktree-post-create`
for per-branch setup. Manages trust via `daft hooks trust`. Concepts introduced:
hooks, trust.

### Level 4 -- "Power user"

Custom layout templates, `--at` for ad-hoc worktree placement, detached HEAD
sandboxes, `daft sync`, multi-remote.

## Layout System

### Layout Definition

A layout is a named template string. Bare repo requirement is inferred from the
template geometry at resolution time.

### Built-in Layouts

| Name          | Template                                          | Inferred bare | Description                                          |
| ------------- | ------------------------------------------------- | ------------- | ---------------------------------------------------- |
| `contained`   | `{{ branch }}`                                    | Yes           | Worktrees as children of project root                |
| `sibling`     | `../{{ repo }}.{{ branch \| sanitize }}`          | No            | Worktrees adjacent to the repo                       |
| `nested`      | `.worktrees/{{ branch \| sanitize }}`             | No            | Worktrees in a hidden subdirectory (auto-gitignored) |
| `centralized` | `~/worktrees/{{ repo }}/{{ branch \| sanitize }}` | No            | Worktrees in a global directory                      |

The built-in default layout is `sibling`.

### Template Variables

| Variable                   | Description                         | Example           |
| -------------------------- | ----------------------------------- | ----------------- |
| `{{ repo_path }}`          | Absolute path to the repo root      | `/home/me/myproj` |
| `{{ repo }}`               | Repository directory name           | `myproj`          |
| `{{ branch }}`             | Raw branch name                     | `feature/auth`    |
| `{{ branch \| sanitize }}` | Filesystem-safe (slashes to dashes) | `feature-auth`    |

Templates that do not start with `~/`, `/`, or `../` are resolved relative to
`{{ repo_path }}`.

### Bare Inference Heuristic

Given a template resolved relative to `repo_path`:

1. Starts with `../` or is absolute or starts with `~/` -- **not bare** (outside
   repo)
2. First path segment starts with `.` -- **not bare** (hidden directory)
3. Otherwise -- **bare required** (worktrees would conflict with working tree)

Bare is a structural implementation detail. Users never need to know about it.
daft infers it, manages it, and hides it.

Custom layouts can override the inference with an explicit `bare` field when the
heuristic produces the wrong result (e.g., a visible subdirectory layout that
should not be bare):

```toml
[layouts.visible-subdir]
template = "worktrees/{{ branch | sanitize }}"
bare = false
```

When `bare` is explicitly set, the heuristic is skipped. When omitted, the
heuristic applies. Built-in layouts never need this field.

### Auto-gitignore for Non-Bare In-Repo Layouts

When a non-bare layout places worktrees inside the repo (e.g., `nested` uses
`.worktrees/`), daft automatically adds the worktree directory to `.gitignore`
when creating the first worktree. This prevents `git status` from showing
worktree checkouts as untracked files. The `.gitignore` entry is added
idempotently (not duplicated if already present).

### Graceful Degradation

When a user's resolved layout implies bare but the repo is non-bare (e.g., user
ran `daft start` on an existing regular clone with default layout set to
`contained`): resolve the template as if non-bare, place worktrees per template
relative to `repo_path`, and warn with a suggestion to run
`daft layout transform`.

### Custom Layouts

Users define custom layouts in their global config:

```toml
# ~/.config/daft/config.toml
[defaults]
layout = "sibling"

[layouts.my-custom]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
```

## Configuration Resolution

```
CLI --layout flag
  |
  v (if not set)
Unified repo store: per-repo layout (repos.json)
  |
  v (if not set)
daft.yml: layout field (committed, team convention)
  |
  v (if not set)
Global config: defaults.layout (~/.config/daft/config.toml)
  |
  v (if not set)
Built-in default: "sibling"
```

### daft.yml Layout Field

The `layout` field in `daft.yml` allows teams to suggest a layout convention for
the repo. It accepts a named layout or an inline template string:

```yaml
# Named layout
layout: contained

# Inline template
layout: "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
```

This is a **suggestion** -- the per-repo store (`repos.json`) and CLI flags take
precedence, allowing individual developers to override the team convention.

### Unified Repo Store

The existing `~/.config/daft/trust.json` is replaced by
`~/.config/daft/repos.json` -- a unified per-repo store that holds trust
settings, layout choice, and future per-repo preferences.

#### Schema

```json
{
  "version": 3,
  "repositories": {
    "/Users/user/projects/myrepo/.git": {
      "trust": {
        "level": "allow",
        "granted_at": 1738060200,
        "granted_by": "user",
        "fingerprint": "https://github.com/user/myrepo.git"
      },
      "layout": "contained"
    }
  },
  "patterns": [
    {
      "pattern": "/Users/user/work/**/.git",
      "trust_level": "allow",
      "comment": "Work projects"
    }
  ]
}
```

Keyed by canonicalized `.git` directory path. Remote URL fingerprint for
identity verification. Auto-pruning of stale entries (repos that no longer exist
on disk).

#### Migration from trust.json

When daft loads and finds `trust.json` but no `repos.json`:

1. Read `trust.json` (V2)
2. Wrap each repository entry's trust fields under a `trust` key
3. Set version to 3
4. Write `repos.json` to a temporary file in the same directory
5. Atomically rename the temporary file to `repos.json`
6. Only if step 5 succeeded: remove `trust.json`. If the rename fails, abort the
   migration and leave `trust.json` intact.

The write-then-rename approach ensures that if daft crashes mid-migration,
either the old `trust.json` is still intact or the new `repos.json` is complete.
No data loss window.

## Command Changes

### `daft clone`

- Resolves layout from: `--layout` flag > global config default > `sibling`
- `--layout` accepts a named layout or an inline template string
- If layout needs bare (inferred) -- `git clone --bare`, create first worktree
- If layout does not need bare -- `git clone`
- Stores chosen layout in `repos.json`
- Runs `post-clone` hook
- For non-bare clones: also fires `worktree-post-create` (the clone both creates
  a repo and results in a worktree)
- **Post-clone layout reconciliation**: after a successful clone, if no
  `--layout` flag was given and no global config default is set, daft reads the
  cloned repo's `daft.yml` from the default branch. If it contains a `layout`
  field, daft applies it: if the current layout differs from the `daft.yml`
  suggestion, daft transforms to the suggested layout and stores it in
  `repos.json`. This ensures team conventions in `daft.yml` take effect for new
  clones without requiring explicit flags.
- First-time informational hint about layout options (shown once, suppressed via
  global config flag)

### `daft start` / `daft go`

- Resolves layout from the config chain
- Computes worktree path from template
- If layout needs bare but repo is not bare -- degrade gracefully (see Graceful
  Degradation above)
- New: `--at <path>` overrides template for this worktree. Worktree is fully
  managed (appears in list, removable by branch name). daft records the `--at`
  placement so that `list` can distinguish intentionally placed worktrees from
  ones that drifted off-template. These get a distinct `--at` indicator, not the
  off-template warning.
- New: `--at <path>` without a branch name creates a detached HEAD sandbox from
  the current branch. Left alone by prune.
- `go` that defaults to `start` when branch/worktree does not exist also
  supports `--at`

### `daft list`

- Layout-agnostic -- reads `git worktree list --porcelain`
- Worktrees not at their template-expected path get an off-template indicator
- Detached HEAD sandboxes shown with a distinct sandbox marker (not the
  off-template indicator, since sandboxes have no template-expected path)

### `daft prune`

- Works across all layouts
- Skips detached HEAD sandboxes
- Path computation uses the repo's resolved layout

### `daft remove`

- Works by branch name regardless of where the worktree lives
- Git tracks the worktree path; daft does not need to compute it for removal

### `daft layout transform <target-layout>`

Replaces `adopt` and `eject` as a general-purpose layout migration:

- Non-bare to bare-needing layout: same mechanics as current `adopt`
- Bare to non-bare layout: same mechanics as current `eject`
- Between two non-bare layouts: move worktrees to new template paths
- Between two bare-needing layouts: move worktrees to new template paths
- Updates `repos.json` with new layout

`adopt` becomes an alias for `layout transform contained`. `eject` becomes an
alias for `layout transform sibling`.

Layout transform must also update git-internal worktree registrations
(`.git/worktrees/<name>/gitdir` paths) when moving worktrees between locations.
This applies to all transform directions, not just bare/non-bare transitions.

### `daft layout list`

Lists all known layouts (built-in + custom from global config) with templates
and whether they infer bare.

### `daft layout show`

Shows the resolved layout for the current repo, including which level of the
config chain it came from.

## Hooks Across Layouts

### Hook Types

Existing hook types work across all layouts without changes:

| Hook                   | When it fires                |
| ---------------------- | ---------------------------- |
| `post-clone`           | After `daft clone` completes |
| `worktree-pre-create`  | Before worktree creation     |
| `worktree-post-create` | After worktree creation      |
| `worktree-pre-remove`  | Before worktree removal      |
| `worktree-post-remove` | After worktree removal       |

### Hook Discovery

Hooks always resolve from the **target branch**:

- `worktree-post-create`, `worktree-pre-remove`, `worktree-post-remove`: read
  `daft.yml` from the target worktree (files are checked out)
- `worktree-pre-create`: read `daft.yml` from the target branch via
  `git show <branch>:daft.yml` (worktree does not exist yet). For new branches
  that have no commits yet, fall back to the base branch's `daft.yml` (the
  branch being forked from). If no base branch is identifiable, fall back to the
  default branch.
- `post-clone`: read `daft.yml` from the cloned repo's default branch

Target-branch resolution ensures that branches with different `daft.yml`
configurations (e.g., a major refactor changing build systems) get the correct
hooks for their needs.

### Clone Hook Overlap

For non-bare `daft clone`, both `post-clone` and `worktree-post-create` fire.
This is intentional: a non-bare clone both creates a repo and results in a
worktree. If hook definitions overlap, users manage that in their `daft.yml`.
This is a natural overlap that will be refined based on real usage patterns.

### Hook Environment Variables

Existing hook env vars (`DAFT_PROJECT_ROOT`, `DAFT_WORKTREE_PATH`,
`DAFT_BRANCH`, etc.) report actual paths. They work across all layouts without
changes.

## Glossary

Terms for the documentation glossary page:

| Term         | Definition                                                                                                                |
| ------------ | ------------------------------------------------------------------------------------------------------------------------- |
| **Layout**   | A template that determines where worktrees are placed on disk. daft ships with built-in layouts and supports custom ones. |
| **Worktree** | A working copy of a branch. Multiple worktrees let you work on branches simultaneously without stashing.                  |
| **Template** | A path pattern with variables (`{{ branch \| sanitize }}`) that daft uses to compute worktree locations.                  |
| **Hook**     | A script or command that runs at lifecycle events (clone, worktree create/remove). Defined in `daft.yml`.                 |
| **Trust**    | A security mechanism that controls whether hooks from a repository are allowed to run.                                    |
| **Sandbox**  | A detached HEAD worktree created with `--at`, for experimentation. Not affected by prune.                                 |

Notably absent: "bare repo." Users never need to encounter this term.
