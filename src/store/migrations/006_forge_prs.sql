-- Cached forge pull/merge requests for `daft list --columns +pr` and
-- `pr:`/`mr:` tab completion.
--
-- One row per (repo, kind, number): a snapshot of the forge's PR/MR list the
-- last time a refresh ran (write-through when `daft go pr:N` resolves one,
-- wholesale via the background refresh after remote-touching commands). A
-- display/completion accelerator, NOT authoritative state — the forge is;
-- `fetched_at` records snapshot age so consumers can judge staleness.
--
-- Conventions follow 001_initial.sql: TEXT ISO-8601 UTC timestamps, composite
-- primary key, no blobs. `title` is attacker-influenced text (anyone can open
-- a PR against a public repo) and every reader renders it into a terminal or
-- a shell completion stream — it is sanitized (control characters stripped)
-- BEFORE persistence so readers can trust the store.
CREATE TABLE forge_prs (
    repo_hash     TEXT    NOT NULL,
    kind          TEXT    NOT NULL, -- 'pr' (GitHub) | 'mr' (GitLab)
    number        INTEGER NOT NULL,
    title         TEXT    NOT NULL,
    state         TEXT    NOT NULL, -- 'open' | 'merged' | 'closed'
    head_branch   TEXT    NOT NULL, -- the PR's source branch name
    is_cross_repo INTEGER NOT NULL, -- 1 = head lives in a fork
    ci_status     TEXT,             -- 'pass' | 'fail' | 'pending'; NULL = no CI
    url           TEXT    NOT NULL,
    author        TEXT    NOT NULL,
    fetched_at    TEXT    NOT NULL,
    PRIMARY KEY (repo_hash, kind, number)
);

-- Outbound lookup: "is there an open PR whose head is this local branch?"
CREATE INDEX idx_forge_prs_head_branch ON forge_prs (repo_hash, head_branch);
