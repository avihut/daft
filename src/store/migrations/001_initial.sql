-- Initial schema for the daft coordinator store.
--
-- Conventions:
--   * Timestamps stored as ISO-8601 UTC text (chrono `to_rfc3339()`).
--   * Hashmap / Vec fields serialized as JSON text. There are no large blobs.
--   * Booleans stored as INTEGER 0/1.
--   * Primary keys are composite where the natural key is composite. SQLite
--     creates the matching index automatically.
--   * Explicit indexes cover lookup patterns the application uses repeatedly
--     and where the PK index would not already serve.
--
-- The `invocations` table is present for forward compatibility but is not
-- populated by the current production code (jobs were always written
-- standalone under the prior redb store). A future feature will start
-- recording invocation rows, and at that point a follow-up migration will
-- add `FOREIGN KEY (repo_hash, invocation_id) REFERENCES invocations(...)
-- ON DELETE CASCADE` to `jobs` and switch `log_clean` to rely on the
-- cascade. Declaring the FK now would refuse every job insert until
-- invocations are actually populated.

CREATE TABLE invocations (
    repo_hash       TEXT    NOT NULL,
    invocation_id   TEXT    NOT NULL,
    trigger_command TEXT    NOT NULL,
    hook_type       TEXT    NOT NULL,
    worktree        TEXT    NOT NULL,
    created_at      TEXT    NOT NULL,
    coordinator_pid INTEGER,
    PRIMARY KEY (repo_hash, invocation_id)
);

CREATE TABLE jobs (
    repo_hash           TEXT    NOT NULL,
    invocation_id       TEXT    NOT NULL,
    name                TEXT    NOT NULL,
    hook_type           TEXT    NOT NULL,
    worktree            TEXT    NOT NULL,
    command             TEXT    NOT NULL,
    working_dir         TEXT    NOT NULL,
    env_json            TEXT    NOT NULL,
    started_at          TEXT    NOT NULL,
    finished_at         TEXT,
    status              TEXT    NOT NULL,
    exit_code           INTEGER,
    pid                 INTEGER,
    pgid                INTEGER,
    background          INTEGER NOT NULL CHECK (background IN (0, 1)),
    needs_json          TEXT    NOT NULL,
    tags_json           TEXT    NOT NULL,
    retention_seconds   INTEGER,
    max_log_size_bytes  INTEGER,
    PRIMARY KEY (repo_hash, invocation_id, name)
);

-- Fast "all jobs for this repo across invocations" scan (used by `ls`,
-- `cancel-matching`, and reconciliation).
CREATE INDEX jobs_repo_hash_idx ON jobs(repo_hash);

-- Fast "active jobs" filter for the reconciler.
CREATE INDEX jobs_status_idx ON jobs(status);

CREATE TABLE repo_policy (
    repo_hash                   TEXT    PRIMARY KEY,
    policy_version              INTEGER NOT NULL,
    max_total_size_bytes        INTEGER,
    keep_last                   INTEGER,
    stale_running_after_seconds INTEGER
);
