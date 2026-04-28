# Remove Repo (`daft repo remove`)

Issue: [#421](https://github.com/avihut/daft/issues/421) Branch:
`daft-421/feat/remove-repo`

## Summary

A new top-level command, `daft repo remove [<path>]`, that deletes a Git
repository — bare directory plus all its checked-out worktrees — while running
`worktree-pre-remove` and `worktree-post-remove` lifecycle hooks per worktree.
Works for any Git repo, daft-managed or vanilla. Refuses non-Git paths.

A companion sandbox helper (`clean-repos`) scans the dev sandbox for any Git
repos and removes each via the new command, ensuring resources spun up by hooks
(Docker, ports, etc.) get torn down properly instead of orphaned by a naive
`rm -rf`.

This is the first command under a new `repo` subcommand category, leaving room
for additional repo-level commands (`daft repo list`, `daft repo info`, …) in
future work.

## Goals & Non-goals

**Goals**

- Remove a Git repository identified by path, including the bare git directory
  and every worktree.
- Run `worktree-pre-remove` and `worktree-post-remove` hooks for each worktree
  (when the repo is daft-managed and trusted), enabling proper cleanup of
  hook-spawned resources.
- Provide a TUI list interface showing per-worktree progress and hook runs.
- Provide a sequential (non-TUI) fallback for non-TTY invocations and `-vv`.
- Sandbox cleanup: a script that finds every repo under the dev sandbox and
  removes it via the new command.
- Work on plain (non-daft) Git repos as a basic file-system + git-worktree
  cleanup, with hook steps becoming no-ops.

**Non-goals**

- Backups, archives, or "undo" of the removed repo.
- New hook types. We reuse the existing `worktree-pre-remove` /
  `worktree-post-remove` types; we do not introduce repo-level hooks.
- Recursive sandbox-style "find every repo on disk and clean it up"
  functionality at the daft level — this lives only in the sandbox helper.
- Layout-specific handling. Worktree paths are enumerated via
  `git worktree list`; the command does not need to know about contained,
  sibling, nested, or centralized layouts.

## User Surface

```text
daft repo remove [<path>] [--force | -y] [--dry-run] [-v]
git-daft repo remove [<path>] [--force | -y] [--dry-run] [-v]

Arguments
  <path>          Path to the repo or any directory inside it.
                  Defaults to the current working directory.

Flags
  --force, -y     Skip the interactive confirmation prompt.
                  Required when stdin is not a TTY.
  --dry-run       Print the removal plan and exit without touching anything.
  -v / -vv        Verbosity. -v shows hook details inline; -vv forces the
                  sequential (non-TUI) output path. Mirrors `daft prune`.
```

There is **no** standalone `git-worktree-*` symlink for this command. It is
strictly a daft subcommand.

### Confirmation

- `--force` / `-y` → skip prompt.
- TTY, no `--force` → prompt:
  `Remove repo at <project_root>? This will delete N worktrees and the bare git dir. [y/N]`
  Anything other than `y`/`Y` aborts (exit 0, "aborted" message).
- Non-TTY, no `--force` → hard error:
  `Refusing to run without --force in non-interactive mode`.

### Dry-run

`--dry-run` prints the plan (worktree paths, bare git dir path, trust DB entry
to be dropped) and exits 0 without modifying anything. Confirmation prompt is
not shown in dry-run.

## Path Resolution

1. Take `<path>` if given; else `std::env::current_dir()`.
2. Run `git rev-parse --git-common-dir` from that path.
3. If step 2 fails → hard error: `<path> is not inside a Git repository`.
4. The bare git directory (the resolved git-common-dir) is the repo identity.
   Project root is its parent directory.
5. Enumerate worktrees via `git worktree list --porcelain` against the bare
   directory. This works uniformly for all four daft layouts and for vanilla
   repos.

## Architecture

### Approach

Reuse the existing `OperationTable` renderer and `SyncDag` / `DagExecutor`
infrastructure that powers `daft prune`. This keeps a single TUI surface and a
single execution model in the codebase. Cost: two new `TaskId` variants
(`RemoveWorktree`, `RemoveBare`) and a new `SyncDag::build_remove_repo`
constructor — all well-precedented additions.

Rejected alternatives:

- **New lightweight linear TUI**: doubles the TUI surface for marginal
  conceptual cleanliness.
- **Reuse `OperationTable` but bypass the DAG (synthetic `DagEvent`s)**:
  introduces a fragile coupling — the table was built around DAG events.

### DAG shape

```
RemoveWorktree(/path/to/main)        ─┐
RemoveWorktree(/path/to/feat-auth)   ─┼─→ RemoveBare(<bare git dir>)
RemoveWorktree(/path/to/bug-login)   ─┘
```

Worktree-removal tasks are independent (no edges between them) and run in
parallel, mirroring `prune`'s parallelization. Worktrees are isolated
environments by design (separate paths, Docker project names, ports, etc.); if
hooks across worktrees share state, that is already a problem under `prune`.

The terminal `RemoveBare` task depends on every `RemoveWorktree`, ensuring the
bare directory is removed only after all worktrees are gone.

## Execution Flow

```
1. Resolve repo from <path>/cwd → bare git dir + project root.
2. Enumerate worktrees via `git worktree list --porcelain`.
3. Read trust state + hooks config (best-effort; missing == no hooks).
4. If --dry-run: print plan and exit 0.
5. Confirmation gate (see User Surface > Confirmation).
6. Build SyncDag (see Architecture > DAG shape).
7. Run DagExecutor with the OperationTable renderer (or sequential output if
   non-TTY or -vv).
8. Per-task work:
     RemoveWorktree(path):
       a. Run worktree-pre-remove hook (if trusted + configured); record
          result; never abort on failure.
       b. `git worktree remove --force <path>`. On failure, fall back to
          `rm -rf <path>`. Same DAFT_CD_FILE handling as prune for the
          currently-checked-out case.
       c. Run worktree-post-remove hook; record result.
     RemoveBare:
       a. `rm -rf` the bare git directory.
       b. Walk upward from the bare git directory's parent and `rmdir` any
          now-empty directories (project root, intermediate `.worktrees`
          dirs in nested layout, etc.). Stop at the first non-empty
          directory.
       c. Drop the bare-git-path entry from the TrustDatabase.
9. Post-TUI: print hook summary (mirrors prune's `Hooks:` epilogue).
10. If cwd was inside the removed repo, write a safe target path
    (project-root parent, falling back to DAFT_DATA_DIR or HOME) to
    DAFT_CD_FILE so the shell wrapper cd's somewhere safe.
```

### Hook failure policy

When a hook exits non-zero we **do not abort**. The user has already committed
to deletion by invoking `daft repo remove`. The filesystem is removed
regardless; failed hooks surface in the post-run summary. This differs
deliberately from `prune`, which aborts the offending branch on hook failure.

The summary line for each failed/warned hook follows the existing prune format:

```
Hooks:
  feat/auth: worktree-pre-remove warned (exit 2, 134ms)
    docker: container already gone
  bug/login: worktree-post-remove failed (exit 1, 412ms)
    Error: cannot release port 5432
```

Exit code: non-zero only if at least one hook failed in a non-warned mode
(matching prune's existing convention). Warned-only runs exit 0.

## Components & File Layout

```
src/commands/repo/
  mod.rs                # `pub mod remove;` + `run()` dispatch
  remove.rs             # clap Args, run(), TUI + sequential paths

src/commands/mod.rs     # add `pub mod repo;`
src/main.rs             # route "repo" subcommand → commands::repo
src/commands/docs.rs    # add to help categories (new `Repo` group)
src/commands/completions/{bash,zsh,fish,fig}.rs
                        # add `repo remove` subcommand and flag completions
xtask/src/main.rs       # add to COMMANDS for man-page generation

src/core/worktree/
  remove_repo.rs        # NEW: pure logic — plan construction, single-task
                        # execution. Mirrors `core/worktree/prune.rs`.
  sync_dag.rs           # add TaskId::RemoveWorktree(PathBuf),
                        # TaskId::RemoveBare; SyncDag::build_remove_repo()

src/commands/sync_shared.rs
                        # add execute_remove_worktree_task,
                        # execute_remove_bare_task; render_remove_repo_result

src/hooks/...           # no changes — reuse worktree-pre-remove,
                        # worktree-post-remove hook types.

mise-tasks/sandbox/
  clean-repos           # NEW: scans ${sandbox_dir} (denylist bin/, config/,
                        # data/, state/), finds every repo, runs
                        # `daft repo remove --force <path>` per repo.
  _default              # add a `clean-repos` helper script + completions
                        # entry to bin/.

man/git-daft-repo-remove.1
                        # generated by `mise run man:gen` and committed.

tests/manual/scenarios/repo/
  remove-basic.yml             # standard daft repo, multiple worktrees
  remove-with-hooks.yml        # exercises pre/post hooks, both ok and failing
  remove-from-inside.yml       # cwd inside a worktree → DAFT_CD_FILE behavior
  remove-vanilla.yml           # plain non-daft git repo, no hooks
  remove-non-git-fails.yml     # path is not a git repo → hard error
  remove-dry-run.yml           # --dry-run prints plan, touches nothing
  remove-force.yml             # --force skips prompt; non-TTY without --force
                               # is an error

tests/integration/repo-remove.bats
                        # bash-driven integration test mirroring
                        # tests/integration/prune.bats patterns.

docs/cli/daft-repo-remove.md
                        # one-page CLI reference, follows daft-doctor.md
                        # template.
```

The split between `commands/repo/remove.rs` (clap, output wiring) and
`core/worktree/remove_repo.rs` (pure logic) mirrors how `commands/prune.rs`
calls into `core::worktree::prune::execute`.

### `repo` subcommand routing

`src/main.rs` will add:

```rust
"repo" => {
    let sub = args.get(2).map(String::as_str).unwrap_or("");
    match sub {
        "remove" => commands::repo::remove::run(),
        "" | "--help" | "-h" => commands::repo::print_help(),
        _ => daft::suggest::handle_unknown_subcommand(
            "daft repo", sub, daft::suggest::DAFT_REPO_SUBCOMMANDS,
        ),
    }
}
```

`DAFT_REPO_SUBCOMMANDS` is a new constant in `src/suggest.rs` listing the known
`repo` subcommand verbs (currently just `["remove"]`).

## TUI

The `OperationTable` renderer is reused unchanged. We feed it:

- Phases at the top: `Confirm → Remove`. The Confirm phase is essentially
  instantaneous (it represents the gate that ran before the table started); we
  show it as already-completed when the table opens. The Remove phase tracks the
  DAG.
- One row per worktree (`branch`, `path`, `status`).
- One terminal row labeled `(bare)` for the bare git directory.

Sample steady-state render:

```
  branch       path                     status
  ────────────────────────────────────────────────────────────
  main         /repos/myproj/main       ✓ removed (pre-hook ok)
  feat/auth    /repos/myproj.feat-auth  ⚠ removed (post-hook warned)
  bug/login    /repos/myproj.bug-login  ✗ removed (hook failed)
  (bare)       /repos/myproj/.git       ✓ removed
```

Hook sub-rows under `-v` follow prune's existing rendering. No new column types
are introduced.

The non-TTY / `-vv` path bypasses the table and prints sequential lines, again
matching prune's `run_prune` path.

## Sandbox: `clean-repos`

```bash
#!/usr/bin/env bash
#MISE description="Run `daft repo remove` on every git repo under the sandbox"
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/_lib.sh"
sandbox_dir="$(sandbox_dir)"

[ -d "$sandbox_dir" ] || { echo "No sandbox at ${sandbox_dir}"; exit 0; }

# Denylist: known non-repo subtrees of the sandbox layout.
mapfile -t repos < <(
  find "$sandbox_dir" \
       -path "${sandbox_dir}/bin"    -prune -o \
       -path "${sandbox_dir}/config" -prune -o \
       -path "${sandbox_dir}/data"   -prune -o \
       -path "${sandbox_dir}/state"  -prune -o \
       -name .git -print 2>/dev/null \
    | xargs -I{} dirname {} \
    | sort -u
)

[ ${#repos[@]} -gt 0 ] || { echo "No repos found under ${sandbox_dir}"; exit 0; }

for repo in "${repos[@]}"; do
  echo "Removing $repo"
  daft repo remove --force "$repo" || echo "  (failed — continuing)"
done
```

A sandbox-level `bin/clean-repos` wrapper delegates to the mise task, matching
the existing `clean-sandbox` / `reset-sandbox` pattern in
`mise-tasks/sandbox/_default`.

The script catches both daft-managed clones in
`${sandbox_dir}/test/<scenario>/work/` and the bare "remote" repos under
`${sandbox_dir}/test/<scenario>/remotes/` — the bare ones simply have no hooks,
so removal is a fast filesystem delete via the same code path. User-created
repos placed anywhere in the sandbox outside the denylist are also caught.

## Error Handling

| Situation                           | Behavior                                                                                                                                                                       |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Path not inside a Git repo          | Hard error before any prompt; non-zero exit.                                                                                                                                   |
| Confirmation declined               | Clean exit 0, "aborted" message.                                                                                                                                               |
| Non-TTY without `--force`           | Hard error: `Refusing to run without --force in non-interactive mode`.                                                                                                         |
| Hook failure (pre or post)          | Recorded; never aborts; surfaces in summary.                                                                                                                                   |
| `git worktree remove` failure       | Retry with `--force`. Still failing → log warning, fall back to `rm -rf`.                                                                                                      |
| `rm -rf` failure on a worktree path | Logged; treated as task failure; remaining worktrees still proceed; the bare-removal task still runs (lossy ok — the user committed to deletion).                              |
| `rm -rf` failure on bare dir        | Hard error; exit non-zero; trust DB entry left untouched (it's still pointing at a valid repo).                                                                                |
| cwd inside the removed repo         | Write a safe target to `DAFT_CD_FILE` (project-root parent → falls back to `DAFT_DATA_DIR` → HOME). If `DAFT_CD_FILE` is unset, print `Run `cd <path>`` hint, mirroring prune. |

## Testing

- **Unit tests** in `core/worktree/remove_repo.rs`:
  - Path resolution: vanilla repo, daft repo, bare-only directory, non-Git path
    → error.
  - DAG construction: 1 + N tasks; terminal `RemoveBare` depends on every
    `RemoveWorktree`; no edges between worktree tasks.
- **YAML manual scenarios** under `tests/manual/scenarios/repo/`. Each scenario
  exercises end-to-end behavior using the sandbox machinery (no mocks; per
  `CLAUDE.md`):
  - `remove-basic.yml` — daft repo with multiple worktrees, no hooks.
  - `remove-with-hooks.yml` — pre/post hooks both succeed; one variant where
    pre-hook fails (verifies removal still happens, summary shown).
  - `remove-from-inside.yml` — cwd is a worktree of the target repo; verify
    `DAFT_CD_FILE` redirect.
  - `remove-vanilla.yml` — plain non-daft repo, no hooks, no trust entry.
  - `remove-non-git-fails.yml` — path is `/tmp` or similar; hard error, no
    filesystem changes.
  - `remove-dry-run.yml` — `--dry-run` prints plan; verify nothing changed on
    disk.
  - `remove-force.yml` — `--force` skips prompt; non-TTY without `--force`
    errors.
- **Integration test** `tests/integration/repo-remove.bats`:
  - Set up a temp daft-cloned repo with a hook that writes a marker file in
    `pre-remove` and another in `post-remove`. Assert the markers exist after
    the run, the worktrees are gone, the bare dir is gone, and the trust DB
    entry is gone.
- **Sandbox-script regression test**: a YAML scenario that creates two repos
  under `${sandbox_dir}/test/`, invokes `mise run sandbox:clean-repos`, asserts
  both repos are gone and that no spurious files were touched in `bin/`,
  `config/`, `data/`, or `state/`.

Per CLAUDE.md, every change above includes regression tests; bug fixes (none yet
at design time) would each add a YAML scenario.

## Pre-commit checklist (CI parity)

- `mise run fmt`
- `mise run clippy` — must pass with zero warnings
- `mise run test:unit`
- `mise run test:integration` for the new bats file
- `mise run man:gen` — commit the generated `man/git-daft-repo-remove.1`
- `mise run docs:site:check` — Biome lint of docs config (if docs changed)

## Open Questions

None. Q&A from brainstorming resolved each open decision (path semantics, hook
failure policy, force/dry-run UX, command surface, sandbox script behavior).

## Out of Scope / Future Work

- Other `daft repo` subcommands (`list`, `info`, etc.) — directory layout and
  routing make these straightforward additions later.
- A `--keep-bare` flag (remove worktrees but keep the bare git dir) — not
  requested; trivial to add later if needed.
- Cleanup of the host machine's daft state outside the repo (cache files, log
  directories specific to the repo) — orthogonal; the trust DB entry is the only
  piece of host state we know about today.
