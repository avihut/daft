-- Initial schema for the daft repo catalog store.
--
-- The catalog is daft's first *global* (cross-repo) database: one file at
-- `<data-dir>/catalog/catalog.db`, its own migration lineage, same
-- conventions as the coordinator store:
--   * Timestamps stored as ISO-8601 UTC text (chrono `to_rfc3339()`).
--   * `removed_at IS NULL` means the entry is live. Removed entries are
--     retained (uuid, name, path) so job logs of removed repos stay
--     addressable and `daft clone <name>` can restore from `remote_url`.
--   * `path` is the project root users interact with; `git_common_dir` is
--     what trust, hooks, and `daft-id` key on. The mapping between the two
--     is layout-dependent, so both are captured at registration time.
--   * `remote_url_normalized` is the match key for relations-manifest
--     resolution (host-lowercased, scheme/user/`.git` stripped);
--     `remote_url` keeps the as-configured form for display and re-clone.
--
-- Partial unique indexes enforce the two catalog invariants at the DB
-- level: live names are unique (auto-suffix collision policy happens above
-- the store), and a path hosts at most one live entry (re-cloning at a
-- path retires the previous uuid).

CREATE TABLE catalog_repos (
    uuid                  TEXT NOT NULL PRIMARY KEY,
    name                  TEXT NOT NULL,
    path                  TEXT NOT NULL,
    git_common_dir        TEXT NOT NULL,
    remote_url            TEXT,
    remote_url_normalized TEXT,
    default_branch        TEXT,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    removed_at            TEXT
);

CREATE UNIQUE INDEX catalog_repos_live_name_idx
    ON catalog_repos(name) WHERE removed_at IS NULL;
CREATE UNIQUE INDEX catalog_repos_live_path_idx
    ON catalog_repos(path) WHERE removed_at IS NULL;
CREATE INDEX catalog_repos_url_idx
    ON catalog_repos(remote_url_normalized);
CREATE INDEX catalog_repos_gcd_idx
    ON catalog_repos(git_common_dir);
