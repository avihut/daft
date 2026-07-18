//! Queries against the `forge_health` singleton table (repo forge health).

use crate::store::error::Result;
use crate::store::models::ForgeHealthRow;
use crate::store::repos::invocations::parse_rfc3339;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub struct ForgeHealthRepo;

impl ForgeHealthRepo {
    /// The repo's health row, or `None` when no refresh has ever run.
    pub fn get(conn: &Connection) -> Result<Option<ForgeHealthRow>> {
        let row = conn
            .query_row(
                "SELECT healthy, error_kind, started_at, finished_at, succeeded_at
                 FROM forge_health WHERE id = 1",
                [],
                row_to_health,
            )
            .optional()?;
        Ok(row)
    }

    /// Stamp the start of a refresh attempt, leaving the last verdict
    /// (healthy/error/finished/succeeded) untouched.
    pub fn record_started(conn: &Connection, at: DateTime<Utc>) -> Result<()> {
        conn.execute(
            "INSERT INTO forge_health (id, started_at) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET started_at = excluded.started_at",
            params![at.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Record a successful refresh: healthy again, no error, attempt
    /// concluded, snapshot taken.
    pub fn record_success(conn: &Connection, at: DateTime<Utc>) -> Result<()> {
        conn.execute(
            "INSERT INTO forge_health (id, healthy, error_kind, finished_at, succeeded_at)
             VALUES (1, 1, NULL, ?1, ?1)
             ON CONFLICT(id) DO UPDATE SET
                 healthy      = 1,
                 error_kind   = NULL,
                 finished_at  = excluded.finished_at,
                 succeeded_at = excluded.succeeded_at",
            params![at.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Mark the forge reachable again without claiming a snapshot — the
    /// write-through path when a single-PR resolve just succeeded (proves
    /// tool + auth + access, but is not a wholesale listing).
    pub fn record_healthy(conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT INTO forge_health (id, healthy, error_kind) VALUES (1, 1, NULL)
             ON CONFLICT(id) DO UPDATE SET healthy = 1, error_kind = NULL",
            [],
        )?;
        Ok(())
    }

    /// Record a failed refresh. A deep failure (`deep_kind` set) flips the
    /// repo unhealthy with that kind; a transient one (`None`) only concludes
    /// the attempt, preserving the previous verdict either way it leaned.
    pub fn record_failure(
        conn: &Connection,
        at: DateTime<Utc>,
        deep_kind: Option<&str>,
    ) -> Result<()> {
        match deep_kind {
            Some(kind) => conn.execute(
                "INSERT INTO forge_health (id, healthy, error_kind, finished_at)
                 VALUES (1, 0, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET
                     healthy     = 0,
                     error_kind  = excluded.error_kind,
                     finished_at = excluded.finished_at",
                params![kind, at.to_rfc3339()],
            )?,
            None => conn.execute(
                "INSERT INTO forge_health (id, finished_at) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET finished_at = excluded.finished_at",
                params![at.to_rfc3339()],
            )?,
        };
        Ok(())
    }
}

fn row_to_health(row: &rusqlite::Row<'_>) -> rusqlite::Result<ForgeHealthRow> {
    let parse = |field: &'static str| -> rusqlite::Result<Option<DateTime<Utc>>> {
        row.get::<_, Option<String>>(field)?
            .map(|s| parse_rfc3339(&s, field))
            .transpose()
    };
    Ok(ForgeHealthRow {
        healthy: row.get("healthy")?,
        error_kind: row.get("error_kind")?,
        started_at: parse("started_at")?,
        finished_at: parse("finished_at")?,
        succeeded_at: parse("succeeded_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{connection, migrate};
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, Connection) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        migrate::run(&mut conn, &path).unwrap();
        (tmp, conn)
    }

    fn at(minute: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 18, 12, minute, 0).unwrap()
    }

    #[test]
    fn missing_row_reads_as_none() {
        let (_tmp, conn) = fresh_db();
        assert_eq!(ForgeHealthRepo::get(&conn).unwrap(), None);
    }

    #[test]
    fn started_then_success_round_trips() {
        let (_tmp, conn) = fresh_db();
        ForgeHealthRepo::record_started(&conn, at(0)).unwrap();

        let h = ForgeHealthRepo::get(&conn).unwrap().unwrap();
        assert!(h.healthy, "a fresh attempt starts from the healthy default");
        assert_eq!(h.started_at, Some(at(0)));
        assert_eq!(h.finished_at, None);
        assert_eq!(h.succeeded_at, None);

        ForgeHealthRepo::record_success(&conn, at(1)).unwrap();
        let h = ForgeHealthRepo::get(&conn).unwrap().unwrap();
        assert!(h.healthy);
        assert_eq!(h.error_kind, None);
        assert_eq!(
            h.started_at,
            Some(at(0)),
            "start stamp survives the verdict"
        );
        assert_eq!(h.finished_at, Some(at(1)));
        assert_eq!(h.succeeded_at, Some(at(1)));
    }

    #[test]
    fn deep_failure_flips_unhealthy_and_success_restores() {
        let (_tmp, conn) = fresh_db();
        ForgeHealthRepo::record_started(&conn, at(0)).unwrap();
        ForgeHealthRepo::record_failure(&conn, at(1), Some("unauthenticated")).unwrap();

        let h = ForgeHealthRepo::get(&conn).unwrap().unwrap();
        assert!(!h.healthy);
        assert_eq!(h.error_kind.as_deref(), Some("unauthenticated"));
        assert_eq!(h.succeeded_at, None);

        ForgeHealthRepo::record_success(&conn, at(2)).unwrap();
        let h = ForgeHealthRepo::get(&conn).unwrap().unwrap();
        assert!(h.healthy, "a successful refresh restores the column");
        assert_eq!(h.error_kind, None);
        assert_eq!(h.succeeded_at, Some(at(2)));
    }

    #[test]
    fn record_healthy_restores_without_claiming_a_snapshot() {
        let (_tmp, conn) = fresh_db();
        ForgeHealthRepo::record_failure(&conn, at(1), Some("unauthenticated")).unwrap();
        ForgeHealthRepo::record_healthy(&conn).unwrap();

        let h = ForgeHealthRepo::get(&conn).unwrap().unwrap();
        assert!(h.healthy);
        assert_eq!(h.error_kind, None);
        assert_eq!(
            h.succeeded_at, None,
            "a single-PR resolve is not a snapshot — the first-load skeleton must survive"
        );
    }

    #[test]
    fn transient_failure_preserves_the_standing_verdict() {
        let (_tmp, conn) = fresh_db();
        // Transient with no prior row: concluded, still healthy by default.
        ForgeHealthRepo::record_failure(&conn, at(1), None).unwrap();
        let h = ForgeHealthRepo::get(&conn).unwrap().unwrap();
        assert!(h.healthy, "a network blip must not hide the column");
        assert_eq!(h.finished_at, Some(at(1)));

        // Transient after a deep failure: stays unhealthy until a success.
        ForgeHealthRepo::record_failure(&conn, at(2), Some("missing-tool")).unwrap();
        ForgeHealthRepo::record_failure(&conn, at(3), None).unwrap();
        let h = ForgeHealthRepo::get(&conn).unwrap().unwrap();
        assert!(!h.healthy);
        assert_eq!(h.error_kind.as_deref(), Some("missing-tool"));
        assert_eq!(h.finished_at, Some(at(3)));
    }
}
