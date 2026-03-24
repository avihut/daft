# Layout Detection and Interactive Resolution

## Overview

Extend the layout system to detect a repository's layout from its filesystem
structure when no explicit layout is configured. Add an optional path argument
to `daft layout show` and introduce an interactive layout picker (using
`dialoguer`) for `daft start` when operating on unmanaged repositories.

## Motivation

Currently, `daft layout show` reports `sibling (default)` for any repository
without an explicit layout setting. This is misleading: a repo freshly cloned
with `git clone` has no daft layout, and a repo that was set up as contained by
hand should be recognized as such. The system should distinguish between
"explicitly configured," "detected from structure," and "unknown."

Additionally, when a user runs `daft start` on a repo daft has never seen, the
system silently applies the default layout. For repos that already have
worktrees arranged in a recognizable pattern, daft should confirm the detection.
For repos with worktrees in an unrecognized arrangement, daft should ask the
user what layout to use going forward.

## Changes

### 1. `daft layout show [path]` -- optional directory argument

The `show` subcommand (and the default `daft layout` command) accepts an
optional positional argument: a path to a git repository. When omitted, it
defaults to the current working directory, preserving backward compatibility.

The path is resolved to its git common directory. All subsequent lookups
(repos.json, daft.yml, detection) operate from that resolved root.

### 2. Extended resolution chain

The resolution priority becomes:

1. CLI `--layout` flag (unchanged)
2. Per-repo store / repos.json (unchanged)
3. daft.yml `layout` field (unchanged)
4. Global config `defaults.layout` (unchanged)
5. **Detected from filesystem** (new)
6. **Unresolved** (replaces the old "default" fallback)

The `LayoutSource` enum changes:

```rust
pub enum LayoutSource {
    Cli,
    RepoStore,
    YamlConfig,
    GlobalConfig,
    Detected,     // NEW: matched from worktree paths / structure
    Unresolved,   // NEW: replaces Default -- no layout could be determined
}
```

When the resolution chain reaches step 5, it checks the `detection` field. If
detection succeeded, it returns the detected layout with
`LayoutSource::Detected`. Otherwise it returns `LayoutSource::Unresolved`.

**Breaking change:** `LayoutSource::Default` is removed. All existing callers
that match on `Default` must be updated to match `Unresolved` instead. The
semantic meaning changes from "I know the layout, it's the built-in default" to
"no layout was determined." This is safe because callers that always have a
higher-priority source (like `daft clone`, which provides `cli_layout` or stores
the layout immediately) never reach the `Default`/`Unresolved` branch. Callers
that do reach it (`daft layout show`, `daft start`) now get more accurate
information.

Affected callers that must be audited:

- `cmd_show` in `src/commands/layout.rs` (display logic)
- `resolve_checkout_layout` in `src/commands/checkout.rs` (worktree creation)
- Any other site that pattern-matches on `LayoutSource`

### 3. Display changes for `daft layout show`

The source label in the output changes:

| Source       | Display                     |
| ------------ | --------------------------- |
| Cli          | `(--layout flag)`           |
| RepoStore    | `(repo setting)`            |
| YamlConfig   | `(daft.yml)`                |
| GlobalConfig | `(global config)`           |
| Detected     | `(detected)`                |
| Unresolved   | Special message (see below) |

When `LayoutSource::Unresolved`:

- If the repo has no linked worktrees and no structural cues: display
  `No layout (no worktrees to detect from)`
- If the repo has worktrees but no template matched: display
  `Unknown layout (worktrees don't match any known template)`

### 4. Detection algorithm

Detection is a new module: `src/core/layout/detect.rs`.

#### Input

The detection function takes:

- `git_common_dir: &Path` -- the `.git` directory (bare or non-bare)
- All available layouts: builtins + custom layouts from global config

The function derives `project_root` internally from `git_common_dir` and
`core.bare`. For bare repos, `project_root` is `git_common_dir.parent()`. For
non-bare repos where `.git` is inside a named subdirectory (potential
contained-classic), the function checks both the immediate parent and the
grandparent as candidate project roots. This avoids requiring the caller to know
the layout before detection.

#### Step 1: Gather worktree data

Parse `git worktree list --porcelain` to collect:

```rust
struct WorktreeInfo {
    path: PathBuf,
    branch: Option<String>,  // None for detached HEAD
    is_main: bool,           // first entry = main worktree
}
```

Reuse or align with the existing `WorktreeEntry` struct from
`src/core/layout/transform/state.rs` where possible to reduce duplication.

Skip detached HEAD worktrees (no branch to match against). If all linked
worktrees are detached HEAD (no branches to match), treat this the same as
having no linked worktrees and proceed to structural detection (step 3).

