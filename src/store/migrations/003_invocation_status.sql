-- Trust-skipped hook fires (#596): record skips in `invocations` so daft can
-- suggest replaying them after `git daft hooks trust`.
--
-- `status` mirrors the `jobs.status` lowercase-string convention:
--   'completed'  default — the historical meaning of a row ("this fired");
--                no production rows existed before this migration.
--   'skipped'    the hook did not run; `skip_reason` says why.
-- `skip_reason` is NULL unless skipped:
--   'untrusted'           trust level Deny — blocked by the trust gate.
--   'prompt-unavailable'  trust level Prompt with no interactive callback
--                         (includes fingerprint-mismatch downgrades).
--
-- Skip rows are advisory replay state, not an audit log (the `jobs` table is
-- the audit log): the writer keeps at most one skipped row per
-- (repo_hash, hook_type, worktree) and deletes it when the trust gate next
-- passes for that pair. No new index: the PK (repo_hash, invocation_id)
-- prefix already serves the only query pattern (all skipped rows for a repo).

ALTER TABLE invocations ADD COLUMN status TEXT NOT NULL DEFAULT 'completed';
ALTER TABLE invocations ADD COLUMN skip_reason TEXT;
