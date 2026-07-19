-- The branch each worktree was created for, so `daft list` can still name a
-- worktree whose HEAD is detached for a reason git records nowhere.
--
-- Identity is normally derived live: the porcelain names the checked-out
-- branch, and a paused rebase records the branch it is replaying (see
-- src/git/op_state.rs). That covers everything git itself knows. What it does
-- not cover is a plain detached checkout — someone checked out a tag or a SHA
-- in this worktree — where nothing on disk connects the worktree back to the
-- branch it exists for. This table remembers that one fact.
--
-- **Derived state always wins.** These rows are a fallback consulted only
-- after live git state has nothing to say, and a cross-check for drift (the
-- record disagreeing with an attached HEAD). Live state cannot be stale; a row
-- here can. Treat it as a hint, never as authority — and never let it
-- contradict an operation in progress.
--
-- Keyed on the worktree's private-gitdir id — the directory name under
-- `<common-dir>/worktrees/` — rather than the path or the branch. That id
-- survives both `git worktree move` and a branch rename, which is exactly what
-- a record of *intent* has to survive to stay useful. Path is stored for
-- display and eviction only.
--
-- Conventions follow 001_initial.sql: TEXT ISO-8601 UTC timestamps, composite
-- primary key, no blobs. `branch` may contain slashes ("feat/x") — it is an
-- opaque TEXT value.
CREATE TABLE worktree_identities (
    repo_hash     TEXT NOT NULL,
    worktree_id   TEXT NOT NULL,
    branch        TEXT NOT NULL,
    worktree_path TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (repo_hash, worktree_id)
);
