-- What *kind* of worktree an identity row describes, so the table can carry
-- anonymous sandboxes (#53) alongside the branch intents it was built for.
--
-- 009 records the branch a worktree was created for. Anonymous worktrees
-- (detached-HEAD sandboxes) have no branch — their identity is the directory
-- name, and their one durable fact is which commit they were pinned at and
-- what spelling asked for it. Rather than a sibling table, the same row now
-- says which of the two it is:
--
--   kind = 'branch'     `branch` holds the branch this worktree is for (009's
--                       meaning, unchanged; the backfill default).
--   kind = 'canonical'  the shared sandbox `daft go <commit-ish>` visits;
--                       `branch` holds the sandbox's directory name.
--   kind = 'fork'       a private sandbox from `daft start --fork`; `branch`
--                       holds the directory name. Never matched by `go`'s
--                       resolution — forks are reachable only by name.
--
-- `source_spelling` is what the user typed (`v1.18.0`, `origin/master`,
-- `HEAD~2`); `pinned_commit` is the full OID the sandbox was created at.
-- Both are NULL on kind='branch' rows. `pinned_commit` is what lets removal
-- warn when commits were made on top, and what dedupes different spellings
-- of one commit onto one canonical sandbox.
--
-- Every branch-keyed delete must now discriminate on `kind`, or removing a
-- branch named like a sandbox directory would take the sandbox's record with
-- it (and vice versa).
ALTER TABLE worktree_identities ADD COLUMN kind TEXT NOT NULL DEFAULT 'branch';
ALTER TABLE worktree_identities ADD COLUMN source_spelling TEXT;
ALTER TABLE worktree_identities ADD COLUMN pinned_commit TEXT;
