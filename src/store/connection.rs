//! SQLite connection bring-up and security PRAGMAs.
//!
//! Every connection — writer or reader — passes through here so the
//! security defaults are non-bypassable. The PRAGMA set:
//!
//! * `application_id` — identifies the file as "daft coordinator". Reject
//!   foreign SQLite files on open.
//! * `journal_mode = WAL` — many readers + one writer cross-process. The
//!   workload daft actually has.
//! * `synchronous = NORMAL` — WAL-safe; sacrifices last-commit durability
//!   on power loss, not corruption resistance.
//! * `foreign_keys = ON` — declared FK constraints are enforced (SQLite
//!   defaults to off for compat).
//! * `busy_timeout` — bounds the kernel-level retry window when another
//!   process holds the write lock. Writer pool uses a generous timeout,
//!   reader pool uses a tight one (callers should retry, not block).
//! * `wal_autocheckpoint` — caps WAL growth so the `-wal` file doesn't
//!   balloon unbounded between explicit checkpoints.

use crate::store::error::{Result, StoreError};
use crate::store::paths;
use rusqlite::Connection;
use std::path::Path;

/// `application_id` PRAGMA value: "DFT" + version byte. Fits in i32 by
/// construction (0x44465401 = 1145389569 < 0x7FFF_FFFF).
pub const APPLICATION_ID: i32 = 0x44_46_54_01;

/// Default `busy_timeout` for writer connections, in milliseconds.
pub const WRITER_BUSY_TIMEOUT_MS: u32 = 5_000;

/// Default `busy_timeout` for reader connections, in milliseconds. Reader
/// callers (tab completion, `daft hooks jobs ls`) should fall back quickly
/// rather than block the user's shell.
pub const READER_BUSY_TIMEOUT_MS: u32 = 300;

/// One-shot bring-up applied to every fresh connection.
///
/// `is_fresh_db` is set by the pool's connection customizer: `true` when
/// the underlying file did not exist before this open. Fresh databases
/// have their `application_id` and file permissions stamped; existing
/// databases are verified instead.
pub(crate) fn bring_up(
    conn: &mut Connection,
    db_path: &Path,
    busy_timeout_ms: u32,
    is_fresh_db: bool,
    read_only: bool,
) -> Result<()> {
    // The busy timeout must be set before any other statement runs so that
    // the bring-up itself doesn't fail when another writer holds the lock.
    conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms as u64))?;

    if read_only {
        // Read-only connections cannot write PRAGMA application_id /
        // journal_mode. They must observe a writer's settings instead.
        verify_application_id(conn, db_path)?;
        return Ok(());
    }

    if is_fresh_db {
        // PRAGMA application_id is persisted in the file header — set once
        // on creation so we can recognize this file later.
        conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID};"))?;
    } else {
        verify_application_id(conn, db_path)?;
    }

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA wal_autocheckpoint = 1000;",
    )?;

    if is_fresh_db {
        // Tighten file permissions before any further use. WAL/SHM
        // sidecars inherit umask, but they're recreated on every connect
        // so a stricter umask later is enough.
        paths::tighten_perms(db_path)?;
    }

    Ok(())
}

fn verify_application_id(conn: &Connection, db_path: &Path) -> Result<()> {
    let found: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    if found != APPLICATION_ID {
        return Err(StoreError::AppIdMismatch {
            path: db_path.to_path_buf(),
            found,
            expected: APPLICATION_ID,
        });
    }
    Ok(())
}

/// Open an existing store file read-only, without a pool.
///
/// The single-connection sibling of the reader pool, for hot paths that
/// open at most once per invocation (tab completion, catalog lookups).
/// Never creates the file — SQLITE_OPEN_READ_ONLY fails on a missing path
/// — and passes through [`bring_up`] so the application-id check still
/// gates foreign files. Callers own the fail-fast contract: treat any
/// error as "no data", never block or print.
pub(crate) fn open_read_only(path: &Path, busy_timeout_ms: u32) -> Result<Connection> {
    let mut conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    bring_up(
        &mut conn,
        path,
        busy_timeout_ms,
        /* is_fresh_db */ false,
        /* read_only */ true,
    )?;
    Ok(conn)
}

/// Single-connection helper for tests that need to inspect or mutate a
/// store file without spinning up the connection pool. Applies the same
/// PRAGMAs the pool would.
#[cfg(test)]
pub(crate) fn open_for_test(path: &Path) -> Result<Connection> {
    let is_fresh = !path.exists();
    let mut conn = Connection::open(path)?;
    bring_up(
        &mut conn,
        path,
        WRITER_BUSY_TIMEOUT_MS,
        is_fresh,
        /* read_only */ false,
    )?;
    if is_fresh {
        crate::store::migrate::run(&mut conn, path)?;
    }
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn fresh_db_gets_application_id_and_wal() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let conn = open_for_test(&path).unwrap();
        let id: i32 = conn
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .unwrap();
        assert_eq!(id, APPLICATION_ID);
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal.to_lowercase(), "wal");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn refuses_to_open_foreign_app_id() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA application_id = 0x12345678;")
                .unwrap();
        }
        // Plain `Connection::open` + bring_up sees the foreign id.
        let mut conn = Connection::open(&path).unwrap();
        let err = bring_up(
            &mut conn,
            &path,
            WRITER_BUSY_TIMEOUT_MS,
            /* is_fresh_db */ false,
            /* read_only */ false,
        )
        .unwrap_err();
        assert!(matches!(err, StoreError::AppIdMismatch { .. }));
    }
}
