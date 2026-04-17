# Repo Identity for Log Store — Design Spec

**Branch:** `feat/background-hook-jobs` (follow-up to the hooks-jobs redesign
basket)

## Problem

The log store is keyed by a hash derived from the **local filesystem path** of
the project root:

```rust
// src/commands/hooks/jobs.rs:442-444
let mut hasher = DefaultHasher::new();
project_root.display().to_string().hash(&mut hasher);
Ok(format!("{:016x}", hasher.finish()))
```

A matching implementation lives in `src/hooks/yaml_executor/mod.rs:436`.

This conflates "repository location" with "repository identity." Deleting a repo
and re-cloning it to the same path produces a new repo that `daft` treats as a
continuation of the old one. Stale invocations — including failures, cancelled
jobs, and cross-worktree retry history — re-attach to a fresh clone that has no
meaningful relationship to them.

## Goal

Key the log store (and coordinator socket) by an identity that lives **with the
repo**, so that:

- Deleting a repo and re-cloning at the same path yields a clean history.
- Moving a repo to a different path preserves its history.
- All worktrees under the same bare `.git` share one identity.
- Two independent clones of the same remote at different paths have distinct
  identities (they are different working copies).

## Identity scheme

On first log-store access, write a UUID v7 into `<git-common-dir>/daft-id`. The
file contents — the canonical 36-char hyphenated UUID string — serve as the repo
identity. All existing call sites that compute a "repo hash" read this file
instead of hashing the path.

### Why `<git-common-dir>/daft-id`

- In daft's bare+worktree layout, the common git dir is the bare. All worktrees
  resolve to it via `git rev-parse --git-common-dir`. Worktrees therefore share
  the ID automatically.
- The file is physically inside the repo, so `rm -rf <project-root>` destroys it
  alongside the repo. Re-cloning produces a fresh identity on next touch.
- No dependence on remotes, config, or history — works for local-only repos,
  shallow clones, and freshly `git init`'d repos.

### Why UUID v7

- 74 random bits → collision-resistant at any plausible repo count per host.
- Crypto-grade randomness (OS RNG via `getrandom`), not a hashed seed.
- Timestamp prefix gives lexicographic ordering of directories when sorted —
  `ls $DAFT_STATE_DIR/jobs/` is ordered by first-touch time, a useful side
  benefit at zero cost.
- Standard 36-char canonical format, parseable by any UUID tooling.

### Why lazy creation

The ID only matters when a log-store directory is created or read. Lazy creation
covers both cases in a single codepath: on every `compute_repo_id()` call, read
the file; if missing, generate a new UUID v7, write it atomically, return it. No
need to touch `daft clone`, `daft init`, or any other repo-creating command.

## Implementation

### New module

**`src/core/repo_identity.rs`** (new file):

```rust
pub fn compute_repo_id() -> Result<String>;
pub fn compute_repo_id_from_common_dir(git_common_dir: &Path) -> Result<String>;
```

`compute_repo_id`:

1. Resolve `git_common_dir` via existing `core::repo::get_git_common_dir()`.
2. Call `compute_repo_id_from_common_dir(&dir)`.

`compute_repo_id_from_common_dir` — race-safe and corruption-aware recipe:

1. Attempt to read `<git-common-dir>/daft-id`. If the read succeeds and the
   contents trim to a well-formed UUID, return the canonical string form.
2. If the file exists but trims to the empty string, treat as absent (step 4) —
   an empty file is almost certainly a crashed previous write.
3. If the file exists and has non-empty but unparseable contents, **error out**
   with a message pointing the user at the file: _"Corrupt repo identity file at
   <path>. Delete it to regenerate (this will orphan existing job logs for this
   repo)."_ Do not silently overwrite — that would orphan a valid repo whose
   identity was temporarily unreadable (bad permissions, transient I/O error,
   partial filesystem view).
4. If the file is absent (or empty per step 2), generate a fresh UUID v7 and
   attempt atomic create-or-fail via
   `OpenOptions::new().write(true).create_new(true).open(path)`. This fails with
   `EEXIST` if another process created the file first.
5. On successful creation, write the 36-byte canonical UUID string (single
   syscall, well below PIPE_BUF — not torn) and return it.
6. On `EEXIST`, loop back to step 1 — the winner's UUID is now on disk and we
   adopt it.

`rename()` is NOT used. POSIX `rename()` atomically replaces existing files,
which would let two racing processes both "win" with the last writer's UUID
becoming canon — silently orphaning the first writer's log-store entries.
`create_new(true)` gives us actual create-if-absent semantics.

The returned string is used verbatim as the log-store directory name — no
additional hashing. UUID v7 canonical form is 36 chars of `[0-9a-f-]`, safe as a
filesystem path component on all platforms daft supports.

### Call-site migration

The path-hash computation is currently duplicated across **eight** sites. All
must migrate to `crate::core::repo_identity::compute_repo_id()`:

1. `src/commands/hooks/jobs.rs:435` — `compute_repo_hash_from_path` (helper
   called from 7 places within the file).
2. `src/hooks/yaml_executor/mod.rs:436` — `compute_repo_hash` (takes a
   `HookEnvironment` arg but hashes the project root).
