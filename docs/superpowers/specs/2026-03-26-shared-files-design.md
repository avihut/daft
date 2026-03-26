# Shared Files Design

Centralize untracked configuration files (`.env`, `.idea/`, `.vscode/`, etc.)
across worktrees using symlinks, with the ability to materialize per-worktree
overrides.

## Problem

Projects often have untracked local configuration files that IDEs, environment
tools, and other software depend on. In a worktree-based workflow, each worktree
starts without these files, forcing developers to manually copy or recreate
them. Files drift between worktrees, and there is no mechanism to keep them in
sync.

## Design Decisions

- **Configuration in `daft.yml` only** (team-shared, tracked). Local-only config
  shadowing is deferred to future work.
- **Centralized storage in `.git/.daft/shared/`** inside the git common dir.
  Layout-agnostic since every layout has a git common dir.
- **Initial population from the current worktree** via `daft shared add`. The
  command moves the file to shared storage, replaces it with a symlink, and adds
  the path to `daft.yml`.
- **Symlinks as the linking mechanism.** The symlink is created at the exact
  declared path. For files like `.env` or directories like `.idea/`, the symlink
  is at the worktree root. For nested paths like `.vscode/settings.json`, daft
  creates intermediate directories (`.vscode/`) if needed and symlinks the leaf
  entry. This means `.vscode/` can contain both shared symlinks and non-shared
  local files.
- **Linking happens during worktree creation, before `worktree-post-create`
  hooks**, so hooks can depend on shared files like `.env`.
- **Conflicts are warned, not resolved.** When a real file exists where a
  symlink should go, daft warns and skips. No automatic replacement.
- **Materialization tracking** via `.git/.daft/materialized.json` (outside
  `shared/` to avoid collisions with user files).
- **Gitignore enforcement.** `add` ensures shared paths are in `.gitignore`
  (prevents tracking symlinks or shared content). `remove` does not remove from
  `.gitignore`.
- **Relative symlinks.** Symlinks use relative paths to the shared storage for
  portability (surviving repo directory moves). Consistent with the existing
  symlink pattern in `shortcuts.rs`.
- **Unix only for now.** Symlinks on Windows require elevated privileges or
  developer mode. Windows support is deferred.

## Storage Architecture

```
.git/                               (bare repo / git common dir)
├── .daft/
│   ├── previous-worktree           (existing)
│   ├── materialized.json           (NEW: tracks per-worktree materializations)
│   └── shared/                     (NEW: centralized file storage)
│       ├── .env                    (actual file content)
│       ├── .idea/                  (actual directory)
│       └── .vscode/
│           └── settings.json

repo.feat-auth/                     (worktree)
├── .env        → .git/.daft/shared/.env        (symlink)
├── .idea/      → .git/.daft/shared/.idea/      (symlink)
├── .vscode/
│   ├── settings.json → .git/.daft/shared/.vscode/settings.json  (symlink)
│   └── launch.json                 (local, not shared)
└── src/

repo.fix-bug-42/                    (worktree, materialized .env)
├── .env                            (real file — materialized copy)
├── .idea/      → .git/.daft/shared/.idea/      (symlink)
└── src/
```

## Configuration

The `shared:` key in `daft.yml` is a list of paths relative to the worktree
root:

```yaml
shared:
  - .env
  - .idea
  - .vscode/settings.json

hooks:
  worktree-post-create:
    jobs:
      - name: install
        run: npm install # can rely on .env being symlinked already
```

## Materialization Tracking

`.git/.daft/materialized.json`:

```json
{
  ".env": ["/Users/dev/projects/repo.fix-bug-42"]
}
```

Keyed by shared path, value is a list of worktree absolute paths that have
materialized that file. Checked during `sync`, `status`, worktree creation, and
worktree removal (cleanup stale entries). `link` removes entries, `materialize`
adds them.

## Command Surface

### `daft shared add <path>...`

Collect file/dir from the current worktree into shared storage.

1. Validate the path exists in the current worktree.
2. Validate the path is not tracked by git. Error if tracked: "`.env` is tracked
   by git. Untrack it first with `git rm --cached .env`".
3. Validate the path is not already shared. Error if so: "`.env` is already
   shared. Use `daft shared link .env` to symlink this worktree's copy."
4. Ensure the path is in `.gitignore` (add if missing).
5. Move the file/dir to `.git/.daft/shared/<path>`.
6. Replace the original with a symlink to the shared location.
7. Add the path to `shared:` in `daft.yml`.

### `daft shared add --declare <path>...`

Declare a path as shared without collecting it. Adds to `daft.yml` and
`.gitignore`. No file is moved. The path will be linked in worktrees once a file
is collected (via `add` without `--declare`). Useful for paths that don't exist
yet (e.g., `.env` that gets created later).

### `daft shared remove <path>...`

Stop sharing a file. Default behavior: materialize in all worktrees that have
symlinks, then delete from shared storage, remove from `daft.yml`. Does not
remove from `.gitignore`.

