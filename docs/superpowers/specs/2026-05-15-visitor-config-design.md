# Visitor Configuration

> **Issue:** [#335](https://github.com/avihut/daft/issues/335) — Daft Visitor
> Configuration.
>
> **Related:** [#493](https://github.com/avihut/daft/issues/493) — `daft pull`
> command, where collision-resolution between visitor and tracked `daft.yml` is
> deferred.

## Problem

Daft today requires a `daft.yml` to be committed to a repository for daft's
features (layout choice, hooks, shared files, etc.) to take effect. That makes
adoption all-or-nothing at the team level: an individual user who wants to use
daft on a repo they don't fully control, or simply doesn't want to muddy with
ad-hoc tooling, has no path forward. The result is a high adoption price for
casual or unilateral adoption.

`daft.local.yml` (today `daft-local.yml`) is the closest existing escape valve —
a recursive overlay that need not be tracked — but it's anchored to a single
worktree. When that worktree is removed, the file dies with it; sibling
worktrees never see it; the file is invisible at clone-level lifecycle events.
That makes it useful for one-off per-worktree tweaks but inadequate as the home
for "daft, as I personally configure it on this clone."

This spec introduces **visitor configuration**: a `daft.yml` whose visitor/team
status is determined by its git tracking state, and which daft propagates
between worktrees through normal branching and merging operations so it survives
the development lifecycle the way a tracked file would.

## Goals

- Allow a user to adopt daft fully on a repository without committing or
  modifying anything in the repo's tracked content.
- Treat untracked `daft.yml` and `daft.local.yml` as first-class artifacts of
  branch development: propagated on branch-out, propagated through merges,
  consistent across the worktrees of a clone via daft-managed copies.
- Reuse the existing recursive-merge infrastructure (`merge_configs`,
  `merge_hook_defs`, `merge_log_configs`) at both load time and on-disk via a
  new `daft file merge` command — one canonical implementation for both.
- Rename the local-override convention from `daft-local.yml` to `daft.local.yml`
  (dot-infix, matching the broader ecosystem), keeping the hyphenated form as a
  deprecated alias for one release cycle.
- Keep scope tight. Defer the collision case (incoming tracked `daft.yml` while
  a visitor file occupies the same path) to the future `daft pull` command via
  issue #493.

## Non-goals

- Wrapping `git pull` / `git fetch` for collision detection or interactive
  resolution. Deferred to `daft pull` (#493).
- Symlink-based or central-storage propagation (akin to shared files). Pure copy
  semantics for v1; may evolve later as the shared-files UX matures.
- Cross-clone reuse of visitor configurations (no XDG mirror, no daft-managed
  ignore-rule writing). Users manage their own ignore rules.
- A `daft config` TUI for editing settings interactively. That's separate future
  work; the new `daft file` verb namespace is chosen so that work and this work
  don't collide.
- Collision-related doctor checks. Those live with the `daft pull` work.

## Design

### File model and identity

A worktree may contain up to two daft config files at its root, both using the
existing `YamlConfig` schema:

| File             | Status                              | Role                                                                                                                                       |
| ---------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `daft.yml`       | Tracked **or** untracked            | Main daft config. Untracked → **visitor**; tracked → team config. Same path, same schema, same loader; the role flips with tracking state. |
| `daft.local.yml` | Always untracked (smell if tracked) | Recursive overlay on top of `daft.yml`. Today's `daft-local.yml`, renamed.                                                                 |

**Discovery names** (priority order). Main config candidates are unchanged:
`daft.yml`, `daft.yaml`, `.daft.yml`, `.daft.yaml`, `.config/daft.yml`,
`.config/daft.yaml`. Local override candidates:

- Preferred (new): `daft.local.yml`, `daft.local.yaml`, `.daft.local.yml`,
  `.daft.local.yaml`.
- Deprecated alias: `daft-local.yml`, `daft-local.yaml`, `.daft-local.yml`,
  `.daft-local.yaml`. Loading these emits a `tracing::warn` and a doctor notice;
  hard removal one release cycle after this lands.

**Visitor classification** is a runtime property, not a separate file format. A
new helper
`classify_main_config(worktree_root) -> ConfigStatus { Tracked, Visitor, Missing }`
resolves the discovered main config file (if any) and runs
`git ls-files --error-unmatch <path>` against it. Conservative fallback: if git
can't answer (no git binary, not a repo, error), treat as `Tracked` to avoid
surprising the user with implicit visitor behavior.

**Ignore-rule responsibility** stays with the user. Daft never writes to
`.gitignore` or `.git/info/exclude`. Doctor surfaces classification status so
the user knows whether they need to act on it.

### Precedence and load-time merging

The effective load stack is unchanged in shape (low → high precedence):

1. Main `daft.yml` (tracked or visitor — same path, same loader).
2. Files referenced by `extends:`.
3. Per-hook YAML files.
4. `daft.local.yml` (or deprecated `daft-local.yml`).

`merge_configs`, `merge_hook_defs`, and `merge_log_configs` continue to do
recursive merging with overlay-wins semantics. They become the canonical merge
implementation shared between load-time overlay resolution and the new on-disk
`daft file merge` command.

**Loader changes**:

- `find_local_config` (`src/hooks/yaml_config_loader.rs:57-76`) is rewritten as
  a lookup over the new candidate list, in preferred-then-deprecated order.
- New `classify_main_config` helper in the same module, used by doctor, the
  propagation flow, and future code paths.
- No changes to `extends:` semantics, per-hook discovery, or the bare/branch-ref
  loaders (`load_config_from_bare`, `load_config_from_branch`).

### Propagation events

Propagation copies the **in-scope untracked daft files** from a source worktree
A to a target worktree B, resolving content via
`merge_configs(B_current, A_current)` so source wins on conflicts. In-scope
files for v1:

- `daft.yml` when currently visitor (untracked) in the source.
- `daft.local.yml`.

Per-hook YAML files and `extends:`-referenced files are out of scope for v1.

**Events that fire propagation:**

1. **Branch-out (worktree create).** During `git-worktree-checkout-branch`,
   `git-worktree-clone`, and `git-worktree-init` flows: after git creates the
   new worktree directory and before user `worktree-post-create` hooks fire,
   daft copies the in-scope files from the current worktree into the new one.
   Source = the worktree daft was invoked from. Code anchor:
   `src/core/worktree/clone.rs`, plus the corresponding init/checkout paths.

2. **`daft merge`.** Atomic write-merge-restore for the in-scope files:
   1. Save B's pre-existing untracked daft-file contents in memory.
   2. Compute the resolved content via `merge_configs`.
   3. Write the resolved content into B.
   4. Run the git merge.
   5. On success, the resolved content persists.
   6. On failure (conflict, abort, refusal), restore B's original content.

   Both pre-merge and post-merge hooks read from the resolved file — they see
   the same effective config. Code anchor: `src/commands/merge.rs`.

3. **Remote-merge detection.** Reuses the existing merge-into-master detection
   from `src/core/worktree/branch_delete.rs`. Cheapest-first gating:
   1. Does the source-branch worktree exist on disk?
   2. Does it contain any in-scope untracked daft file?
   3. If both yes, run the existing merge-detection logic; on "merged",
      propagate into the merge target's worktree.

   Skip entirely otherwise — repos that don't use visitor configs pay no cost.

**Worktree removal as a safety boundary.** `daft worktree remove` (and any other
path that destroys a worktree) refuses to delete a worktree whose in-scope
untracked daft files differ from the merge target's. The refusal is interactive:
daft prompts the user and suggests `daft file merge` to consolidate. `--force`
overrides for scripted use. This closes the gap where remote-merge propagation
could be missed because the source worktree was removed before any daft command
observed the merge.

### Collision handling (visitor `daft.yml` meets tracked `daft.yml`)

**Deferred to issue #493** (`daft pull` command). The collision moment happens
inside a `git pull` / `git checkout` operation that daft does not currently own;
passive detection from doctor can only warn anticipatorily and was considered
insufficient. Active detection belongs in a daft-owned pull command where the
resolution paths can be presented interactively. See #493 for the list of
decisions the pull design must land: detection mechanism, prompt vs
flag/config-driven resolution, default behavior (current intent: relegate to
`daft.local.yml`), secondary collision on existing `daft.local.yml` (current
intent: rename to `daft.old.yml` with a hard error), and whether `daft fetch`,
`daft sync`, `daft update`, `daft prune` adopt the same handling.

### New commands

**`daft install`** (modeled on `lefthook install`).

Bootstraps a starter `daft.yml` at the current worktree root, containing a
commented skeleton with the major sections (`hooks:`, `shared:`, `layout:`).
Refuses if `daft.yml` already exists, pointing the user at `$EDITOR daft.yml`.
No git side effects (no ignore-rule writes — users own their ignore rules).

Code location: `src/commands/install.rs`. Routed via the multicall binary, added
to `xtask` `COMMANDS`, `commands/docs.rs`, man pages, and shell completions like
any other command.

**`daft file merge <TARGET> <SOURCE>`** (collapsed form:
`daft file merge <SOURCE>`).

Recursive YAML merge of `<SOURCE>` into `<TARGET>` using `merge_configs` and its
companion mergers. Source wins on conflicts; structured nodes (hooks, jobs)
merge by name rather than fully replacing. Behavior details:

- Collapsed form's implied target: `daft.yml` in the current worktree.
- After successful merge, the source file is deleted. `--keep-source` opts out
  of deletion.
- If the target is currently untracked (visitor), daft prompts for confirmation
  before writing, because undoing an unversioned write is manual. `--yes` (or
  `--force`) skips the prompt for scripted use.

Code location: `src/commands/file/mod.rs` + `src/commands/file/merge.rs`. The
new top-level `file` verb is registered in the multicall binary; the namespace
leaves room for future siblings (`daft file diff`, `daft file validate`,
`daft file edit`) without colliding with whatever `daft config` becomes (the
candidate TUI surface).

Neither command needs `DAFT_CD_FILE` integration — neither changes the
filesystem layout the user is standing inside.

### Doctor checks

Three additions, none collision-related (those live with #493):

1. **Tracked `daft.local.yml` smell.** Run `git ls-files --error-unmatch`
   against all `daft.local.yml` aliases (including the deprecated hyphenated
   form). If tracked, warn: the file is intended as a personal overlay and
   should be untracked; suggest `git rm --cached` and a `.gitignore` entry.

2. **Deprecated alias notice.** If any `daft-local.yml` variant exists, suggest
   renaming to the corresponding `daft.local.yml` variant. Soft notice for one
   release cycle; promoted to a hard failure thereafter.

3. **Visitor classification info.** Extend the existing config-source check
   (`src/doctor/hooks_checks.rs:57-87`) to surface whether `daft.yml` is tracked
   or visitor. Informational, not a warning.

No doctor work for collision anticipation, fresh-fetch advisories, or
ignore-rule auditing in this branch.

## Affected code paths

- `src/hooks/yaml_config_loader.rs` — extend `find_local_config` candidate list;
  add `classify_main_config` helper.
- `src/hooks/yaml_config.rs` — no schema changes.
- `src/commands/install.rs` — new module.
- `src/commands/file/mod.rs`, `src/commands/file/merge.rs` — new modules.
- `src/commands/mod.rs` — register new modules.
- `src/main.rs` — multicall routing for `daft install` and `daft file`.
- `src/core/worktree/clone.rs` and related init/checkout paths — propagation on
  branch-out.
- `src/commands/merge.rs` — atomic propagation on `daft merge`.
- `src/core/worktree/branch_delete.rs` — wire in remote-merge propagation
  alongside the existing merge-detection logic.
- Worktree-remove path — divergence check and interactive prompt.
- `src/doctor/hooks_checks.rs` — three new checks (and supporting helpers as
  needed).
- `xtask/src/main.rs` — `COMMANDS` and `get_command_for_name()` entries.
- `src/commands/docs.rs` — help-output entries.
- `src/commands/completions/{mod,bash,zsh,fish,fig}.rs` — register new verbs and
  any new flags.
- `man/` — regenerate via `mise run man:gen`.

## Testing

- Unit tests for `find_local_config` covering the new preferred names, the
  deprecated aliases, priority among them, and the absence cases.
- Unit tests for `classify_main_config` against tempdirs in different git
  states: untracked, tracked, ignored, missing, non-git directory.
- Unit tests for `daft file merge`: scalar overrides, hook-by-name merging,
  job-by-name and unnamed-job semantics, source deletion default,
  `--keep-source`, untracked-target confirmation flow, `--yes`/`--force` bypass.
- YAML manual test scenarios:
  - `tests/manual/scenarios/install/` — bootstrap, refuse-on-existing.
  - `tests/manual/scenarios/file-merge/` — explicit form, collapsed form,
    source-delete, `--keep-source`, untracked-target prompt.
  - `tests/manual/scenarios/visitor-propagation/` — branch-out copy, daft merge
    atomic propagation (success + failure rollback), worktree-remove divergence
    refusal and confirmation.
- Integration tests for the shell wrapper covering `daft install` and
  `daft file merge` — neither should affect cd state, and the regression test
  should pin that down.
- Doctor tests for the three new checks against tempdirs with the relevant
  fixture states.

## Documentation

- `docs/about/glossary.md` — entry for **visitor configuration**.
- `docs/recipes/` — new recipe walking through unilateral daft adoption with
  `daft install`, propagation behavior, and the upgrade path to a tracked team
  baseline.
- `docs/reference/cli/daft-install.md` — CLI reference page.
- `docs/reference/cli/daft-file.md` — CLI reference page for the new `file` verb
  and the `merge` subcommand. Reserves the namespace for future siblings without
  committing to them.
- `docs/about/faq.md` — entry for "Do I have to commit `daft.yml` to use daft?"
  pointing at visitor configuration.
- `SKILL.md` — update to teach agents about visitor classification, the
  propagation contract, and the new commands. The classification helper and the
  `--keep-source` flag are the agent-relevant surfaces.

## Out of scope (tracked elsewhere)

- Visitor-vs-tracked collision detection and resolution, and the `daft pull`
  command itself — issue #493.
- Symlink-based propagation, mirroring the shared-files mechanism — possible
  future evolution once shared-files UX issues are resolved.
- Cross-clone reuse of visitor configurations via an XDG mirror — no motivating
  use case today.
- A `daft config` TUI screen — separate future work; the `daft file` namespace
  is chosen to leave room for it.
