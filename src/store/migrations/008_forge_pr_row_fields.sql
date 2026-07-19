-- Fields for the default open-PR rows in `daft list`: every open PR now
-- surfaces as a row (not just a decoration on an existing one), so the cache
-- carries what a synthesized row displays beyond 006's columns.
--
-- `head_repo_owner` is the head (fork) repository's owner login, rendered as
-- the `owner:branch` prefix that keeps fork branch names — per-fork
-- namespaces, prone to `patch-1`/`main` collisions — visually distinct from
-- local ones. '' = same-repo head, or the platform's listing didn't carry it
-- (GitLab's REST listing names source projects only by ID), or the row came
-- from the single-PR write-through. Forge-controlled text, sanitized before
-- persistence like `title` (006).
--
-- `updated_at` is the PR's last-activity timestamp (TEXT ISO-8601 UTC per
-- 001_initial.sql), filling the Age cell on synthesized rows. NULL = the
-- platform/write-through path didn't supply one.
ALTER TABLE forge_prs ADD COLUMN head_repo_owner TEXT NOT NULL DEFAULT '';
ALTER TABLE forge_prs ADD COLUMN updated_at TEXT;
