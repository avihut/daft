//! Schema migration runner.
//!
//! Migrations are checked-in `.sql` files in `src/store/migrations/`,
//! numbered `NNN_<name>.sql`. Each file is one logical schema upgrade and
//! runs in its own transaction. The on-disk schema version is stored in
//! SQLite's `user_version` PRAGMA so we don't need a sidecar table.
//!
//! Two safety rules:
//!   1. `migrations()` is monotonically appended to — new versions only.
//!   2. Opening a DB whose `user_version` is *higher* than the binary's
//!      highest migration is rejected: a newer daft wrote data this binary
//!      doesn't understand.

use crate::store::error::{Result, StoreError};
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};
use std::path::Path;

/// Returns the full migration set. Append new migrations to the bottom of
/// the vector — never reorder, never edit a shipped migration in place.
pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(include_str!("migrations/001_initial.sql"))])
}

/// Highest schema version this binary understands. Mirrors
/// `migrations().pending_migrations(...)` math but is cheap and side-effect
/// free so we can call it from the open-time refuse-newer check.
pub fn current_version() -> i64 {
    // rusqlite_migration's version counter is `migrations.len() as u32` after
    // every migration is applied. We return that as i64 for consistency with
    // the on-disk `user_version` PRAGMA type.
    1
}

/// Apply all pending migrations against `conn`. Refuses if the on-disk
/// `user_version` is *higher* than the binary's [`current_version`].
pub fn run(conn: &mut Connection, db_path: &Path) -> Result<()> {
    let on_disk: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let expected = current_version();
    if on_disk > expected {
        return Err(StoreError::SchemaTooNew {
            path: db_path.to_path_buf(),
            found: on_disk,
            expected,
        });
    }
    migrations().to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection;
    use tempfile::TempDir;

    #[test]
    fn migrations_validate() {
        migrations()
            .validate()
            .expect("migration set is well-formed");
    }

    #[test]
    fn fresh_db_lands_at_current_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, current_version());
    }

    #[test]
    fn refuses_open_when_user_version_is_newer() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        // Pretend a future daft wrote here.
        conn.execute_batch(&format!(
            "PRAGMA user_version = {};",
            current_version() + 99
        ))
        .unwrap();
        let err = run(&mut conn, &path).unwrap_err();
        assert!(matches!(err, StoreError::SchemaTooNew { .. }));
    }

    #[test]
    fn runs_pending_migrations_idempotently() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        run(&mut conn, &path).unwrap();
        // Second run is a no-op.
        run(&mut conn, &path).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, current_version());
    }
}