**`--delete` flag:** Delete the shared file from storage and remove all symlinks
across worktrees. No materialization. Remove from `daft.yml`.

Completions: from `shared:` list in `daft.yml`.

### `daft shared materialize <path>...`

Replace the symlink with a copy of the shared file in the current worktree.
Records the materialization in `materialized.json`.

- If the path is already a real file (not a symlink to shared): no-op with
  message.
- **`--override` flag:** Force materialization even if a non-shared real file
  exists at the path.

Completions: from `shared:` list in `daft.yml`.

### `daft shared link <path>...`

Replace a local file with a symlink to the shared version. Removes the worktree
from `.materialized.json` for that path.

- If the local file differs from the shared version: error with message "Local
  `.env` differs from shared version. Use `--override` to replace."
- **`--override` flag:** Replace without checking for differences.
- If the path is already a symlink to shared: no-op with message.

Completions: from `shared:` list in `daft.yml`.

### `daft shared status`

Display all shared files and their per-worktree state:

```
Shared files:

  .env
    repo.main          linked
    repo.feat-auth     linked
    repo.fix-bug-42    materialized

  .idea
    repo.main          linked
    repo.feat-auth     linked
    repo.fix-bug-42    linked

  .vscode/settings.json (declared, not yet collected)
```

States: `linked`, `materialized`, `missing`, `conflict` (real file exists, not
shared), `broken` (symlink target missing).

### `daft shared sync`

Ensure all worktrees have symlinks for all declared shared files.

- Iterates all worktrees from `git worktree list`.
- For each declared shared file in each worktree:
  - If symlink already exists and points to shared: skip.
  - If shared storage has the file and worktree has no file: create symlink.
  - If worktree has a real file (conflict): warn, skip.
  - If worktree is in `.materialized.json` for this path: skip.
  - If shared storage is empty for this path (declared only): skip.
  - If symlink exists but is broken: warn.

## Worktree Lifecycle Integration

### Creation (clone / checkout)

After `git worktree add` completes and before `worktree-post-create` hooks:

1. Read `shared:` from `daft.yml`.
2. For each declared path:
   - Shared storage has the file → create symlink.
   - Worktree already has a real file → warn ("`.env` exists but is not shared.
     Run `daft shared link .env` to replace."). Continue.
   - Neither exists (declared only) → skip silently.

### Removal

No special action. Symlinks and materialized files are deleted with the
worktree. Shared storage is untouched. Stale entries in `.materialized.json` are
cleaned up on next `sync` or `status`.

## Error Cases

| Scenario                                   | Behavior                                         |
| ------------------------------------------ | ------------------------------------------------ |
| `add` on a git-tracked file                | Error: untrack first                             |
| `add` on an already-shared path            | Error: use `link` instead                        |
| `add` when file doesn't exist              | Error: file not found (use `--declare`)          |
| `link` when local differs from shared      | Error: use `--override`                          |
| `materialize` on a non-symlink             | No-op with message                               |
| `remove` with broken symlinks in worktrees | Delete broken symlinks, continue                 |
| Shared storage file manually deleted       | `status` shows "broken", `sync` warns            |
| Worktree created outside daft              | `sync` handles it (iterates `git worktree list`) |

## Shell Completions

- `add`: filesystem completion (default).
- `add --declare`: filesystem completion (default).
- `remove`, `materialize`, `link`: complete from `shared:` list in `daft.yml`.
- `status`, `sync`: no path arguments.

## Implementation Scope

### New files

- `src/commands/shared.rs` — command routing and clap `Args`
- `src/core/shared.rs` — core logic (add, remove, materialize, link, sync,
  status)

### Modified files

- `src/main.rs` — add routing for `daft shared` / `git-worktree-shared`
- `src/commands/mod.rs` — add `shared` module
- `src/core/mod.rs` — add `shared` module
- `src/hooks/yaml_config.rs` — add `shared` field to `YamlConfig`
- `xtask/src/main.rs` — add to `COMMANDS` array
- `src/commands/docs.rs` — add to help categories
- Shell completion files (`bash.rs`, `zsh.rs`, `fish.rs`, `fig.rs`, `mod.rs`) —
  add `shared` subcommand and verb completions
- Man page generation after implementation

### Worktree creation integration

Shared file linking must run before `PostCreate` hooks. The hook dispatch sites
are spread across multiple modules:

- `src/core/worktree/checkout.rs` (~line 302) — single-branch checkout
- `src/core/worktree/checkout_branch.rs` (~line 174) — branch checkout flow
- `src/commands/clone.rs` — multiple call sites for single-branch, multi-branch
  TUI, and sequential flows (~lines 448, 614, 856, 1017, 1297)

The shared file linking function should live in `src/core/shared.rs` and be
called at each of these sites immediately before the `PostCreate` hook dispatch.
Alternatively, it could be integrated into the hook executor itself as a
pre-hook step — this decision is left to implementation.