3. `src/core/worktree/prune.rs:835-841` — inline `DefaultHasher` block, used to
   resolve the coordinator socket for pruning.
4. `src/commands/complete.rs:794-805` — inline block in
   `complete_job_addresses`.
5. `src/commands/complete.rs:1003-1014` — inline block in
   `complete_retry_targets`.
6. `src/commands/complete.rs:1115-1126` — inline block in
   `complete_retry_worktrees`.
7. `src/commands/complete.rs:1174-1185` — inline block in
   `complete_listing_worktrees`.
8. `src/commands/complete.rs:1220-1231` — inline block in `complete_hook_types`.

After migration, `DefaultHasher` imports are removed from every file that
previously computed the path hash. All consumers use the UUID string as a
drop-in replacement for the previous 16-hex string.

### `list_all_repo_hashes()` behavior

`src/commands/hooks/jobs.rs:448 — list_all_repo_hashes` walks the `jobs/` dir by
directory name. After the cut, that dir may contain:

- New 36-char UUID v7 directories (valid repos under the new scheme).
- Orphaned 16-hex-char path-hash directories from before this change.

Filter out entries that don't parse as valid UUIDs. This keeps
`daft hooks jobs clean --all` focused on real repos and silently ignores the
orphaned directories. The orphans can be cleaned manually with `rm -rf`.

### Cargo dependency

Add `uuid = { version = "1", features = ["v7"] }` to `Cargo.toml`. The `v7`
feature pulls in `getrandom` transitively.

## Behavior on existing state

This branch is pre-release; no user has persisted data that matters. Existing
path-hash directories under `$DAFT_STATE_DIR/jobs/` are orphaned. Dev
environments can wipe them manually:

```bash
rm -rf "$DAFT_STATE_DIR/jobs/"
```

No migration code is written. No fallback to the old hash scheme. The change is
a clean cut.

## Coordinator socket compatibility

The coordinator socket path also uses the repo hash (via
`crate::coordinator::client::...`). Switching to UUID v7 changes socket paths
for running coordinators across the cut. Impact: any coordinator started with
the old hash scheme becomes orphaned at upgrade time — the new daft binary opens
a new socket under the new ID, and the old coordinator has no fresh clients.

Since this is a pre-release branch, no orchestration is needed. Users upgrading
their dev build should kill any stale coordinator processes (or reboot their
sandbox). No code handling required.

## File inventory

| File                             | Change                                                                                                    |
| -------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `Cargo.toml`                     | Add `uuid = { version = "1", features = ["v7"] }`                                                         |
| `src/core/mod.rs`                | Add `pub mod repo_identity;`                                                                              |
| `src/core/repo_identity.rs`      | New: `compute_repo_id`, `compute_repo_id_from_common_dir`, tests                                          |
| `src/commands/hooks/jobs.rs`     | Remove `compute_repo_hash_from_path`; update 7 call sites; filter non-UUID dirs in `list_all_repo_hashes` |
| `src/hooks/yaml_executor/mod.rs` | Remove `compute_repo_hash`; update 2 call sites                                                           |
| `src/core/worktree/prune.rs`     | Replace inline `DefaultHasher` block with `compute_repo_id()`                                             |
| `src/commands/complete.rs`       | Replace 5 inline `DefaultHasher` blocks (one per completion helper) with `compute_repo_id()`              |

## Testing

### Unit tests in `src/core/repo_identity.rs`

- Creates file when absent, returns generated UUID.
- Reuses existing file across calls (idempotent per-repo).
- Generated UUID is valid v7 (version field = 7 in the canonical string).
- Two separate temp common-dirs produce distinct IDs.
- Empty file is treated as absent and overwritten (crashed-write recovery).
- Non-empty, non-UUID contents produce an error (refuse to silently orphan a
  valid repo on corruption).
- Concurrent creation (two threads calling in parallel against the same temp
  dir) converges on one ID for both — race resolved by `create_new` EEXIST.

### Unit test for `list_all_repo_hashes`

- A jobs dir containing both a UUID-named dir and a 16-hex-char-named dir
  returns only the UUID entry.

### Integration scenario

New scenario `tests/manual/scenarios/hooks/repo-identity-on-reclone.yml`:

1. Clone a test repo, trigger a hook that fails, verify `daft hooks jobs` shows
   the failed invocation.
2. Remove the repo directory (`rm -rf`).
3. Re-clone the same remote to the same path.
4. Run `daft hooks jobs` — verify **no invocations are shown** (fresh ID, fresh
   log store view).
5. Trigger a hook, verify a new invocation appears and the old one stays hidden.

## Non-goals

- **Migrating old path-hash log entries** — orphaned, as discussed.
- **Cross-machine identity** — the daft-id file is per-host; two clones on
  different machines have different IDs. Fine, since the log store is local.
- **Repo identity for non-daft consumers** — this ID is internal to daft's log
  store and coordinator. Not exposed as a public API or surfaced in user output.
- **Handling deliberate ID collisions** — if a user hand-copies a `daft-id` file
  between repos to deliberately share history, that's on them.
