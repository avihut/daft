-- Learned resource profiles for repository hook scripts (#678), plus the
-- governor's event log. Written by the sync push resource governor; keyed
-- by the resolved hook file's content hash so a changed hook re-profiles
-- from scratch.

CREATE TABLE hook_profiles (
    repo_hash      TEXT NOT NULL,
    stage          TEXT NOT NULL,
    hook_hash      TEXT NOT NULL,
    peak_rss_bytes INTEGER NOT NULL,
    wall_ms        INTEGER NOT NULL,
    runs           INTEGER NOT NULL DEFAULT 1,
    updated_at     TEXT NOT NULL,
    PRIMARY KEY (repo_hash, stage, hook_hash)
);

-- What the governor did and why (throttles, freezes, kill-requeues,
-- timeouts) — the raw material for `daft hooks jobs` / doctor explanations.
CREATE TABLE governor_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_hash   TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    kind        TEXT NOT NULL,
    branch      TEXT,
    detail_ms   INTEGER,
    rss_bytes   INTEGER
);

CREATE INDEX governor_events_repo_idx ON governor_events (repo_hash, occurred_at);
