-- Cached working-tree disk sizes for `daft list --columns +size`.
--
-- One row per (repo, branch): the size of that worktree's directory tree the
-- last time it was walked, so the Size column can render a last-known value
-- instantly and refresh it in the background. This is a display accelerator,
-- NOT authoritative state — a full walk always runs and overwrites the value,
-- and `measured_at` lets the UI mark a not-yet-refreshed value as stale.
--
-- (Unlike the SHA-keyed cell caches, a directory size is not a pure function of
-- any content hash — see core/worktree/cell_cache.rs — which is exactly why it
-- lives here with explicit staleness rather than being memoized as if correct.)
--
-- Conventions follow 001_initial.sql: TEXT ISO-8601 UTC timestamps, composite
-- primary key, no blobs. `branch_slug` may contain slashes ("feat/x") — it is
-- an opaque TEXT key component. The key is (repo_hash, branch_slug) so a cached
-- size survives worktree moves/renames; `worktree_path` is stored for eviction
-- and the removed-target guard (a vanished path must not clobber a good size).
CREATE TABLE worktree_sizes (
    repo_hash     TEXT    NOT NULL,
    branch_slug   TEXT    NOT NULL,
    worktree_path TEXT    NOT NULL,
    size_bytes    INTEGER NOT NULL,
    measured_at   TEXT    NOT NULL,
    PRIMARY KEY (repo_hash, branch_slug)
);
