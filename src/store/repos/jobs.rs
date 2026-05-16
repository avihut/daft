//! Queries against the `jobs` table.
//!
//! `env`, `needs`, and `tags` round-trip as JSON text in their respective
//! `_json` columns. Bare JSON (no schema versioning) is fine because the
//! shapes are bounded by `JobRow` itself — adding a field to one of those
//! collections means changing `JobRow`, which means a migration.

use crate::store::error::Result;
use crate::store::models::JobRow;
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

pub struct JobsRepo;

impl JobsRepo {
    /// Insert or replace a job row. The caller must have already inserted
    /// the matching invocation row — FK constraints are enforced.
    pub fn upsert(conn: &Connection, row: &JobRow) -> Result<()> {
        let env_json =
            serde_json::to_string(&row.env).expect("HashMap<String,String> is JSON-safe");
        let needs_json = serde_json::to_string(&row.needs).expect("Vec<String> is JSON-safe");
        let tags_json = serde_json::to_string(&row.tags).expect("Vec<String> is JSON-safe");
        conn.execute(
            "INSERT INTO jobs
                 (repo_hash, invocation_id, name, hook_type, worktree, command, working_dir,
                  env_json, started_at, finished_at, status, exit_code, pid, pgid,
                  background, needs_json, tags_json, retention_seconds, max_log_size_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(repo_hash, invocation_id, name) DO UPDATE SET
                 hook_type          = excluded.hook_type,
                 worktree           = excluded.worktree,
                 command            = excluded.command,
                 working_dir        = excluded.working_dir,
                 env_json           = excluded.env_json,
                 started_at         = excluded.started_at,
                 finished_at        = excluded.finished_at,
                 status             = excluded.status,
                 exit_code          = excluded.exit_code,
                 pid                = excluded.pid,
                 pgid               = excluded.pgid,
                 background         = excluded.background,
                 needs_json         = excluded.needs_json,
                 tags_json          = excluded.tags_json,
                 retention_seconds  = excluded.retention_seconds,
                 max_log_size_bytes = excluded.max_log_size_bytes",
            params![
                row.repo_hash,
                row.invocation_id,
                row.name,
                row.hook_type,
                row.worktree,
                row.command,
                row.working_dir,
                env_json,
                row.started_at.to_rfc3339(),
                row.finished_at.map(|t| t.to_rfc3339()),
                row.status,
                row.exit_code,
                row.pid,
                row.pgid,
                row.background as i64,
                needs_json,
                tags_json,
                row.retention_seconds,
                row.max_log_size_bytes.map(|n| n as i64),
            ],
        )?;
        Ok(())
    }

    pub fn get(
        conn: &Connection,
        repo_hash: &str,
        invocation_id: &str,
        name: &str,
    ) -> Result<Option<JobRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, invocation_id, name, hook_type, worktree, command, working_dir,
                        env_json, started_at, finished_at, status, exit_code, pid, pgid,
                        background, needs_json, tags_json, retention_seconds, max_log_size_bytes
                 FROM jobs
                 WHERE repo_hash = ?1 AND invocation_id = ?2 AND name = ?3",
                params![repo_hash, invocation_id, name],
                row_to_job,
            )
            .optional()?;
        Ok(row)
    }

    /// All jobs for one repo across every invocation, ordered by
    /// `started_at ASC`. Used by `daft hooks jobs ls` and the reconciler.
    pub fn list_by_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<JobRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, invocation_id, name, hook_type, worktree, command, working_dir,
                    env_json, started_at, finished_at, status, exit_code, pid, pgid,
                    background, needs_json, tags_json, retention_seconds, max_log_size_bytes
             FROM jobs
             WHERE repo_hash = ?1
             ORDER BY started_at ASC",
        )?;
        let rows: rusqlite::Result<Vec<JobRow>> =
            stmt.query_map(params![repo_hash], row_to_job)?.collect();
        Ok(rows?)
    }

    /// All jobs in one invocation, ordered by `started_at ASC`. Tab
    /// completion uses this to batch one query instead of N `get` calls.
    pub fn list_by_invocation(
        conn: &Connection,
        repo_hash: &str,
        invocation_id: &str,
    ) -> Result<Vec<JobRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, invocation_id, name, hook_type, worktree, command, working_dir,
                    env_json, started_at, finished_at, status, exit_code, pid, pgid,
                    background, needs_json, tags_json, retention_seconds, max_log_size_bytes
             FROM jobs
             WHERE repo_hash = ?1 AND invocation_id = ?2
             ORDER BY started_at ASC",
        )?;
        let rows: rusqlite::Result<Vec<JobRow>> = stmt
            .query_map(params![repo_hash, invocation_id], row_to_job)?
            .collect();
        Ok(rows?)
    }

    /// Jobs whose status matches either of two over-the-wire lowercase
    /// tags (e.g. `"running"`, `"cancelling"` — the reconciler's active
    /// set). The caller picks the two; the repo doesn't bake in business
    /// logic like "which statuses count as active".
    ///
    /// Fixed at exactly two statuses so the SQL stays parameterized with
    /// hardcoded placeholders — the `no format!` invariant in
    /// `src/store/repos/` is enforced by a CI grep-gate, and dynamic
    /// `IN (?2, ?3, ?4, ...)` construction would have to use `format!`.
    /// Add a sibling method (e.g. `list_by_repo_and_three_statuses`) if
    /// the reconciler grows a third lifecycle state.
    pub fn list_by_repo_and_two_statuses(
        conn: &Connection,
        repo_hash: &str,
        status_a: &str,
        status_b: &str,
    ) -> Result<Vec<JobRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, invocation_id, name, hook_type, worktree, command, working_dir,
                    env_json, started_at, finished_at, status, exit_code, pid, pgid,
                    background, needs_json, tags_json, retention_seconds, max_log_size_bytes
             FROM jobs
             WHERE repo_hash = ?1 AND status IN (?2, ?3)
             ORDER BY started_at ASC",
        )?;
        let rows: rusqlite::Result<Vec<JobRow>> = stmt
            .query_map(params![repo_hash, status_a, status_b], row_to_job)?
            .collect();
        Ok(rows?)
    }
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    let env_json: String = row.get("env_json")?;
    let needs_json: String = row.get("needs_json")?;
    let tags_json: String = row.get("tags_json")?;
    let env: HashMap<String, String> = serde_json::from_str(&env_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let needs: Vec<String> = serde_json::from_str(&needs_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let started_at_str: String = row.get("started_at")?;
    let started_at = parse_rfc3339(&started_at_str, "started_at")?;
    let finished_at = match row.get::<_, Option<String>>("finished_at")? {
        Some(s) => Some(parse_rfc3339(&s, "finished_at")?),
        None => None,
    };
    let background: i64 = row.get("background")?;
    let max_log_size_bytes = row
        .get::<_, Option<i64>>("max_log_size_bytes")?
        .map(|n| n as u64);
    Ok(JobRow {
        repo_hash: row.get("repo_hash")?,
        invocation_id: row.get("invocation_id")?,
        name: row.get("name")?,
        hook_type: row.get("hook_type")?,
        worktree: row.get("worktree")?,
        command: row.get("command")?,
        working_dir: row.get("working_dir")?,
        env,
        started_at,
        finished_at,
        status: row.get("status")?,
        exit_code: row.get::<_, Option<i32>>("exit_code")?,
        pid: row.get::<_, Option<u32>>("pid")?,
        pgid: row.get::<_, Option<u32>>("pgid")?,
        background: background != 0,
        needs,
        tags,
        retention_seconds: row.get::<_, Option<i64>>("retention_seconds")?,
        max_log_size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection;
    use crate::store::migrate;
    use crate::store::models::InvocationRow;
    use crate::store::repos::InvocationsRepo;
    use chrono::Utc;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, Connection) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        migrate::run(&mut conn, &path).unwrap();
        (tmp, conn)
    }

    fn seed_inv(conn: &Connection, repo: &str, inv: &str) {
        InvocationsRepo::upsert(
            conn,
            &InvocationRow {
                repo_hash: repo.into(),
                invocation_id: inv.into(),
                trigger_command: "test".into(),
                hook_type: "worktree-post-create".into(),
                worktree: "feat/test".into(),
                created_at: Utc::now(),
                coordinator_pid: None,
            },
        )
        .unwrap();
    }

    fn sample_job(repo: &str, inv: &str, name: &str) -> JobRow {
        JobRow {
            repo_hash: repo.into(),
            invocation_id: inv.into(),
            name: name.into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/test".into(),
            command: "echo hi".into(),
            working_dir: "/tmp".into(),
            env: HashMap::from([("FOO".to_string(), "bar".to_string())]),
            started_at: Utc::now(),
            finished_at: None,
            status: "running".into(),
            exit_code: None,
            pid: Some(12345),
            pgid: Some(12345),
            background: true,
            needs: vec!["dep1".into()],
            tags: vec!["slow".into(), "build".into()],
            retention_seconds: None,
            max_log_size_bytes: None,
        }
    }

    #[test]
    fn upsert_and_get_round_trip() {
        let (_tmp, conn) = fresh_db();
        seed_inv(&conn, "r", "inv");
        let row = sample_job("r", "inv", "build");
        JobsRepo::upsert(&conn, &row).unwrap();
        let back = JobsRepo::get(&conn, "r", "inv", "build").unwrap().unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let (_tmp, conn) = fresh_db();
        seed_inv(&conn, "r", "inv");
        let mut row = sample_job("r", "inv", "build");
        JobsRepo::upsert(&conn, &row).unwrap();
        row.status = "completed".into();
        row.exit_code = Some(0);
        JobsRepo::upsert(&conn, &row).unwrap();
        let back = JobsRepo::get(&conn, "r", "inv", "build").unwrap().unwrap();
        assert_eq!(back.status, "completed");
        assert_eq!(back.exit_code, Some(0));
    }

    #[test]
    fn list_by_repo_returns_all_invocations_in_order() {
        let (_tmp, conn) = fresh_db();
        seed_inv(&conn, "r", "i1");
        seed_inv(&conn, "r", "i2");
        // Insert in reverse-chronological order to assert sort.
        let mut a = sample_job("r", "i1", "a");
        a.started_at = Utc::now() - chrono::Duration::seconds(10);
        let mut b = sample_job("r", "i2", "b");
        b.started_at = Utc::now();
        JobsRepo::upsert(&conn, &b).unwrap();
        JobsRepo::upsert(&conn, &a).unwrap();
        let rows = JobsRepo::list_by_repo(&conn, "r").unwrap();
        let names: Vec<_> = rows.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn list_by_repo_filters_out_other_repos() {
        let (_tmp, conn) = fresh_db();
        seed_inv(&conn, "rA", "i");
        seed_inv(&conn, "rB", "i");
        JobsRepo::upsert(&conn, &sample_job("rA", "i", "a")).unwrap();
        JobsRepo::upsert(&conn, &sample_job("rB", "i", "b")).unwrap();
        let only_a = JobsRepo::list_by_repo(&conn, "rA").unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].name, "a");
    }

    #[test]
    fn list_by_invocation_filters_by_invocation_id() {
        let (_tmp, conn) = fresh_db();
        seed_inv(&conn, "r", "i1");
        seed_inv(&conn, "r", "i2");
        JobsRepo::upsert(&conn, &sample_job("r", "i1", "a")).unwrap();
        JobsRepo::upsert(&conn, &sample_job("r", "i1", "b")).unwrap();
        JobsRepo::upsert(&conn, &sample_job("r", "i2", "c")).unwrap();
        let rows = JobsRepo::list_by_invocation(&conn, "r", "i1").unwrap();
        let names: Vec<_> = rows.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn list_by_repo_and_two_statuses_filters_to_requested_pair() {
        let (_tmp, conn) = fresh_db();
        seed_inv(&conn, "r", "i");
        let mut running = sample_job("r", "i", "running");
        let mut completed = sample_job("r", "i", "completed");
        completed.status = "completed".into();
        let mut cancelling = sample_job("r", "i", "cancelling");
        cancelling.status = "cancelling".into();
        let mut crashed = sample_job("r", "i", "crashed");
        crashed.status = "crashed".into();
        running.status = "running".into();
        for row in [&running, &completed, &cancelling, &crashed] {
            JobsRepo::upsert(&conn, row).unwrap();
        }
        let active =
            JobsRepo::list_by_repo_and_two_statuses(&conn, "r", "running", "cancelling").unwrap();
        let mut names: Vec<_> = active.iter().map(|j| j.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["cancelling", "running"]);
    }

    #[test]
    fn deleting_invocation_does_not_touch_jobs_today() {
        // The jobs ↔ invocations FK is deliberately *not* declared in
        // 001_initial.sql (see the migration comment). A future migration
        // will add it with ON DELETE CASCADE; until then, deleting an
        // invocation leaves orphan job rows behind. This test pins current
        // behavior so when the FK lands the change is visible.
        let (_tmp, conn) = fresh_db();
        seed_inv(&conn, "r", "i");
        JobsRepo::upsert(&conn, &sample_job("r", "i", "a")).unwrap();
        JobsRepo::upsert(&conn, &sample_job("r", "i", "b")).unwrap();
        conn.execute(
            "DELETE FROM invocations WHERE repo_hash = ?1 AND invocation_id = ?2",
            params!["r", "i"],
        )
        .unwrap();
        let rows = JobsRepo::list_by_repo(&conn, "r").unwrap();
        assert_eq!(rows.len(), 2, "no cascade without FK; got {rows:?}");
    }

    #[test]
    fn upsert_succeeds_without_invocation_row() {
        // No FK enforcement today (see 001_initial.sql). Production code
        // never populates `invocations`, so requiring the row would refuse
        // every job insert. Pins the current "jobs can stand alone" shape.
        let (_tmp, conn) = fresh_db();
        JobsRepo::upsert(&conn, &sample_job("r", "i-orphan", "a")).unwrap();
        let back = JobsRepo::get(&conn, "r", "i-orphan", "a").unwrap();
        assert!(back.is_some());
    }
}
