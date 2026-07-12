//! Connection pools for store consumers.
//!
//! Two pools per store:
//!
//! * **Writer pool** (size 1) — enforces single-writer ordering. Matches
//!   WAL's "one writer at a time" semantics: serializing here is cheaper
//!   than waiting on SQLITE_BUSY. Used by the coordinator (long-lived
//!   process running many bg jobs in parallel) for upserts.
//! * **Reader pool** — opened `SQLITE_OPEN_READ_ONLY` with a tight
//!   `busy_timeout`. Used by CLI handlers and the tab-completion hot
//!   path. Readers don't block writers and vice versa under WAL.
//!
//! Both pools share the same DB file but each connection has its own
//! `Connection` handle (r2d2_sqlite manages that). The custom connection
//! manager applies daft's PRAGMA bring-up via the `connection` module so
//! no consumer can accidentally skip security defaults.

use crate::store::connection::{READER_BUSY_TIMEOUT_MS, WRITER_BUSY_TIMEOUT_MS, bring_up};
use crate::store::error::{Result, StoreError};
use crate::store::{migrate, paths};
use r2d2::{ManageConnection, Pool as R2d2Pool, PooledConnection};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Reusable handle that hands out writer and reader connections.
#[derive(Clone)]
pub struct Pool {
    inner: Arc<PoolInner>,
}

struct PoolInner {
    writer: R2d2Pool<DaftSqliteManager>,
    reader: R2d2Pool<DaftSqliteManager>,
    path: PathBuf,
}

impl Pool {
    /// Open or create the per-repo coordinator DB at `path`. See
    /// [`Pool::open_with`]; this applies the coordinator migration lineage.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, &migrate::coordinator_set())
    }

    /// Open or create the DB at `path`, apply the lineage's pending
    /// migrations on a throwaway connection, then build the writer/reader
    /// pools. The migration runs once at open time so subsequent pool
    /// checkouts skip the version check.
    pub fn open_with(path: &Path, set: &migrate::MigrationSet) -> Result<Self> {
        let is_fresh_db = !path.exists();
        // Migration must run on a writable connection before the read-only
        // pool ever hands one out (read-only can't bootstrap the schema).
        let mut bootstrap = Connection::open(path)?;
        bring_up(
            &mut bootstrap,
            path,
            WRITER_BUSY_TIMEOUT_MS,
            is_fresh_db,
            /* read_only */ false,
        )?;
        migrate::run_set(set, &mut bootstrap, path)?;
        drop(bootstrap);

        // Verify file/parent permissions are at most 0o600/0o700 before any
        // checkout. Fresh DBs were just tightened by `bring_up`; existing
        // DBs are checked here to catch tampering (e.g. an admin process
        // replacing the file with a world-readable copy between daft
        // invocations). Runs once per pool open — the writer/reader
        // checkouts don't pay the stat.
        paths::verify_perms(path)?;

        let writer_mgr =
            DaftSqliteManager::new(path, WRITER_BUSY_TIMEOUT_MS, /* read_only */ false);
        let writer = R2d2Pool::builder()
            .max_size(1)
            .build(writer_mgr)
            .map_err(StoreError::from)?;

        let reader_mgr =
            DaftSqliteManager::new(path, READER_BUSY_TIMEOUT_MS, /* read_only */ true);
        let reader = R2d2Pool::builder()
            .max_size(8)
            .build(reader_mgr)
            .map_err(StoreError::from)?;

        Ok(Self {
            inner: Arc::new(PoolInner {
                writer,
                reader,
                path: path.to_path_buf(),
            }),
        })
    }

    /// Checkout a writer connection. The pool has one writer slot, so the
    /// call blocks while another writer is in flight.
    pub fn writer(&self) -> Result<PooledConnection<DaftSqliteManager>> {
        self.inner.writer.get().map_err(StoreError::from)
    }

    /// Checkout a read-only connection. Tight `busy_timeout` means the
    /// caller fails fast and falls back rather than blocking the user.
    pub fn reader(&self) -> Result<PooledConnection<DaftSqliteManager>> {
        self.inner.reader.get().map_err(StoreError::from)
    }

    /// Path the pool was opened against. Useful for diagnostics.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("path", &self.inner.path)
            .finish()
    }
}

/// r2d2 connection manager that wraps `SqliteConnectionManager` with
/// daft's PRAGMA bring-up.
pub struct DaftSqliteManager {
    inner: SqliteConnectionManager,
    path: PathBuf,
    busy_timeout_ms: u32,
    read_only: bool,
}

impl DaftSqliteManager {
    fn new(path: &Path, busy_timeout_ms: u32, read_only: bool) -> Self {
        let inner = if read_only {
            SqliteConnectionManager::file(path)
                .with_flags(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        } else {
            SqliteConnectionManager::file(path).with_flags(
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
        };
        Self {
            inner,
            path: path.to_path_buf(),
            busy_timeout_ms,
            read_only,
        }
    }
}

impl ManageConnection for DaftSqliteManager {
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> std::result::Result<Connection, rusqlite::Error> {
        let mut conn = self.inner.connect()?;
        // Pool init already ran migrations on a fresh DB, so subsequent
        // connections never observe a fresh-create case here.
        bring_up(
            &mut conn,
            &self.path,
            self.busy_timeout_ms,
            /* is_fresh_db */ false,
            self.read_only,
        )
        .map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                Some(format!("daft bring_up: {e}")),
            )
        })?;
        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Connection) -> std::result::Result<(), rusqlite::Error> {
        conn.execute_batch("SELECT 1;")
    }

    fn has_broken(&self, _conn: &mut Connection) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_creates_db_and_runs_migrations() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let pool = Pool::open(&path).unwrap();
        let conn = pool.reader().unwrap();
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(
            tables.contains(&"invocations".to_string()),
            "got {tables:?}"
        );
        assert!(tables.contains(&"jobs".to_string()), "got {tables:?}");
        assert!(
            tables.contains(&"repo_policy".to_string()),
            "got {tables:?}"
        );
    }

    #[test]
    fn writer_pool_has_capacity_one() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let pool = Pool::open(&path).unwrap();
        let _first = pool.writer().unwrap();
        // Second checkout must time out (the pool has 1 writer slot).
        let second_result = pool
            .inner
            .writer
            .get_timeout(std::time::Duration::from_millis(50));
        assert!(
            second_result.is_err(),
            "second writer should have timed out"
        );
    }

    #[test]
    fn reader_pool_allows_concurrent_checkouts() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let pool = Pool::open(&path).unwrap();
        let _r1 = pool.reader().unwrap();
        let _r2 = pool.reader().unwrap();
        // Both readers succeed because the reader pool has multiple slots.
    }

    #[test]
    #[cfg(unix)]
    fn pool_rejects_world_readable_db() {
        use crate::store::error::StoreError;
        use std::os::unix::fs::PermissionsExt;

        // First open tightens to 0o600.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        drop(Pool::open(&path).unwrap());

        // Simulate an admin tool that loosened the perms between daft
        // invocations. Parent stays 0o700 from the first open.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let err = Pool::open(&path).unwrap_err();
        assert!(
            matches!(err, StoreError::PermissionsTooOpen { .. }),
            "expected PermissionsTooOpen, got {err:?}"
        );
    }
}
