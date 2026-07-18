-- Forge-integration health for this repo, driving the default `pr` column's
-- visibility in `daft list`. A single row (id = 1) — the coordinator store is
-- per-repo, so this is repo-level state.
--
-- `healthy = 0` records a *deep* refresh failure: one that keeps failing
-- until the user intervenes (gh/glab not installed, authentication dead,
-- repo access lost — `error_kind` says which), and it silently hides the
-- default-sourced `pr` column from the next `daft list` on. Transient
-- failures (network, rate limit) never flip it. The next successful refresh
-- flips it back and the column reappears — both verdicts persist.
--
-- Timestamps are TEXT ISO-8601 UTC per 001_initial.sql. `started_at` is the
-- background-refresh throttle key (spawned refreshes are skipped while a
-- recent attempt exists); `finished_at` concludes the live table's
-- refresh-in-flight display state (statusless cells settle when it
-- advances); `succeeded_at` NULL means no snapshot was ever taken, which
-- drives the first-load skeleton in the PR column.
CREATE TABLE forge_health (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    healthy      INTEGER NOT NULL DEFAULT 1,
    error_kind   TEXT,             -- 'missing-tool' | 'unauthenticated' | 'repo-access'
    started_at   TEXT,
    finished_at  TEXT,
    succeeded_at TEXT
);
