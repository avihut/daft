-- Cached repo disk sizes for `daft repo list --columns +size`.
--
-- One row per catalog repo (keyed by its uuid): the size of the repo's
-- directory tree the last time it was walked, so the Size column renders a
-- last-known value instantly and refreshes in the background. Same rationale as
-- the coordinator lineage's worktree_sizes — a display accelerator, never
-- authoritative: a full walk always runs and overwrites the value, and
-- `measured_at` lets the UI mark a not-yet-refreshed value as stale.
--
-- Keyed by `uuid` (the catalog primary key), so a cached size survives repo
-- rename/move; re-cloning at a path retires the old uuid and inserts a new one,
-- which correctly yields a fresh (empty) size entry. No foreign key: catalog
-- rows are soft-deleted (removed_at), so an ON DELETE CASCADE would never fire —
-- the join is by uuid, and `repo_path` is kept for the removed/moved-target
-- guard (a vanished path must not overwrite a good size with 0).
--
-- Conventions follow 001_catalog.sql: ISO-8601 UTC text timestamps, no blobs.
CREATE TABLE repo_sizes (
    uuid        TEXT    NOT NULL PRIMARY KEY,
    repo_path   TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL,
    measured_at TEXT    NOT NULL
);