If no linked worktrees exist (only the main worktree), proceed to structural
detection (step 3).

#### Step 2: Template matching (multi-worktree)

For each candidate layout (builtins + custom):

1. **Filter by bare compatibility**: check `core.bare` of the repo and compare
   with the layout's `needs_bare()`. Skip layouts that are incompatible (e.g.,
   skip `contained` for a non-bare repo, skip `sibling` for a bare repo).
2. Build a `TemplateContext` from the repo's `project_root` and each worktree's
   branch name
3. Render the template and resolve the path
4. Compare the resolved path to the worktree's actual path
5. Count matches

Score each layout:

```rust
struct DetectionScore {
    layout: Layout,
    matches: usize,      // worktrees whose path matches this template
    total: usize,        // total worktrees with branches (excludes detached)
}
```

**Selection rule:** A layout is detected if and only if:

- It is the **only** layout with at least one match
- No other layout also has matches (ambiguity = no detection)

Non-conforming worktrees (matching no template at all) do not prevent detection.
They are simply worktrees placed at custom paths.

If two or more layouts tie because their templates evaluate to the same paths
for the branches present (e.g., `contained` and `contained-flat` when no branch
has slashes), apply the **specificity tiebreaker**: prefer the layout whose
template has fewer `|` filter operators. If still tied, prefer the layout that
appears earlier in the builtin list. Note: because candidates are pre-filtered
by bare compatibility (step 2.1), a bare repo will never produce a tie between
`contained` and `contained-classic` -- `contained-classic` is filtered out since
it is non-bare.

#### Step 3: Structural detection (single worktree or fallback)

When there are no linked worktrees, or template matching was inconclusive, use
structural cues from the main worktree and `.git` directory:

| Condition                                                                                                                                                                   | Detected layout                                           |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| `core.bare = true` AND main worktree is a direct child of repo root                                                                                                         | `contained` (prefer over `contained-flat` by specificity) |
| `core.bare = false` AND `.git` is inside a named subdirectory of a wrapper dir AND that subdirectory is the main worktree AND directory name matches the checked-out branch | `contained-classic`                                       |
| Main worktree IS the repo root, `.worktrees/` subdir exists (even if empty)                                                                                                 | `nested`                                                  |
| Main worktree IS the repo root, no `.worktrees/` subdir                                                                                                                     | No detection -- looks identical to plain `git clone`      |

The `contained-classic` detection is best-effort. It relies on the directory
name matching the default branch, which can be wrong if the user renamed the
directory. The interactive flow (section 5) gives the user a chance to correct
false detections.

For repos where the main worktree is the repo root and there are no structural
cues, detection returns `Unresolved`. These repos look identical to a plain
`git clone`, and guessing would be wrong.

### 5. `daft start` interactive flow

When `daft start` (checkout) resolves the layout and gets
`LayoutSource::Unresolved` or `LayoutSource::Detected`, trigger an interactive
flow before creating the worktree.

#### Flow A: No worktrees, no layout (plain git repo, first `daft start`)

Proceed silently with the user's configured default layout (global config or
built-in default). **Store the layout in repos.json.** No prompt needed -- this
is the progressive adoption Level 0 experience.

This persistence is intentional: it locks in the layout for this repo so that
subsequent `daft start` commands on the same repo are consistent. If the user
later changes their global default, this repo keeps its original layout. To
change it, the user can run `daft layout transform`.

#### Flow B: Layout detected

```
Detected layout: contained (3 of 4 worktrees match)
Use this layout for future worktrees? [Y/n]
```

- **Y** (default): Store the detected layout in repos.json, proceed with
  worktree creation.
- **n**: Open the layout picker (see below). After selection, ask about
  consolidation.

#### Flow C: Unknown layout (worktrees exist, no match)

```
Found 3 worktrees in an unrecognized arrangement.
Choose a layout for new worktrees:
```

Open the layout picker. After selection, ask about consolidation.

#### Layout picker

An arrow-key navigable selection menu using `dialoguer::Select`. Lists all
available layouts (builtins + custom) with their template:

```
> contained         {{ repo_path }}/{{ branch }}
  contained-classic {{ repo_path }}/{{ branch | repo }}
  contained-flat    {{ repo_path }}/{{ branch | sanitize }}
  sibling           {{ repo }}.{{ branch | sanitize }}
  nested            {{ repo }}/.worktrees/{{ branch | sanitize }}
  centralized       {{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch | sanitize }}
```

The user's global default (if set) is pre-selected. If a layout was detected but
rejected (Flow B, "n"), the detected layout is pre-selected instead.

