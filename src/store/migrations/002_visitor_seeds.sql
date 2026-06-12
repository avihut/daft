-- Visitor-config seed provenance.
--
-- One row per (repo, branch, filename): the exact content daft last wrote
-- INTO that branch's worktree for an untracked daft file (daft.yml seeded at
-- worktree creation, daft.local.yml overlays, consolidation refreshes).
-- Lifecycle commands compare the worktree's current bytes against `content`
-- to classify the copy as pristine (byte-equal) or refined (edited since),
-- and use `content` as the base of three-way merges.
--
-- Conventions follow 001_initial.sql: TEXT ISO-8601 UTC timestamps,
-- composite primary key, no blobs. `branch_slug` may contain slashes
-- (e.g. "feat/x") — it is an opaque TEXT key component.
--
-- `seeded_at` is the original provenance timestamp and survives upserts;
-- `updated_at` tracks the latest refresh (re-seed or consolidation).
CREATE TABLE visitor_seeds (
    repo_hash   TEXT NOT NULL,
    branch_slug TEXT NOT NULL,
    filename    TEXT NOT NULL,
    content     TEXT NOT NULL,
    seeded_at   TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (repo_hash, branch_slug, filename)
);