The picker uses `dialoguer::Select` with the `console` backend (already a
dependency). Add `dialoguer` to `Cargo.toml`.

#### Consolidation prompt

After selecting a layout that differs from the detected/existing arrangement:

```
Consolidate N existing worktrees to match "contained" layout? [y/N]
```

Where N is the count of worktrees that would be relocated.

- **y**: Run `layout transform` to the chosen layout.
- **N** (default): Only new worktrees use the chosen layout. Existing worktrees
  stay where they are.

#### Non-interactive fallback

When stdin is not a terminal (CI, scripts, piped input):

- Skip all prompts
- If detected: use detected layout silently
- If unknown: use the default layout silently
- Never block on input

### 6. New dependency: `dialoguer`

Add `dialoguer` to `Cargo.toml`. It is built on the `console` crate (already a
dependency). Use `dialoguer::Select` for the layout picker and
`dialoguer::Confirm` for yes/no prompts where appropriate.

The existing `src/prompt.rs` single-keypress prompt remains for its current use
cases (simple Y/n prompts inline with output). The `dialoguer` picker is for the
new multi-option selection flow.

### 7. Changes to `resolve_layout`

The `resolve_layout` function signature changes to support detection:

```rust
pub struct LayoutResolutionContext<'a> {
    pub cli_layout: Option<&'a str>,
    pub repo_store_layout: Option<&'a str>,
    pub yaml_layout: Option<&'a str>,
    pub global_config: &'a GlobalConfig,
    pub detection: Option<DetectionResult>,  // NEW
}
```

Where `DetectionResult` is:

```rust
pub enum DetectionResult {
    Detected(Layout),
    Ambiguous,
    NoWorktrees,
    NoMatch,
}
```

The caller is responsible for running detection before calling `resolve_layout`.
This keeps detection as an opt-in step -- callers that don't need it (like
`daft clone`, which always knows the layout) pass `detection: None` and the
chain skips straight from global config to `LayoutSource::Unresolved`.

When `detection` is `Some(DetectionResult::Detected(layout))`, the resolver uses
it at priority 5. All other `DetectionResult` variants map to
`LayoutSource::Unresolved`.

### 8. Multi-remote interaction

Multi-remote mode (`src/core/multi_remote/path.rs`) adds an extra path component
(the remote name) to worktree paths. Template matching does not account for
multi-remote path prefixes.

For the initial implementation, detection is skipped for repos with multi-remote
enabled (detectable via `daft.yml` multi-remote config or the presence of
multiple configured remotes). These repos should already have a layout set in
repos.json or daft.yml from the initial setup. If neither is present, detection
returns `Unresolved`.

### 9. What detection does NOT do

- Detection does **not** write to repos.json. It is read-only. Only explicit
  user actions (clone, transform, start with confirmation) persist a layout.
- Detection does **not** consider the global default layout. The global default
  is a preference, not an observation. Detection is purely filesystem-based.

## Edge Cases

### Bare repo with no worktrees

A bare repo (no main worktree, no linked worktrees) cannot be detected. This is
rare in practice (daft always creates at least a main worktree checkout).
Result: `LayoutSource::Unresolved` with "no worktrees to detect from."

### Worktrees at custom paths (`--at`)

Worktrees created with `--at` will not match any template. They are
non-conforming and do not affect detection of other worktrees that do conform.

### Custom layouts in global config

Custom layouts participate in detection alongside builtins. If a custom layout
matches, it is returned as detected. Custom layouts are checked after builtins
to give builtins priority in case of identical templates.

### Centralized layout detection

The centralized layout places worktrees in `$DAFT_DATA_DIR/worktrees/...`.
Detection renders the template with the actual `daft_data_dir` value and checks
if worktree paths match. Since the data dir is user-specific, this works across
machines only if the XDG paths happen to match.

## Files to create/modify

| Action | File                          | Purpose                                              |
| ------ | ----------------------------- | ---------------------------------------------------- |
| Create | `src/core/layout/detect.rs`   | Detection algorithm                                  |
| Modify | `src/core/layout/mod.rs`      | Add `pub mod detect;`                                |
| Modify | `src/core/layout/resolver.rs` | Add `Detected`/`Unresolved`, extend chain            |
| Modify | `src/commands/layout.rs`      | Add path arg to show, use detection, display changes |
| Modify | `src/commands/checkout.rs`    | Interactive flow for unmanaged repos                 |
| Modify | `Cargo.toml`                  | Add `dialoguer` dependency                           |
| Create | tests for detection           | Unit tests in detect.rs, integration scenarios       |
